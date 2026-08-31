use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use i2p_client::{I2pClient, LocalRouterDetector, Mode, Status};

#[test]
fn detector_reports_nothing_on_a_closed_port() {
    let d = LocalRouterDetector {
        sam_port: 7699,
        timeout: Duration::from_millis(300),
        ..Default::default()
    };
    assert!(!d.is_local_router_running());
}

#[test]
fn auto_mode_without_a_router_resolves_to_embedded_and_start_is_clean() {
    let mut cfg = HashMap::new();
    cfg.insert("ra.i2p.samPort".into(), "7699".into()); // nothing there
    let client = I2pClient::from_config(&cfg);
    assert_eq!(client.mode(), Mode::Auto);
    // No `embedded` feature in the default test build -> start() fails cleanly.
    assert!(!client.start());
    assert!(matches!(
        client.status(),
        Status::Error | Status::Disconnected
    ));
}

#[test]
fn local_mode_speaks_sam_to_a_fake_bridge() {
    // A fake SAM bridge: HELLO + SESSION CREATE + NAMING LOOKUP ME.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { continue };
            let mut buf = [0u8; 512];
            // HELLO (the detector probes first and closes; skip that connection)
            let n = s.read(&mut buf).unwrap_or(0);
            if !String::from_utf8_lossy(&buf[..n]).starts_with("HELLO") {
                continue;
            }
            s.write_all(b"HELLO REPLY RESULT=OK VERSION=3.3\n").unwrap();
            // SESSION CREATE
            let n = s.read(&mut buf).unwrap();
            let line = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(line.contains("SESSION CREATE STYLE=DATAGRAM"));
            s.write_all(b"SESSION STATUS RESULT=OK DESTINATION=abcdEF~full-priv-dest-AAAA\n")
                .unwrap();
            // NAMING LOOKUP ME
            let n = s.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("NAMING LOOKUP NAME=ME"));
            s.write_all(b"NAMING REPLY RESULT=OK NAME=ME VALUE=shortDestAAAA\n")
                .unwrap();
            return;
        }
    });

    let mut cfg = HashMap::new();
    cfg.insert("ra.i2p.mode".into(), "local".into());
    cfg.insert("ra.i2p.samPort".into(), port.to_string());
    let client = I2pClient::from_config(&cfg);
    assert_eq!(client.mode(), Mode::Local);
    assert!(
        client.start(),
        "start() should succeed against the fake bridge"
    );
    assert_eq!(client.status(), Status::Connected);
    assert_eq!(client.local_destination(), "shortDestAAAA");

    // send() pushes a datagram to the SAM UDP address; just check it doesn't error
    // on a well-formed envelope (UDP send to a likely-closed port still returns Ok).
    let mut env = seda_bus::Envelope::new("shortDestAAAA", b"ping".to_vec());
    let _ = client.send(&mut env);

    let mut env2 = seda_bus::Envelope::new("", Vec::new());
    assert!(!client.send(&mut env2)); // no destination
    assert!(env2.headers.contains_key("error"));
}
