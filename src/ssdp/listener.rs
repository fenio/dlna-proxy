use log::{debug, error, info, trace, warn};

use std::borrow::Cow;
use std::{collections::HashMap, sync::Arc};
use tokio::net::UdpSocket;

use httparse::{Request, EMPTY_HEADER};

use anyhow::Context;
use anyhow::Result;

use crate::ssdp::utils::InteractiveSSDP;

/*
    SSDP RFC for reference: https://tools.ietf.org/html/draft-cai-ssdp-v1-03
*/

pub(crate) fn parse_ssdp(buffer: &[u8]) -> Result<(String, HashMap<String, Cow<'_, str>>)> {
    let mut headers = [EMPTY_HEADER; 16];
    let mut req = Request::new(&mut headers);

    req.parse(buffer)
        .context("Failed to parse packet as SSDP.")?;

    let method = req
        .method
        .map(String::from)
        .ok_or(super::error::Error::NoSSDPMethod)?;

    let mut header_map: HashMap<String, Cow<'_, str>> = HashMap::with_capacity(headers.len());
    let mut i = 0;
    while !headers[i].name.is_empty() {
        let name = String::from(headers[i].name).to_uppercase();
        let value = String::from_utf8_lossy(headers[i].value);

        header_map.insert(name, value);
        i += 1;
    }

    Ok((method, header_map))
}

