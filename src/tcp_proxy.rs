use log::{debug, error, info, trace, warn};

use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinHandle,
    time::timeout,
};

//Adapted from https://github.com/hishboy/rust-tcp-proxy/

/// Maximum body size (10 MB) for content that needs URL rewriting.
/// Bodies larger than this will be passed through without rewriting to prevent OOM.
const MAX_REWRITABLE_BODY_SIZE: usize = 10 * 1024 * 1024;

/// Maximum number of concurrent proxy connections.
/// Provides backpressure to prevent resource exhaustion.
const MAX_CONCURRENT_CONNECTIONS: usize = 100;

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

    pub async fn start(self, to: SocketAddr, from: SocketAddr) -> io::Result<JoinHandle<()>> {
        let listener = TcpListener::bind(from).await.map_err(|e| {
            error!(target: "dlnaproxy", "Failed to bind TCP proxy to {}: {}", from, e);
            e
        })?;

        info!(target: "dlnaproxy", "Proxying TCP connections from {} to {} (with URL rewriting)", from, to);

        let connect_timeout = self.connect_timeout;
        let stream_timeout = self.stream_timeout;
        let origin_url_base = self.origin_url_base;
        let proxy_url_base = self.proxy_url_base;

        Ok(tokio::spawn(async move {
            listen_loop(
                listener,
                to,
                connect_timeout,
                stream_timeout,
                origin_url_base,
                proxy_url_base,
            )
            .await
        }))
    }
}

async fn listen_loop(
    listener: TcpListener,
    origin: SocketAddr,
    connect_timeout: Duration,
    _stream_timeout: Duration,
    origin_url_base: String,
    proxy_url_base: String,
) {
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

    loop {
        let (proxied_stream, peer_addr) = match listener.accept().await {
            Ok((stream, addr)) => (stream, addr),
            Err(e) => {
                warn!(target: "dlnaproxy", "Failed to accept incoming connection: {}", e);
                continue;
            }
        };

        // Acquire permit for connection limiting (waits if at capacity)
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                // Semaphore was closed, exit the loop
                warn!(target: "dlnaproxy", "Connection semaphore closed, stopping listener");
                break;
            }
        };

        // Connect to origin with timeout
        let to_stream = match timeout(connect_timeout, TcpStream::connect(origin)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                warn!(target: "dlnaproxy", "Failed to connect to origin {}: {}", origin, e);
                // permit is dropped here, releasing the slot
                continue;
            }
            Err(_) => {
                warn!(target: "dlnaproxy", "Timeout connecting to origin {}", origin);
                // permit is dropped here, releasing the slot
                continue;
            }
        };

        let origin_base = origin_url_base.clone();
        let proxy_base = proxy_url_base.clone();

        // Spawn handler task - permit is moved in and released when task completes
        tokio::spawn(async move {
            handle_conn(proxied_stream, to_stream, peer_addr, origin_base, proxy_base).await;
            drop(permit); // Explicitly release permit when connection closes
        });

        debug!(target: "dlnaproxy", "Successfully established a connection with client: {}", peer_addr);
    }
}

