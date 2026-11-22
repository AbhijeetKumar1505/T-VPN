//! The WireGuard data plane.
//!
//! One engine serves both client and server roles: it owns a set of peers,
//! routes outgoing packets to a peer by longest-prefix match on AllowedIPs
//! (cryptokey routing), and maps incoming datagrams to peers by handshake
//! public key or session index. Peers with a configured endpoint are
//! "active": the engine keeps a session to them alive and retries forever.

use crate::device::TunDevice;
use crate::keys::Key;
use boringtun::noise::errors::WireGuardError;
use boringtun::noise::handshake::parse_handshake_anon;
use boringtun::noise::rate_limiter::RateLimiter;
use boringtun::noise::{Packet, Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use ipnet::IpNet;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const BUF_SIZE: usize = 65_536 + 256;
const TIMER_TICK: Duration = Duration::from_millis(250);
/// Handshake initiations per second (across all peers) before cookies are demanded.
const HANDSHAKE_RATE_LIMIT: u64 = 100;

/// Runtime configuration for a single peer.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerConfig {
    pub name: Option<String>,
    pub public_key: Key,
    pub preshared_key: Option<Key>,
    pub allowed_ips: Vec<IpNet>,
    /// Configured endpoint. Peers with an endpoint are actively connected to.
    pub endpoint: Option<SocketAddr>,
    pub persistent_keepalive: Option<u16>,
}

/// Point-in-time view of a peer, for status output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerStatus {
    pub name: Option<String>,
    pub public_key: String,
    pub endpoint: Option<SocketAddr>,
    pub allowed_ips: Vec<IpNet>,
    /// Seconds since the current session was established; `None` = no session.
    pub last_handshake_secs: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl PeerStatus {
    pub fn connected(&self) -> bool {
        self.last_handshake_secs.is_some()
    }

    pub fn label(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => {
                let mut k = self.public_key.clone();
                k.truncate(8);
                k + "…"
            }
        }
    }
}

/// UDP sockets used by the engine (one per address family).
pub struct Sockets {
    pub v4: Option<UdpSocket>,
    pub v6: Option<UdpSocket>,
}

impl Sockets {
    /// Bind UDP sockets. `port = 0` picks an ephemeral port.
    pub fn bind(port: u16, v4: bool, v6: bool) -> io::Result<Sockets> {
        let s4 = if v4 { Some(bind_udp(SocketAddr::from(([0, 0, 0, 0], port)))?) } else { None };
        let s6 = if v6 {
            Some(bind_udp(SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)))?)
        } else {
            None
        };
        Ok(Sockets { v4: s4, v6: s6 })
    }

    /// Like [`Sockets::bind`] for servers: IPv4 is required, IPv6 is best-effort.
    pub fn bind_server(port: u16) -> io::Result<(Sockets, Option<io::Error>)> {
        let v4 = bind_udp(SocketAddr::from(([0, 0, 0, 0], port)))?;
        match bind_udp(SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port))) {
            Ok(v6) => Ok((Sockets { v4: Some(v4), v6: Some(v6) }, None)),
            Err(e) => Ok((Sockets { v4: Some(v4), v6: None }, Some(e))),
        }
    }

    pub fn local_port(&self) -> Option<u16> {
        self.v4.as_ref().or(self.v6.as_ref()).and_then(|s| s.local_addr().ok()).map(|a| a.port())
    }
}

fn bind_udp(addr: SocketAddr) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    if addr.is_ipv6() {
        sock.set_only_v6(true)?;
    }
    // Bigger buffers noticeably help throughput; failures are harmless.
    let _ = sock.set_recv_buffer_size(4 << 20);
    let _ = sock.set_send_buffer_size(4 << 20);
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    UdpSocket::from_std(sock.into())
}

