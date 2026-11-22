//! Self-hosted server: config store, peer management and client config generation.
//!
//! Layout under `<config>/server/`:
//! * `server.conf`  - WireGuard config of the server (also usable by wg-quick)
//! * `server.json`  - settings used to generate client configs
//! * `peers/<name>.conf` - generated client configs

use crate::config::{Config, Endpoint, Interface, Peer};
use crate::keys::Key;
use crate::paths;
use anyhow::{Context, Result, anyhow, bail};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerMeta {
    /// Public `host:port` clients connect to.
    pub endpoint: String,
    pub dns: Vec<IpAddr>,
    /// AllowedIPs written into client configs (default: full tunnel).
    pub client_allowed_ips: Vec<IpNet>,
    pub keepalive: u16,
}

pub struct InitOptions {
    pub endpoint_host: String,
    pub port: u16,
    pub subnet_v4: Ipv4Net,
    pub subnet_v6: Option<Ipv6Net>,
    pub dns: Vec<IpAddr>,
}

pub struct ServerStore {
    dir: PathBuf,
}

pub struct NewPeer {
    pub name: String,
    pub client_config: Config,
    pub path: PathBuf,
}

impl ServerStore {
    pub fn new(dir: PathBuf) -> Self {
        ServerStore { dir }
    }