async fn handle_conn(
    client_stream: TcpStream,
    origin_stream: TcpStream,
    peer_addr: SocketAddr,
    origin_url_base: String,
    proxy_url_base: String,
) {
    // Split streams for bidirectional communication
    let (client_read, client_write) = client_stream.into_split();
    let (origin_read, origin_write) = origin_stream.into_split();

    // Client -> Origin: forward requests without modification
    let peer_addr_copy = peer_addr;
    let client_to_origin = tokio::spawn(async move {
        let mut client_read = client_read;
        let mut origin_write = origin_write;
        match tokio::io::copy(&mut client_read, &mut origin_write).await {
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
    let origin_to_client = tokio::spawn(async move {
        if let Err(e) = proxy_response_with_rewrite(
            origin_read,
            client_write,
            &origin_url_base,
            &proxy_url_base,
            peer_addr_copy,
        )
        .await
        {
            trace!(target: "dlnaproxy", "Response proxy ended for {}: {}", peer_addr_copy, e);
        }
    });

    // Wait for both directions to complete
    if let Err(e) = client_to_origin.await {
        warn!(target: "dlnaproxy", "Client->origin task panicked for {}: {:?}", peer_addr, e);
    }
    if let Err(e) = origin_to_client.await {
        warn!(target: "dlnaproxy", "Origin->client task panicked for {}: {:?}", peer_addr, e);
    }

    trace!(target: "dlnaproxy", "Closed connection with: {}", peer_addr);
}

/// Read a line (until \n) as raw bytes, without requiring valid UTF-8.
/// This is essential for handling binary data that might appear in streams.
async fn read_line_bytes<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).await?;
    Ok(line)
}

/// Parse a chunk size from raw bytes (ASCII hex digits)
fn parse_chunk_size(line: &[u8]) -> io::Result<usize> {
    // Find the end of the hex digits (ignore extensions after ';' and whitespace)
    let hex_end = line
        .iter()
        .position(|&b| b == b';' || b == b'\r' || b == b'\n' || b == b' ')
        .unwrap_or(line.len());

    let hex_str = std::str::from_utf8(&line[..hex_end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid chunk size encoding"))?;

    usize::from_str_radix(hex_str.trim(), 16).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid chunk size '{}': {}", hex_str, e),
        )
    })
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
async fn proxy_response_with_rewrite(
    origin_read: tokio::net::tcp::OwnedReadHalf,
    mut client_write: tokio::net::tcp::OwnedWriteHalf,
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

        // Read headers line by line (as raw bytes to handle non-UTF8 gracefully)
        loop {
            let line = read_line_bytes(&mut reader).await?;
            if line.is_empty() {
                // Connection closed
                return Ok(());
            }

            // Convert to string for header matching (lossy conversion is fine for headers)
            let line_str = String::from_utf8_lossy(&line);

            // Check for Content-Length header
            if line_str.to_lowercase().starts_with("content-length:") {
                if let Some(len_str) = line_str.split(':').nth(1) {
                    content_length = len_str.trim().parse().ok();
                }
            }

            // Check for Transfer-Encoding: chunked
            if line_str.to_lowercase().starts_with("transfer-encoding:")
                && line_str.to_lowercase().contains("chunked")
            {
                is_chunked = true;
            }

            header_buf.extend_from_slice(&line);

            // End of headers (check raw bytes for \r\n or \n)
            if line == b"\r\n" || line == b"\n" {
                break;
            }
        }

        // If we got no headers at all, connection is closed
        if header_buf.is_empty() {
            return Ok(());
        }

        let headers_str = String::from_utf8_lossy(&header_buf);
        // Only log the first line (status line), and sanitize it for display
        // Use is_ascii_graphic() to only allow printable ASCII (0x21-0x7E) plus space
        // This filters out control chars, UTF-8 replacement chars, and other non-ASCII
        let status_line = headers_str
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_graphic() || *c == ' ')
            .take(100) // Limit length to avoid log spam
            .collect::<String>();
        trace!(target: "dlnaproxy", "Response headers for {}: {}", peer_addr, status_line);

        // Check if this is text/XML content that needs URL rewriting
        let needs_rewrite = should_rewrite_content(&headers_str);

        // Handle responses without Content-Length and not chunked
        // This is typically a streaming response - read until connection close
        if !is_chunked && content_length.is_none() {
            client_write.write_all(&header_buf).await?;
            client_write.flush().await?;

            // Stream remaining data until origin closes connection
            let bytes_copied = tokio::io::copy(&mut reader, &mut client_write).await?;
            trace!(target: "dlnaproxy", "Streamed {} bytes for {} (no Content-Length)", bytes_copied, peer_addr);
            return Ok(()); // Connection is done after streaming
        }

        // Check if body is too large for URL rewriting (to prevent OOM)
        let body_too_large = content_length.is_some_and(|len| len > MAX_REWRITABLE_BODY_SIZE);
        if body_too_large {
            warn!(target: "dlnaproxy", "Body too large for URL rewriting ({} bytes), passing through for {}",
                  content_length.unwrap_or(0), peer_addr);
        }

        // For binary content or bodies too large for rewriting, pass through without modification
        if !needs_rewrite || body_too_large {
            client_write.write_all(&header_buf).await?;

            if is_chunked {
                // Pass through chunked data as-is
                pass_through_chunked(&mut reader, &mut client_write).await?;
            } else if let Some(len) = content_length {
                // Pass through fixed-length binary data
                let mut remaining = len;
                let mut buf = [0u8; 8192];
                while remaining > 0 {
                    let to_read = std::cmp::min(remaining, buf.len());
                    let bytes_read = reader.read(&mut buf[..to_read]).await?;
                    if bytes_read == 0 {
                        break;
                    }
                    client_write.write_all(&buf[..bytes_read]).await?;
                    remaining -= bytes_read;
                }
            }

            client_write.flush().await?;
            trace!(target: "dlnaproxy", "Proxied binary response for {} ({} bytes)",
                   peer_addr, content_length.unwrap_or(0));
            continue;
        }

        // Read body for text/XML content that needs URL rewriting
        let body = if is_chunked {
            read_chunked_body(&mut reader, MAX_REWRITABLE_BODY_SIZE).await?
        } else if let Some(len) = content_length {
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).await?;
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
        client_write.write_all(updated_headers.as_bytes()).await?;

        if is_chunked {
            // Re-encode as chunked
            write_chunked_body(&mut client_write, rewritten_bytes).await?;
        } else {
            client_write.write_all(rewritten_bytes).await?;
        }

        client_write.flush().await?;

        trace!(target: "dlnaproxy", "Proxied response with URL rewriting for {} ({} -> {} bytes)",
               peer_addr, body.len(), rewritten_bytes.len());
    }
}

