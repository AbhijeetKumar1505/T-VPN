//! `tvpn up`: connect as a client. [`connect`] is shared with `tvpn cloud up`.

use super::session::{self, Running, Shutdown};
use crate::cli::UpArgs;
use crate::cloud::CloudServer;
use crate::config::Config;
use crate::control::{self, Mode, SessionState};
use crate::engine::{Engine, PeerConfig, PeerStatus, Sockets};
use crate::netcfg::{self, Applied, ClientPlan};
use crate::tun::{self, TunOptions};
use crate::ui::{self, StatusLine};
use crate::{keys, privilege, profiles};
use anyhow::{Context, Result, anyhow, bail};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// Exit code when the user cancels with Ctrl+C before connecting.
pub const CANCELLED: i32 = 130;

pub async fn run(args: UpArgs) -> Result<i32> {
    let profile = profiles::resolve(args.profile.as_deref())?;
    let parsed = profiles::load(&profile.path)?;
    for w in &parsed.warnings {
        ui::warn(w);
    }
    let cfg = parsed.config;
    cfg.validate_client().map_err(|e| anyhow!("{}: {e}", profile.path.display()))?;

    privilege::ensure_elevated(!args.no_sudo)?;
    let mut shutdown = Shutdown::install();
    ensure_no_client_session().await?;
    connect(
        Connect {
            name: profile.name,
            config: cfg,
            no_dns: args.no_dns,
            interface: args.interface,
            quiet: args.quiet,
            wait_for_server: None,
            cloud: None,
        },
        &mut shutdown,
    )
    .await
}

/// Refuse to start while a client session is running; clean up a crashed one.
pub async fn ensure_no_client_session() -> Result<()> {
    if let Some(prev) = SessionState::load(Mode::Client) {
        if control::is_alive(&prev).await {
            bail!(
                "already connected with profile `{}` (interface {}). Run `tvpn down` first.",
                prev.profile,
                prev.interface
            );
        }
        ui::warn("a previous session did not shut down cleanly; restoring its network settings");
        session::cleanup_stale(&prev).await;
    }
    Ok(())
}

pub struct Connect {
    /// Profile or session name shown in status output.
    pub name: String,
    pub config: Config,
    pub no_dns: bool,
    pub interface: Option<String>,
    pub quiet: bool,
    /// Wait (up to this long) for the first handshake before routing any
    /// traffic, so the user keeps their internet while a new server boots.
    pub wait_for_server: Option<Duration>,
    pub cloud: Option<CloudServer>,
}

