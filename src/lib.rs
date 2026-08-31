//! # i2p-client (Rust)
//!
//! An I2P client for **1M5**, a Rust port of the design in
//! [`i2p-java`](https://github.com/resolvingarchitecture/i2p-java). It routes
//! [`Envelope`]s over I2P as repliable datagrams via a router's
//! [SAMv3](https://geti2p.net/en/docs/api/samv3) bridge, in one of three modes
//! ([`Mode`], config key `ra.i2p.mode`):
//!
//! | mode | behaviour |
//! |------|-----------|
//! | `local` | attach to a router already running on this host (SAM bridge on `127.0.0.1:7656`) |
//! | `embedded` | start a pure-Rust router via [emissary](https://github.com/eepnet/emissary) (requires the `embedded` feature) and attach to its SAM bridge |
//! | `auto` (default) | `local` if a SAM bridge is reachable, else `embedded` |
//!
//! Used as the I2P **protocol service** for `1m5-core-rust`
//! (`onemfive_core::protocol::I2pProtocolService`).

mod detector;
#[cfg(feature = "embedded")]
mod embedded;
mod sam;

pub use detector::{LocalRouterDetector, DEFAULT_SAM_HOST, DEFAULT_SAM_PORT};
pub use sam::{DatagramSession, SigType, DEFAULT_SAM_TCP, DEFAULT_SAM_UDP};

/// Re-exported so callers need one crate. `1m5-core-rust` provides its own.
pub use seda_bus::Envelope;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use log::{info, warn};

/// Router mode. Maps to i2p-java's `ra.i2p.mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Local,
    Embedded,
    Auto,
}

impl Mode {
    fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Mode::Local,
            "embedded" => Mode::Embedded,
            _ => Mode::Auto,
        }
    }
}

/// Connection status. Mirrors `i2p-java`'s `NetworkStatus` closely enough for
/// `1m5-core-rust` to map onto its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Disconnected,
    Connecting,
    Connected,
    /// Router or network reports us blocked / in a strict country.
    Blocked,
    /// SAM / router port already in use by something else.
    PortConflict,
    Error,
}

fn status_to_u8(s: Status) -> u8 {
    match s {
        Status::Disconnected => 0,
        Status::Connecting => 1,
        Status::Connected => 2,
        Status::Blocked => 3,
        Status::PortConflict => 4,
        Status::Error => 5,
    }
}
fn status_from_u8(v: u8) -> Status {
    match v {
        1 => Status::Connecting,
        2 => Status::Connected,
        3 => Status::Blocked,
        4 => Status::PortConflict,
        5 => Status::Error,
        _ => Status::Disconnected,
    }
}

/// An I2P client. One datagram session; send/receive repliable datagrams.
pub struct I2pClient {
    detector: LocalRouterDetector,
    mode: Mode,
    nickname: String,
    data_dir: Option<PathBuf>,
    session_timeout: Duration,
    status: AtomicU8,
    session: Mutex<Option<DatagramSession>>,
    #[cfg(feature = "embedded")]
    embedded: Mutex<Option<embedded::EmbeddedRouter>>,
}

impl I2pClient {
    pub fn new() -> I2pClient {
        I2pClient {
            detector: LocalRouterDetector::default(),
            mode: Mode::Auto,
            nickname: "1m5".to_string(),
            data_dir: None,
            session_timeout: Duration::from_secs(180),
            status: AtomicU8::new(status_to_u8(Status::Disconnected)),
            session: Mutex::new(None),
            #[cfg(feature = "embedded")]
            embedded: Mutex::new(None),
        }
    }

    /// Config keys: `ra.i2p.mode` (`local`/`embedded`/`auto`), `ra.i2p.samHost`,
    /// `ra.i2p.samPort`, `ra.i2p.nickname`, `ra.i2p.dataDir`,
    /// `ra.i2p.sessionTimeoutSecs`.
    pub fn from_config(cfg: &HashMap<String, String>) -> I2pClient {
        let mut c = I2pClient::new();
        if let Some(m) = cfg.get("ra.i2p.mode") {
            c.mode = Mode::parse(m);
        }
        if let Some(h) = cfg.get("ra.i2p.samHost") {
            c.detector.host = h.clone();
        }
        if let Some(p) = cfg.get("ra.i2p.samPort").and_then(|s| s.parse().ok()) {
            c.detector.sam_port = p;
        }
        if let Some(n) = cfg.get("ra.i2p.nickname") {
            c.nickname = n.clone();
        }
        if let Some(d) = cfg.get("ra.i2p.dataDir") {
            c.data_dir = Some(PathBuf::from(d));
        }
        if let Some(t) = cfg
            .get("ra.i2p.sessionTimeoutSecs")
            .and_then(|s| s.parse().ok())
        {
            c.session_timeout = Duration::from_secs(t);
        }
        c
    }

    pub fn detector(&self) -> &LocalRouterDetector {
        &self.detector
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn status(&self) -> Status {
        status_from_u8(self.status.load(Ordering::Acquire))
    }

    fn set_status(&self, s: Status) {
        self.status.store(status_to_u8(s), Ordering::Release);
    }

    /// The base64 destination other peers send to. Empty until [`start`] succeeds.
    ///
    /// [`start`]: Self::start
    pub fn local_destination(&self) -> String {
        self.session
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.local_dest.clone())
            .unwrap_or_default()
    }

    /// Resolve [`Mode::Auto`] to a concrete mode.
    fn effective_mode(&self) -> Mode {
        match self.mode {
            Mode::Auto => {
                if self.detector.is_local_router_running() {
                    Mode::Local
                } else {
                    Mode::Embedded
                }
            }
            m => m,
        }
    }