/// Pass through chunked data without buffering the entire body
async fn pass_through_chunked<R, W>(reader: &mut R, writer: &mut W) -> io::Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    loop {
        // Read chunk size line as raw bytes
        let size_line = read_line_bytes(reader).await?;
        if size_line.is_empty() {
            break;
        }
        writer.write_all(&size_line).await?;

        // Parse chunk size (hex) from raw bytes
        let chunk_size = parse_chunk_size(&size_line)?;

        if chunk_size == 0 {
            // Read and forward trailing CRLF after last chunk
            let mut trailer = Vec::new();
            reader.read_until(b'\n', &mut trailer).await?;
            writer.write_all(&trailer).await?;
            break;
        }

        // Forward chunk data
        let mut remaining = chunk_size;
        let mut buf = [0u8; 8192];
        while remaining > 0 {
            let to_read = std::cmp::min(remaining, buf.len());
            let bytes_read = reader.read(&mut buf[..to_read]).await?;
            if bytes_read == 0 {
                break;
            }
            writer.write_all(&buf[..bytes_read]).await?;
            remaining -= bytes_read;
        }

        // Read and forward trailing CRLF after chunk
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).await?;
        writer.write_all(&crlf).await?;
    }

    Ok(())
}

/// Read a chunked HTTP body with a maximum size limit to prevent OOM
async fn read_chunked_body<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    max_size: usize,
) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();

    loop {
        // Read chunk size line as raw bytes
        let size_line = read_line_bytes(reader).await?;
        if size_line.is_empty() {
            break;
        }

        // Parse chunk size (hex) from raw bytes
        let chunk_size = parse_chunk_size(&size_line)?;

        if chunk_size == 0 {
            // Read trailing CRLF after last chunk
            let mut trailer = Vec::new();
            reader.read_until(b'\n', &mut trailer).await?;
            break;
        }

        // Check if this chunk would exceed the maximum size
        if body.len() + chunk_size > max_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Chunked body exceeds maximum size ({} bytes)",
                    max_size
                ),
            ));
        }

        // Read chunk data
        let mut chunk = vec![0u8; chunk_size];
        reader.read_exact(&mut chunk).await?;
        body.extend_from_slice(&chunk);

        // Read trailing CRLF after chunk
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).await?;
    }

    Ok(body)
}