struct PeerSlot {
    cfg: PeerConfig,
    /// Normalized (truncated) networks used for routing decisions.
    routes: Vec<IpNet>,
    tunn: Tunn,
    /// Current endpoint: configured, or learned from authenticated packets (roaming).
    endpoint: Option<SocketAddr>,
}

impl PeerSlot {
    fn allows_source(&self, ip: IpAddr) -> bool {
        self.routes.iter().any(|n| n.contains(&ip))
    }

    fn is_active(&self) -> bool {
        self.cfg.endpoint.is_some()
    }
}

struct State {
    secret: StaticSecret,
    public: PublicKey,
    rate_limiter: Arc<RateLimiter>,
    /// Slot index == WireGuard session index (upper 24 bits). Slots are never reused.
    slots: Vec<Option<PeerSlot>>,
    by_key: HashMap<[u8; 32], usize>,
}

enum Action {
    None,
    Tunnel(Range<usize>),
    Network { to: SocketAddr, range: Range<usize>, flush: Option<usize> },
}

/// Range of `sub` within the buffer starting at `base`.
fn span(base: *const u8, sub: &[u8]) -> Range<usize> {
    let start = sub.as_ptr() as usize - base as usize;
    start..start + sub.len()
}

fn normalize(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::from((v4, v6.port())),
            None => addr,
        },
        v4 => v4,
    }
}

impl State {
    fn add_peer(&mut self, cfg: PeerConfig) {
        let idx = self.slots.len();
        let tunn = Tunn::new(
            self.secret.clone(),
            cfg.public_key.x25519_public(),
            cfg.preshared_key.map(|k| *k.as_bytes()),
            cfg.persistent_keepalive,
            idx as u32,
            Some(self.rate_limiter.clone()),
        );
        let routes = cfg.allowed_ips.iter().map(IpNet::trunc).collect();
        self.by_key.insert(*cfg.public_key.as_bytes(), idx);
        self.slots.push(Some(PeerSlot { endpoint: cfg.endpoint, routes, tunn, cfg }));
    }

    fn route(&self, dst: IpAddr) -> Option<usize> {
        let mut best: Option<(u8, usize)> = None;
        for (idx, slot) in self.slots.iter().enumerate() {
            let Some(slot) = slot else { continue };
            for net in &slot.routes {
                if net.contains(&dst) && best.is_none_or(|(len, _)| net.prefix_len() > len) {
                    best = Some((net.prefix_len(), idx));
                }
            }
        }
        best.map(|(_, idx)| idx)
    }

    fn lookup(&self, packet: &Packet) -> Option<usize> {
        let idx = match packet {
            Packet::HandshakeInit(p) => {
                let half = parse_handshake_anon(&self.secret, &self.public, p).ok()?;
                *self.by_key.get(&half.peer_static_public)?
            }
            Packet::HandshakeResponse(p) => (p.receiver_idx >> 8) as usize,
            Packet::PacketCookieReply(p) => (p.receiver_idx >> 8) as usize,
            Packet::PacketData(p) => (p.receiver_idx >> 8) as usize,
        };
        matches!(self.slots.get(idx), Some(Some(_))).then_some(idx)
    }

    /// Outgoing packet from the TUN device.
    fn encapsulate(&mut self, pkt: &[u8], dst: &mut [u8]) -> Option<(SocketAddr, Range<usize>)> {
        let dst_ip = Tunn::dst_address(pkt)?;
        let idx = self.route(dst_ip)?;
        let slot = self.slots[idx].as_mut()?;
        let endpoint = slot.endpoint?;
        let base = dst.as_ptr();
        match slot.tunn.encapsulate(pkt, dst) {
            TunnResult::WriteToNetwork(b) => Some((endpoint, span(base, b))),
            TunnResult::Err(e) => {
                tracing::debug!("encapsulate error for {}: {e:?}", slot.cfg.public_key.short());
                None
            }
            _ => None,
        }
    }

