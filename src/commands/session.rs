//! Runtime shared by `tvpn up` and `tvpn server up`: signal handling, the
//! control channel, the live status view and an orderly teardown.

use crate::control::{self, Command, LiveStatus, Mode, SessionState};
use crate::engine::{Engine, PeerConfig, PeerStatus};
use crate::netcfg::{self, Applied};
use crate::ui::{self, StatusLine};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

/// Ctrl+C / SIGTERM / SIGHUP / console close, installed as early as possible so
/// that an interrupt during setup still leads to a clean teardown.
pub struct Shutdown {
    rx: watch::Receiver<bool>,
}

impl Shutdown {
    pub fn install() -> Self {
        let (tx, rx) = watch::channel(false);
        tokio::spawn(async move {
            wait_for_signal().await;
            let _ = tx.send(true);
        });
        Shutdown { rx }
    }

    pub fn triggered(&self) -> bool {
        *self.rx.borrow()
    }

    pub async fn wait(&mut self) {
        while !*self.rx.borrow() {
            if self.rx.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut hup = signal(SignalKind::hangup()).expect("SIGHUP handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
        _ = hup.recv() => {}
    }
}

#[cfg(windows)]
async fn wait_for_signal() {
    use tokio::signal::windows;
    let mut brk = windows::ctrl_break().expect("Ctrl+Break handler");
    let mut close = windows::ctrl_close().expect("console close handler");
    let mut shutdown = windows::ctrl_shutdown().expect("shutdown handler");
    let mut logoff = windows::ctrl_logoff().expect("logoff handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = brk.recv() => {}
        _ = close.recv() => {}
        _ = shutdown.recv() => {}
        _ = logoff.recv() => {}
    }
}

/// Everything needed to run an established session.
pub struct Running {
    pub mode: Mode,
    pub state: SessionState,
    pub token: String,
    pub listener: TcpListener,
    pub engine: Engine,
    pub device: Arc<tun_rs::AsyncDevice>,
    pub applied: Applied,
    pub quiet: bool,
    pub started: Instant,
}

/// Run until interrupted, `tvpn down`, or an engine failure; then tear down.
pub async fn run<R>(mut s: Running, shutdown: &mut Shutdown, mut reload: R) -> Result<i32>
where
    R: FnMut() -> Result<Vec<PeerConfig>>,
{
    let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
    let handle = s.engine.handle();
    let status_fn = {
        let handle = handle.clone();
        let state = s.state.clone();
        let started = s.started;
        let port = s.engine.local_port();
        move || LiveStatus {
            mode: state.mode,
            version: state.version.clone(),
            profile: state.profile.clone(),
            interface: state.interface.clone(),
            addresses: state.addresses.clone(),
            listen_port: port,
            uptime_secs: started.elapsed().as_secs(),
            peers: handle.status(),
            cloud: state.cloud.clone(),
        }
    };
    let ctl = tokio::spawn(control::serve(s.listener, s.token.clone(), status_fn, cmd_tx));

    let mut view = LiveView::new(s.mode, s.quiet, s.started);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut exit_code = 0;
    let reason = loop {
        if shutdown.triggered() {
            break "interrupted";
        }
        tokio::select! {
            _ = shutdown.wait() => break "interrupted",
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::Down) => break "stopped by `tvpn down`",
                Some(Command::Reload(reply)) => {
                    let result = match reload() {
                        Ok(peers) => {
                            let (added, removed) = handle.set_peers(peers);
                            let msg = format!("reloaded peers: {added} added, {removed} removed");
                            view.event(format!("{}{msg}", ui::OK));
                            Ok(msg)
                        }
                        Err(e) => Err(format!("{e:#}")),
                    };
                    let _ = reply.send(result);
                }
                None => {}
            },
            msg = s.engine.failed() => {
                view.clear();
                ui::error(&msg);
                exit_code = 1;
                break "error";
            }
            _ = tick.tick() => view.update(&handle.status()),
        }
    };

    // Teardown: stop traffic, undo system changes, then close the interface.
    view.clear();
    let final_peers = handle.status();
    if !s.quiet {
        println!("{}", ui::dim(format!("Disconnecting ({reason})…")));
    }
    ctl.abort();
    s.engine.shutdown().await;
    let warnings = netcfg::revert(&s.applied);
    drop(s.device);
    SessionState::remove(s.mode);
    for w in warnings {
        ui::warn(w);
    }
    let (rx, tx) = final_peers.iter().fold((0, 0), |(r, t), p| (r + p.rx_bytes, t + p.tx_bytes));
    let label = if s.mode == Mode::Client { "Disconnected." } else { "Server stopped." };
    ui::success(format!(
        "{label} {}",
        ui::dim(format!(
            "Session {}{}{}",
            ui::duration(s.started.elapsed().as_secs()),
            ui::DOT,
            ui::transfer(rx, tx)
        ))
    ));
    Ok(exit_code)
}

/// Turns periodic peer snapshots into a status line plus event messages.
struct LiveView {
    mode: Mode,
    line: StatusLine,
    quiet: bool,
    started: Instant,
    connected_once: bool,
    connected: bool,
    warned_slow: bool,
    peers_online: HashMap<String, bool>,
}