/// Write body as chunked encoding
async fn write_chunked_body<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    body: &[u8],
) -> io::Result<()> {
    // Write single chunk with all data
    let size_line = format!("{:x}\r\n", body.len());
    writer.write_all(size_line.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.write_all(b"\r\n").await?;
    // Write terminating chunk
    writer.write_all(b"0\r\n\r\n").await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ============================================
    // parse_chunk_size() tests
    // ============================================

    #[test]
    fn test_parse_chunk_size_zero() {
        assert_eq!(parse_chunk_size(b"0\r\n").unwrap(), 0);
    }

    #[test]
    fn test_parse_chunk_size_single_digit() {
        assert_eq!(parse_chunk_size(b"a\r\n").unwrap(), 10);
        assert_eq!(parse_chunk_size(b"f\r\n").unwrap(), 15);
        assert_eq!(parse_chunk_size(b"5\r\n").unwrap(), 5);
    }

    #[test]
    fn test_parse_chunk_size_uppercase() {
        assert_eq!(parse_chunk_size(b"A\r\n").unwrap(), 10);
        assert_eq!(parse_chunk_size(b"FF\r\n").unwrap(), 255);
        assert_eq!(parse_chunk_size(b"DEADBEEF\r\n").unwrap(), 0xDEADBEEF);
    }

    #[test]
    fn test_parse_chunk_size_mixed_case() {
        assert_eq!(parse_chunk_size(b"aB\r\n").unwrap(), 0xAB);
        assert_eq!(parse_chunk_size(b"DeAdBeEf\r\n").unwrap(), 0xDEADBEEF);
    }

    #[test]
    fn test_parse_chunk_size_with_extension() {
        assert_eq!(parse_chunk_size(b"10;name=value\r\n").unwrap(), 16);
        assert_eq!(parse_chunk_size(b"ff;ext\r\n").unwrap(), 255);
    }

    #[test]
    fn test_parse_chunk_size_with_trailing_space() {
        // Trailing space before CRLF is handled
        assert_eq!(parse_chunk_size(b"10 \r\n").unwrap(), 16);
    }

    #[test]
    fn test_parse_chunk_size_newline_only() {
        assert_eq!(parse_chunk_size(b"10\n").unwrap(), 16);
    }

    #[test]
    fn test_parse_chunk_size_invalid_hex() {
        assert!(parse_chunk_size(b"xyz\r\n").is_err());
        assert!(parse_chunk_size(b"gg\r\n").is_err());
    }

    #[test]
    fn test_parse_chunk_size_empty() {
        assert!(parse_chunk_size(b"\r\n").is_err());
    }

    // ============================================
    // should_rewrite_content() tests
    // ============================================

    #[test]
    fn test_should_rewrite_text_plain() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n";
        assert!(should_rewrite_content(headers));
    }

    #[test]
    fn test_should_rewrite_text_xml() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\n\r\n";
        assert!(should_rewrite_content(headers));
    }

    #[test]
    fn test_should_rewrite_application_xml() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\n\r\n";
        assert!(should_rewrite_content(headers));
    }

    #[test]
    fn test_should_rewrite_json() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n";
        assert!(should_rewrite_content(headers));
    }

    #[test]
    fn test_should_rewrite_html() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n";
        assert!(should_rewrite_content(headers));
    }

    #[test]
    fn test_should_rewrite_text_html_charset() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n";
        assert!(should_rewrite_content(headers));
    }

    #[test]
    fn test_should_not_rewrite_video() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\n\r\n";
        assert!(!should_rewrite_content(headers));
    }

    #[test]
    fn test_should_not_rewrite_audio() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\n\r\n";
        assert!(!should_rewrite_content(headers));
    }

    #[test]
    fn test_should_not_rewrite_image() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\n\r\n";
        assert!(!should_rewrite_content(headers));
    }

    #[test]
    fn test_should_not_rewrite_octet_stream() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\r\n";
        assert!(!should_rewrite_content(headers));
    }

    #[test]
    fn test_should_not_rewrite_missing_content_type() {
        let headers = "HTTP/1.1 200 OK\r\nServer: Test\r\n\r\n";
        assert!(!should_rewrite_content(headers));
    }

    #[test]
    fn test_should_rewrite_case_insensitive() {
        let headers = "HTTP/1.1 200 OK\r\nCONTENT-TYPE: TEXT/XML\r\n\r\n";
        assert!(should_rewrite_content(headers));
    }

    // ============================================
    // update_content_length() tests
    // ============================================

    #[test]
    fn test_update_content_length_basic() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n";
        let result = update_content_length(headers, 200);
        assert!(result.contains("Content-Length: 200"));
        assert!(!result.contains("Content-Length: 100"));
    }

    #[test]
    fn test_update_content_length_case_insensitive() {
        let headers = "HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n";
        let result = update_content_length(headers, 50);
        assert!(result.contains("Content-Length: 50"));
    }

    #[test]
    fn test_update_content_length_preserves_other_headers() {
        let headers = "HTTP/1.1 200 OK\r\nServer: Test\r\nContent-Length: 100\r\nConnection: close\r\n\r\n";
        let result = update_content_length(headers, 150);
        assert!(result.contains("Server: Test"));
        assert!(result.contains("Content-Length: 150"));
        assert!(result.contains("Connection: close"));
    }

    #[test]
    fn test_update_content_length_no_header() {
        let headers = "HTTP/1.1 200 OK\r\nServer: Test\r\n\r\n";
        let result = update_content_length(headers, 100);
        // Should just return the headers unchanged (no Content-Length to update)
        assert!(!result.contains("Content-Length:"));
    }

    // ============================================
    // read_line_bytes() tests
    // ============================================

    #[tokio::test]
    async fn test_read_line_bytes_simple() {
        let data = b"Hello\nWorld\n";
        let mut cursor = Cursor::new(&data[..]);
        let line = read_line_bytes(&mut cursor).await.unwrap();
        assert_eq!(line, b"Hello\n");
    }

    #[tokio::test]
    async fn test_read_line_bytes_crlf() {
        let data = b"Hello\r\nWorld\r\n";
        let mut cursor = Cursor::new(&data[..]);
        let line = read_line_bytes(&mut cursor).await.unwrap();
        assert_eq!(line, b"Hello\r\n");
    }

    #[tokio::test]
    async fn test_read_line_bytes_empty() {
        let data = b"";
        let mut cursor = Cursor::new(&data[..]);
        let line = read_line_bytes(&mut cursor).await.unwrap();
        assert_eq!(line, b"");
    }

    #[tokio::test]
    async fn test_read_line_bytes_binary_data() {
        let data = [0x00, 0xFF, 0x80, b'\n', 0x01, 0x02];
        let mut cursor = Cursor::new(&data[..]);
        let line = read_line_bytes(&mut cursor).await.unwrap();
        assert_eq!(line, &[0x00, 0xFF, 0x80, b'\n']);
    }

    // ============================================
    // read_chunked_body() tests
    // ============================================

    #[tokio::test]
    async fn test_read_chunked_body_single_chunk() {
        let data = b"5\r\nHello\r\n0\r\n\r\n";
        let mut cursor = Cursor::new(&data[..]);
        let body = read_chunked_body(&mut cursor, MAX_REWRITABLE_BODY_SIZE).await.unwrap();
        assert_eq!(body, b"Hello");
    }

    #[tokio::test]
    async fn test_read_chunked_body_multiple_chunks() {
        let data = b"5\r\nHello\r\n6\r\n World\r\n0\r\n\r\n";
        let mut cursor = Cursor::new(&data[..]);
        let body = read_chunked_body(&mut cursor, MAX_REWRITABLE_BODY_SIZE).await.unwrap();
        assert_eq!(body, b"Hello World");
    }

    #[tokio::test]
    async fn test_read_chunked_body_empty() {
        let data = b"0\r\n\r\n";
        let mut cursor = Cursor::new(&data[..]);
        let body = read_chunked_body(&mut cursor, MAX_REWRITABLE_BODY_SIZE).await.unwrap();
        assert_eq!(body, b"");
    }

    #[tokio::test]
    async fn test_read_chunked_body_hex_size() {
        let data = b"a\r\n0123456789\r\n0\r\n\r\n";
        let mut cursor = Cursor::new(&data[..]);
        let body = read_chunked_body(&mut cursor, MAX_REWRITABLE_BODY_SIZE).await.unwrap();
        assert_eq!(body.len(), 10);
        assert_eq!(body, b"0123456789");
    }

    #[tokio::test]
    async fn test_read_chunked_body_binary_data() {
        let chunk_data: Vec<u8> = vec![0x00, 0xFF, 0x80, 0x7F, 0x01];
        let mut data = format!("{:x}\r\n", chunk_data.len()).into_bytes();
        data.extend_from_slice(&chunk_data);
        data.extend_from_slice(b"\r\n0\r\n\r\n");

        let mut cursor = Cursor::new(data);
        let body = read_chunked_body(&mut cursor, MAX_REWRITABLE_BODY_SIZE).await.unwrap();
        assert_eq!(body, chunk_data);
    }

    #[tokio::test]
    async fn test_read_chunked_body_exceeds_max_size() {
        // Create chunked data that exceeds a small limit
        let data = b"10\r\n0123456789abcdef\r\n0\r\n\r\n"; // 16 bytes
        let mut cursor = Cursor::new(&data[..]);
        let result = read_chunked_body(&mut cursor, 10).await; // limit to 10 bytes
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

    // ============================================
    // write_chunked_body() tests
    // ============================================

    #[tokio::test]
    async fn test_write_chunked_body_simple() {
        let mut output = Vec::new();
        write_chunked_body(&mut output, b"Hello").await.unwrap();
        assert_eq!(output, b"5\r\nHello\r\n0\r\n\r\n");
    }

    #[tokio::test]
    async fn test_write_chunked_body_empty() {
        let mut output = Vec::new();
        write_chunked_body(&mut output, b"").await.unwrap();
        assert_eq!(output, b"0\r\n\r\n0\r\n\r\n");
    }

    #[tokio::test]
    async fn test_write_chunked_body_larger() {
        let body = b"This is a longer test body with multiple words";
        let mut output = Vec::new();
        write_chunked_body(&mut output, body).await.unwrap();

        // Verify format: hex_size\r\nbody\r\n0\r\n\r\n
        let expected_size = format!("{:x}\r\n", body.len());
        assert!(output.starts_with(expected_size.as_bytes()));
        assert!(output.ends_with(b"\r\n0\r\n\r\n"));
    }

    #[tokio::test]
    async fn test_write_chunked_body_binary() {
        let body: Vec<u8> = vec![0x00, 0xFF, 0x80, 0x7F];
        let mut output = Vec::new();
        write_chunked_body(&mut output, &body).await.unwrap();

        // Verify the body appears in the output
        // Format: "4\r\n" (3 bytes) + body (4 bytes) + "\r\n0\r\n\r\n"
        assert!(output.starts_with(b"4\r\n"));
        assert_eq!(&output[3..7], &body[..]);
    }

    // ============================================
    // Round-trip tests: read then write
    // ============================================

    #[tokio::test]
    async fn test_chunked_roundtrip() {
        let original_body = b"Test body content for round trip";

        // Write as chunked
        let mut encoded = Vec::new();
        write_chunked_body(&mut encoded, original_body).await.unwrap();

        // Read back
        let mut cursor = Cursor::new(encoded);
        let decoded = read_chunked_body(&mut cursor, MAX_REWRITABLE_BODY_SIZE).await.unwrap();

        assert_eq!(decoded, original_body);
    }

    #[tokio::test]
    async fn test_chunked_roundtrip_binary() {
        let original_body: Vec<u8> = (0..=255).collect();

        // Write as chunked
        let mut encoded = Vec::new();
        write_chunked_body(&mut encoded, &original_body).await.unwrap();

        // Read back
        let mut cursor = Cursor::new(encoded);
        let decoded = read_chunked_body(&mut cursor, MAX_REWRITABLE_BODY_SIZE).await.unwrap();

        assert_eq!(decoded, original_body);
    }

    // ============================================
    // URL replacement logic tests
    // ============================================

    #[test]
    fn test_url_replacement_basic() {
        let body = "Location: http://192.168.1.41:55555/desc.xml";
        let origin = "http://192.168.1.41:55555";
        let proxy = "http://192.168.1.52:8100";

        let result = body.replace(origin, proxy);
        assert_eq!(result, "Location: http://192.168.1.52:8100/desc.xml");
    }

    #[test]
    fn test_url_replacement_multiple() {
        let body = "<url>http://192.168.1.41:55555/a</url>\n<url>http://192.168.1.41:55555/b</url>";
        let origin = "http://192.168.1.41:55555";
        let proxy = "http://192.168.1.52:8100";

        let result = body.replace(origin, proxy);
        assert!(result.contains("http://192.168.1.52:8100/a"));
        assert!(result.contains("http://192.168.1.52:8100/b"));
        assert!(!result.contains("192.168.1.41"));
    }

    #[test]
    fn test_url_replacement_xml_body() {
        let body = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <URLBase>http://192.168.1.41:55555/</URLBase>
  <device>
    <presentationURL>http://192.168.1.41:55555/index.html</presentationURL>
  </device>
</root>"#;
        let origin = "http://192.168.1.41:55555";
        let proxy = "http://192.168.1.52:8100";

        let result = body.replace(origin, proxy);
        assert!(result.contains("http://192.168.1.52:8100/"));
        assert!(result.contains("http://192.168.1.52:8100/index.html"));
        assert!(!result.contains("192.168.1.41:55555"));
    }

    #[test]
    fn test_url_replacement_no_match() {
        let body = "Some content without URLs";
        let origin = "http://192.168.1.41:55555";
        let proxy = "http://192.168.1.52:8100";

        let result = body.replace(origin, proxy);
        assert_eq!(result, body);
    }
}