    /// Incoming datagram from the network.
    fn decapsulate(&mut self, from: SocketAddr, datagram: &[u8], dst: &mut [u8]) -> Action {
        let base = dst.as_ptr();
        let packet = match self.rate_limiter.verify_packet(Some(from.ip()), datagram, dst) {
            Ok(p) => p,
            Err(TunnResult::WriteToNetwork(cookie)) => {
                return Action::Network { to: from, range: span(base, cookie), flush: None };
            }
            Err(_) => return Action::None,
        };
        let Some(idx) = self.lookup(&packet) else {
            tracing::trace!("dropping datagram from {from}: unknown peer");
            return Action::None;
        };
        let slot = self.slots[idx].as_mut().expect("lookup checked slot");
        match slot.tunn.decapsulate(Some(from.ip()), datagram, dst) {
            TunnResult::Done => {
                slot.endpoint = Some(from);
                Action::None
            }
            TunnResult::WriteToNetwork(b) => {
                slot.endpoint = Some(from);
                Action::Network { to: from, range: span(base, b), flush: Some(idx) }
            }
            TunnResult::WriteToTunnelV4(b, src) => Self::deliver(slot, from, IpAddr::V4(src), span(base, b)),
            TunnResult::WriteToTunnelV6(b, src) => Self::deliver(slot, from, IpAddr::V6(src), span(base, b)),
            TunnResult::Err(e) => {
                tracing::trace!("decapsulate error from {from}: {e:?}");
                Action::None
            }
        }
    }

    fn deliver(slot: &mut PeerSlot, from: SocketAddr, src: IpAddr, range: Range<usize>) -> Action {
        // Cryptokey routing: a peer may only send from addresses in its AllowedIPs.
        if !slot.allows_source(src) {
            tracing::debug!(
                "dropping packet from {src}: not in AllowedIPs of {}",
                slot.cfg.public_key.short()
            );
            return Action::None;
        }
        slot.endpoint = Some(from);
        Action::Tunnel(range)
    }

    /// Drain packets queued while a handshake was in flight.
    fn flush(&mut self, idx: usize, dst: &mut [u8]) -> Action {
        let Some(Some(slot)) = self.slots.get_mut(idx) else { return Action::None };
        let Some(endpoint) = slot.endpoint else { return Action::None };
        let base = dst.as_ptr();
        match slot.tunn.decapsulate(None, &[], dst) {
            TunnResult::WriteToNetwork(b) => {
                Action::Network { to: endpoint, range: span(base, b), flush: Some(idx) }
            }
            _ => Action::None,
        }
    }

    fn tick(&mut self, dst: &mut [u8], last_reset: &mut Instant) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        if last_reset.elapsed() >= Duration::from_secs(1) {
            self.rate_limiter.reset_count();
            *last_reset = Instant::now();
        }
        for slot in self.slots.iter_mut().flatten() {
            match slot.tunn.update_timers(dst) {
                TunnResult::WriteToNetwork(b) => {
                    if let Some(ep) = slot.endpoint {
                        out.push((ep, b.to_vec()));
                    }
                }
                TunnResult::Err(WireGuardError::ConnectionExpired) => {}
                TunnResult::Err(e) => tracing::trace!("timer error: {e:?}"),
                _ => {}
            }
            // Active peers: (re)establish a session whenever there is none.
            // Returns Done while a handshake is already in flight.
            if slot.is_active()
                && slot.tunn.time_since_last_handshake().is_none()
                && let (Some(ep), TunnResult::WriteToNetwork(b)) =
                    (slot.endpoint, slot.tunn.format_handshake_initiation(dst, false))
            {
                out.push((ep, b.to_vec()));
            }
        }
        out
    }

    fn status(&self) -> Vec<PeerStatus> {
        self.slots
            .iter()
            .flatten()
            .map(|s| {
                let (hs, tx, rx, _, _) = s.tunn.stats();
                PeerStatus {
                    name: s.cfg.name.clone(),
                    public_key: s.cfg.public_key.to_base64(),
                    endpoint: s.endpoint,
                    allowed_ips: s.cfg.allowed_ips.clone(),
                    last_handshake_secs: hs.map(|d| d.as_secs()),
                    rx_bytes: rx as u64,
                    tx_bytes: tx as u64,
                }
            })
            .collect()
    }
}

