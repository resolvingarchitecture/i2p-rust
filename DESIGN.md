# i2p-client (Rust) — Design

Routes 1M5 `Envelope`s over I2P. A Rust port of
[`i2p-java`](https://github.com/resolvingarchitecture/i2p-java), talking to an
I2P router over **SAMv3** rather than linking the router's API directly.

## Where it sits

    1m5-core-rust  ──wraps──►  i2p_client::I2pClient
      onemfive_core::protocol::I2pProtocolService (impl Service + Transport)
                                     │  SAMv3 (TCP control + UDP datagrams)
                     ┌───────────────┴───────────────┐
              local router                    embedded router
              SAM bridge 127.0.0.1:7656       emissary (feature "embedded"),
              (I2P / i2pd, SAM enabled)       SAM bound to OS-assigned ports

`1m5-core-rust`'s `RoutingService` discovers the protocol service by name and
pushes a routing-slip hop; the adapter calls `I2pClient::send` with the
destination in `envelope.headers["destination"]`.

## Components

    detector::LocalRouterDetector   TCP-probes the SAM port to choose local vs embedded
    sam::SamConnection              one SAM control socket: HELLO, NAMING LOOKUP,
                                    DEST GENERATE, line protocol + error mapping
    sam::DatagramSession            SESSION CREATE STYLE=DATAGRAM + a bound UDP socket;
                                    send/receive repliable datagrams over SAMv3 UDP
    embedded::EmbeddedRouter        (feature "embedded") boots emissary on a Tokio
                                    thread, reseeds, drives the router future
    I2pClient                       mode resolution, active-backend tracking +
                                    runtime local<->embedded switching, status,
                                    destination persistence, Envelope <-> datagram

## Modes

`Mode { Local, Embedded, Auto }` from `ra.i2p.mode`. `effective_mode()` resolves
`Auto` at `start()` → `Local` if the SAM port answers, else `Embedded`. Same
shape as `i2p-java`'s `RouterMode`, `1m5-android`'s `I2P`/`I2PEmbedded`/
`I2PLocal`, and `tor-client-rust`'s `ra.tor.mode`.

### Runtime backend switching (auto only)

An `active: AtomicU8` (`None`/`Local`/`Embedded`) tracks which backend the
current session runs on. At the top of every `send()`, `maybe_switch_backend()`
re-probes the local SAM port — rate-limited to once per `LOCAL_REPROBE_INTERVAL`
(30s) via an epoch-millis `AtomicU64`:

- active `Local` + SAM port gone → open a new session against the embedded
  router (`start_embedded()`, which is idempotent and keeps the router warm).
- active `Embedded` + SAM port back → open a new session against the local
  router; the embedded router is left running so a flapping local router
  doesn't cost repeated reseed/bootstrap. `stop()` drops it.

`current_dest()` feeds the *current* session's full destination into the new
session (falling back to the persisted/transient value), so this node's address
survives the switch. Detection is port-probe based, not session-error based:
outbound is UDP to the SAM forwarder, which does not fail when the router dies.
The switch shows up to `1m5-core-rust` only as a `NetworkStatus` no-op
(`Connected` throughout).

### Embedded (emissary)

Follows emissary's `docs/embedding-rust.md`:

1. `Storage::new(<dataDir>/emissary)` then `storage.load()` for keys + known routers.
2. If no known routers, `Reseeder::reseed()` over HTTPS; store the results.
3. Build `emissary_core::Config` — NTCP2 + SSU2 enabled (OS-assigned port),
   `SamConfig` on `127.0.0.1:0/0`, ML-KEM-768, transit tunnels capped at 1000.
4. `Router::new(config, None, Some(storage))`, persist the returned router info
   for a stable router ID, read `protocol_address_info()` for the SAM addresses.
5. Spawn the `Router` future on the runtime; `Drop` calls `Router::shutdown()`.

Everything else in the crate then treats the embedded router like any SAM bridge.
The NAT-PMP/UPnP `PortMapper` from the recipe is deliberately left out for now
(smaller surface; lower inbound reachability).

## Message flow

**Outbound** — `1m5-core-rust` routes an `Envelope` whose
`headers["destination"]` (or `to`) is a base64 I2P destination. `I2pClient::send`
UDP-forwards `3.3 <nickname> <dest>\n<payload>` to the SAM UDP port.

**Inbound** — the SAM bridge forwards received datagrams to the client's bound
UDP socket as `<from_dest>\n<payload>`. `I2pClient::receive` returns the next one
(non-blocking-ish: a 500 ms UDP read timeout). Wiring this into the bus as
inbound `Envelope`s is a `1m5-core-rust` concern (`TODO`).

## Identity / destination

`DEST GENERATE` (or `SESSION CREATE DESTINATION=TRANSIENT`) yields a keypair. If
`ra.i2p.dataDir` is set, the full private destination is written to
`<dataDir>/i2p/dest.b64` and reused on the next start for a stable address —
same intent as `i2p-java` persisting the destination and `1m5-android`'s
`I2PEmbedded` key handling.

## Status model

`Status { Disconnected, Connecting, Connected, Blocked, PortConflict, Error }` as
an `AtomicU8`. `1m5-core-rust`'s `I2pProtocolService::map_status` maps these onto
its own `NetworkStatus`. `start()` never panics or blocks indefinitely; it
returns `false` and a `Disconnected`/`Error` status when I2P is unavailable.

## Rust adaptations vs. the Java service

- No `NetworkService` base class — `I2pClient` is a plain struct; the bus
  lifecycle lives in `1m5-core-rust`'s adapter.
- SAMv3 (external, process-isolatable) instead of linking a router library on
  the JVM; the embedded router is a separate pure-Rust stack (emissary) driven
  on its own thread, not an in-process Java router.
- Hand-rolled SAM line parsing (no `nom`, unlike `i2p-rust` 0.2.x).
- No `CheckRouterStatus` task — status is set at lifecycle transitions; a live
  poll would query the router console / I2CP.

## Not here

- I2P streaming (`STREAM`), I2CP, hidden-service / eepsite hosting.
- Router console / I2PControl status polling.
- Peer-list exchange with a network-manager service (`NetOpReq`/`NetOpRes` in Java).
- emissary `PortMapper` (NAT-PMP/UPnP) wiring.
- Datagram size chunking above the ~32 KB SAM limit.
