//! A minimal blocking [SAMv3](https://geti2p.net/en/docs/api/samv3) client.
//!
//! Enough to open a `DATAGRAM` session against a router's SAM bridge
//! (`127.0.0.1:7656` by default), look up destinations, and send / receive
//! repliable datagrams over the SAMv3 UDP forwarding path (`127.0.0.1:7655`).
//!
//! Ported and modernised from the original `i2p-rust` 0.2.x (dropping the
//! `nom` v2 dependency for hand-rolled line parsing).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use log::{debug, warn};

pub const DEFAULT_SAM_TCP: &str = "127.0.0.1:7656";
pub const DEFAULT_SAM_UDP: &str = "127.0.0.1:7655";
const MIN_VERSION: &str = "3.1";
const MAX_VERSION: &str = "3.3";

/// Signature type for generated destinations. Ed25519 is the current default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigType {
    EdDsaSha512Ed25519,
    RedDsaSha512Ed25519,
    DsaSha1,
}

impl SigType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SigType::EdDsaSha512Ed25519 => "EdDSA_SHA512_Ed25519",
            SigType::RedDsaSha512Ed25519 => "RedDSA_SHA512_Ed25519",
            SigType::DsaSha1 => "DSA_SHA1",
        }
    }
}

/// Parse a SAM reply line (`HELLO REPLY RESULT=OK VERSION=3.3`) into its
/// `KEY=VALUE` pairs. Bare tokens (the command echo) are ignored.
fn parse_reply(line: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for tok in line.split_whitespace() {
        if let Some((k, v)) = tok.split_once('=') {
            map.insert(k.to_string(), v.trim_matches('"').to_string());
        }
    }
    map
}

fn check_result(map: &HashMap<String, String>) -> io::Result<()> {
    match map.get("RESULT").map(String::as_str).unwrap_or("OK") {
        "OK" => Ok(()),
        "CANT_REACH_PEER" | "KEY_NOT_FOUND" | "PEER_NOT_FOUND" => {
            Err(io::Error::new(io::ErrorKind::NotFound, message(map)))
        }
        "DUPLICATED_DEST" | "DUPLICATED_ID" => {
            Err(io::Error::new(io::ErrorKind::AlreadyExists, message(map)))
        }
        "INVALID_KEY" | "INVALID_ID" => {
            Err(io::Error::new(io::ErrorKind::InvalidInput, message(map)))
        }
        "TIMEOUT" => Err(io::Error::new(io::ErrorKind::TimedOut, message(map))),
        other => Err(io::Error::other(format!(
            "SAM error {other}: {}",
            message(map)
        ))),
    }
}

fn message(map: &HashMap<String, String>) -> String {
    map.get("MESSAGE").cloned().unwrap_or_default()
}

/// One SAM control connection (a TCP socket speaking the SAM line protocol).
pub struct SamConnection {
    stream: TcpStream,
}

impl SamConnection {
    pub fn connect<A: ToSocketAddrs>(addr: A, timeout: Duration) -> io::Result<SamConnection> {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("no SAM address"))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_read_timeout(Some(timeout.max(Duration::from_secs(60))))?;
        stream.set_write_timeout(Some(timeout))?;
        let mut c = SamConnection { stream };
        c.hello()?;
        Ok(c)
    }

    fn send_line(&mut self, line: &str) -> io::Result<HashMap<String, String>> {
        debug!("-> {}", line.trim_end());
        let mut framed = line.trim_end().to_string();
        framed.push('\n');
        self.stream.write_all(framed.as_bytes())?;
        let mut reader = BufReader::new(&self.stream);
        let mut buf = String::new();
        reader.read_line(&mut buf)?;
        debug!("<- {}", buf.trim_end());
        let map = parse_reply(&buf);
        check_result(&map)?;
        Ok(map)
    }

    fn hello(&mut self) -> io::Result<()> {
        let reply = self.send_line(&format!(
            "HELLO VERSION MIN={MIN_VERSION} MAX={MAX_VERSION}"
        ))?;
        if reply.get("RESULT").map(String::as_str) == Some("NOVERSION") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SAM bridge supports no compatible version",
            ));
        }
        Ok(())
    }

    /// Resolve a name (`"ME"` for this session's own destination) to a full
    /// base64 destination.
    pub fn naming_lookup(&mut self, name: &str) -> io::Result<String> {
        let reply = self.send_line(&format!("NAMING LOOKUP NAME={name}"))?;
        reply
            .get("VALUE")
            .cloned()
            .ok_or_else(|| io::Error::other("NAMING REPLY without VALUE"))
    }

    /// Generate a fresh destination keypair. Returns `(public_dest, private_key)`.
    pub fn generate_dest(&mut self, sig: SigType) -> io::Result<(String, String)> {
        let reply = self.send_line(&format!("DEST GENERATE SIGNATURE_TYPE={}", sig.as_str()))?;
        let pubk = reply.get("PUB").cloned();
        let privk = reply.get("PRIV").cloned();
        match (pubk, privk) {
            (Some(p), Some(s)) => Ok((p, s)),
            _ => Err(io::Error::other("DEST REPLY missing PUB/PRIV")),
        }
    }

    fn try_clone(&self) -> io::Result<SamConnection> {
        Ok(SamConnection {
            stream: self.stream.try_clone()?,
        })
    }
}