struct Shared {
    state: Mutex<State>,
    v4: Option<UdpSocket>,
    v6: Option<UdpSocket>,
    fatal: watch::Sender<Option<String>>,
}

impl Shared {
    async fn send_to(&self, to: SocketAddr, buf: &[u8]) {
        let sock = if to.is_ipv4() { self.v4.as_ref() } else { self.v6.as_ref() };
        let Some(sock) = sock else {
            tracing::debug!("no socket for address family of {to}");
            return;
        };
        if let Err(e) = sock.send_to(buf, to).await {
            tracing::debug!("send to {to} failed: {e}");
        }
    }
}

/// Cheap, cloneable handle for querying/updating a running engine.
#[derive(Clone)]
pub struct EngineHandle(Arc<Shared>);

impl EngineHandle {
    pub fn status(&self) -> Vec<PeerStatus> {
        self.0.state.lock().status()
    }

    /// Replace the peer set (used by `tvpn server reload`). Existing peers with
    /// unchanged keys keep their sessions. Returns (added, removed).
    pub fn set_peers(&self, peers: Vec<PeerConfig>) -> (usize, usize) {
        let mut st = self.0.state.lock();
        let wanted: HashMap<[u8; 32], PeerConfig> =
            peers.into_iter().map(|p| (*p.public_key.as_bytes(), p)).collect();

        let mut removed = 0;
        for idx in 0..st.slots.len() {
            let Some(slot) = &st.slots[idx] else { continue };
            let key = *slot.cfg.public_key.as_bytes();
            match wanted.get(&key) {
                None => {
                    st.slots[idx] = None;
                    st.by_key.remove(&key);
                    removed += 1;
                }
                Some(new)
                    if new.preshared_key != slot.cfg.preshared_key
                        || new.persistent_keepalive != slot.cfg.persistent_keepalive =>
                {
                    // Session parameters changed: rebuild the peer from scratch.
                    st.slots[idx] = None;
                    st.by_key.remove(&key);
                }
                Some(new) => {
                    let slot = st.slots[idx].as_mut().expect("checked above");
                    slot.routes = new.allowed_ips.iter().map(IpNet::trunc).collect();
                    if new.endpoint.is_some() && new.endpoint != slot.cfg.endpoint {
                        slot.endpoint = new.endpoint;
                    }
                    slot.cfg = new.clone();
                }
            }
        }
        let mut added = 0;
        for (key, cfg) in wanted {
            if !st.by_key.contains_key(&key) {
                st.add_peer(cfg);
                added += 1;
            }
        }
        (added, removed)
    }
}

/// A running WireGuard engine. Dropping it stops all its tasks.
pub struct Engine {
    handle: EngineHandle,
    tasks: Vec<JoinHandle<()>>,
    fatal: watch::Receiver<Option<String>>,
    local_port: Option<u16>,
}

