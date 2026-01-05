use std::{net::Ipv4Addr, os::fd::AsFd as _, sync::Arc, time::Duration};
use tokio::net::UdpSocket;

use anyhow::{Context, Result};

use log::info;

#[cfg(any(target_os = "android", target_os = "linux"))]
use nix::sys::socket::sockopt::BindToDevice;

use nix::sys::socket::{self, sockopt::ReuseAddr};

use broadcast::broadcast_task;
use listener::listen_task;

use crate::ssdp::broadcast::SSDPBroadcast;
use crate::ssdp::utils::InteractiveSSDP;

pub mod broadcast;
mod error;
pub mod listener;
pub mod packet;
pub mod utils;

// Listen socket binds to port 1900 to receive M-SEARCH queries
pub static LISTEN_ADDRESS: (Ipv4Addr, u16) = (Ipv4Addr::new(0, 0, 0, 0), 1900);

// Broadcast socket uses ephemeral port - some clients/network equipment
// ignore NOTIFY packets originating from port 1900
pub static BROADCAST_ADDRESS: (Ipv4Addr, u16) = (Ipv4Addr::new(0, 0, 0, 0), 0);

pub static SSDP_ADDRESS: (Ipv4Addr, u16) = (Ipv4Addr::new(239, 255, 255, 250), 1900);

pub struct SSDPManager {
    broadcast_period: Duration,
    listen_socket: Arc<UdpSocket>,
    broadcast_socket: Arc<UdpSocket>,
    interactive_ssdp: Arc<InteractiveSSDP>,
    broadcaster: Arc<SSDPBroadcast>,
}

impl SSDPManager {
    pub async fn new(
        endpoint_desc_url: &str,
        broadcast_period: Duration,
        connect_timeout: Option<Duration>,
        broadcast_iface: Option<String>,
    ) -> Result<Self> {
        let mut http_client = reqwest::Client::builder();

        if let Some(timeout) = connect_timeout {
            http_client = http_client.connect_timeout(timeout);
        }

        let http_client = http_client.build().context("Failed to build HTTP client")?;

        let (listen_socket, broadcast_socket) = ssdp_sockets(broadcast_iface).await?;

        let cache_max_age = match broadcast_period.as_secs() {
            n if n < 20 => 20,
            n => n * 2,
        } as usize;

        let interactive_ssdp = Arc::new(InteractiveSSDP::new(
            http_client,
            endpoint_desc_url,
            cache_max_age,
        ));

        let broadcaster = Arc::new(SSDPBroadcast::new(broadcast_socket.clone(), interactive_ssdp.clone()));

        Ok(SSDPManager {
            broadcast_period,
            listen_socket,
            broadcast_socket,
            interactive_ssdp,
            broadcaster,
        })
    }
}

async fn ssdp_sockets(broadcast_iface: Option<String>) -> Result<(Arc<UdpSocket>, Arc<UdpSocket>)> {
    // Listen socket on port 1900 for M-SEARCH queries
    let listen_socket = UdpSocket::bind(LISTEN_ADDRESS)
        .await
        .context("Failed to bind SSDP listen socket")?;

    socket::setsockopt(&listen_socket.as_fd(), ReuseAddr, &true)
        .context("Failed to set SO_REUSEADDR on listen socket.")?;

    // Broadcast socket on ephemeral port for NOTIFY announcements
    let broadcast_socket = UdpSocket::bind(BROADCAST_ADDRESS)
        .await
        .context("Failed to bind SSDP broadcast socket")?;

    if let Some(_iface) = broadcast_iface {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            let iface: std::ffi::OsString = std::ffi::OsString::from(_iface);

            socket::setsockopt(&listen_socket.as_fd(), BindToDevice, &iface)
                .context("Failed to set SO_BINDTODEVICE on listen socket.")?;

            socket::setsockopt(&broadcast_socket.as_fd(), BindToDevice, &iface)
                .context("Failed to set SO_BINDTODEVICE on broadcast socket.")?;
        }

        #[cfg(target_os = "macos")]
        panic!("Cannot set broadcast address on MacOS (yet)")
    }

    listen_socket
        .join_multicast_v4(SSDP_ADDRESS.0, Ipv4Addr::UNSPECIFIED)
        .context("Failed to join SSDP multicast group on listen socket.")?;

    broadcast_socket
        .join_multicast_v4(SSDP_ADDRESS.0, Ipv4Addr::UNSPECIFIED)
        .context("Failed to join SSDP multicast group on broadcast socket.")?;

    Ok((Arc::new(listen_socket), Arc::new(broadcast_socket)))
}

pub async fn main_task(ssdp: SSDPManager) -> Result<()> {
    info!(target: "dlnaproxy", "Launched main task...");

    //We send an initial byebye before all else because... that's how MiniDLNA does it.
    //Guessing that it's for clearing any cache that might exist on listening remote devices.
    ssdp.interactive_ssdp
        .send_byebye(&ssdp.broadcast_socket, SSDP_ADDRESS)
        .await
        .context("Failed to send initial ssdp:byebye !")?;

    let _broadcast_handle =
        tokio::task::spawn(broadcast_task(ssdp.broadcaster, ssdp.broadcast_period));

    // Listen task uses the socket bound to port 1900 to receive M-SEARCH queries
    let _listener_handle =
        tokio::task::spawn(listen_task(ssdp.listen_socket, ssdp.interactive_ssdp.clone()));

    let _ = _listener_handle.await;

    Ok(())
}