/// Bring up the tunnel for `c.config` and run the session until it ends.
pub async fn connect(c: Connect, shutdown: &mut Shutdown) -> Result<i32> {
    let cfg = c.config;

    // Resolve endpoints before routes or DNS change.
    let mut peers = Vec::with_capacity(cfg.peers.len());
    let mut endpoints = Vec::new();
    for p in &cfg.peers {
        let endpoint = match &p.endpoint {
            Some(ep) => {
                let addr =
                    ep.resolve_preferred().await.with_context(|| format!("cannot resolve endpoint {ep}"))?;
                endpoints.push((ep.to_string(), addr));
                Some(addr)
            }
            None => None,
        };
        peers.push(PeerConfig {
            name: p.name.clone(),
            public_key: p.public_key,
            preshared_key: p.preshared_key,
            allowed_ips: p.allowed_ips.clone(),
            endpoint,
            persistent_keepalive: p.persistent_keepalive,
        });
    }
    let want_v4 = peers.iter().any(|p| p.endpoint.is_some_and(|e| e.is_ipv4()));
    let want_v6 = peers.iter().any(|p| p.endpoint.is_some_and(|e| e.is_ipv6()));

    let requested_name = c.interface.clone().unwrap_or_else(|| tun::CLIENT_IFNAME.to_string());
    let device = tun::create(&TunOptions {
        name: &requested_name,
        addresses: &cfg.interface.addresses,
        mtu: cfg.mtu(),
        guid: tun::CLIENT_GUID,
    })?;
    let ifname = device.name().unwrap_or(requested_name);
    let if_index = device.if_index().context("cannot read the tunnel interface index")?;
    let device = Arc::new(device);

    let sockets = Sockets::bind(cfg.interface.listen_port.unwrap_or(0), want_v4, want_v6)
        .context("cannot open a UDP socket")?;
    let started = Instant::now();
    let engine = Engine::start(&cfg.interface.private_key, peers.clone(), sockets, device.clone());

    // Record the session before changing the system, so a crash is recoverable.
    let token = keys::random_hex(32);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.context("cannot open the control port")?;
    let mut state = SessionState {
        mode: Mode::Client,
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: session::pid(),
        profile: c.name.clone(),
        interface: ifname.clone(),
        addresses: cfg.interface.addresses.clone(),
        control_port: listener.local_addr()?.port(),
        started_at: control::now_unix(),
        applied: Applied::default(),
        cloud: c.cloud.clone(),
    };
    state.save(&token)?;

    if let Some(max) = c.wait_for_server
        && !wait_for_handshake(&engine, max, shutdown, c.quiet).await
    {
        engine.shutdown().await;
        drop(device);
        SessionState::remove(Mode::Client);
        if shutdown.triggered() {
            return Ok(CANCELLED);
        }
        bail!("the server did not answer within {}", ui::duration(max.as_secs()));
    }

    let all_allowed: Vec<_> = cfg.peers.iter().flat_map(|p| p.allowed_ips.iter().copied()).collect();
    let plan = ClientPlan {
        interface: &ifname,
        if_index,
        allowed_ips: all_allowed.clone(),
        endpoints: endpoints.iter().map(|(_, a)| a.ip()).collect(),
        dns: &cfg.interface.dns,
        dns_search: &cfg.interface.dns_search,
        configure_dns: !c.no_dns,
    };
    let mut applied = Applied::default();
    let mut notes = Vec::new();
    let result = {
        let mut persist = |a: &Applied| {
            let mut s = state.clone();
            s.applied = a.clone();
            let _ = s.save_state_only();
        };
        netcfg::apply_client(&plan, &mut applied, &mut persist, &mut notes)
    };

    #[cfg(windows)]
    if result.is_ok() && !c.no_dns && !cfg.interface.dns.is_empty() {
        match device.set_dns_servers(&cfg.interface.dns) {
            Ok(()) => applied.dns = Some(netcfg::DnsRecord::WindowsAdapter),
            Err(e) => notes.push(format!("DNS was not configured: {e}")),
        }
    }

    state.applied = applied.clone();
    let _ = state.save_state_only();
    if let Err(e) = result {
        engine.shutdown().await;
        for w in netcfg::revert(&applied) {
            ui::warn(w);
        }
        drop(device);
        SessionState::remove(Mode::Client);
        return Err(e);
    }

    if !c.quiet {
        print_banner(&c.name, &ifname, &cfg, &endpoints, &applied, c.no_dns);
    }
    for n in notes {
        ui::warn(n);
    }

    session::run(
        Running {
            mode: Mode::Client,
            state,
            token,
            listener,
            engine,
            device,
            applied,
            quiet: c.quiet,
            started,
        },
        shutdown,
        || Err(anyhow!("reload is only available for server sessions")),
    )
    .await
}

/// Wait until the engine has a session with any peer. Returns false on timeout or Ctrl+C.
async fn wait_for_handshake(engine: &Engine, max: Duration, shutdown: &mut Shutdown, quiet: bool) -> bool {
    let start = Instant::now();
    let mut line = StatusLine::new(!quiet);
    if !line.is_enabled() && !quiet {
        println!("Waiting for the server to finish booting…");
    }
    loop {
        if engine.status().iter().any(PeerStatus::connected) {
            line.clear();
            return true;
        }
        if shutdown.triggered() || start.elapsed() >= max {
            line.clear();
            return false;
        }
        line.set(&format!(
            "  {}{} {}",
            ui::yellow(ui::WAIT),
            ui::yellow("waiting for the server to finish booting…"),
            ui::dim(ui::duration(start.elapsed().as_secs()))
        ));
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = shutdown.wait() => {}
        }
    }
}

fn print_banner(
    profile: &str,
    ifname: &str,
    cfg: &Config,
    endpoints: &[(String, std::net::SocketAddr)],
    applied: &Applied,
    no_dns: bool,
) {
    println!("{} {}{}profile {}", ui::bold("tvpn"), env!("CARGO_PKG_VERSION"), ui::DOT, ui::cyan(profile));
    let addrs: Vec<String> = cfg.interface.addresses.iter().map(ToString::to_string).collect();
    ui::kv("interface", format!("{ifname} ({})", addrs.join(", ")));
    for (name, addr) in endpoints {
        if name == &addr.to_string() {
            ui::kv("endpoint", name);
        } else {
            ui::kv("endpoint", format!("{name} {}", ui::dim(format!("({addr})"))));
        }
    }
    let allowed: Vec<_> = cfg.peers.iter().flat_map(|p| p.allowed_ips.iter().copied()).collect();
    let routes = if netcfg::is_full_tunnel(&allowed) {
        format!("all traffic {}", ui::dim(format!("({})", join(&allowed))))
    } else {
        join(&allowed)
    };
    ui::kv("routes", routes);
    let dns = if no_dns {
        ui::dim("unchanged (--no-dns)")
    } else if cfg.interface.dns.is_empty() {
        ui::dim("unchanged (profile has no DNS)")
    } else if applied.dns.is_some() {
        cfg.interface.dns.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    } else {
        ui::yellow("not configured")
    };
    ui::kv("dns", dns);
}

fn join(nets: &[ipnet::IpNet]) -> String {
    nets.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