impl Engine {
    /// Start the data plane on the current Tokio runtime.
    pub fn start<D: TunDevice>(
        private_key: &Key,
        peers: Vec<PeerConfig>,
        sockets: Sockets,
        device: Arc<D>,
    ) -> Engine {
        let secret = private_key.static_secret();
        let public = PublicKey::from(&secret);
        let rate_limiter = Arc::new(RateLimiter::new(&public, HANDSHAKE_RATE_LIMIT));
        let mut state = State { secret, public, rate_limiter, slots: Vec::new(), by_key: HashMap::new() };
        for p in peers {
            state.add_peer(p);
        }
        let local_port = sockets.local_port();
        let (fatal_tx, fatal_rx) = watch::channel(None);
        let shared =
            Arc::new(Shared { state: Mutex::new(state), v4: sockets.v4, v6: sockets.v6, fatal: fatal_tx });

        let mut tasks = vec![
            tokio::spawn(tun_loop(shared.clone(), device.clone())),
            tokio::spawn(timer_loop(shared.clone())),
        ];
        if shared.v4.is_some() {
            tasks.push(tokio::spawn(udp_loop(shared.clone(), device.clone(), false)));
        }
        if shared.v6.is_some() {
            tasks.push(tokio::spawn(udp_loop(shared.clone(), device, true)));
        }
        Engine { handle: EngineHandle(shared), tasks, fatal: fatal_rx, local_port }
    }

    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }

    pub fn status(&self) -> Vec<PeerStatus> {
        self.handle.status()
    }

    pub fn local_port(&self) -> Option<u16> {
        self.local_port
    }

    /// Resolves when the engine hits an unrecoverable error (e.g. the TUN device vanished).
    pub async fn failed(&mut self) -> String {
        loop {
            if let Some(msg) = self.fatal.borrow().clone() {
                return msg;
            }
            if self.fatal.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    /// Stop all tasks and wait until they have released the device and sockets.
    pub async fn shutdown(mut self) {
        for t in &self.tasks {
            t.abort();
        }
        for t in self.tasks.drain(..) {
            let _ = t.await;
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ConnectionReset // Windows reports ICMP port-unreachable this way
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::TimedOut
    )
}

async fn tun_loop<D: TunDevice>(shared: Arc<Shared>, dev: Arc<D>) {
    let mut src = vec![0u8; BUF_SIZE];
    let mut dst = vec![0u8; BUF_SIZE];
    loop {
        let n = match dev.recv(&mut src).await {
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) => continue,
            Err(e) => {
                let _ = shared.fatal.send(Some(format!("tunnel device read failed: {e}")));
                return;
            }
        };
        if n == 0 {
            continue;
        }
        let out = shared.state.lock().encapsulate(&src[..n], &mut dst);
        if let Some((to, range)) = out {
            shared.send_to(to, &dst[range]).await;
        }
    }
}

async fn udp_loop<D: TunDevice>(shared: Arc<Shared>, dev: Arc<D>, v6: bool) {
    let mut src = vec![0u8; BUF_SIZE];
    let mut dst = vec![0u8; BUF_SIZE];
    let mut failures = 0u32;
    loop {
        let sock = if v6 { shared.v6.as_ref() } else { shared.v4.as_ref() }.expect("socket exists");
        let (n, from) = match sock.recv_from(&mut src).await {
            Ok(v) => {
                failures = 0;
                v
            }
            Err(e) if transient(&e) => continue,
            Err(e) => {
                failures += 1;
                tracing::warn!("UDP receive error: {e}");
                if failures > 50 {
                    let _ = shared.fatal.send(Some(format!("UDP socket failed: {e}")));
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let from = normalize(from);
        let mut action = shared.state.lock().decapsulate(from, &src[..n], &mut dst);
        loop {
            match action {
                Action::None => break,
                Action::Tunnel(range) => {
                    if let Err(e) = dev.send(&dst[range]).await {
                        tracing::debug!("write to tunnel device failed: {e}");
                    }
                    break;
                }
                Action::Network { to, range, flush } => {
                    shared.send_to(to, &dst[range]).await;
                    match flush {
                        Some(idx) => action = shared.state.lock().flush(idx, &mut dst),
                        None => break,
                    }
                }
            }
        }
    }
}

async fn timer_loop(shared: Arc<Shared>) {
    let mut interval = tokio::time::interval(TIMER_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dst = vec![0u8; BUF_SIZE];
    let mut last_reset = Instant::now();
    loop {
        interval.tick().await;
        let out = shared.state.lock().tick(&mut dst, &mut last_reset);
        for (to, pkt) in out {
            shared.send_to(to, &pkt).await;
        }
    }
}
