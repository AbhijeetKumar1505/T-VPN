//! End-to-end tests of the WireGuard engine: a client and a server engine talk
//! over real UDP sockets on localhost, with in-memory TUN devices standing in
//! for the OS interface. No privileges required.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tvpn::device::{MemoryDeviceHandle, memory_device};
use tvpn::engine::{Engine, PeerConfig, Sockets};
use tvpn::keys::Key;

fn ipv4_packet(src: [u8; 4], dst: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let total = 20 + payload.len();
    let mut p = vec![0u8; total];
    p[0] = 0x45;
    p[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    p[8] = 64;
    p[9] = 17;
    p[12..16].copy_from_slice(&src);
    p[16..20].copy_from_slice(&dst);
    p[20..].copy_from_slice(payload);
    let mut sum: u32 = (0..20).step_by(2).map(|i| u16::from_be_bytes([p[i], p[i + 1]]) as u32).sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    p[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    p
}

async fn wait_until(mut f: impl FnMut() -> bool, max: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < max {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    f()
}

struct Pair {
    server: Engine,
    server_io: MemoryDeviceHandle,
    client: Engine,
    client_io: MemoryDeviceHandle,
    client_peer: PeerConfig,
}

fn client_peer_for(client: &Key, psk: Key) -> PeerConfig {
    PeerConfig {
        name: Some("laptop".into()),
        public_key: client.public_key(),
        preshared_key: Some(psk),
        allowed_ips: vec!["10.9.0.2/32".parse().unwrap()],
        endpoint: None,
        persistent_keepalive: None,
    }
}

fn start_pair(server_knows_client: bool, server_pub_override: Option<Key>) -> Pair {
    let server_key = Key::generate_private();
    let client_key = Key::generate_private();
    let psk = Key::generate_preshared();
    let client_peer = client_peer_for(&client_key, psk);

    let (server_dev, server_io) = memory_device();
    let server_sockets = Sockets::bind(0, true, false).unwrap();
    let port = server_sockets.local_port().unwrap();
    let server_peers = if server_knows_client { vec![client_peer.clone()] } else { vec![] };
    let server = Engine::start(&server_key, server_peers, server_sockets, Arc::new(server_dev));

    let (client_dev, client_io) = memory_device();
    let client = Engine::start(
        &client_key,
        vec![PeerConfig {
            name: Some("server".into()),
            public_key: server_pub_override.unwrap_or_else(|| server_key.public_key()),
            preshared_key: Some(psk),
            allowed_ips: vec!["0.0.0.0/0".parse().unwrap()],
            endpoint: Some(SocketAddr::from(([127, 0, 0, 1], port))),
            persistent_keepalive: Some(25),
        }],
        Sockets::bind(0, true, false).unwrap(),
        Arc::new(client_dev),
    );
    Pair { server, server_io, client, client_io, client_peer }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_and_server_exchange_packets() {
    let mut p = start_pair(true, None);

    // The client connects on its own, without any traffic.
    assert!(wait_until(|| p.client.status()[0].connected(), Duration::from_secs(5)).await, "no handshake");

    let up = ipv4_packet([10, 9, 0, 2], [10, 9, 0, 1], b"hello server");
    p.client_io.to_engine.send(up.clone()).await.unwrap();
    let got = timeout(Duration::from_secs(5), p.server_io.from_engine.recv()).await.unwrap().unwrap();
    assert_eq!(got, up);

    // The server learned the client's endpoint and can answer.
    let down = ipv4_packet([10, 9, 0, 1], [10, 9, 0, 2], b"hello client");
    p.server_io.to_engine.send(down.clone()).await.unwrap();
    let got = timeout(Duration::from_secs(5), p.client_io.from_engine.recv()).await.unwrap().unwrap();
    assert_eq!(got, down);

    // A bulk burst arrives intact and in order.
    for i in 0..200u16 {
        let pkt = ipv4_packet([10, 9, 0, 2], [10, 9, 0, 1], &i.to_be_bytes().repeat(300));
        p.client_io.to_engine.send(pkt).await.unwrap();
    }
    for i in 0..200u16 {
        let got = timeout(Duration::from_secs(5), p.server_io.from_engine.recv()).await.unwrap().unwrap();
        assert_eq!(&got[20..22], &i.to_be_bytes());
        assert_eq!(got.len(), 620);
    }

    let s = &p.server.status()[0];
    assert!(s.connected());
    assert!(s.endpoint.is_some());
    assert!(s.rx_bytes > 120_000);
    assert_eq!(s.name.as_deref(), Some("laptop"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spoofed_source_addresses_are_dropped() {
    let mut p = start_pair(true, None);
    assert!(wait_until(|| p.client.status()[0].connected(), Duration::from_secs(5)).await);

    // 10.9.0.99 is not in the client's AllowedIPs on the server.
    let spoof = ipv4_packet([10, 9, 0, 99], [10, 9, 0, 1], b"spoofed");
    p.client_io.to_engine.send(spoof).await.unwrap();
    assert!(timeout(Duration::from_millis(500), p.server_io.from_engine.recv()).await.is_err());

    // Traffic for addresses the server doesn't route anywhere is dropped too.
    let nowhere = ipv4_packet([10, 9, 0, 1], [192, 0, 2, 1], b"nowhere");
    p.server_io.to_engine.send(nowhere).await.unwrap();
    assert!(timeout(Duration::from_millis(500), p.client_io.from_engine.recv()).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_server_key_never_connects() {
    let mut p = start_pair(true, Some(Key::generate_private().public_key()));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!p.client.status()[0].connected());
    assert!(!p.server.status()[0].connected());
    let pkt = ipv4_packet([10, 9, 0, 2], [10, 9, 0, 1], b"x");
    p.client_io.to_engine.send(pkt).await.unwrap();
    assert!(timeout(Duration::from_millis(500), p.server_io.from_engine.recv()).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peers_can_be_added_and_removed_at_runtime() {
    let mut p = start_pair(false, None);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!p.client.status()[0].connected(), "unknown peer must not connect");

    let (added, removed) = p.server.handle().set_peers(vec![p.client_peer.clone()]);
    assert_eq!((added, removed), (1, 0));
    // The client retries the handshake every few seconds.
    assert!(wait_until(|| p.client.status()[0].connected(), Duration::from_secs(12)).await);

    let pkt = ipv4_packet([10, 9, 0, 2], [10, 9, 0, 1], b"after reload");
    p.client_io.to_engine.send(pkt.clone()).await.unwrap();
    let got = timeout(Duration::from_secs(5), p.server_io.from_engine.recv()).await.unwrap().unwrap();
    assert_eq!(got, pkt);

    // Re-applying the same set keeps the session; removing drops the peer.
    assert_eq!(p.server.handle().set_peers(vec![p.client_peer.clone()]), (0, 0));
    assert!(p.server.status()[0].connected());
    assert_eq!(p.server.handle().set_peers(vec![]), (0, 1));
    assert!(p.server.status().is_empty());
    p.client_io.to_engine.send(pkt).await.unwrap();
    assert!(timeout(Duration::from_millis(500), p.server_io.from_engine.recv()).await.is_err());
}