    /// Start the embedded router if needed, open the datagram session. Returns
    /// `false` cleanly (never panics) if I2P is unavailable.
    pub fn start(&self) -> bool {
        self.set_status(Status::Connecting);

        let (sam_tcp, sam_udp) = match self.effective_mode() {
            Mode::Local => {
                if !self.detector.is_local_router_running() {
                    warn!(
                        "No I2P SAM bridge on {} - enable it in the router console \
                         (Clients -> SAM application bridge) or use ra.i2p.mode=embedded.",
                        self.detector.sam_tcp_addr()
                    );
                    self.set_status(Status::Disconnected);
                    return false;
                }
                (self.detector.sam_tcp_addr(), DEFAULT_SAM_UDP.to_string())
            }
            Mode::Embedded => match self.start_embedded() {
                Ok(v) => v,
                Err(e) => {
                    warn!("embedded I2P router failed to start: {e}");
                    self.set_status(Status::Error);
                    return false;
                }
            },
            Mode::Auto => unreachable!("resolved by effective_mode"),
        };

        let destination = self.load_or_transient_dest();
        match DatagramSession::open(
            &sam_tcp,
            &sam_udp,
            &self.nickname,
            &destination,
            self.session_timeout,
        ) {
            Ok(session) => {
                self.persist_dest(&session.local_full_dest);
                info!(
                    "I2P datagram session open (nickname={}, dest={} chars)",
                    self.nickname,
                    session.local_dest.len()
                );
                *self.session.lock().unwrap() = Some(session);
                self.set_status(Status::Connected);
                true
            }
            Err(e) => {
                warn!("could not open I2P datagram session: {e}");
                self.set_status(Status::Error);
                false
            }
        }
    }

    #[cfg(feature = "embedded")]
    fn start_embedded(&self) -> std::io::Result<(String, String)> {
        let base = self
            .data_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("emissary");
        let router = embedded::EmbeddedRouter::start(base)?;
        let addrs = (router.sam_tcp_addr(), router.sam_udp_addr());
        *self.embedded.lock().unwrap() = Some(router);
        Ok(addrs)
    }

    #[cfg(not(feature = "embedded"))]
    fn start_embedded(&self) -> std::io::Result<(String, String)> {
        Err(std::io::Error::other(
            "embedded I2P router requires the `embedded` feature (emissary); \
             build i2p-rust with --features embedded, or run a local I2P router",
        ))
    }

    fn dest_file(&self) -> Option<PathBuf> {
        self.data_dir
            .as_ref()
            .map(|d| d.join("i2p").join("dest.b64"))
    }

    /// A persisted full destination for a stable address, or `"TRANSIENT"`.
    fn load_or_transient_dest(&self) -> String {
        if let Some(path) = self.dest_file() {
            if let Ok(s) = fs::read_to_string(&path) {
                let s = s.trim().to_string();
                if !s.is_empty() {
                    info!("reusing persisted I2P destination from {}", path.display());
                    return s;
                }
            }
        }
        "TRANSIENT".to_string()
    }

    fn persist_dest(&self, full_dest: &str) {
        let Some(path) = self.dest_file() else { return };
        if full_dest.is_empty() || full_dest == "TRANSIENT" {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(&path, full_dest) {
            warn!(
                "could not persist I2P destination to {}: {e}",
                path.display()
            );
        }
    }

    pub fn stop(&self) -> bool {
        *self.session.lock().unwrap() = None;
        #[cfg(feature = "embedded")]
        {
            *self.embedded.lock().unwrap() = None; // Drop shuts the router down
        }
        self.set_status(Status::Disconnected);
        true
    }

    /// Send `envelope.payload` to the destination in `envelope.headers`
    /// (`destination`, else `to`). On error, records `envelope.headers["error"]`.
    pub fn send(&self, envelope: &mut Envelope) -> bool {
        let dest = envelope
            .headers
            .get("destination")
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| Some(envelope.to.clone()).filter(|s| !s.is_empty()));
        let Some(dest) = dest else {
            envelope
                .headers
                .insert("error".into(), "no I2P destination".into());
            return false;
        };
        let guard = self.session.lock().unwrap();
        let Some(session) = guard.as_ref() else {
            envelope
                .headers
                .insert("error".into(), "I2P session not started".into());
            return false;
        };
        match session.send(&dest, &envelope.payload) {
            Ok(()) => true,
            Err(e) => {
                warn!("I2P datagram send failed: {e}");
                envelope.headers.insert("error".into(), e.to_string());
                false
            }
        }
    }

    /// Poll for one inbound datagram: `(from_destination, payload)`. Returns
    /// `None` if nothing has arrived (within the session's UDP read timeout).
    pub fn receive(&self) -> Option<(String, Vec<u8>)> {
        let guard = self.session.lock().unwrap();
        let session = guard.as_ref()?;
        match session.receive() {
            Ok(v) => Some(v),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => None,
            Err(e) => {
                warn!("I2P datagram receive error: {e}");
                None
            }
        }
    }
}

impl Default for I2pClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses() {
        assert_eq!(Mode::parse("local"), Mode::Local);
        assert_eq!(Mode::parse("EMBEDDED"), Mode::Embedded);
        assert_eq!(Mode::parse("whatever"), Mode::Auto);
    }

    #[test]
    fn status_round_trips() {
        for s in [
            Status::Disconnected,
            Status::Connecting,
            Status::Connected,
            Status::Blocked,
            Status::PortConflict,
            Status::Error,
        ] {
            assert_eq!(status_from_u8(status_to_u8(s)), s);
        }
    }
}
