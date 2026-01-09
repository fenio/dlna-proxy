use log::{debug, info, trace, warn};

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::Duration,
};

//Adapted from https://github.com/hishboy/rust-tcp-proxy/

pub struct TCPProxy {
    connect_timeout: Duration,
    stream_timeout: Duration,
    origin_url_base: String,
    proxy_url_base: String,
}

impl TCPProxy {
    pub fn new(
        connect_timeout: Duration,
        stream_timeout: Duration,
        origin_addr: SocketAddr,
        proxy_addr: SocketAddr,
    ) -> Self {
        // Create URL bases for rewriting (e.g., "http://192.168.1.41:55555" -> "http://192.168.1.52:8100")
        let origin_url_base = format!("http://{}:{}", origin_addr.ip(), origin_addr.port());
        let proxy_url_base = format!("http://{}:{}", proxy_addr.ip(), proxy_addr.port());

        TCPProxy {
            connect_timeout,
            stream_timeout,
            origin_url_base,
            proxy_url_base,
        }
    }

    pub fn start(self, to: SocketAddr, from: SocketAddr) -> JoinHandle<()> {
        let listener = TcpListener::bind(from).expect("Unable to bind proxy addr");

        info!(target: "dlnaproxy", "Proxying TCP connections from {} to {} (with URL rewriting)", from, to);

        thread::spawn(self.listen_loop(listener, to))
    }

    fn listen_loop(self, listener: TcpListener, origin: SocketAddr) -> impl FnOnce() {
        let connect_timeout = self.connect_timeout;
        let stream_timeout = self.stream_timeout;
        let origin_url_base = self.origin_url_base;
        let proxy_url_base = self.proxy_url_base;

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

                let origin_base = origin_url_base.clone();
                let proxy_base = proxy_url_base.clone();

                // Spawn handler thread
                thread::spawn(move || {
                    handle_conn(
                        proxied_stream,
                        to_stream,
                        peer_addr,
                        origin_base,
                        proxy_base,
                    )
                });

                debug!(target: "dlnaproxy", "Successfully established a connection with client: {}", peer_addr);
            }
        }
    }
}

fn handle_conn(
    client_stream: TcpStream,
    origin_stream: TcpStream,
    peer_addr: SocketAddr,
    origin_url_base: String,
    proxy_url_base: String,
) {
    // Clone streams for bidirectional communication
    let client_read = match client_stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            warn!(target: "dlnaproxy", "Failed to clone client stream for {}: {}", peer_addr, e);
            return;
        }
    };
    let client_write = client_stream;

    let origin_read = match origin_stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            warn!(target: "dlnaproxy", "Failed to clone origin stream for {}: {}", peer_addr, e);
            return;
        }
    };
    let origin_write = origin_stream;

    // Client -> Origin: forward requests without modification
    let peer_addr_copy = peer_addr;
    let client_to_origin = thread::spawn(move || {
        let mut client_read = client_read;
        let mut origin_write = origin_write;
        match io::copy(&mut client_read, &mut origin_write) {
            Ok(bytes) => {
                trace!(target: "dlnaproxy", "Copied {} bytes client->origin for {}", bytes, peer_addr_copy)
            }
            Err(e) => {
                trace!(target: "dlnaproxy", "Copy client->origin ended for {}: {}", peer_addr_copy, e)
            }
        }
    });

    // Origin -> Client: rewrite URLs in responses
    let peer_addr_copy = peer_addr;
    let origin_to_client = thread::spawn(move || {
        if let Err(e) = proxy_response_with_rewrite(
            origin_read,
            client_write,
            &origin_url_base,
            &proxy_url_base,
            peer_addr_copy,
        ) {
            trace!(target: "dlnaproxy", "Response proxy ended for {}: {}", peer_addr_copy, e);
        }
    });

    // Wait for both directions to complete
    if let Err(e) = client_to_origin.join() {
        warn!(target: "dlnaproxy", "Client->origin thread panicked for {}: {:?}", peer_addr, e);
    }
    if let Err(e) = origin_to_client.join() {
        warn!(target: "dlnaproxy", "Origin->client thread panicked for {}: {:?}", peer_addr, e);
    }

    trace!(target: "dlnaproxy", "Closed connection with: {}", peer_addr);
}

