//! `tvpn doctor`: explain what works on this machine and what to fix.

use crate::control::{self, Mode, SessionState};
use crate::server::ServerStore;
use crate::{paths, privilege, profiles, ui};
use anyhow::Result;

struct Report {
    problems: usize,
}

impl Report {
    fn ok(&self, msg: impl std::fmt::Display) {
        println!("{}{msg}", ui::green(ui::OK));
    }
    fn warn(&self, msg: impl std::fmt::Display) {
        println!("{}{msg}", ui::yellow(ui::WARN));
    }
    fn bad(&mut self, msg: impl std::fmt::Display) {
        self.problems += 1;
        println!("{}{msg}", console::style(ui::ERR).red());
    }
    fn info(&self, msg: impl std::fmt::Display) {
        println!("{}{msg}", ui::dim(ui::OFF));
    }
}

pub async fn run() -> Result<i32> {
    let mut r = Report { problems: 0 };
    println!(
        "{} {} {}",
        ui::bold("tvpn"),
        env!("CARGO_PKG_VERSION"),
        ui::dim(format!("({} {}, {})", std::env::consts::OS, std::env::consts::ARCH, os_description()))
    );
    println!();

    if privilege::is_elevated() {
        r.ok(if cfg!(windows) { "running as administrator" } else { "running as root" });
    } else if cfg!(windows) {
        r.warn("not elevated: `tvpn up` must be run from an administrator terminal");
    } else {
        r.warn("not root: `tvpn up` will re-run itself with sudo");
    }

    platform_checks(&mut r);

    let profiles = profiles::list().unwrap_or_default();
    r.ok(format!(
        "config directory {} ({} profile{})",
        paths::config_dir().display(),
        profiles.len(),
        if profiles.len() == 1 { "" } else { "s" }
    ));
    match super::cloud::credentials_summary() {
        Some(source) => r.ok(format!("cloud: AWS credentials from {source}")),
        None => r.info("cloud: not set up (optional: `tvpn cloud setup` to change region on demand)"),
    }
    let store = ServerStore::default_location();
    match store.load() {
        Ok((cfg, meta)) => {
            r.ok(format!("server configured: endpoint {}, {} peer(s)", meta.endpoint, cfg.peers.len()))
        }
        Err(_) if !store.exists() => r.info("no server configured (optional: `tvpn server init`)"),
        Err(e) => r.bad(format!("server config is invalid: {e:#}")),
    }

    for mode in [Mode::Client, Mode::Server] {
        if let Some(state) = SessionState::load(mode) {
            if control::is_alive(&state).await {
                r.ok(format!(
                    "{} session active ({}, interface {})",
                    mode.as_str(),
                    state.profile,
                    state.interface
                ));
            } else {
                r.bad(format!(
                    "stale {} session from pid {}: run `tvpn {}down` to restore network settings",
                    mode.as_str(),
                    state.pid,
                    if mode == Mode::Server { "server " } else { "" }
                ));
            }
        }
    }

    println!();
    if r.problems == 0 {
        ui::success("Everything looks good.");
        Ok(0)
    } else {
        ui::error(format!("{} problem(s) found.", r.problems));
        Ok(1)
    }
}

#[cfg(target_os = "linux")]
fn os_description() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        })
        .unwrap_or_else(|| "Linux".into())
}

#[cfg(target_os = "macos")]
fn os_description() -> String {
    crate::netcfg::run("sw_vers", &["-productVersion"])
        .map(|v| format!("macOS {}", v.trim()))
        .unwrap_or_else(|_| "macOS".into())
}

#[cfg(windows)]
fn os_description() -> String {
    "Windows".into()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn os_description() -> String {
    std::env::consts::OS.into()
}

#[cfg(target_os = "linux")]
fn platform_checks(r: &mut Report) {
    use std::path::Path;
    let tun = Path::new("/dev/net/tun");
    if tun.exists() {
        r.ok("TUN driver available (/dev/net/tun)");
    } else {
        r.bad("/dev/net/tun is missing: load the module with `sudo modprobe tun`");
    }

    let resolv = Path::new("/etc/resolv.conf");
    let resolved_link = std::fs::read_link(resolv).ok();
    if privilege::which("resolvectl").is_some()
        && resolved_link.as_ref().is_some_and(|t| t.to_string_lossy().contains("systemd/resolve"))
    {
        r.ok("DNS: systemd-resolved (configured per interface with resolvectl)");
    } else if let Some(target) = resolved_link {
        r.ok(format!(
            "DNS: /etc/resolv.conf -> {} (replaced while connected, then restored)",
            target.display()
        ));
    } else {
        r.ok("DNS: /etc/resolv.conf (replaced while connected, then restored)");
    }

    if privilege::which("iptables").is_some() {
        r.ok("iptables found (used by `tvpn server up` for NAT)");
    } else {
        r.info("iptables not found: only needed to run a server with internet access for peers");
    }

    match std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward") {
        Ok(v) if v.trim() == "1" => r.ok("IPv4 forwarding enabled"),
        Ok(_) => r.info("IPv4 forwarding off (`tvpn server up` turns it on while running)"),
        Err(_) => {}
    }

    let version = std::fs::read_to_string("/proc/version").unwrap_or_default().to_lowercase();
    if version.contains("microsoft") {
        r.warn("WSL detected: a tunnel started here only affects the WSL VM, not Windows apps");
    }
}

#[cfg(target_os = "macos")]
fn platform_checks(r: &mut Report) {
    if privilege::which("networksetup").is_some() {
        r.ok("networksetup found (used for DNS)");
    } else {
        r.warn("networksetup not found: DNS will not be configured");
    }
    r.ok("utun driver is built into macOS");
}

#[cfg(windows)]
fn platform_checks(r: &mut Report) {
    match crate::wintun::find_local() {
        Some(p) => r.ok(format!("Wintun driver found ({})", p.display())),
        None => r.info(format!(
            "Wintun driver {} not installed yet: it is downloaded and verified on first `tvpn up`",
            crate::wintun::WINTUN_VERSION
        )),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_checks(r: &mut Report) {
    r.warn("this OS is not officially supported");
}
