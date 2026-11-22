//! Creating the OS-level TUN interface (Linux tun, macOS utun, Windows wintun).

use anyhow::{Result, anyhow};
use ipnet::IpNet;

pub struct TunOptions<'a> {
    /// Interface name (ignored on macOS, where the kernel assigns `utunN`).
    pub name: &'a str,
    pub addresses: &'a [IpNet],
    pub mtu: u16,
    /// Stable adapter GUID on Windows, so Windows keeps one network profile.
    pub guid: u128,
}

pub const CLIENT_IFNAME: &str = if cfg!(windows) { "tvpn" } else { "tvpn0" };
pub const SERVER_IFNAME: &str = if cfg!(windows) { "tvpn-server" } else { "tvpns0" };
pub const CLIENT_GUID: u128 = 0x7476_706e_0001_4000_8000_0000_0000_0001;
pub const SERVER_GUID: u128 = 0x7476_706e_0002_4000_8000_0000_0000_0002;

pub fn create(opts: &TunOptions<'_>) -> Result<tun_rs::AsyncDevice> {
    let mut builder = tun_rs::DeviceBuilder::new().mtu(opts.mtu);
    if !cfg!(target_os = "macos") {
        builder = builder.name(opts.name);
    }
    let mut v4_set = false;
    let mut extra_v4 = Vec::new();
    for addr in opts.addresses {
        match addr {
            IpNet::V4(n) if !v4_set => {
                builder = builder.ipv4(n.addr(), n.prefix_len(), None);
                v4_set = true;
            }
            IpNet::V4(n) => extra_v4.push(*n),
            IpNet::V6(n) => builder = builder.ipv6(n.addr(), n.prefix_len()),
        }
    }

    #[cfg(windows)]
    {
        let dll = crate::wintun::ensure()?;
        builder = builder
            .wintun_file(dll.to_string_lossy().into_owned())
            .description("T-VPN Tunnel")
            .device_guid(opts.guid)
            .metric(5);
    }
    #[cfg(not(windows))]
    let _ = opts.guid;

    let dev = builder.build_async().map_err(|e| friendly_error(e, opts.name))?;
    for n in extra_v4 {
        #[cfg(not(windows))]
        if let Err(e) = dev.add_address_v4(n.addr(), n.prefix_len()) {
            tracing::warn!("could not add extra address {n}: {e}");
        }
        #[cfg(windows)]
        tracing::warn!("only one IPv4 address per interface is supported on Windows; ignoring {n}");
    }
    Ok(dev)
}

fn friendly_error(e: std::io::Error, name: &str) -> anyhow::Error {
    #[cfg(unix)]
    {
        match e.raw_os_error() {
            Some(libc::EPERM) | Some(libc::EACCES) => {
                return anyhow!(
                    "permission denied creating the tunnel interface (run as root or grant CAP_NET_ADMIN): {e}"
                );
            }
            Some(libc::ENOENT) | Some(libc::ENODEV) => {
                return anyhow!(
                    "the TUN driver is not available (is /dev/net/tun present? try `modprobe tun`): {e}"
                );
            }
            Some(libc::EBUSY) | Some(libc::EEXIST) => {
                return anyhow!("interface `{name}` is already in use (is another tvpn session running?)");
            }
            _ => {}
        }
    }
    let _ = name;
    anyhow!("failed to create the tunnel interface: {e}")
}