    pub fn default_location() -> Self {
        Self::new(paths::server_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn conf_path(&self) -> PathBuf {
        self.dir.join("server.conf")
    }

    fn meta_path(&self) -> PathBuf {
        self.dir.join("server.json")
    }

    pub fn peers_dir(&self) -> PathBuf {
        self.dir.join("peers")
    }

    pub fn exists(&self) -> bool {
        self.conf_path().is_file()
    }

    pub fn init(&self, opts: &InitOptions, force: bool) -> Result<Config> {
        if self.exists() && !force {
            bail!(
                "a server is already set up at {} (use --force to replace it and all its peers)",
                self.dir.display()
            );
        }
        let host = validate_endpoint_host(&opts.endpoint_host)?;
        if opts.port == 0 {
            bail!("--port must not be 0");
        }
        if opts.subnet_v4.prefix_len() > 30 {
            bail!("subnet {} is too small (use /30 or larger, e.g. 10.8.0.0/24)", opts.subnet_v4);
        }
        let v4 = opts.subnet_v4.trunc();
        let server_v4 = first_host_v4(&v4).ok_or_else(|| anyhow!("subnet {v4} has no usable addresses"))?;
        let mut addresses = vec![IpNet::V4(Ipv4Net::new(server_v4, v4.prefix_len())?)];
        if let Some(v6) = opts.subnet_v6 {
            let v6 = v6.trunc();
            let server_v6 = Ipv6Addr::from(u128::from(v6.network()) + 1);
            addresses.push(IpNet::V6(Ipv6Net::new(server_v6, v6.prefix_len())?));
        }
        let config = Config {
            interface: Interface {
                private_key: Key::generate_private(),
                addresses,
                listen_port: Some(opts.port),
                dns: Vec::new(),
                dns_search: Vec::new(),
                mtu: None,
            },
            peers: Vec::new(),
        };
        let endpoint = Endpoint { host, port: opts.port }.to_string();
        let meta = ServerMeta {
            endpoint,
            dns: opts.dns.clone(),
            client_allowed_ips: vec!["0.0.0.0/0".parse()?, "::/0".parse()?],
            keepalive: 25,
        };
        if force && self.peers_dir().exists() {
            fs::remove_dir_all(self.peers_dir()).ok();
        }
        self.save(&config, &meta)?;
        Ok(config)
    }

    pub fn load(&self) -> Result<(Config, ServerMeta)> {
        if !self.exists() {
            bail!("no server set up yet; run `tvpn server init` first");
        }
        let text = fs::read_to_string(self.conf_path())
            .with_context(|| format!("cannot read {}", self.conf_path().display()))?;
        let config =
            Config::parse(&text).with_context(|| format!("invalid {}", self.conf_path().display()))?.config;
        let meta_text = fs::read_to_string(self.meta_path())
            .with_context(|| format!("cannot read {}", self.meta_path().display()))?;
        let meta: ServerMeta = serde_json::from_str(&meta_text)
            .with_context(|| format!("invalid {}", self.meta_path().display()))?;
        Ok((config, meta))
    }

    pub fn save(&self, config: &Config, meta: &ServerMeta) -> Result<()> {
        paths::write_private(&self.conf_path(), config.to_wg_string().as_bytes())?;
        paths::write_private(&self.meta_path(), serde_json::to_string_pretty(meta)?.as_bytes())?;
        Ok(())
    }

    pub fn add_peer(&self, name: &str, allowed_ips: Option<Vec<IpNet>>) -> Result<NewPeer> {
        crate::profiles::validate_name(name)?;
        let (mut server, meta) = self.load()?;
        if server.peers.iter().any(|p| p.name.as_deref() == Some(name)) {
            bail!("a peer named `{name}` already exists");
        }

        let mut client_addrs = Vec::new();
        for net in &server.interface.addresses {
            let used: HashSet<IpAddr> = server
                .peers
                .iter()
                .flat_map(|p| p.allowed_ips.iter().map(|n| n.addr()))
                .chain(server.interface.addresses.iter().map(|n| n.addr()))
                .collect();
            let ip =
                next_free(net, &used).ok_or_else(|| anyhow!("no free addresses left in {}", net.trunc()))?;
            client_addrs.push(IpNet::from(ip));
        }

        let client_key = Key::generate_private();
        let psk = Key::generate_preshared();
        server.peers.push(Peer {
            name: Some(name.to_string()),
            public_key: client_key.public_key(),
            preshared_key: Some(psk),
            allowed_ips: client_addrs.clone(),
            endpoint: None,
            persistent_keepalive: None,
        });

        let client = Config {
            interface: Interface {
                private_key: client_key,
                addresses: client_addrs,
                listen_port: None,
                dns: meta.dns.clone(),
                dns_search: Vec::new(),
                mtu: None,
            },
            peers: vec![Peer {
                name: Some("tvpn server".into()),
                public_key: server.interface.private_key.public_key(),
                preshared_key: Some(psk),
                allowed_ips: allowed_ips.unwrap_or_else(|| meta.client_allowed_ips.clone()),
                endpoint: Some(meta.endpoint.parse().map_err(|e: String| anyhow!(e))?),
                persistent_keepalive: (meta.keepalive > 0).then_some(meta.keepalive),
            }],
        };

        let path = self.peers_dir().join(format!("{name}.conf"));
        paths::write_private(&path, client.to_wg_string().as_bytes())?;
        self.save(&server, &meta)?;
        Ok(NewPeer { name: name.to_string(), client_config: client, path })
    }

    pub fn remove_peer(&self, name: &str) -> Result<()> {
        let (mut server, meta) = self.load()?;
        let before = server.peers.len();
        server.peers.retain(|p| p.name.as_deref() != Some(name));
        if server.peers.len() == before {
            bail!("no peer named `{name}`");
        }
        self.save(&server, &meta)?;
        let _ = fs::remove_file(self.peers_dir().join(format!("{name}.conf")));
        Ok(())
    }

    pub fn peer_config(&self, name: &str) -> Result<(PathBuf, String)> {
        crate::profiles::validate_name(name)?;
        let path = self.peers_dir().join(format!("{name}.conf"));
        let text =
            fs::read_to_string(&path).map_err(|_| anyhow!("no client config stored for peer `{name}`"))?;
        Ok((path, text))
    }
}

/// Check the public hostname or IP given with `--endpoint` (the port comes
/// from `--port`). Returns the normalized host.
pub fn validate_endpoint_host(host: &str) -> Result<String> {
    let host = host.trim();
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if bare.is_empty() {
        bail!(
            "the endpoint is empty: pass the public hostname or IP clients connect to, \
             e.g. --endpoint vpn.example.com"
        );
    }
    if bare.parse::<IpAddr>().is_ok() {
        return Ok(bare.to_string());
    }
    if host.contains("://") {
        bail!("`{host}` looks like a URL: pass only the hostname or IP, e.g. --endpoint vpn.example.com");
    }
    if host.contains(':') {
        bail!("`{host}` looks like host:port: pass the host with --endpoint and the port with --port");
    }
    let valid_label = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if host.len() > 253 || !host.trim_end_matches('.').split('.').all(valid_label) {
        bail!("`{host}` is not a valid hostname or IP address");
    }
    Ok(host.to_string())
}

fn first_host_v4(net: &Ipv4Net) -> Option<Ipv4Addr> {
    net.hosts().next()
}

/// Next unused host address in `net` (skipping network/broadcast).
fn next_free(net: &IpNet, used: &HashSet<IpAddr>) -> Option<IpAddr> {
    match net.trunc() {
        IpNet::V4(n) => n.hosts().map(IpAddr::V4).find(|ip| !used.contains(ip)),
        IpNet::V6(n) => {
            let base = u128::from(n.network());
            // Plenty of room in a /64; cap the scan to keep it cheap.
            (1u128..=65_535)
                .map(|i| Ipv6Addr::from(base + i))
                .take_while(|ip| n.contains(ip))
                .map(IpAddr::V6)
                .find(|ip| !used.contains(ip))
        }
    }
}

/// Best-effort local IP used for outbound traffic (no packets are sent).
pub fn guess_public_ip() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:53").ok()?;
    Some(sock.local_addr().ok()?.ip())
}

pub fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ServerStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ServerStore::new(dir.path().join("server"));
        (dir, store)
    }

    fn opts() -> InitOptions {
        InitOptions {
            endpoint_host: "vpn.example.com".into(),
            port: 51820,
            subnet_v4: "10.8.0.0/24".parse().unwrap(),
            subnet_v6: Some("fd42:42:42::/64".parse().unwrap()),
            dns: vec!["1.1.1.1".parse().unwrap()],
        }
    }

    #[test]
    fn init_add_remove_peers() {
        let (_tmp, store) = store();
        let server = store.init(&opts(), false).unwrap();
        assert_eq!(server.interface.addresses[0].to_string(), "10.8.0.1/24");
        assert_eq!(server.interface.addresses[1].to_string(), "fd42:42:42::1/64");
        assert!(store.init(&opts(), false).is_err(), "must not overwrite without force");

        let a = store.add_peer("laptop", None).unwrap();
        let b = store.add_peer("phone", None).unwrap();
        assert_eq!(a.client_config.interface.addresses[0].to_string(), "10.8.0.2/32");
        assert_eq!(b.client_config.interface.addresses[0].to_string(), "10.8.0.3/32");
        assert_eq!(b.client_config.interface.addresses[1].to_string(), "fd42:42:42::3/128");
        assert!(store.add_peer("phone", None).is_err(), "duplicate names rejected");

        // Client config is valid and points at the server.
        b.client_config.validate_client().unwrap();
        let (srv, _) = store.load().unwrap();
        assert_eq!(b.client_config.peers[0].public_key, srv.interface.private_key.public_key());
        assert_eq!(b.client_config.peers[0].endpoint.as_ref().unwrap().to_string(), "vpn.example.com:51820");
        assert_eq!(srv.peers.len(), 2);
        srv.validate_server().unwrap();

        store.remove_peer("laptop").unwrap();
        // Freed address is reused.
        let c = store.add_peer("tablet", None).unwrap();
        assert_eq!(c.client_config.interface.addresses[0].to_string(), "10.8.0.2/32");
        assert!(store.remove_peer("nope").is_err());
    }

    #[test]
    fn subnet_exhaustion() {
        let (_tmp, store) = store();
        let mut o = opts();
        o.subnet_v4 = "10.9.0.0/30".parse().unwrap();
        o.subnet_v6 = None;
        store.init(&o, false).unwrap();
        store.add_peer("one", None).unwrap(); // .1 server, .2 peer
        assert!(store.add_peer("two", None).is_err());
    }

    #[test]
    fn endpoint_host_validation() {
        assert_eq!(validate_endpoint_host(" vpn.example.com ").unwrap(), "vpn.example.com");
        assert_eq!(validate_endpoint_host("203.0.113.7").unwrap(), "203.0.113.7");
        assert_eq!(validate_endpoint_host("[2001:db8::1]").unwrap(), "2001:db8::1");
        assert_eq!(validate_endpoint_host("2001:db8::1").unwrap(), "2001:db8::1");
        for bad in
            ["", "  ", "[]", "https://vpn.example.com", "vpn.example.com:51820", "bad host", "-x.com", "a..b"]
        {
            assert!(validate_endpoint_host(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn init_rejects_empty_endpoint_without_writing() {
        let (_tmp, store) = store();
        let mut o = opts();
        o.endpoint_host = String::new();
        assert!(store.init(&o, false).is_err());
        assert!(!store.exists(), "nothing may be written for an invalid endpoint");
    }

    #[test]
    fn private_ip_detection() {
        assert!(is_private(&"192.168.1.10".parse().unwrap()));
        assert!(is_private(&"100.64.1.1".parse().unwrap()));
        assert!(!is_private(&"203.0.113.9".parse().unwrap()));
    }
}
