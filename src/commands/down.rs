//! `tvpn down` / `tvpn server down`.

use super::session;
use crate::control::{self, Mode, Request, SessionState};
use crate::{privilege, ui};
use anyhow::{Result, bail};
use std::time::Duration;

pub async fn stop(mode: Mode, allow_sudo: bool) -> Result<i32> {
    let Some(state) = SessionState::load(mode) else {
        match mode {
            Mode::Client => println!("{}Not connected.", ui::OFF),
            Mode::Server => println!("{}The server is not running.", ui::OFF),
        }
        return Ok(0);
    };

    if control::is_alive(&state).await {
        privilege::ensure_elevated(allow_sudo)?;
        let token = SessionState::read_token(mode)?;
        let resp = control::request(state.control_port, &Request::Down { token }).await?;
        if !resp.ok {
            bail!("the session refused to stop: {}", resp.error.unwrap_or_default());
        }
        for _ in 0..150 {
            if !SessionState::exists(mode) {
                ui::success(match mode {
                    Mode::Client => format!("Disconnected from {}.", state.profile),
                    Mode::Server => "Server stopped.".to_string(),
                });
                return Ok(0);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!("the session (pid {}) did not stop within 15 seconds", state.pid);
    }

    privilege::ensure_elevated(allow_sudo)?;
    ui::warn(format!(
        "the {} session (pid {}) is not running anymore; restoring network settings it changed",
        mode.as_str(),
        state.pid
    ));
    session::cleanup_stale(&state).await;
    ui::success("Cleaned up.");
    Ok(0)
}
