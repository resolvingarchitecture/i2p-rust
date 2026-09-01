# Changelog

## 0.4.0 — 2026-08-31

Runtime backend switching for `auto` mode, matching the fallback behaviour added
to `tor-client-rust` 0.2.0 (both driven by `1m505`).

- **`auto` mode now switches at runtime.** An `active` backend (`AtomicU8`) plus
  `maybe_switch_backend()` at the top of `send()`: re-probes the local SAM port
  (rate-limited to every 30s) and opens a new session against the embedded
  router if the local one vanished, or back against the local router when it
  returns. The embedded router is kept warm across flaps; `stop()` drops it.
- `start_embedded()` is now idempotent (returns the running router's SAM
  addresses instead of starting a second one).
- `current_dest()` carries the live session's destination into the replacement
  session, so this node's address is stable across a switch.
- `start()` refactored around `open_session_on(backend, first_start)`; a failed
  runtime switch keeps the existing session instead of forcing `Error`.
- `Status` variants and config keys unchanged — the
  `onemfive_core::protocol::I2pProtocolService` adapter needs no change.

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