/// A SAMv3 `DATAGRAM` session plus a bound UDP socket for repliable datagrams.
pub struct DatagramSession {
    control: SamConnection,
    udp: UdpSocket,
    sam_udp_addr: String,
    nickname: String,
    /// Full base64 destination (dest + private keys) for this session.
    pub local_full_dest: String,
    /// Short base64 destination others send to (516 bytes).
    pub local_dest: String,
}

impl DatagramSession {
    /// Open a datagram session. `destination` is `"TRANSIENT"` or a full
    /// base64 private destination to reuse a stable address.
    pub fn open(
        sam_tcp: &str,
        sam_udp: &str,
        nickname: &str,
        destination: &str,
        timeout: Duration,
    ) -> io::Result<DatagramSession> {
        let udp = UdpSocket::bind("127.0.0.1:0")?;
        udp.set_read_timeout(Some(Duration::from_millis(500)))?;
        let udp_port = udp.local_addr()?.port();

        let mut control = SamConnection::connect(sam_tcp, timeout)?;
        let reply = control.send_line(&format!(
            "SESSION CREATE STYLE=DATAGRAM ID={nickname} DESTINATION={destination} \
             HOST=127.0.0.1 PORT={udp_port}"
        ))?;
        let local_full_dest = reply
            .get("DESTINATION")
            .cloned()
            .unwrap_or_else(|| destination.to_string());
        let local_dest = control.naming_lookup("ME").unwrap_or_default();

        Ok(DatagramSession {
            control,
            udp,
            sam_udp_addr: sam_udp.to_string(),
            nickname: nickname.to_string(),
            local_full_dest,
            local_dest,
        })
    }

    pub fn naming_lookup(&mut self, name: &str) -> io::Result<String> {
        self.control.naming_lookup(name)
    }

    pub fn generate_dest(&mut self, sig: SigType) -> io::Result<(String, String)> {
        self.control.generate_dest(sig)
    }

    /// Send a repliable datagram to `dest` (a full or short base64 destination
    /// or a hostname the router can resolve).
    pub fn send(&self, dest: &str, payload: &[u8]) -> io::Result<()> {
        if payload.len() > 31_744 {
            warn!(
                "I2P datagram is {} bytes; keep well under 32 KB (ideally < 11 KB)",
                payload.len()
            );
        }
        let header = format!("3.3 {} {}\n", self.nickname, dest);
        let mut packet = Vec::with_capacity(header.len() + payload.len());
        packet.extend_from_slice(header.as_bytes());
        packet.extend_from_slice(payload);
        self.udp.send_to(&packet, &self.sam_udp_addr)?;
        Ok(())
    }

    /// Receive one repliable datagram: `(from_destination, payload)`. Blocks up
    /// to the UDP read timeout; `WouldBlock` / `TimedOut` mean "nothing yet".
    pub fn receive(&self) -> io::Result<(String, Vec<u8>)> {
        let mut buf = vec![0u8; 65_536];
        let (n, _) = self.udp.recv_from(&mut buf)?;
        buf.truncate(n);
        // Forwarded datagram = one header line ("<from_dest>\n") then raw payload.
        let split = buf.iter().position(|&b| b == b'\n').unwrap_or(0);
        let from = String::from_utf8_lossy(&buf[..split]).trim().to_string();
        let payload = buf[split + 1..].to_vec();
        Ok((from, payload))
    }

    /// A second handle to the control connection (e.g. for a reader thread).
    pub fn try_clone_control(&self) -> io::Result<SamConnection> {
        self.control.try_clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_hello_reply() {
        let m = parse_reply("HELLO REPLY RESULT=OK VERSION=3.3");
        assert_eq!(m.get("RESULT").unwrap(), "OK");
        assert_eq!(m.get("VERSION").unwrap(), "3.3");
        assert!(check_result(&m).is_ok());
    }

    #[test]
    fn maps_a_sam_error_to_io_error() {
        let m = parse_reply("SESSION STATUS RESULT=DUPLICATED_ID MESSAGE=\"in use\"");
        let e = check_result(&m).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    }
}