/// Check if Content-Type indicates text/XML content that should have URL rewriting
fn should_rewrite_content(headers: &str) -> bool {
    let headers_lower = headers.to_lowercase();
    for line in headers_lower.lines() {
        if line.starts_with("content-type:") {
            let content_type = line.split(':').nth(1).unwrap_or("").trim();
            // Rewrite text and XML content types
            return content_type.starts_with("text/")
                || content_type.contains("xml")
                || content_type.contains("json")
                || content_type.contains("html");
        }
    }
    // If no Content-Type header, default to binary pass-through.
    // DLNA text responses (XML) always have Content-Type headers,
    // while media streams may omit them.
    false
}

/// Proxy HTTP responses from origin to client, rewriting URLs in the body
fn proxy_response_with_rewrite(
    origin_read: TcpStream,
    mut client_write: TcpStream,
    origin_url_base: &str,
    proxy_url_base: &str,
    peer_addr: SocketAddr,
) -> io::Result<()> {
    let mut reader = BufReader::new(origin_read);

    loop {
        // Read the HTTP response status line and headers
        let mut header_buf = Vec::new();
        let mut content_length: Option<usize> = None;
        let mut is_chunked = false;

        // Read headers line by line
        loop {
            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                // Connection closed
                return Ok(());
            }

            // Check for Content-Length header
            if line.to_lowercase().starts_with("content-length:") {
                if let Some(len_str) = line.split(':').nth(1) {
                    content_length = len_str.trim().parse().ok();
                }
            }

            // Check for Transfer-Encoding: chunked
            if line.to_lowercase().starts_with("transfer-encoding:") {
                if line.to_lowercase().contains("chunked") {
                    is_chunked = true;
                }
            }

            header_buf.extend_from_slice(line.as_bytes());

            // End of headers
            if line == "\r\n" || line == "\n" {
                break;
            }
        }

        // If we got no headers at all, connection is closed
        if header_buf.is_empty() {
            return Ok(());
        }

        let headers_str = String::from_utf8_lossy(&header_buf);
        trace!(target: "dlnaproxy", "Response headers for {}: {}", peer_addr, headers_str.lines().next().unwrap_or(""));

        // Check if this is text/XML content that needs URL rewriting
        let needs_rewrite = should_rewrite_content(&headers_str);

        // Handle responses without body or with unknown length
        if !is_chunked && content_length.is_none() {
            // No Content-Length and not chunked - could be connection close or no body
            // For HTTP/1.0 or responses without body (204, 304, etc.), just send headers
            client_write.write_all(&header_buf)?;
            client_write.flush()?;
            continue;
        }

        // For binary content, pass through without modification
        if !needs_rewrite {
            client_write.write_all(&header_buf)?;

            if is_chunked {
                // Pass through chunked data as-is
                pass_through_chunked(&mut reader, &mut client_write)?;
            } else if let Some(len) = content_length {
                // Pass through fixed-length binary data
                let mut remaining = len;
                let mut buf = [0u8; 8192];
                while remaining > 0 {
                    let to_read = std::cmp::min(remaining, buf.len());
                    let bytes_read = reader.read(&mut buf[..to_read])?;
                    if bytes_read == 0 {
                        break;
                    }
                    client_write.write_all(&buf[..bytes_read])?;
                    remaining -= bytes_read;
                }
            }

            client_write.flush()?;
            trace!(target: "dlnaproxy", "Proxied binary response for {} ({} bytes)", 
                   peer_addr, content_length.unwrap_or(0));
            continue;
        }

        // Read body for text/XML content that needs URL rewriting
        let body = if is_chunked {
            read_chunked_body(&mut reader)?
        } else if let Some(len) = content_length {
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body)?;
            body
        } else {
            // Already handled above
            continue;
        };

        // Rewrite URLs in the body
        let body_str = String::from_utf8_lossy(&body);
        let rewritten_body = body_str.replace(origin_url_base, proxy_url_base);
        let rewritten_bytes = rewritten_body.as_bytes();

        // Update Content-Length if body was rewritten and size changed
        let updated_headers = if content_length.is_some() && rewritten_bytes.len() != body.len() {
            // Need to update Content-Length
            update_content_length(&headers_str, rewritten_bytes.len())
        } else {
            headers_str.to_string()
        };

        // Send updated headers and body
        client_write.write_all(updated_headers.as_bytes())?;

        if is_chunked {
            // Re-encode as chunked
            write_chunked_body(&mut client_write, rewritten_bytes)?;
        } else {
            client_write.write_all(rewritten_bytes)?;
        }

        client_write.flush()?;

        trace!(target: "dlnaproxy", "Proxied response with URL rewriting for {} ({} -> {} bytes)", 
               peer_addr, body.len(), rewritten_bytes.len());
    }
}

