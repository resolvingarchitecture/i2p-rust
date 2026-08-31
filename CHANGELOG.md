# Changelog

## 0.3.0 — 2026-08-31

Rewrite as a 1M5 protocol client, porting the design of `i2p-java` 1.7.x.
(0.2.x was a standalone SAMv3 datagram CLI.)

- `sam` — blocking SAMv3 client: `SamConnection` (HELLO / NAMING LOOKUP / DEST
  GENERATE, line protocol with SAM-result → `io::Error` mapping) and
  `DatagramSession` (`SESSION CREATE STYLE=DATAGRAM` + a bound UDP socket for
  repliable datagrams). Hand-rolled parsing, no `nom`.
- `LocalRouterDetector` — TCP-probes the SAM bridge port (`7656`).
- `I2pClient` — `Mode { Local, Embedded, Auto }` (config `ra.i2p.mode`),
  `from_config`, `start` / `stop` / `send` / `receive`, `local_destination`,
  `Status { Disconnected, Connecting, Connected, Blocked, PortConflict, Error }`
  as an `AtomicU8`. Persists the destination key to `<dataDir>/i2p/dest.b64`
  for a stable address.
- `embedded` feature — starts a pure-Rust I2P router via
  [emissary](https://github.com/eepnet/emissary) (`emissary-core` +
  `emissary-util`, pinned by git rev) on a Tokio thread, reseeds on first run,
  and exposes its SAM bridge. Follows emissary's `docs/embedding-rust.md`.
- Consumed by `1m5-core-rust` as `onemfive_core::protocol::I2pProtocolService`.

Not yet field-tested against a live router. Not yet: streaming, I2CP, eepsite
hosting, inbound-datagram → bus wiring, emissary `PortMapper`.
