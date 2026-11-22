//! `tvpn status`.

use crate::cli::StatusArgs;
use crate::control::{self, LiveStatus, Mode, Request, SessionState};
use crate::ui;
use anyhow::Result;
use serde_json::json;

/// Exit code when nothing is connected (handy in scripts: `tvpn status >/dev/null && ...`).
pub const NOT_CONNECTED: i32 = 3;

pub async fn query(mode: Mode) -> Option<Result<LiveStatus, SessionState>> {
    let state = SessionState::load(mode)?;
    match control::request(state.control_port, &Request::Status).await {
        Ok(r) if r.ok => r.status.map(Ok),
        _ => Some(Err(state)),
    }
}

pub async fn run(args: StatusArgs) -> Result<i32> {
    let mut live = Vec::new();
    let mut stale = Vec::new();
    for mode in [Mode::Client, Mode::Server] {
        match query(mode).await {
            Some(Ok(s)) => live.push(s),
            Some(Err(state)) => stale.push(state),
            None => {}
        }
    }

    if args.json {
        let out = json!({
            "connected": live.iter().any(|s| s.mode == Mode::Client),
            "sessions": live,
            "stale": stale.iter().map(|s| json!({"mode": s.mode, "profile": s.profile, "pid": s.pid})).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(if live.is_empty() { NOT_CONNECTED } else { 0 });
    }

    for s in &stale {
        ui::warn(format!(
            "a {} session for `{}` (pid {}) ended without cleaning up; run `tvpn {}down` to restore network settings",
            s.mode.as_str(),
            s.profile,
            s.pid,
            if s.mode == Mode::Server { "server " } else { "" }
        ));
    }
    if live.is_empty() {
        println!("{}Not connected.", ui::OFF);
        return Ok(NOT_CONNECTED);
    }
    for (i, s) in live.iter().enumerate() {
        if i > 0 {
            println!();
        }
        match s.mode {
            Mode::Client => print_client(s),
            Mode::Server => print_server(s),
        }
    }
    Ok(0)
}

fn addrs(s: &LiveStatus) -> String {
    s.addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

fn print_client(s: &LiveStatus) {
    let connected = s.peers.iter().any(|p| p.connected());
    let head = if connected {
        format!("{}{}", ui::green(ui::ON), ui::green(ui::bold("Connected")))
    } else {
        format!("{}{}", ui::yellow(ui::WAIT), ui::yellow(ui::bold("Connecting")))
    };
    println!("{head}{}profile {}", ui::DOT, ui::cyan(&s.profile));
    ui::kv("interface", format!("{} ({})", s.interface, addrs(s)));
    ui::kv("uptime", ui::duration(s.uptime_secs));
    if let Some(server) = &s.cloud {
        ui::kv("cloud", super::cloud::describe(server));
    }
    for p in &s.peers {
        println!();
        ui::kv("peer", format!("{} {}", ui::bold(p.label()), ui::dim(short_key(&p.public_key))));
        if let Some(ep) = p.endpoint {
            ui::kv("endpoint", ep);
        }
        ui::kv("handshake", ui::ago(p.last_handshake_secs));
        ui::kv("transfer", ui::transfer(p.rx_bytes, p.tx_bytes));
        ui::kv("routes", p.allowed_ips.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "));
    }
}

fn print_server(s: &LiveStatus) {
    let online = s.peers.iter().filter(|p| p.connected()).count();
    println!(
        "{}{}{}{} of {} peers online",
        ui::green(ui::ON),
        ui::green(ui::bold("Server running")),
        ui::DOT,
        online,
        s.peers.len()
    );
    ui::kv("interface", format!("{} ({})", s.interface, addrs(s)));
    if let Some(port) = s.listen_port {
        ui::kv("listening", format!("UDP {port}"));
    }
    ui::kv("uptime", ui::duration(s.uptime_secs));
    if s.peers.is_empty() {
        println!();
        ui::hint("No peers yet. Add one with `tvpn server add-peer <name>`.");
        return;
    }
    println!();
    print_peer_table(&s.peers);
}

pub fn print_peer_table(peers: &[crate::engine::PeerStatus]) {
    let rows: Vec<(String, Vec<String>)> = peers
        .iter()
        .map(|p| {
            let state = if p.connected() { ui::green(ui::ON) } else { ui::dim(ui::OFF) };
            let addr = p.allowed_ips.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            let ep = p.endpoint.map(|e| e.to_string()).unwrap_or_else(|| "-".into());
            (
                state,
                vec![
                    p.label(),
                    addr,
                    ep,
                    ui::ago(p.last_handshake_secs),
                    ui::transfer(p.rx_bytes, p.tx_bytes),
                ],
            )
        })
        .collect();
    ui::table(&["PEER", "ADDRESS", "ENDPOINT", "HANDSHAKE", "TRANSFER"], &rows);
}

fn short_key(k: &str) -> String {
    format!("{}…", &k[..k.len().min(8)])
}