pub async fn listen_task(ssdp_socket: Arc<UdpSocket>, ssdp_helper: Arc<InteractiveSSDP>) {
    debug!(target: "dlnaproxy", "Listen task up and running!");

    loop {
        let mut buffer: [u8; 1024] = [0; 1024];

        let (bytes_read, src_addr) = match ssdp_socket.recv_from(&mut buffer).await {
            Ok(result) => result,
            Err(e) => {
                error!(target: "dlnaproxy", "Failed to receive SSDP packet: {}. Continuing...", e);
                continue;
            }
        };

        trace!(target: "dlnaproxy", "Read {amount} bytes sent by {sender}.", amount=bytes_read, sender=src_addr);

        let (ssdp_method, ssdp_headers) = match parse_ssdp(&buffer) {
            Ok(parsed_data) => parsed_data,
            Err(e) => {
                warn!(target:"dlnaproxy", "{}", e);
                continue;
            }
        };

        let st_header = ssdp_headers.get("ST");
        let _man_header = ssdp_headers.get("MAN");

        //We have a valid ssdp:discover request, although the rfc is soooooo vague it hurts.
        if let Some(header) = st_header {
            // Respond to M-SEARCH requests for:
            // - MediaServer:1 (specific device type)
            // - ssdp:all (discover all devices)
            // - upnp:rootdevice (discover all root devices)
            let should_respond = ssdp_method == "M-SEARCH"
                && (header == "urn:schemas-upnp-org:device:MediaServer:1"
                    || header == "ssdp:all"
                    || header == "upnp:rootdevice");

            if should_respond {
                info!(target: "dlnaproxy", "Responding to M-SEARCH request (ST: {st}) from {sender}.", st=header, sender=src_addr);

                if let Err(msg) = ssdp_helper.send_ok(&ssdp_socket, src_addr).await {
                    warn!(target: "dlnaproxy", "Couldn't send ssdp:alive: {}", msg);
                } else {
                    info!(target: "dlnaproxy", "Sent ssdp:ok on local SSDP channel!");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================
    // parse_ssdp() M-SEARCH parsing tests
    // ============================================

    #[test]
    fn test_parse_ssdp_msearch_ssdp_all() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            HOST: 239.255.255.250:1900\r\n\
            MAN: \"ssdp:discover\"\r\n\
            MX: 3\r\n\
            ST: ssdp:all\r\n\
            \r\n";

        let (method, headers) = parse_ssdp(packet).unwrap();
        assert_eq!(method, "M-SEARCH");
        assert_eq!(headers.get("ST").map(|s| s.as_ref()), Some("ssdp:all"));
    }

    #[test]
    fn test_parse_ssdp_msearch_mediaserver() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            HOST: 239.255.255.250:1900\r\n\
            MAN: \"ssdp:discover\"\r\n\
            MX: 3\r\n\
            ST: urn:schemas-upnp-org:device:MediaServer:1\r\n\
            \r\n";

        let (method, headers) = parse_ssdp(packet).unwrap();
        assert_eq!(method, "M-SEARCH");
        assert_eq!(
            headers.get("ST").map(|s| s.as_ref()),
            Some("urn:schemas-upnp-org:device:MediaServer:1")
        );
    }

    #[test]
    fn test_parse_ssdp_msearch_rootdevice() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            HOST: 239.255.255.250:1900\r\n\
            MAN: \"ssdp:discover\"\r\n\
            MX: 3\r\n\
            ST: upnp:rootdevice\r\n\
            \r\n";

        let (method, headers) = parse_ssdp(packet).unwrap();
        assert_eq!(method, "M-SEARCH");
        assert_eq!(headers.get("ST").map(|s| s.as_ref()), Some("upnp:rootdevice"));
    }

    // ============================================
    // Header extraction tests
    // ============================================

    #[test]
    fn test_parse_ssdp_extracts_man_header() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            HOST: 239.255.255.250:1900\r\n\
            MAN: \"ssdp:discover\"\r\n\
            ST: ssdp:all\r\n\
            \r\n";

        let (_, headers) = parse_ssdp(packet).unwrap();
        assert_eq!(headers.get("MAN").map(|s| s.as_ref()), Some("\"ssdp:discover\""));
    }

    #[test]
    fn test_parse_ssdp_extracts_host_header() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            HOST: 239.255.255.250:1900\r\n\
            ST: ssdp:all\r\n\
            \r\n";

        let (_, headers) = parse_ssdp(packet).unwrap();
        assert_eq!(headers.get("HOST").map(|s| s.as_ref()), Some("239.255.255.250:1900"));
    }

    #[test]
    fn test_parse_ssdp_extracts_mx_header() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            HOST: 239.255.255.250:1900\r\n\
            MX: 5\r\n\
            ST: ssdp:all\r\n\
            \r\n";

        let (_, headers) = parse_ssdp(packet).unwrap();
        assert_eq!(headers.get("MX").map(|s| s.as_ref()), Some("5"));
    }

    // ============================================
    // Header name normalization tests
    // ============================================

    #[test]
    fn test_parse_ssdp_normalizes_headers_to_uppercase() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            host: 239.255.255.250:1900\r\n\
            man: \"ssdp:discover\"\r\n\
            st: ssdp:all\r\n\
            \r\n";

        let (_, headers) = parse_ssdp(packet).unwrap();
        // Headers should be normalized to uppercase
        assert!(headers.contains_key("HOST"));
        assert!(headers.contains_key("MAN"));
        assert!(headers.contains_key("ST"));
    }

    #[test]
    fn test_parse_ssdp_mixed_case_headers() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n\
            Host: 239.255.255.250:1900\r\n\
            Man: \"ssdp:discover\"\r\n\
            St: ssdp:all\r\n\
            \r\n";

        let (_, headers) = parse_ssdp(packet).unwrap();
        // Headers should be normalized to uppercase
        assert!(headers.contains_key("HOST"));
        assert!(headers.contains_key("MAN"));
        assert!(headers.contains_key("ST"));
    }

    // ============================================
    // Malformed input tests
    // ============================================

    #[test]
    fn test_parse_ssdp_empty_buffer() {
        let packet = b"";
        let result = parse_ssdp(packet);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_ssdp_garbage_data() {
        let packet = b"not a valid http request at all\x00\xff\xfe";
        let result = parse_ssdp(packet);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_ssdp_incomplete_request() {
        let packet = b"M-SEARCH * HTTP/1.1\r\n";
        // This should still parse the method even without complete headers
        let result = parse_ssdp(packet);
        // May succeed with just method or fail depending on httparse behavior
        // The important thing is it doesn't panic
        let _ = result;
    }
}
