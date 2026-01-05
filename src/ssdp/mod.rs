use std::{net::{Ipv4Addr, SocketAddrV4}, sync::Arc, time::Duration};
use tokio::net::UdpSocket;
use socket2::{Domain, Protocol, Socket, Type};

use anyhow::{Context, Result};

use log::info;

#[cfg(any(target_os = "android", target_os = "linux"))]
use std::os::fd::AsFd as _;

#[cfg(any(target_os = "android", target_os = "linux"))]
use nix::sys::socket::{self, sockopt::BindToDevice};

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
    // Create listen socket using socket2 to set SO_REUSEADDR/SO_REUSEPORT BEFORE binding
    let listen_socket = {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .context("Failed to create listen socket")?;

        // Set SO_REUSEADDR before binding - allows multiple processes to bind to the same port
        socket.set_reuse_address(true)
            .context("Failed to set SO_REUSEADDR on listen socket")?;

        // On Linux, also set SO_REUSEPORT for multicast
        #[cfg(target_os = "linux")]
        socket.set_reuse_port(true)
            .context("Failed to set SO_REUSEPORT on listen socket")?;

        // Bind to port 1900 for M-SEARCH queries
        let addr = SocketAddrV4::new(LISTEN_ADDRESS.0, LISTEN_ADDRESS.1);
        socket.bind(&addr.into())
            .context("Failed to bind SSDP listen socket")?;

        socket.set_nonblocking(true)
            .context("Failed to set non-blocking on listen socket")?;

        // Convert to tokio UdpSocket
        let std_socket: std::net::UdpSocket = socket.into();
        UdpSocket::from_std(std_socket)
            .context("Failed to convert listen socket to tokio")?
    };

    // Create broadcast socket using socket2
    let broadcast_socket = {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .context("Failed to create broadcast socket")?;

        socket.set_reuse_address(true)
            .context("Failed to set SO_REUSEADDR on broadcast socket")?;

        #[cfg(target_os = "linux")]
        socket.set_reuse_port(true)
            .context("Failed to set SO_REUSEPORT on broadcast socket")?;

        // Bind to ephemeral port for NOTIFY announcements
        let addr = SocketAddrV4::new(BROADCAST_ADDRESS.0, BROADCAST_ADDRESS.1);
        socket.bind(&addr.into())
            .context("Failed to bind SSDP broadcast socket")?;

        socket.set_nonblocking(true)
            .context("Failed to set non-blocking on broadcast socket")?;

        let std_socket: std::net::UdpSocket = socket.into();
        UdpSocket::from_std(std_socket)
            .context("Failed to convert broadcast socket to tokio")?
    };

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