impl LiveView {
    fn new(mode: Mode, quiet: bool, started: Instant) -> Self {
        LiveView {
            mode,
            line: StatusLine::new(!quiet),
            quiet,
            started,
            connected_once: false,
            connected: false,
            warned_slow: false,
            peers_online: HashMap::new(),
        }
    }

    fn event(&mut self, text: String) {
        if self.line.is_enabled() {
            self.line.println(text);
        } else if !self.quiet {
            println!("{text}");
        }
    }

    fn clear(&mut self) {
        self.line.clear();
    }

    fn update(&mut self, peers: &[PeerStatus]) {
        match self.mode {
            Mode::Client => self.update_client(peers),
            Mode::Server => self.update_server(peers),
        }
    }

    fn update_client(&mut self, peers: &[PeerStatus]) {
        let elapsed = self.started.elapsed();
        let connected = peers.iter().any(PeerStatus::connected);
        if connected && !self.connected {
            if self.connected_once {
                self.event(format!("{}Reconnected", ui::OK));
            } else {
                self.event(format!(
                    "{}{} {}",
                    ui::OK,
                    ui::green(ui::bold("Connected")),
                    ui::dim(format!("(handshake after {:.1}s)", elapsed.as_secs_f32()))
                ));
            }
            self.connected_once = true;
        } else if !connected && self.connected {
            self.event(format!("{}{}", ui::WARN, ui::yellow("Connection lost, retrying…")));
        }
        self.connected = connected;

        if !self.connected_once && !self.warned_slow && elapsed > Duration::from_secs(12) {
            self.warned_slow = true;
            self.line.clear();
            ui::warn("no handshake yet. Check that:");
            ui::hint(
                "- the server is running and its UDP port is reachable (firewall / cloud security group)",
            );
            ui::hint("- the keys in this profile match the server's peer entry");
            ui::hint("- your network does not block outgoing UDP");
        }

        let (rx, tx) = peers.iter().fold((0, 0), |(r, t), p| (r + p.rx_bytes, t + p.tx_bytes));
        let text = if connected {
            let hs = peers.iter().filter_map(|p| p.last_handshake_secs).min();
            format!(
                "{}{}{}{}{}handshake {}{}up {}",
                ui::green(ui::ON),
                ui::green("connected"),
                ui::DOT,
                ui::transfer(rx, tx),
                ui::DOT,
                ui::ago(hs),
                ui::DOT,
                ui::duration(elapsed.as_secs())
            )
        } else {
            format!(
                "{}{} {}",
                ui::yellow(ui::WAIT),
                ui::yellow(if self.connected_once { "reconnecting…" } else { "connecting…" }),
                ui::dim(ui::duration(elapsed.as_secs()))
            )
        };
        self.line.set(&format!("  {text}  {}", ui::dim("Ctrl+C to disconnect")));
    }

    fn update_server(&mut self, peers: &[PeerStatus]) {
        for p in peers {
            let now = p.connected();
            let before = self.peers_online.insert(p.public_key.clone(), now).unwrap_or(false);
            if now && !before {
                let from = p.endpoint.map(|e| format!(" from {e}")).unwrap_or_default();
                self.event(format!(
                    "{}{} connected{}",
                    ui::green(ui::ON),
                    ui::bold(p.label()),
                    ui::dim(from)
                ));
            } else if !now && before {
                self.event(format!("{}{} disconnected", ui::OFF, ui::bold(p.label())));
            }
        }
        self.peers_online.retain(|k, _| peers.iter().any(|p| &p.public_key == k));
        let online = peers.iter().filter(|p| p.connected()).count();
        let (rx, tx) = peers.iter().fold((0, 0), |(r, t), p| (r + p.rx_bytes, t + p.tx_bytes));
        let text = format!(
            "{}{}/{} peers online{}{}{}up {}",
            if online > 0 { ui::green(ui::ON) } else { ui::dim(ui::OFF) },
            online,
            peers.len(),
            ui::DOT,
            ui::transfer(rx, tx),
            ui::DOT,
            ui::duration(self.started.elapsed().as_secs())
        );
        self.line.set(&format!("  {text}  {}", ui::dim("Ctrl+C to stop")));
    }
}

/// Clean up after a session that died without tearing down: restore network
/// settings and delete its cloud server, if it had one.
pub async fn cleanup_stale(state: &SessionState) {
    for w in netcfg::revert(&state.applied) {
        ui::warn(w);
    }
    if let Some(server) = state.cloud.clone() {
        let id = server.instance_id.clone();
        match tokio::task::spawn_blocking(move || server.terminate()).await {
            Ok(Ok(())) => ui::success(format!("Deleted its cloud server {id}")),
            Ok(Err(e)) => ui::warn(format!(
                "could not delete cloud server {id}: {e:#}\n  run `tvpn cloud cleanup` to retry"
            )),
            Err(e) => ui::warn(format!("could not delete cloud server {id}: {e}")),
        }
    }
    SessionState::remove(state.mode);
}

pub fn pid() -> u32 {
    std::process::id()
}
