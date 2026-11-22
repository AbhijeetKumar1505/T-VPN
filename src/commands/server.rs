//! `tvpn server ...`: run your own VPN server.

use super::session::{self, Running, Shutdown};
use crate::cli::{AddPeerArgs, PeerNameArgs, ServerCommand, ServerInitArgs, ServerUpArgs, ShowPeerArgs};
use crate::config::Config;
use crate::control::{self, Mode, Request, SessionState};
use crate::engine::{Engine, PeerConfig, Sockets};
use crate::netcfg::{self, Applied, ServerPlan};
use crate::server::{self, InitOptions, ServerStore};
use crate::tun::{self, TunOptions};
use crate::{keys, privilege, profiles, ui};
use anyhow::{Context, Result, anyhow, bail};
use ipnet::{IpNet, Ipv6Net};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;

pub async fn run(cmd: ServerCommand) -> Result<i32> {
    match cmd {
        ServerCommand::Init(a) => init(a),
        ServerCommand::Up(a) => up(a).await,
        ServerCommand::Down(a) => super::down::stop(Mode::Server, !a.no_sudo).await,
        ServerCommand::AddPeer(a) => add_peer(a).await,
        ServerCommand::RemovePeer(a) => remove_peer(a).await,
        ServerCommand::Peers => peers().await,
        ServerCommand::ShowPeer(a) => show_peer(a),
        ServerCommand::Reload(a) => reload(!a.no_sudo, true).await,
    }
}

fn init(args: ServerInitArgs) -> Result<i32> {
    let store = ServerStore::default_location();
    let host = match args.endpoint {
        Some(h) => h,
        None => {
            let ip = server::guess_public_ip()
                .ok_or_else(|| anyhow!("could not detect this machine's IP; pass --endpoint <host>"))?;
            if server::is_private(&ip) {
                ui::warn(format!(
                    "detected IP {ip} is private; clients outside your network can't reach it. \
                     Pass --endpoint <public-host-or-ip> (and forward UDP {} on your router).",
                    args.port
                ));
            }
            ip.to_string()
        }
    };
    let subnet_v6 = if args.ipv6 { Some(random_ula()) } else { None };
    let cfg = store.init(
        &InitOptions {
            endpoint_host: host.clone(),
            port: args.port,
            subnet_v4: args.subnet,
            subnet_v6,
            dns: args.dns,
        },
        args.force,
    )?;
    let endpoint = crate::config::Endpoint { host, port: args.port };
    ui::success("Server configured");
    ui::kv("endpoint", &endpoint);
    ui::kv(
        "subnet",
        cfg.interface.addresses.iter().map(|a| a.trunc().to_string()).collect::<Vec<_>>().join(", "),
    );
    ui::kv("public key", cfg.interface.private_key.public_key());
    ui::kv("config", store.conf_path().display());
    println!();
    println!("Next steps:");
    ui::hint("1. tvpn server add-peer laptop --qr     # create a client config");
    ui::hint("2. tvpn server up                        # start serving (Ctrl+C stops)");
    ui::hint(format!("3. allow UDP {} in your firewall / cloud security group", args.port));
    Ok(0)
}

fn random_ula() -> Ipv6Net {
    // fdXX:XXXX:XXXX::/64 with a random 40-bit global ID (RFC 4193).
    let r = keys::Key::generate_preshared();
    let b = r.as_bytes();
    let addr = std::net::Ipv6Addr::new(
        0xfd00 | b[0] as u16,
        u16::from_be_bytes([b[1], b[2]]),
        u16::from_be_bytes([b[3], b[4]]),
        0,
        0,
        0,
        0,
        0,
    );
    Ipv6Net::new(addr, 64).expect("valid prefix")
}

fn peer_configs(cfg: &Config) -> Vec<PeerConfig> {
    cfg.peers
        .iter()
        .map(|p| PeerConfig {
            name: p.name.clone(),
            public_key: p.public_key,
            preshared_key: p.preshared_key,
            allowed_ips: p.allowed_ips.clone(),
            endpoint: None,
            persistent_keepalive: p.persistent_keepalive,
        })
        .collect()
}

