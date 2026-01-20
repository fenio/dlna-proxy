use log::{debug, info, warn};
use tokio::net::UdpSocket;
use tokio::{signal, time};

#[cfg(unix)]
use tokio::signal::unix::{signal as unix_signal, SignalKind};

use std::borrow::Borrow as _;
use std::time::Duration;
use std::{process, sync::Arc};

use anyhow::Result;

use crate::ssdp::utils::InteractiveSSDP;
use crate::ssdp::SSDP_ADDRESS;

pub struct SSDPBroadcast {
    ssdp_socket: Arc<UdpSocket>,
    ssdp_helper: Arc<InteractiveSSDP>,
}

impl SSDPBroadcast {
    pub fn new(ssdp_socket: Arc<UdpSocket>, ssdp_helper: Arc<InteractiveSSDP>) -> Self {
        SSDPBroadcast {
            ssdp_socket,
            ssdp_helper,
        }
    }

    pub async fn do_ssdp_alive(&self) -> Result<()> {
        self.ssdp_helper
            .send_alive(self.ssdp_socket.borrow(), SSDP_ADDRESS)
            .await
    }
}

pub async fn broadcast_task(broadcaster: Arc<SSDPBroadcast>, period: Duration) {
    let _handle = tokio::spawn(shutdown_handler(broadcaster.clone()));

    debug!(target: "dlnaproxy", "About to schedule broadcast every {}s", period.as_secs());

    let mut interval = time::interval(period);

    loop {
        if let Err(msg) = broadcaster.do_ssdp_alive().await {
            warn!(target: "dlnaproxy", "Couldn't send ssdp:alive: {}. Will retry next interval.", msg);
            // Continue instead of break - origin may come back online
        } else {
            info!(target: "dlnaproxy", "Broadcasted on local SSDP channel!");
        }

        interval.tick().await;
    }
}

/// Waits for a shutdown signal (SIGINT or SIGTERM on Unix, Ctrl+C on Windows)
async fn wait_for_shutdown_signal() -> Result<&'static str> {
    #[cfg(unix)]
    {
        let mut sigterm = unix_signal(SignalKind::terminate())
            .map_err(|e| anyhow::anyhow!("Failed to install SIGTERM handler: {}", e))?;

        let signal_name = tokio::select! {
            result = signal::ctrl_c() => {
                result.map_err(|e| anyhow::anyhow!("Failed to wait for SIGINT: {}", e))?;
                "SIGINT"
            }
            _ = sigterm.recv() => "SIGTERM",
        };
        Ok(signal_name)
    }

    #[cfg(not(unix))]
    {
        signal::ctrl_c()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to install Ctrl+C handler: {}", e))?;
        Ok("Ctrl+C")
    }
}

pub async fn shutdown_handler(broadcaster: Arc<SSDPBroadcast>) -> Result<()> {
    debug!(target:"dlnaproxy", "Shutdown handler waiting for SIGINT/SIGTERM...");

    let signal_name = match wait_for_shutdown_signal().await {
        Ok(name) => name,
        Err(e) => {
            warn!(target: "dlnaproxy", "Failed to set up signal handler: {}. Shutdown handler disabled.", e);
            // Wait indefinitely since we can't catch signals
            std::future::pending::<()>().await;
            unreachable!()
        }
    };

    let socket = broadcaster.ssdp_socket.clone();
    let helper = broadcaster.ssdp_helper.clone();

    debug!(target:"dlnaproxy", "{} received, sending ssdp:byebye!", signal_name);

    // Use a timeout for the byebye message to ensure we exit promptly
    let byebye_future = helper.send_byebye(&socket, SSDP_ADDRESS);
    match time::timeout(Duration::from_secs(2), byebye_future).await {
        Ok(Ok(())) => {}
        Ok(Err(msg)) => warn!(target: "dlnaproxy", "Failed to send ssdp:byebye: {}", msg),
        Err(_) => warn!(target: "dlnaproxy", "Timeout sending ssdp:byebye"),
    }

    info!(target: "dlnaproxy", "Exiting!");

    process::exit(0);
}
