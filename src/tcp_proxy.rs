use log::{debug, info, trace, warn};

use std::{
    io,
    net::{SocketAddr, TcpListener, TcpStream},
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

//Adapted from https://github.com/hishboy/rust-tcp-proxy/

pub struct TCPProxy {
    connect_timeout: Duration,
    stream_timeout: Duration,
}

impl TCPProxy {
    pub fn new(connect_timeout: Duration, stream_timeout: Duration) -> Self {
        TCPProxy {
            connect_timeout,
            stream_timeout,
        }
    }

    pub fn start(self, to: SocketAddr, from: SocketAddr) -> JoinHandle<()> {
        let listener = TcpListener::bind(from).expect("Unable to bind proxy addr");

        info!(target: "dlnaproxy", "Proxing TCP connections from {} to {}.", from, to);

        thread::spawn(self.listen_loop(listener, to))
    }

    fn listen_loop(self, listener: TcpListener, origin: SocketAddr) -> impl FnOnce() {
        let connect_timeout = self.connect_timeout;
        let stream_timeout = self.stream_timeout;

        move || {
            for incoming_stream in listener.incoming() {
                let proxied_stream = match incoming_stream {
                    Ok(stream) => stream,
                    Err(e) => {
                        warn!(target: "dlnaproxy", "Failed to accept incoming connection: {}", e);
                        continue;
                    }
                };

                let peer_addr = match proxied_stream.peer_addr() {
                    Ok(addr) => addr,
                    Err(e) => {
                        warn!(target: "dlnaproxy", "Failed to get peer address: {}", e);
                        continue;
                    }
                };

                // Set timeouts on the incoming stream
                if let Err(e) = proxied_stream.set_read_timeout(Some(stream_timeout)) {
                    warn!(target: "dlnaproxy", "Failed to set read timeout: {}", e);
                }
                if let Err(e) = proxied_stream.set_write_timeout(Some(stream_timeout)) {
                    warn!(target: "dlnaproxy", "Failed to set write timeout: {}", e);
                }

                // Connect to origin with timeout
                let to_stream = match TcpStream::connect_timeout(&origin, connect_timeout) {
                    Ok(stream) => stream,
                    Err(e) => {
                        warn!(target: "dlnaproxy", "Failed to connect to origin {}: {}", origin, e);
                        continue;
                    }
                };

                // Set timeouts on the origin stream
                if let Err(e) = to_stream.set_read_timeout(Some(stream_timeout)) {
                    warn!(target: "dlnaproxy", "Failed to set read timeout on origin: {}", e);
                }
                if let Err(e) = to_stream.set_write_timeout(Some(stream_timeout)) {
                    warn!(target: "dlnaproxy", "Failed to set write timeout on origin: {}", e);
                }

                // Spawn handler thread (fire and forget, but handle_conn now handles cleanup properly)
                thread::spawn(move || handle_conn(proxied_stream, to_stream, peer_addr));

                debug!(target: "dlnaproxy", "Successfully established a connection with client: {}", peer_addr);
            }
        }
    }
}

fn handle_conn(lhs_stream: TcpStream, rhs_stream: TcpStream, peer_addr: SocketAddr) {
    let lhs_arc = Arc::new(lhs_stream);
    let rhs_arc = Arc::new(rhs_stream);

    // Clone streams for bidirectional copy
    let (lhs_tx, lhs_rx) = match (lhs_arc.try_clone(), lhs_arc.try_clone()) {
        (Ok(tx), Ok(rx)) => (tx, rx),
        (Err(e), _) | (_, Err(e)) => {
            warn!(target: "dlnaproxy", "Failed to clone client stream for {}: {}", peer_addr, e);
            return;
        }
    };

    let (rhs_tx, rhs_rx) = match (rhs_arc.try_clone(), rhs_arc.try_clone()) {
        (Ok(tx), Ok(rx)) => (tx, rx),
        (Err(e), _) | (_, Err(e)) => {
            warn!(target: "dlnaproxy", "Failed to clone origin stream for {}: {}", peer_addr, e);
            return;
        }
    };

    // Spawn copy threads with proper error handling
    let peer_addr_copy = peer_addr;
    let lhs_to_rhs = thread::spawn(move || {
        let mut lhs_tx = lhs_tx;
        let mut rhs_rx = rhs_rx;
        match io::copy(&mut lhs_tx, &mut rhs_rx) {
            Ok(bytes) => {
                trace!(target: "dlnaproxy", "Copied {} bytes client->origin for {}", bytes, peer_addr_copy)
            }
            Err(e) => {
                trace!(target: "dlnaproxy", "Copy client->origin ended for {}: {}", peer_addr_copy, e)
            }
        }
    });

    let peer_addr_copy = peer_addr;
    let rhs_to_lhs = thread::spawn(move || {
        let mut rhs_tx = rhs_tx;
        let mut lhs_rx = lhs_rx;
        match io::copy(&mut rhs_tx, &mut lhs_rx) {
            Ok(bytes) => {
                trace!(target: "dlnaproxy", "Copied {} bytes origin->client for {}", bytes, peer_addr_copy)
            }
            Err(e) => {
                trace!(target: "dlnaproxy", "Copy origin->client ended for {}: {}", peer_addr_copy, e)
            }
        }
    });

    // Wait for both directions to complete (or error out)
    if let Err(e) = lhs_to_rhs.join() {
        warn!(target: "dlnaproxy", "Client->origin thread panicked for {}: {:?}", peer_addr, e);
    }
    if let Err(e) = rhs_to_lhs.join() {
        warn!(target: "dlnaproxy", "Origin->client thread panicked for {}: {:?}", peer_addr, e);
    }

    trace!(target: "dlnaproxy", "Closed connection with: {}", peer_addr);
}
