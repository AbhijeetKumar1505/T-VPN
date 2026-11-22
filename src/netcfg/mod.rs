//! System network configuration: routes, DNS, forwarding and NAT.
//!
//! Every change is recorded in [`Applied`] *as it is made* and persisted in the
//! session state file, so it can be reverted on disconnect, on error halfway
//! through setup, or by `tvpn down` after a crash.

use anyhow::{Context, Result, bail};
use ipnet::IpNet;
use route_manager::{Route, RouteManager};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::process::Command;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Applied {
    #[serde(default)]
    pub routes: Vec<RouteRecord>,
    #[serde(default)]
    pub dns: Option<DnsRecord>,
    #[serde(default)]
    pub sysctls: Vec<SysctlRecord>,
    #[serde(default)]
    pub firewall: Vec<FirewallRule>,
}

impl Applied {
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && self.dns.is_none() && self.sysctls.is_empty() && self.firewall.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRecord {
    pub dest: IpAddr,
    pub prefix: u8,
    pub gateway: Option<IpAddr>,
    pub if_index: Option<u32>,
}

impl RouteRecord {
    fn to_route(&self) -> Route {
        let mut r = Route::new(self.dest, self.prefix);
        if let Some(gw) = self.gateway {
            r = r.with_gateway(gw);
        }
        if let Some(idx) = self.if_index {
            r = r.with_if_index(idx);
        }
        r
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DnsRecord {
    /// systemd-resolved per-link DNS (reverted with `resolvectl revert`).
    Resolved { interface: String },
    /// /etc/resolv.conf replaced; original saved at `backup` (or was a symlink).
    ResolvConf { backup: PathBuf, symlink_target: Option<PathBuf> },
    /// macOS: per network service DNS settings that were replaced.
    MacServices { services: Vec<MacDnsBackup> },
    /// Windows: DNS set on the tunnel adapter; disappears with the adapter.
    WindowsAdapter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacDnsBackup {
    pub service: String,
    pub servers: Vec<String>,
    pub search: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysctlRecord {
    pub path: PathBuf,
    pub previous: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirewallRule {
    pub program: String,
    pub table: String,
    pub chain: String,
    pub spec: Vec<String>,
}

pub struct ClientPlan<'a> {
    pub interface: &'a str,
    pub if_index: u32,
    pub allowed_ips: Vec<IpNet>,
    pub endpoints: Vec<IpAddr>,
    pub dns: &'a [IpAddr],
    pub dns_search: &'a [String],
    pub configure_dns: bool,
}

pub struct ServerPlan<'a> {
    pub interface: &'a str,
    pub subnets: Vec<IpNet>,
    pub listen_port: u16,
    pub nat: bool,
    pub wan: Option<String>,
    pub firewall: bool,
}

/// Does this set of AllowedIPs capture all traffic?
pub fn is_full_tunnel(allowed: &[IpNet]) -> bool {
    allowed.iter().any(|n| n.prefix_len() == 0)
}

/// Routes to install for a set of AllowedIPs. A default route is installed as
/// two halves (0/1 + 128/1), which beats the system default route by prefix
/// length without having to remove or modify it.
pub fn expand_routes(allowed: &[IpNet]) -> Vec<IpNet> {
    let mut out: Vec<IpNet> = Vec::new();
    for net in allowed.iter().map(IpNet::trunc) {
        let parts: Vec<IpNet> = match net {
            IpNet::V4(n) if n.prefix_len() == 0 => vec![
                IpNet::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 1).expect("valid"),
                IpNet::new(IpAddr::V4(Ipv4Addr::new(128, 0, 0, 0)), 1).expect("valid"),
            ],
            IpNet::V6(n) if n.prefix_len() == 0 => vec![
                IpNet::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 1).expect("valid"),
                IpNet::new(IpAddr::V6(Ipv6Addr::new(0x8000, 0, 0, 0, 0, 0, 0, 0)), 1).expect("valid"),
            ],
            other => vec![other],
        };
        for p in parts {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

fn host_prefix(ip: &IpAddr) -> u8 {
    if ip.is_ipv4() { 32 } else { 128 }
}

fn already_exists(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::AlreadyExists {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(code) => code == libc::EEXIST,
        #[cfg(windows)]
        Some(code) => code == 5010 || code == 183, // ERROR_OBJECT_ALREADY_EXISTS, ERROR_ALREADY_EXISTS
        None => false,
    }
}

fn already_gone(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::NotFound {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(code) => code == libc::ESRCH || code == libc::ENODEV || code == libc::ENOENT,
        #[cfg(windows)]
        Some(code) => code == 1168 || code == 2 || code == 1910, // ERROR_NOT_FOUND, FILE_NOT_FOUND, ERROR_OBJECT_NOT_FOUND
        None => false,
    }
}

/// The route the system currently uses to reach `dest`, ignoring our tunnel.
fn original_route(mgr: &mut RouteManager, dest: IpAddr, tun_index: u32) -> io::Result<Option<Route>> {
    #[cfg(windows)]
    {
        let r = mgr.find_route(&dest)?;
        if r.as_ref().is_some_and(|r| r.if_index() != Some(tun_index)) {
            return Ok(r);
        }
    }
    let mut candidates: Vec<Route> = mgr
        .list()?
        .into_iter()
        .filter(|r| r.destination().is_ipv4() == dest.is_ipv4())
        .filter(|r| r.contains(&dest))
        .filter(|r| r.if_index() != Some(tun_index))
        .filter(|r| r.gateway().is_some() || r.if_index().is_some())
        .collect();
    #[cfg(target_os = "linux")]
    candidates.retain(|r| r.table() == 254 && r.kind() == route_manager::RouteKind::Unicast);
    #[cfg(target_os = "macos")]
    {
        candidates.retain(|r| r.kind() == route_manager::RouteKind::Unicast);
        if candidates.iter().any(|r| !r.if_scope()) {
            candidates.retain(|r| !r.if_scope());
        }
    }
    candidates.sort();
    Ok(candidates.pop())
}

fn add_route(mgr: &mut RouteManager, rec: &RouteRecord) -> io::Result<bool> {
    match mgr.add(&rec.to_route()) {
        Ok(()) => Ok(true),
        Err(e) if already_exists(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Configure routes and DNS for a client session.
pub fn apply_client(
    plan: &ClientPlan<'_>,
    applied: &mut Applied,
    persist: &mut dyn FnMut(&Applied),
    notes: &mut Vec<String>,
) -> Result<()> {
    let routes = expand_routes(&plan.allowed_ips);
    let mut mgr = RouteManager::new().context("cannot open the routing table")?;

    // 1. Keep the endpoint reachable outside the tunnel. Must be looked up
    //    before the tunnel routes exist.
    let mut bypass = Vec::new();
    for ep in &plan.endpoints {
        if !routes.iter().any(|n| n.contains(ep)) {
            continue;
        }
        match original_route(&mut mgr, *ep, plan.if_index) {
            Ok(Some(r)) => bypass.push(RouteRecord {
                dest: *ep,
                prefix: host_prefix(ep),
                gateway: r.gateway(),
                if_index: r.if_index(),
            }),
            Ok(None) => notes.push(format!("no existing route to endpoint {ep}; it may be unreachable")),
            Err(e) => notes.push(format!("could not look up the route to {ep}: {e}")),
        }
    }
    for rec in bypass {
        match add_route(&mut mgr, &rec) {
            Ok(true) => {
                applied.routes.push(rec);
                persist(applied);
            }
            Ok(false) => {}
            Err(e) => bail!("cannot add route to endpoint {}: {e}", rec.dest),
        }
    }

    // 2. Send AllowedIPs into the tunnel.
    for net in routes {
        let rec = RouteRecord {
            dest: net.network(),
            prefix: net.prefix_len(),
            gateway: None,
            if_index: Some(plan.if_index),
        };
        match add_route(&mut mgr, &rec) {
            Ok(true) => {
                applied.routes.push(rec);
                persist(applied);
            }
            Ok(false) => {}
            Err(e) if net.addr().is_ipv6() => {
                notes.push(format!("could not route {net} through the tunnel ({e}); IPv6 may be disabled"))
            }
            Err(e) => bail!("cannot add route {net} via {}: {e}", plan.interface),
        }
    }

    // 3. DNS (failures are warnings: the tunnel itself works without it).
    if plan.configure_dns && !plan.dns.is_empty() {
        let full = is_full_tunnel(&plan.allowed_ips);
        match apply_dns(plan, full) {
            Ok(Some(rec)) => {
                applied.dns = Some(rec);
                persist(applied);
            }
            Ok(None) => {}
            Err(e) => notes.push(format!("DNS was not configured: {e:#}")),
        }
    }
    Ok(())
}

#[allow(unused_variables)]
fn apply_dns(plan: &ClientPlan<'_>, full_tunnel: bool) -> Result<Option<DnsRecord>> {
    #[cfg(target_os = "linux")]
    {
        linux::apply_dns(plan.interface, plan.dns, plan.dns_search, full_tunnel).map(Some)
    }
    #[cfg(target_os = "macos")]
    {
        macos::apply_dns(plan.dns, plan.dns_search).map(Some)
    }
    #[cfg(windows)]
    {
        // Applied directly on the adapter by the caller (needs the device handle).
        Ok(None)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        anyhow::bail!("automatic DNS configuration is not supported on this OS")
    }
}

/// Configure forwarding, firewall and NAT for a server session.
#[allow(unused_variables)]
pub fn apply_server(
    plan: &ServerPlan<'_>,
    applied: &mut Applied,
    persist: &mut dyn FnMut(&Applied),
    notes: &mut Vec<String>,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::apply_server(plan, applied, persist, notes)
    }
    #[cfg(not(target_os = "linux"))]
    {
        if plan.nat {
            notes.push(
                "forwarding/NAT is only automated on Linux: peers can reach this machine, \
                 but not the internet through it"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Undo everything recorded in `applied`. Returns warnings for anything that
/// could not be reverted; already-removed items are silently skipped.
pub fn revert(applied: &Applied) -> Vec<String> {
    let mut warnings = Vec::new();

    if let Some(dns) = &applied.dns {
        revert_dns(dns, &mut warnings);
    }

    if !applied.routes.is_empty() {
        match RouteManager::new() {
            Ok(mut mgr) => {
                for rec in applied.routes.iter().rev() {
                    if let Err(e) = mgr.delete(&rec.to_route())
                        && !already_gone(&e)
                        && rec.gateway.is_some()
                    {
                        warnings.push(format!("could not remove route {}/{}: {e}", rec.dest, rec.prefix));
                    }
                }
            }
            Err(e) => warnings.push(format!("cannot open the routing table: {e}")),
        }
    }

    for rule in applied.firewall.iter().rev() {
        let mut args: Vec<&str> = vec!["-t", &rule.table, "-D", &rule.chain];
        args.extend(rule.spec.iter().map(String::as_str));
        if let Err(e) = run(&rule.program, &args) {
            let msg = format!("{e:#}");
            if !msg.contains("does a matching rule exist") && !msg.contains("Bad rule") {
                warnings.push(format!("could not remove firewall rule: {msg}"));
            }
        }
    }

    for s in applied.sysctls.iter().rev() {
        if let Err(e) = std::fs::write(&s.path, &s.previous) {
            warnings.push(format!("could not restore {}: {e}", s.path.display()));
        }
    }
    warnings
}

#[allow(unused_variables)]
fn revert_dns(rec: &DnsRecord, warnings: &mut Vec<String>) {
    match rec {
        DnsRecord::WindowsAdapter => {}
        #[cfg(target_os = "linux")]
        DnsRecord::Resolved { .. } | DnsRecord::ResolvConf { .. } => linux::revert_dns(rec, warnings),
        #[cfg(target_os = "macos")]
        DnsRecord::MacServices { .. } => macos::revert_dns(rec, warnings),
        #[allow(unreachable_patterns)]
        _ => warnings.push("DNS change recorded by another OS cannot be reverted here".into()),
    }
}

/// Run a command, returning stdout, or an error including stderr.
pub(crate) fn run(program: &str, args: &[&str]) -> Result<String> {
    let out =
        Command::new(program).args(args).output().with_context(|| format!("failed to run `{program}`"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let detail =
            if stderr.trim().is_empty() { stdout.trim().to_string() } else { stderr.trim().to_string() };
        bail!("`{program} {}` failed: {detail}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Name of the interface carrying the default route (used as the NAT uplink).
pub fn default_route_interface() -> Option<String> {
    let mut mgr = RouteManager::new().ok()?;
    let probe = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
    let route = original_route(&mut mgr, probe, u32::MAX).ok()??;
    if let Some(name) = route.if_name() {
        return Some(name.clone());
    }
    interface_name(route.if_index()?)
}

#[cfg(unix)]
pub fn interface_name(index: u32) -> Option<String> {
    let mut buf = [0 as libc::c_char; libc::IF_NAMESIZE];
    let p = unsafe { libc::if_indextoname(index, buf.as_mut_ptr()) };
    if p.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(name.to_string_lossy().into_owned())
}

#[cfg(windows)]
pub fn interface_name(index: u32) -> Option<String> {
    Some(format!("interface #{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(v: &[&str]) -> Vec<IpNet> {
        v.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn default_routes_are_split() {
        let r = expand_routes(&nets(&["0.0.0.0/0", "::/0"]));
        assert_eq!(r, nets(&["0.0.0.0/1", "128.0.0.0/1", "::/1", "8000::/1"]));
    }

    #[test]
    fn routes_are_truncated_and_deduped() {
        let r = expand_routes(&nets(&["10.1.2.3/8", "10.0.0.0/8", "192.168.5.0/24"]));
        assert_eq!(r, nets(&["10.0.0.0/8", "192.168.5.0/24"]));
    }

    #[test]
    fn full_tunnel_detection() {
        assert!(is_full_tunnel(&nets(&["10.0.0.0/8", "0.0.0.0/0"])));
        assert!(!is_full_tunnel(&nets(&["10.0.0.0/8"])));
    }
}