/// Pass through chunked data without buffering the entire body
fn pass_through_chunked<R: BufRead, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<()> {
    loop {
        // Read and forward chunk size line
        let mut size_line = String::new();
        reader.read_line(&mut size_line)?;
        writer.write_all(size_line.as_bytes())?;

        // Parse chunk size (hex)
        let size_str = size_line.trim();
        let chunk_size = usize::from_str_radix(size_str, 16).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid chunk size '{}': {}", size_str, e),
            )
        })?;

        if chunk_size == 0 {
            // Read and forward trailing CRLF after last chunk
            let mut trailer = String::new();
            reader.read_line(&mut trailer)?;
            writer.write_all(trailer.as_bytes())?;
            break;
        }

        // Forward chunk data
        let mut remaining = chunk_size;
        let mut buf = [0u8; 8192];
        while remaining > 0 {
            let to_read = std::cmp::min(remaining, buf.len());
            let bytes_read = reader.read(&mut buf[..to_read])?;
            if bytes_read == 0 {
                break;
            }
            writer.write_all(&buf[..bytes_read])?;
            remaining -= bytes_read;
        }

        // Read and forward trailing CRLF after chunk
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
        writer.write_all(&crlf)?;
    }

    Ok(())
}

/// Read a chunked HTTP body
fn read_chunked_body<R: BufRead>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();

    loop {
        // Read chunk size line
        let mut size_line = String::new();
        reader.read_line(&mut size_line)?;

        // Parse chunk size (hex)
        let size_str = size_line.trim();
        let chunk_size = usize::from_str_radix(size_str, 16).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid chunk size '{}': {}", size_str, e),
            )
        })?;

        if chunk_size == 0 {
            // Read trailing CRLF after last chunk
            let mut trailer = String::new();
            reader.read_line(&mut trailer)?;
            break;
        }

        // Read chunk data
        let mut chunk = vec![0u8; chunk_size];
        reader.read_exact(&mut chunk)?;
        body.extend_from_slice(&chunk);

        // Read trailing CRLF after chunk
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
    }

    Ok(body)
}

/// Write body as chunked encoding
fn write_chunked_body<W: Write>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    // Write single chunk with all data
    write!(writer, "{:x}\r\n", body.len())?;
    writer.write_all(body)?;
    writer.write_all(b"\r\n")?;
    // Write terminating chunk
    writer.write_all(b"0\r\n\r\n")?;
    Ok(())
}

/// Update Content-Length header in the headers string
fn update_content_length(headers: &str, new_length: usize) -> String {
    let mut result = String::new();

    for line in headers.lines() {
        if line.to_lowercase().starts_with("content-length:") {
            result.push_str(&format!("Content-Length: {}\r\n", new_length));
        } else {
            result.push_str(line);
            result.push_str("\r\n");
        }
    }

    result
}
