# i2p-client (Rust) — TODO

## P0 — auto backend switching (done)
- [x] `active` backend tracking + runtime `local`↔`embedded` switch in `auto`
      mode (rate-limited local SAM re-probe in `send()`), matching
      `tor-client-rust`.
- [x] Idempotent `start_embedded()`; keep the embedded router warm across flaps.
- [ ] Field-test the switch: kill/restart a local router under load, confirm
      the session moves and this node's destination stays stable.
- [ ] Also re-probe from `receive()` (or a low-rate ticker) so a long-idle
      sender still recovers without an outbound send.

## P1 — datagram path hardening
- [ ] Field-test `local` mode against a live I2P / i2pd router.
- [ ] Handle SAM async status lines (session dropped, router restart) — reader
      thread on the control socket updating `Status` (a more precise switch
      trigger than the port probe).
- [ ] Reconnect / re-establish the session on drop (i2p-java's `restart()`).
- [ ] Chunk payloads above the SAM datagram limit, or reject with a clear error.
- [ ] Surface `Blocked` / `PortConflict` from real router conditions, not just
      the enum being present.

## P2 — embedded (emissary)
- [ ] Field-test the `embedded` feature end to end (reseed → tunnels → send).
- [ ] **Redox support** — `emissary-util` won't cross-compile to
      `x86_64-unknown-redox` (`netdev`/`natpmp`/`igd-next` port-mapping deps are
      unconditional; `netdev` has no Redox impl). Fork `eepnet/emissary` →
      `resolvingarchitecture/emissary` branch `port-mapping-optional`: add a
      `port-mapping` feature (in `default`), make those deps `optional`,
      `#[cfg]`-gate `PortMapper`. Point the git dep at the fork rev with
      `default-features = false` and no `port-mapping`; submit upstream as a PR.
      Then clear the next blocker (likely `reqwest`). See
      `1m5/1m505/docs/redox-build-spike.md` §2.1.
- [ ] Document the emissary-fork upgrade procedure (rebase on upstream per bump
      → build Linux + `x86_64-unknown-redox` → live `embedded_*` test → bump
      rev + CHANGELOG).
- [ ] Readiness signal: use `EventSubscriber` (or a SAM probe) to report
      `Connecting` → `Connected` when tunnels are actually built, instead of the
      first `SESSION CREATE` blocking.
- [ ] Wire emissary's `PortMapper` (NAT-PMP/UPnP) for inbound reachability.
- [ ] Pin emissary by tag/version once it publishes to crates.io.
- [ ] Graceful-shutdown timeout tuning; make sure `Drop` never hangs the bus.

## P3 — inbound into the bus
- [ ] A receive loop that turns inbound datagrams into `Envelope`s and hands
      them to `1m5-core-rust` (mirrors `I2PServiceSession.messageAvailable`).
- [ ] Record sender destination / fingerprint on the envelope.

## P4 — feature parity with i2p-java
- [ ] `STREAM` sessions for request/response and eepsite fetches.
- [ ] Hidden-service (eepsite) hosting for this node.
- [ ] I2PControl / router-console status polling.
- [ ] Peer-list exchange (`NetOpReq`/`NetOpRes`).

## Testing / ops
- [ ] Integration test behind a `live` feature (real local router).
- [ ] CI: `cargo test`, `cargo test --features embedded`, `cargo clippy -- -D warnings`.
- [ ] Publish to crates.io once the API settles.

## Cross-repo
- [ ] Keep `Status` and config keys aligned with `i2p-java` 1.7.x and the
      `onemfive_core::protocol::I2pProtocolService` adapter.
