use chrono::Utc;
use std::fmt;
use tokio::net::{ToSocketAddrs, UdpSocket};

use anyhow::{Context, Result};

pub enum SSDPPacket {
    Alive {
        desc_url: String,
        server_ua: String,
        unique_device_name: String,
        device_type: String,
        cache_max_age: usize,
    },
    Ok {
        desc_url: String,
        server_ua: String,
        unique_device_name: String,
        device_type: String,
        cache_max_age: usize,
    },
    ByeBye {
        unique_device_name: String,
        device_type: String,
    },
}

impl SSDPPacket {
    pub async fn send_to(&self, socket: &UdpSocket, dest: impl ToSocketAddrs) -> Result<()> {
        socket
            .send_to(self.to_string().as_bytes(), dest)
            .await
            .context("Failed to send SSDP packet on UDP socket")?;

        Ok(())
    }
}

impl fmt::Display for SSDPPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SSDPPacket::Alive {
                desc_url,
                server_ua,
                unique_device_name,
                device_type,
                cache_max_age,
            } => {
                write!(
                    f,
                    "\
NOTIFY * HTTP/1.1\r\n\
HOST:239.255.255.250:1900\r\n\
CACHE-CONTROL:max-age={cache_max_age}\r\n\
LOCATION:{location}\r\n\
SERVER: {server_ua}\r\n\
NT:{device_type}\r\n\
USN:{udn}::{device_type}\r\n\
NTS:ssdp:alive\r\n\
\r\n",
                    cache_max_age = cache_max_age,
                    location = desc_url,
                    server_ua = server_ua,
                    device_type = device_type,
                    udn = unique_device_name
                )
            }

            SSDPPacket::Ok {
                desc_url,
                server_ua,
                unique_device_name,
                device_type,
                cache_max_age,
            } => {
                let now = Utc::now().to_rfc2822().replace("+0000", "GMT");

                write!(
                    f,
                    "\
HTTP/1.1 200 OK\r\n\
CACHE-CONTROL:max-age={cache_max_age}\r\n\
DATE: {date}\r\n\
ST: {device_type}\r\n\
USN:{udn}::{device_type}\r\n\
EXT:\r\n\
SERVER: {server_ua}\r\n\
LOCATION:{location}\r\n\
Content-Length: 0\r\n\
\r\n",
                    cache_max_age = cache_max_age,
                    location = desc_url,
                    server_ua = server_ua,
                    device_type = device_type,
                    udn = unique_device_name,
                    date = now
                )
            }

            SSDPPacket::ByeBye {
                unique_device_name,
                device_type,
            } => {
                write!(
                    f,
                    "\
NOTIFY * HTTP/1.1\r\n\
HOST:239.255.255.250:1900\r\n\
NT:{device_type}\r\n\
USN:{udn}::{device_type}\r\n\
NTS:ssdp:byebye\r\n\
\r\n",
                    device_type = device_type,
                    udn = unique_device_name
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================
    // SSDPPacket::Alive Display tests
    // ============================================

    #[test]
    fn test_alive_starts_with_notify() {
        let packet = SSDPPacket::Alive {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.starts_with("NOTIFY * HTTP/1.1\r\n"));
    }

    #[test]
    fn test_alive_has_host_header() {
        let packet = SSDPPacket::Alive {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("HOST:239.255.255.250:1900\r\n"));
    }

    #[test]
    fn test_alive_has_cache_control() {
        let packet = SSDPPacket::Alive {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("CACHE-CONTROL:max-age=1800\r\n"));
    }

    #[test]
    fn test_alive_has_location() {
        let packet = SSDPPacket::Alive {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("LOCATION:http://192.168.1.1:8080/desc.xml\r\n"));
    }

    #[test]
    fn test_alive_has_server() {
        let packet = SSDPPacket::Alive {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0 UPnP/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("SERVER: Test/1.0 UPnP/1.0\r\n"));
    }

    #[test]
    fn test_alive_has_nt_usn_nts() {
        let packet = SSDPPacket::Alive {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device-123".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("NT:urn:schemas-upnp-org:device:MediaServer:1\r\n"));
        assert!(output.contains("USN:uuid:test-device-123::urn:schemas-upnp-org:device:MediaServer:1\r\n"));
        assert!(output.contains("NTS:ssdp:alive\r\n"));
    }

    #[test]
    fn test_alive_ends_with_empty_line() {
        let packet = SSDPPacket::Alive {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.ends_with("\r\n\r\n"));
    }

    // ============================================
    // SSDPPacket::Ok Display tests
    // ============================================

    #[test]
    fn test_ok_starts_with_http_200() {
        let packet = SSDPPacket::Ok {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn test_ok_has_date_header() {
        let packet = SSDPPacket::Ok {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        // DATE header should be present with GMT suffix (RFC 2822 format)
        assert!(output.contains("DATE:"));
        assert!(output.contains("GMT"));
    }

    #[test]
    fn test_ok_has_st_header() {
        let packet = SSDPPacket::Ok {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("ST: urn:schemas-upnp-org:device:MediaServer:1\r\n"));
    }

    #[test]
    fn test_ok_has_ext_header() {
        let packet = SSDPPacket::Ok {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("EXT:\r\n"));
    }

    #[test]
    fn test_ok_has_content_length_zero() {
        let packet = SSDPPacket::Ok {
            desc_url: "http://192.168.1.1:8080/desc.xml".to_string(),
            server_ua: "Test/1.0".to_string(),
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
            cache_max_age: 1800,
        };
        let output = packet.to_string();
        assert!(output.contains("Content-Length: 0\r\n"));
    }

    // ============================================
    // SSDPPacket::ByeBye Display tests
    // ============================================

    #[test]
    fn test_byebye_starts_with_notify() {
        let packet = SSDPPacket::ByeBye {
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
        };
        let output = packet.to_string();
        assert!(output.starts_with("NOTIFY * HTTP/1.1\r\n"));
    }

    #[test]
    fn test_byebye_has_host() {
        let packet = SSDPPacket::ByeBye {
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
        };
        let output = packet.to_string();
        assert!(output.contains("HOST:239.255.255.250:1900\r\n"));
    }

    #[test]
    fn test_byebye_has_nts_byebye() {
        let packet = SSDPPacket::ByeBye {
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
        };
        let output = packet.to_string();
        assert!(output.contains("NTS:ssdp:byebye\r\n"));
    }

    #[test]
    fn test_byebye_no_cache_control() {
        let packet = SSDPPacket::ByeBye {
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
        };
        let output = packet.to_string();
        assert!(!output.contains("CACHE-CONTROL"));
    }

    #[test]
    fn test_byebye_no_location() {
        let packet = SSDPPacket::ByeBye {
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
        };
        let output = packet.to_string();
        assert!(!output.contains("LOCATION"));
    }

    #[test]
    fn test_byebye_no_server() {
        let packet = SSDPPacket::ByeBye {
            unique_device_name: "uuid:test-device".to_string(),
            device_type: "urn:schemas-upnp-org:device:MediaServer:1".to_string(),
        };
        let output = packet.to_string();
        assert!(!output.contains("SERVER"));
    }
}
