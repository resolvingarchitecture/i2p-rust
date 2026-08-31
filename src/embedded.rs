//! Embedded I2P router via [emissary](https://github.com/eepnet/emissary), a
//! pure-Rust I2P stack. Enabled by the `embedded` feature.
//!
//! [`EmbeddedRouter::start`] boots a Tokio runtime on a dedicated thread, reseeds
//! on first run, builds an `emissary_core::Router` with the SAMv3 bridge enabled
//! on OS-assigned ports, and drives it in the background. The rest of `i2p-rust`
//! then talks to it over SAMv3 exactly like it would to a system router — see
//! [`crate::sam`].
//!
//! Follows emissary's `docs/embedding-rust.md` recipe.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use emissary_core::{
    router::{ProtocolAddressInfo, Router},
    Config, Ntcp2Config, SamConfig, Ssu2Config, TransitConfig,
};
use emissary_util::{
    reseeder::Reseeder,
    runtime::tokio::Runtime,
    storage::{Storage, StorageBundle},
};
use log::{info, warn};
use tokio::sync::oneshot;

/// A running embedded emissary router. Dropping it asks the router to shut down.
pub struct EmbeddedRouter {
    sam_tcp: SocketAddr,
    sam_udp: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl EmbeddedRouter {
    /// Start the router, storing its files under `base_path` (e.g.
    /// `~/.1m5/core/data/i2p/emissary`). Blocks until the SAM bridge is bound
    /// (not until tunnels are built — the first SAM `SESSION CREATE` waits for
    /// that and can take a minute or two on a cold router).
    pub fn start(base_path: PathBuf) -> io::Result<EmbeddedRouter> {
        let (ready_tx, ready_rx) = mpsc::channel::<io::Result<(SocketAddr, SocketAddr)>>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let handle = thread::Builder::new()
            .name("i2p-emissary".into())
            .spawn(move || run(base_path, ready_tx, shutdown_rx))
            .map_err(|e| io::Error::other(format!("spawn emissary thread: {e}")))?;

        let (sam_tcp, sam_udp) = ready_rx
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| io::Error::other("emissary did not become ready within 120s"))??;

        info!("embedded I2P (emissary) SAM bridge on tcp={sam_tcp} udp={sam_udp}");
        Ok(EmbeddedRouter {
            sam_tcp,
            sam_udp,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        })
    }

    /// `host:port` of the SAM TCP control listener.
    pub fn sam_tcp_addr(&self) -> String {
        self.sam_tcp.to_string()
    }

    /// `host:port` of the SAM UDP datagram listener.
    pub fn sam_udp_addr(&self) -> String {
        self.sam_udp.to_string()
    }
}

impl Drop for EmbeddedRouter {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(
    base_path: PathBuf,
    ready_tx: mpsc::Sender<io::Result<(SocketAddr, SocketAddr)>>,
    shutdown_rx: oneshot::Receiver<()>,
) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready_tx.send(Err(io::Error::other(format!("tokio runtime: {e}"))));
            return;
        }
    };

    rt.block_on(async move {
        let (mut router, addrs) = match build_router(base_path).await {
            Ok(v) => v,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        if ready_tx.send(Ok(addrs)).is_err() {
            return; // caller gave up
        }

        tokio::select! {
            _ = &mut router => warn!("embedded I2P router exited on its own"),
            _ = shutdown_rx => {
                info!("shutting down embedded I2P router");
                router.shutdown();
                // give graceful shutdown a moment
                let _ = tokio::time::timeout(Duration::from_secs(5), &mut router).await;
            }
        }
    });
}

async fn build_router(
    base_path: PathBuf,
) -> io::Result<(Router<Runtime>, (SocketAddr, SocketAddr))> {
    let storage = Storage::new::<Runtime>(Some(base_path))
        .await
        .map_err(|e| io::Error::other(format!("emissary storage: {e}")))?;

    let StorageBundle {
        ntcp2_iv,
        ntcp2_key,
        profiles,
        router_info,
        mut routers,
        signing_key,
        static_key,
        ssu2_intro_key,
        ssu2_static_key,
    } = storage.load().await;

    if routers.is_empty() {
        info!("reseeding embedded I2P router (first run)");
        match Reseeder::reseed::<Runtime>(None, false).await {
            Ok(reseed_routers) => {
                for info in reseed_routers {
                    let _ = storage
                        .store_router_info(info.name.to_string(), info.router_info.clone())
                        .await;
                    routers.push(info.router_info);
                }
            }
            Err(e) if routers.is_empty() => {
                return Err(io::Error::other(format!(
                    "reseed failed, cannot start: {e}"
                )));
            }
            Err(e) => warn!(
                "reseed failed, starting with {} routers: {e}",
                routers.len()
            ),
        }
    }

    let config = Config {
        ntcp2: Some(Ntcp2Config {
            port: 0,
            key: ntcp2_key,
            iv: ntcp2_iv,
            publish_ipv4: true,
            publish_ipv6: true,
            ipv4_host: None,
            ipv6_host: None,
            ipv4: true,
            ipv6: true,
            ml_kem: Some(4),
            disable_pq: false,
            max_connections: None,
        }),
        ssu2: Some(Ssu2Config {
            intro_key: ssu2_intro_key,
            static_key: ssu2_static_key,
            ipv4: true,
            ipv4_host: None,
            ipv6: true,
            ipv6_host: None,
            port: 0,
            publish_ipv4: true,
            publish_ipv6: true,
            ipv4_mtu: None,
            ipv6_mtu: None,
            disable_pq: false,
            ml_kem: Some("4".to_string()),
            max_connections: None,
        }),
        samv3_config: Some(SamConfig {
            tcp_port: 0,
            udp_port: 0,
            host: "127.0.0.1".to_string(),
        }),
        routers,
        profiles,
        router_info,
        static_key: Some(static_key),
        signing_key: Some(signing_key),
        transit: Some(TransitConfig {
            max_tunnels: Some(1000),
        }),
        ..Default::default()
    };

    let (router, _events, local_router_info) =
        Router::<Runtime>::new(config, None, Some(std::sync::Arc::new(storage.clone())))
            .await
            .map_err(|e| io::Error::other(format!("emissary router: {e}")))?;

    storage
        .store_local_router_info(local_router_info)
        .await
        .map_err(|e| io::Error::other(format!("store router info: {e}")))?;

    // Note: emissary's `PortMapper` (NAT-PMP/UPnP) is intentionally not wired in
    // here to keep the surface small; inbound reachability may be lower without
    // it. See emissary `docs/embedding-rust.md` to add it.

    let ProtocolAddressInfo {
        sam_tcp, sam_udp, ..
    } = *router.protocol_address_info();
    let sam_tcp = sam_tcp.ok_or_else(|| io::Error::other("emissary SAM TCP not bound"))?;
    let sam_udp = sam_udp.ok_or_else(|| io::Error::other("emissary SAM UDP not bound"))?;

    Ok((router, (sam_tcp, sam_udp)))
}
