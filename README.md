# i2p-client (Rust)

An **I2P client for 1M5** — routes [`Envelope`]s over I2P as repliable datagrams
through a router's [SAMv3](https://geti2p.net/en/docs/api/samv3) bridge. A Rust
port of the design in
[`i2p-java`](https://github.com/resolvingarchitecture/i2p-java).

Used as the I2P **protocol service** for
[`1m5-core-rust`](https://github.com/1m5/1m5-core-rust) via
`onemfive_core::protocol::I2pProtocolService`.

## Modes (`ra.i2p.mode`)

| mode | behaviour |
|------|-----------|
| `local` | attach to an I2P router already running on this host — needs the SAM bridge enabled (router console → *Clients* → *SAM application bridge*), default `127.0.0.1:7656` |
| `embedded` | start a pure-Rust I2P router in-process via [emissary](https://github.com/eepnet/emissary) and attach to its SAM bridge — **requires the `embedded` feature** |
| `auto` *(default)* | `local` if a SAM bridge answers, else `embedded` |

The `local`/`embedded`/`auto` split mirrors `i2p-java` and `1m5-android`'s
`I2P` / `I2PEmbedded` / `I2PLocal`.

## Use

```rust
use std::collections::HashMap;
use i2p_client::{I2pClient, Status};

let mut cfg = HashMap::new();
cfg.insert("ra.i2p.mode".into(), "auto".into());
cfg.insert("ra.i2p.dataDir".into(), "/home/me/.1m5/core/data/i2p".into());

let client = I2pClient::from_config(&cfg);
if client.start() {                       // false (cleanly) if I2P is unavailable
    assert_eq!(client.status(), Status::Connected);
    println!("my destination: {}", client.local_destination());

    let mut env = /* seda_bus::Envelope */;
    env.headers.insert("destination".into(), peer_dest);
    client.send(&mut env);

    if let Some((from, bytes)) = client.receive() { /* ... */ }
}
```

### Config keys

| key | default | meaning |
|-----|---------|---------|
| `ra.i2p.mode` | `auto` | `local` / `embedded` / `auto` |
| `ra.i2p.samHost` | `127.0.0.1` | SAM bridge host (local mode) |
| `ra.i2p.samPort` | `7656` | SAM bridge TCP port (local mode) |
| `ra.i2p.nickname` | `1m5` | SAM session id |
| `ra.i2p.dataDir` | *none* | where to persist the destination key and the embedded router's files |
| `ra.i2p.sessionTimeoutSecs` | `180` | SAM session-create timeout (cold routers are slow) |

## The `embedded` feature

```
cargo build --features embedded
```

Pulls in [emissary](https://github.com/eepnet/emissary) (`emissary-core` +
`emissary-util`, pinned by git rev) and a Tokio runtime. On first start the
router **reseeds over HTTPS** and then stores router infos under
`<dataDir>/emissary`, so subsequent starts are offline-capable. Tunnel build on a
cold router takes a minute or two — the first `send` blocks until the session's
tunnels are up.

Without the feature, `ra.i2p.mode=embedded` (or `auto` with no local router)
fails cleanly with a message pointing at a local router.

## Build

```
cargo test                       # default (SAM client only)
cargo test --features embedded    # + emissary embedded router
cargo clippy --all-targets
```

## Status

Early. The SAM datagram path (`local` mode) is implemented and unit-tested
against a fake bridge but **not yet field-tested** against a live router. The
`embedded` path follows emissary's `docs/embedding-rust.md` recipe. No hidden
service / streaming, no I2CP. See `DESIGN.md` and `TODO.md`.
