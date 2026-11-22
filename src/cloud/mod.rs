//! `tvpn cloud`: on-demand VPN servers in the region of your choice.
//!
//! `tvpn cloud up <region>` launches a small server on the user's own AWS
//! account, installs tvpn on it through first-boot user data, connects to it,
//! and terminates it on disconnect. Two server-side safety nets (idle and
//! maximum-lifetime shutdown) make sure a forgotten server stops costing money.

pub mod aws;
pub mod bootstrap;
pub mod regions;
pub mod sigv4;
pub mod xml;

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// A server created by `tvpn cloud up`, recorded in the session state so a
/// crashed session can still be cleaned up by `tvpn down`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudServer {
    pub provider: String,
    pub region: String,
    pub instance_id: String,
    pub public_ip: Option<IpAddr>,
    pub launched_at: u64,
}

impl CloudServer {
    /// Terminate the server (blocking network call).
    pub fn terminate(&self) -> anyhow::Result<()> {
        let creds = aws::Credentials::load()?;
        aws::Aws::new(creds, &self.region).terminate(&self.instance_id)
    }
}

/// GitHub `owner/repo` the server downloads tvpn from, derived from Cargo.toml.
pub fn release_repo() -> &'static str {
    env!("CARGO_PKG_REPOSITORY").trim_start_matches("https://github.com/").trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    #[test]
    fn repo_slug() {
        let repo = super::release_repo();
        assert_eq!(repo.split('/').count(), 2, "{repo}");
        assert!(!repo.contains("github.com"));
    }
}
