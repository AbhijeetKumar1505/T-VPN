//! Live session bookkeeping and the local control channel.
//!
//! A running `tvpn up` / `tvpn server up` writes `<runtime>/<mode>.json`
//! describing the session and every system change it made (so a crashed
//! session can still be cleaned up by `tvpn down`), plus a secret token in
//! `<mode>.token` (root/admin only). It listens on 127.0.0.1 for line-delimited
//! JSON requests: `status` is open to local users, `down`/`reload` need the token.

use crate::engine::PeerStatus;
use crate::netcfg::Applied;
use crate::paths;
use anyhow::{Context, Result, anyhow};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Client,
    Server,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Client => "client",
            Mode::Server => "server",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub mode: Mode,
    pub version: String,
    pub pid: u32,
    pub profile: String,
    pub interface: String,
    pub addresses: Vec<IpNet>,
    pub control_port: u16,
    pub started_at: u64,
    #[serde(default)]
    pub applied: Applied,
}

fn state_path(mode: Mode) -> PathBuf {
    paths::runtime_dir().join(format!("{}.json", mode.as_str()))
}

fn token_path(mode: Mode) -> PathBuf {
    paths::runtime_dir().join(format!("{}.token", mode.as_str()))
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl SessionState {
    pub fn load(mode: Mode) -> Option<SessionState> {
        let text = fs::read_to_string(state_path(mode)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Persist state (world-readable) and token (owner-only).
    pub fn save(&self, token: &str) -> Result<()> {
        let dir = paths::runtime_dir();
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let json = serde_json::to_string_pretty(self)?;
        let tmp = state_path(self.mode).with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, state_path(self.mode))?;
        write_token(&token_path(self.mode), token)?;
        Ok(())
    }

    /// Update just the recorded system changes (as they are applied).
    pub fn save_state_only(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        let tmp = state_path(self.mode).with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, state_path(self.mode))?;
        Ok(())
    }

    pub fn remove(mode: Mode) {
        let _ = fs::remove_file(state_path(mode));
        let _ = fs::remove_file(token_path(mode));
    }

    pub fn read_token(mode: Mode) -> Result<String> {
        fs::read_to_string(token_path(mode))
            .map(|t| t.trim().to_string())
            .map_err(|e| anyhow!("cannot read session token ({e}); are you root/administrator?"))
    }

    pub fn exists(mode: Mode) -> bool {
        state_path(mode).exists()
    }
}

fn write_token(path: &std::path::Path, token: &str) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).with_context(|| format!("cannot write {}", path.display()))?;
    std::io::Write::write_all(&mut f, token.as_bytes())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    Status,
    Down { token: String },
    Reload { token: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<LiveStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Response { ok: true, ..Default::default() }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Response { ok: false, error: Some(msg.into()), ..Default::default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStatus {
    pub mode: Mode,
    pub version: String,
    pub profile: String,
    pub interface: String,
    pub addresses: Vec<IpNet>,
    pub listen_port: Option<u16>,
    pub uptime_secs: u64,
    pub peers: Vec<PeerStatus>,
}

/// Commands that the session's main loop must act on.
pub enum Command {
    Down,
    Reload(oneshot::Sender<Result<String, String>>),
}

/// Serve control requests until the listener is dropped.
pub async fn serve<F>(listener: TcpListener, token: String, status: F, commands: mpsc::Sender<Command>)
where
    F: Fn() -> LiveStatus + Send + Sync + Clone + 'static,
{
    loop {
        let Ok((stream, peer)) = listener.accept().await else { continue };
        if !peer.ip().is_loopback() {
            continue;
        }
        let token = token.clone();
        let status = status.clone();
        let commands = commands.clone();
        tokio::spawn(async move {
            let _ =
                tokio::time::timeout(Duration::from_secs(10), handle_conn(stream, &token, status, commands))
                    .await;
        });
    }
}

async fn handle_conn<F>(
    stream: TcpStream,
    token: &str,
    status: F,
    commands: mpsc::Sender<Command>,
) -> Result<()>
where
    F: Fn() -> LiveStatus,
{
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read.take(64 * 1024)).read_line(&mut line).await?;
    let resp = match serde_json::from_str::<Request>(line.trim()) {
        Err(e) => Response::err(format!("bad request: {e}")),
        Ok(Request::Status) => Response { ok: true, status: Some(status()), ..Default::default() },
        Ok(Request::Down { token: t }) => {
            if !constant_time_eq(t.as_bytes(), token.as_bytes()) {
                Response::err("invalid token")
            } else {
                let _ = commands.send(Command::Down).await;
                Response::ok()
            }
        }
        Ok(Request::Reload { token: t }) => {
            if !constant_time_eq(t.as_bytes(), token.as_bytes()) {
                Response::err("invalid token")
            } else {
                let (tx, rx) = oneshot::channel();
                let _ = commands.send(Command::Reload(tx)).await;
                match rx.await {
                    Ok(Ok(msg)) => Response { ok: true, message: Some(msg), ..Default::default() },
                    Ok(Err(e)) => Response::err(e),
                    Err(_) => Response::err("session is shutting down"),
                }
            }
        }
    };
    let mut out = serde_json::to_vec(&resp)?;
    out.push(b'\n');
    write.write_all(&out).await?;
    write.shutdown().await.ok();
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Send a request to a running session.
pub async fn request(port: u16, req: &Request) -> Result<Response> {
    let fut = async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        let mut body = serde_json::to_vec(req)?;
        body.push(b'\n');
        stream.write_all(&body).await?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).await?;
        let resp: Response = serde_json::from_str(line.trim())?;
        anyhow::Ok(resp)
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .map_err(|_| anyhow!("timed out talking to the tvpn session"))?
}

/// Is the recorded session actually running (answering on its control port)?
pub async fn is_alive(state: &SessionState) -> bool {
    request(state.control_port, &Request::Status)
        .await
        .map(|r| r.ok && r.status.is_some_and(|s| s.profile == state.profile))
        .unwrap_or(false)
}