async fn up(args: ServerUpArgs) -> Result<i32> {
    let store = ServerStore::default_location();
    let (cfg, meta) = store.load()?;
    cfg.validate_server().map_err(|e| anyhow!("{}: {e}", store.conf_path().display()))?;
    privilege::ensure_elevated(!args.no_sudo)?;
    let mut shutdown = Shutdown::install();

    if let Some(prev) = SessionState::load(Mode::Server) {
        if control::is_alive(&prev).await {
            bail!("the server is already running (pid {}); stop it with `tvpn server down`", prev.pid);
        }
        ui::warn("a previous server session did not shut down cleanly; restoring its settings");
        session::cleanup_stale(&prev).await;
    }

    let port = cfg.interface.listen_port.expect("validated");
    let device = tun::create(&TunOptions {
        name: tun::SERVER_IFNAME,
        addresses: &cfg.interface.addresses,
        mtu: cfg.mtu(),
        guid: tun::SERVER_GUID,
    })?;
    let ifname = device.name().unwrap_or_else(|_| tun::SERVER_IFNAME.to_string());
    let device = Arc::new(device);

    let mut notes = Vec::new();
    let (sockets, v6_err) = Sockets::bind_server(port)
        .with_context(|| format!("cannot listen on UDP port {port} (already in use?)"))?;
    if let Some(e) = v6_err {
        notes.push(format!("IPv6 listener unavailable ({e}); serving IPv4 only"));
    }
    let started = Instant::now();
    let engine = Engine::start(&cfg.interface.private_key, peer_configs(&cfg), sockets, device.clone());

    let token = keys::random_hex(32);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.context("cannot open the control port")?;
    let mut state = SessionState {
        mode: Mode::Server,
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: session::pid(),
        profile: "server".into(),
        interface: ifname.clone(),
        addresses: cfg.interface.addresses.clone(),
        control_port: listener.local_addr()?.port(),
        started_at: control::now_unix(),
        applied: Applied::default(),
        cloud: None,
    };
    state.save(&token)?;

    let plan = ServerPlan {
        interface: &ifname,
        subnets: cfg.interface.addresses.iter().map(IpNet::trunc).collect(),
        listen_port: port,
        nat: !args.no_nat,
        wan: args.wan.clone(),
        firewall: !args.no_firewall,
    };
    let mut applied = Applied::default();
    let result = {
        let mut persist = |a: &Applied| {
            let mut s = state.clone();
            s.applied = a.clone();
            let _ = s.save_state_only();
        };
        netcfg::apply_server(&plan, &mut applied, &mut persist, &mut notes)
    };
    state.applied = applied.clone();
    let _ = state.save_state_only();
    if let Err(e) = result {
        engine.shutdown().await;
        for w in netcfg::revert(&applied) {
            ui::warn(w);
        }
        drop(device);
        SessionState::remove(Mode::Server);
        return Err(e);
    }

    if !args.quiet {
        println!("{} {}{}server", ui::bold("tvpn"), env!("CARGO_PKG_VERSION"), ui::DOT);
        let addrs: Vec<String> = cfg.interface.addresses.iter().map(ToString::to_string).collect();
        ui::kv("interface", format!("{ifname} ({})", addrs.join(", ")));
        ui::kv("listening", format!("UDP {port}"));
        ui::kv("endpoint", format!("{} {}", meta.endpoint, ui::dim("(given to clients)")));
        ui::kv("peers", cfg.peers.len());
        let nat = if args.no_nat {
            ui::dim("off (--no-nat)")
        } else if applied.firewall.iter().any(|r| r.chain == "POSTROUTING") {
            ui::green("on")
        } else {
            ui::yellow("not configured")
        };
        ui::kv("nat", nat);
        if cfg.peers.is_empty() {
            ui::hint("No peers yet: run `tvpn server add-peer <name>` in another terminal.");
        }
    }
    for n in notes {
        ui::warn(n);
    }

    let reload_store = ServerStore::default_location();
    session::run(
        Running {
            mode: Mode::Server,
            state,
            token,
            listener,
            engine,
            device,
            applied,
            quiet: args.quiet,
            started,
        },
        &mut shutdown,
        move || {
            let (cfg, _) = reload_store.load()?;
            Ok(peer_configs(&cfg))
        },
    )
    .await
}

