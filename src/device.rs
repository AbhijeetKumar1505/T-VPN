//! Abstraction over the virtual network interface the engine reads IP packets from.

use std::future::Future;
use std::io;
use tokio::sync::{Mutex, mpsc};

/// Something that carries raw IP packets: a real TUN interface or an in-memory pipe.
pub trait TunDevice: Send + Sync + 'static {
    fn recv(&self, buf: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;
    fn send(&self, buf: &[u8]) -> impl Future<Output = io::Result<usize>> + Send;
}

impl TunDevice for tun_rs::AsyncDevice {
    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        tun_rs::AsyncDevice::recv(self, buf).await
    }

    async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        tun_rs::AsyncDevice::send(self, buf).await
    }
}

/// In-memory device, used by tests and benchmarks (no privileges required).
pub struct MemoryDevice {
    inbound: Mutex<mpsc::Receiver<Vec<u8>>>,
    outbound: mpsc::Sender<Vec<u8>>,
}

/// The "outside" end of a [`MemoryDevice`]: inject packets as if an app sent
/// them, and observe packets the engine delivered.
pub struct MemoryDeviceHandle {
    pub to_engine: mpsc::Sender<Vec<u8>>,
    pub from_engine: mpsc::Receiver<Vec<u8>>,
}

pub fn memory_device() -> (MemoryDevice, MemoryDeviceHandle) {
    let (to_engine, inbound) = mpsc::channel(1024);
    let (outbound, from_engine) = mpsc::channel(1024);
    (MemoryDevice { inbound: Mutex::new(inbound), outbound }, MemoryDeviceHandle { to_engine, from_engine })
}

impl TunDevice for MemoryDevice {
    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let pkt = self
            .inbound
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "memory device closed"))?;
        let n = pkt.len().min(buf.len());
        buf[..n].copy_from_slice(&pkt[..n]);
        Ok(n)
    }

    async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.outbound
            .send(buf.to_vec())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "memory device closed"))?;
        Ok(buf.len())
    }
}
