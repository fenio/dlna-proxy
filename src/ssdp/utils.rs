use log::{debug, trace};
use tokio::net::ToSocketAddrs;
use tokio::net::UdpSocket;

use anyhow::Context;
use anyhow::Result;
use reqwest::header::SERVER;
use serde::Deserialize;

use crate::ssdp::packet::SSDPPacket;

#[derive(Debug, Deserialize)]
pub(crate) struct DLNADevice {
    #[serde(rename = "deviceType")]
    pub(crate) device_type: String,

    #[serde(rename = "UDN")]
    pub(crate) unique_device_name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DLNADescription {
    pub(crate) device: DLNADevice,
}

pub struct EndpointInfo {
    pub device_type: String,
    pub unique_device_name: String,
    pub server: String,
}

pub struct InteractiveSSDP {
    http_client: reqwest::Client,
    remote_desc_url: String,
    cache_max_age: usize,
}

impl InteractiveSSDP {
    pub fn new(client: reqwest::Client, url: &str, cache_max_age: usize) -> Self {
        InteractiveSSDP {
            http_client: client,
            remote_desc_url: url.into(),
            cache_max_age,
        }
    }

    async fn fetch_endpoint_info(&self) -> Result<EndpointInfo> {
        trace!(target: "dlnaproxy", "Fetching remote server's info.");

        let endpoint_response = self
            .http_client
            .get(&self.remote_desc_url)
            .send()
            .await
            .context("Failed to get description of remote endpoint.")?;

        let server_ua = endpoint_response
            .headers()
            .get(SERVER)
            .map(|hv| String::from_utf8_lossy(hv.as_bytes()).to_string())
            .unwrap_or_else(|| "DLNAProxy/1.0".into());

        let body = endpoint_response
            .text()
            .await
            .context("Failed to parse response's body as text.")?;

        let device_description: DLNADescription =
            quick_xml::de::from_str(&body).context("Failed to parse device's XML description.")?;

        Ok(EndpointInfo {
            device_type: device_description.device.device_type,
            unique_device_name: device_description.device.unique_device_name,
            server: server_ua,
        })
    }

    async fn send_to(
        &self,
        socket: &UdpSocket,
        dest: impl ToSocketAddrs,
        ssdp_packet: SSDPPacket,
        p_type: &str,
    ) -> Result<()> {
        trace!(target: "dlnaproxy", "{}", ssdp_packet);

        ssdp_packet.send_to(socket, dest).await?;

        debug!(target: "dlnaproxy", "Sent ssdp:{} packet !", p_type);
        Ok(())
    }

    pub async fn send_alive(&self, socket: &UdpSocket, dest: impl ToSocketAddrs) -> Result<()> {
        let info = self.fetch_endpoint_info().await?;

        let ssdp_alive = SSDPPacket::Alive {
            desc_url: self.remote_desc_url.clone(),
            server_ua: info.server,
            device_type: info.device_type,
            unique_device_name: info.unique_device_name,
            cache_max_age: self.cache_max_age,
        };

        self.send_to(socket, dest, ssdp_alive, "alive").await
    }

    pub async fn send_ok(&self, socket: &UdpSocket, dest: impl ToSocketAddrs) -> Result<()> {
        let info = self.fetch_endpoint_info().await?;

        let ssdp_ok = SSDPPacket::Ok {
            desc_url: self.remote_desc_url.clone(),
            unique_device_name: info.unique_device_name,
            device_type: info.device_type,
            server_ua: info.server,
            cache_max_age: self.cache_max_age,
        };

        self.send_to(socket, dest, ssdp_ok, "ok").await
    }

    pub async fn send_byebye(&self, socket: &UdpSocket, dest: impl ToSocketAddrs) -> Result<()> {
        let info = self.fetch_endpoint_info().await?;

        let ssdp_byebye = SSDPPacket::ByeBye {
            unique_device_name: info.unique_device_name,
            device_type: info.device_type,
        };

        self.send_to(socket, dest, ssdp_byebye, "byebye").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================
    // XML parsing tests for DLNADescription
    // ============================================

    #[test]
    fn test_parse_dlna_description_basic() {
        let xml = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
    <device>
        <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
        <UDN>uuid:4d696e69-444c-164e-9d41-ecf4bb8d1234</UDN>
    </device>
</root>"#;

        let desc: DLNADescription = quick_xml::de::from_str(xml).unwrap();
        assert_eq!(desc.device.device_type, "urn:schemas-upnp-org:device:MediaServer:1");
        assert_eq!(desc.device.unique_device_name, "uuid:4d696e69-444c-164e-9d41-ecf4bb8d1234");
    }

    #[test]
    fn test_parse_dlna_description_with_extra_fields() {
        let xml = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
    <specVersion>
        <major>1</major>
        <minor>0</minor>
    </specVersion>
    <device>
        <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
        <friendlyName>My Media Server</friendlyName>
        <manufacturer>Test Company</manufacturer>
        <modelName>Test Model</modelName>
        <modelNumber>1.0</modelNumber>
        <UDN>uuid:test-device-udn</UDN>
        <serviceList>
            <service>
                <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
            </service>
        </serviceList>
    </device>
</root>"#;

        let desc: DLNADescription = quick_xml::de::from_str(xml).unwrap();
        // Should parse successfully, ignoring extra fields
        assert_eq!(desc.device.device_type, "urn:schemas-upnp-org:device:MediaServer:1");
        assert_eq!(desc.device.unique_device_name, "uuid:test-device-udn");
    }

    #[test]
    fn test_parse_dlna_description_minimal() {
        // Test with minimal required fields only
        let xml = r#"<root>
    <device>
        <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
        <UDN>uuid:minimal-device</UDN>
    </device>
</root>"#;

        let desc: DLNADescription = quick_xml::de::from_str(xml).unwrap();
        assert_eq!(desc.device.device_type, "urn:schemas-upnp-org:device:MediaRenderer:1");
        assert_eq!(desc.device.unique_device_name, "uuid:minimal-device");
    }

    #[test]
    fn test_parse_dlna_description_missing_device_type() {
        let xml = r#"<root>
    <device>
        <UDN>uuid:test-device</UDN>
    </device>
</root>"#;

        let result: Result<DLNADescription, _> = quick_xml::de::from_str(xml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dlna_description_missing_udn() {
        let xml = r#"<root>
    <device>
        <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    </device>
</root>"#;

        let result: Result<DLNADescription, _> = quick_xml::de::from_str(xml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dlna_description_missing_device() {
        let xml = r#"<root>
</root>"#;

        let result: Result<DLNADescription, _> = quick_xml::de::from_str(xml);
        assert!(result.is_err());
    }
}
