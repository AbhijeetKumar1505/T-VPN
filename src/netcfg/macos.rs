//! macOS specifics: DNS via `networksetup` (same approach as wg-quick on macOS).

use super::{DnsRecord, MacDnsBackup, run};
use anyhow::{Result, bail};
use std::net::IpAddr;

fn services() -> Result<Vec<String>> {
    let out = run("networksetup", &["-listallnetworkservices"])?;
    Ok(out
        .lines()
        .skip(1) // "An asterisk (*) denotes that a network service is disabled."
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('*'))
        .map(String::from)
        .collect())
}

fn read_list(flag: &str, service: &str) -> Vec<String> {
    match run("networksetup", &[flag, service]) {
        Ok(out) if !out.contains("There aren't any") => {
            out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
        }
        _ => Vec::new(),
    }
}

fn set_list(flag: &str, service: &str, values: &[String]) -> Result<()> {
    let mut args = vec![flag, service];
    if values.is_empty() {
        args.push("Empty");
    } else {
        args.extend(values.iter().map(String::as_str));
    }
    run("networksetup", &args).map(|_| ())
}

fn flush_cache() {
    let _ = run("dscacheutil", &["-flushcache"]);
    let _ = run("killall", &["-HUP", "mDNSResponder"]);
}

pub fn apply_dns(dns: &[IpAddr], search: &[String]) -> Result<DnsRecord> {
    let servers: Vec<String> = dns.iter().map(ToString::to_string).collect();
    let mut backups = Vec::new();
    for service in services()? {
        let backup = MacDnsBackup {
            servers: read_list("-getdnsservers", &service),
            search: read_list("-getsearchdomains", &service),
            service: service.clone(),
        };
        if set_list("-setdnsservers", &service, &servers).is_ok() {
            let _ = set_list("-setsearchdomains", &service, search);
            backups.push(backup);
        }
    }
    if backups.is_empty() {
        bail!("no network service accepted the DNS settings");
    }
    flush_cache();
    Ok(DnsRecord::MacServices { services: backups })
}

pub fn revert_dns(rec: &DnsRecord, warnings: &mut Vec<String>) {
    if let DnsRecord::MacServices { services } = rec {
        for b in services {
            if let Err(e) = set_list("-setdnsservers", &b.service, &b.servers) {
                warnings.push(format!("could not restore DNS for {}: {e:#}", b.service));
            }
            let _ = set_list("-setsearchdomains", &b.service, &b.search);
        }
        flush_cache();
    }
}
