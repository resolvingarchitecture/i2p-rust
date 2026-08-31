//! Detects a local I2P router by probing its SAM bridge.

use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use log::debug;

pub const DEFAULT_SAM_HOST: &str = "127.0.0.1";
/// SAM bridge TCP port. Enable it in the router console
/// (Configuration → Clients → "SAM application bridge").
pub const DEFAULT_SAM_PORT: u16 = 7656;

/// Probes the SAM bridge port so [`crate::I2pClient::start`] can decide between
/// attaching to a running router and starting the embedded one.
#[derive(Clone, Debug)]
pub struct LocalRouterDetector {
    pub host: String,
    pub sam_port: u16,
    pub timeout: Duration,
}

impl Default for LocalRouterDetector {
    fn default() -> Self {
        LocalRouterDetector {
            host: DEFAULT_SAM_HOST.to_string(),
            sam_port: DEFAULT_SAM_PORT,
            timeout: Duration::from_millis(750),
        }
    }
}

impl LocalRouterDetector {
    /// True if something accepts a TCP connection on the SAM port.
    pub fn is_local_router_running(&self) -> bool {
        match connect(&self.host, self.sam_port, self.timeout) {
            Ok(_) => true,
            Err(e) => {
                debug!("no SAM bridge on {}:{} ({e})", self.host, self.sam_port);
                false
            }
        }
    }

    pub fn sam_tcp_addr(&self) -> String {
        format!("{}:{}", self.host, self.sam_port)
    }
}

fn connect(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::other("no address"))?;
    TcpStream::connect_timeout(&addr, timeout)
}