/// Ask a running server to re-read its peers. Returns Ok(false) if not running.
async fn reload(allow_sudo: bool, verbose: bool) -> Result<i32> {
    let Some(state) = SessionState::load(Mode::Server) else {
        if verbose {
            println!("{}The server is not running; changes apply on the next `tvpn server up`.", ui::OFF);
        }
        return Ok(0);
    };
    if !control::is_alive(&state).await {
        bail!("the server session (pid {}) is not responding", state.pid);
    }
    privilege::ensure_elevated(allow_sudo)?;
    let token = SessionState::read_token(Mode::Server)?;
    let resp = control::request(state.control_port, &Request::Reload { token }).await?;
    if !resp.ok {
        bail!("reload failed: {}", resp.error.unwrap_or_default());
    }
    ui::success(format!("Running server updated: {}", resp.message.unwrap_or_default()));
    Ok(0)
}

/// After a peer change, apply it to a running server if we can do so without prompting.
async fn reload_if_running() {
    let Some(state) = SessionState::load(Mode::Server) else { return };
    if !control::is_alive(&state).await {
        return;
    }
    if !privilege::is_elevated() {
        ui::hint("The server is running: apply this change with `sudo tvpn server reload`.");
        return;
    }
    if let Err(e) = reload(false, false).await {
        ui::warn(format!("{e:#}"));
    }
}

async fn add_peer(args: AddPeerArgs) -> Result<i32> {
    let store = ServerStore::default_location();
    let new = store.add_peer(&args.name, args.allowed_ips)?;
    let text = new.client_config.to_wg_string();

    ui::success(format!("Added peer {}", ui::cyan(&new.name)));
    ui::kv(
        "address",
        new.client_config.interface.addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
    );
    ui::kv("config", new.path.display());
    if let Some(out) = &args.output {
        crate::paths::write_private(out, text.as_bytes())?;
        ui::kv("written to", out.display());
    }
    if args.import {
        let path = profiles::save(&new.name, &new.client_config, false)?;
        ui::kv("profile", path.display());
    }
    println!();
    if args.qr {
        println!("{}", ui::qr(&text)?);
        ui::hint("Scan with the WireGuard app (Add tunnel > Scan from QR code).");
    } else {
        println!("{}", ui::dim("# --- client config (paste into the WireGuard app, or `tvpn import`) ---"));
        println!("{text}");
    }
    reload_if_running().await;
    Ok(0)
}

async fn remove_peer(args: PeerNameArgs) -> Result<i32> {
    ServerStore::default_location().remove_peer(&args.name)?;
    ui::success(format!("Removed peer {}", args.name));
    reload_if_running().await;
    Ok(0)
}

fn show_peer(args: ShowPeerArgs) -> Result<i32> {
    let (path, text) = ServerStore::default_location().peer_config(&args.name)?;
    if args.qr {
        println!("{}", ui::qr(&text)?);
    } else {
        println!("{}", ui::dim(format!("# {}", path.display())));
        print!("{text}");
    }
    Ok(0)
}

async fn peers() -> Result<i32> {
    let store = ServerStore::default_location();
    let (cfg, meta) = store.load()?;
    let live = match super::status::query(Mode::Server).await {
        Some(Ok(s)) => Some(s.peers),
        _ => None,
    };
    println!(
        "{} {}{}endpoint {}",
        ui::bold("Server"),
        if live.is_some() { ui::green("running") } else { ui::dim("stopped") },
        ui::DOT,
        meta.endpoint
    );
    if cfg.peers.is_empty() {
        ui::hint("No peers yet. Add one with `tvpn server add-peer <name>`.");
        return Ok(0);
    }
    println!();
    match live {
        Some(peers) => super::status::print_peer_table(&peers),
        None => {
            let rows: Vec<(String, Vec<String>)> = cfg
                .peers
                .iter()
                .map(|p| {
                    let addr = p.allowed_ips.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                    (
                        String::new(),
                        vec![p.name.clone().unwrap_or_else(|| "-".into()), addr, p.public_key.short()],
                    )
                })
                .collect();
            ui::table(&["PEER", "ADDRESS", "PUBLIC KEY"], &rows);
        }
    }
    Ok(0)
}
