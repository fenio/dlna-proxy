# Changelog

All notable changes to this project will be documented in this file.

This project is a fork of [Nic0w/dlnaproxy](https://github.com/Nic0w/dlnaproxy).

## [0.4.1] - 2025-01-07

### Added

- **Wait option (`-w, --wait`)**: Wait for remote server to become available at startup with configurable retry interval (default: 30s). Useful when running via crontab at boot before VPN is ready.
- **Configurable timeouts**:
  - `--connect-timeout`: HTTP connect timeout for fetching XML description (default: 2s)
  - `--proxy-timeout`: TCP connect timeout for proxy connections to origin (default: 10s)
  - `--stream-timeout`: TCP read/write timeout for active proxy streams (default: 300s)
- All new options available via both CLI and config file

### Fixed

- **Broadcast resilience**: Broadcast loop no longer stops when origin server becomes temporarily unavailable. It now logs a warning and continues retrying on the next interval, allowing automatic recovery when the server comes back online.

## [0.4.0] - 2025-01-06

First release of the fork with significant bug fixes and new features.

### Added

- Docker support with multi-arch images (amd64, arm64) published to ghcr.io
- GitHub Actions workflows for Docker image builds and binary releases
- Multi-arch binary releases (x86_64, aarch64, MIPS targets)
- Comprehensive README with Docker and config file examples

### Fixed

- **SSDP source port issue**: Split into separate listen/broadcast sockets. Some clients (like certain TVs) ignore NOTIFY packets originating from port 1900. Broadcasts now use an ephemeral port for better compatibility.
- **M-SEARCH response**: Now responds to `ssdp:all` and `upnp:rootdevice` queries, not just MediaServer-specific ones. Fixes discovery with certain TVs and media players.
- **TCP proxy file descriptor leak**: Added connection timeout (10s) and read/write timeouts (5min). Replaced panics with proper error handling. Prevents "Too many open files" errors from hanging connections.
- **SO_REUSEADDR timing**: Set socket options before bind, allowing dlna-proxy to coexist with other SSDP listeners (e.g., Home Assistant's SSDP integration) on port 1900.

### Changed

- Renamed project from `dlnaproxy` to `dlna-proxy`

## [0.3.2] - Upstream

Last version from upstream [Nic0w/dlnaproxy](https://github.com/Nic0w/dlnaproxy).
