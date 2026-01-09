# Changelog

All notable changes to this project will be documented in this file.

This project is a fork of [Nic0w/dlnaproxy](https://github.com/Nic0w/dlnaproxy).

## [0.5.0] - 2026-01-09

### Added

- **Windows and macOS build targets**: Binary releases now include Windows (64-bit and 32-bit) and macOS (Intel and Apple Silicon) builds in addition to Linux/MIPS targets.

### Changed

- **Updated all dependencies to latest versions**:
  - `fern` 0.6.2 → 0.7.1
  - `nix` 0.29.0 → 0.30.1
  - `quick-xml` 0.36.2 → 0.38.4
  - `thiserror` 1.0.x → 2.0.17
  - `toml` 0.8.23 → 0.9.11 (TOML 1.1 spec support)
  - `reqwest` 0.12.28 → 0.13.1
  - `socket2` 0.5.10 → 0.6.1

### Fixed

- **TCP proxy: properly sanitize log output**: Fixed log sanitization to only allow ASCII printable characters. The previous fix still allowed UTF-8 replacement characters (�) to appear in logs when binary data was received.
- **TCP proxy: handle streaming responses without Content-Length**: Responses without Content-Length and not chunked (common for live media streams) are now properly streamed until connection close. Previously, the proxy would loop back expecting another HTTP response, causing repeated log spam and incorrect handling.
- **Code quality**: Fixed all Clippy warnings and applied consistent rustfmt formatting.

## [0.4.8] - 2026-01-09

### Fixed

- **TCP proxy: properly sanitize log output**: Fixed log sanitization to only allow ASCII printable characters. The previous fix still allowed UTF-8 replacement characters (�) to appear in logs when binary data was received.
- **TCP proxy: handle streaming responses without Content-Length**: Responses without Content-Length and not chunked (common for live media streams) are now properly streamed until connection close. Previously, the proxy would loop back expecting another HTTP response, causing repeated log spam and incorrect handling.

## [0.4.7] - 2026-01-09

### Fixed

- **TCP proxy: sanitize log output for non-HTTP data**: When non-HTTP data arrives (e.g., binary protocols), the log now filters control characters and truncates long lines to prevent garbage in logs.
- **Ctrl+C exit reliability**: Added a 2-second timeout when sending ssdp:byebye on shutdown. Previously, if the network was slow or unresponsive, the byebye message could hang indefinitely preventing clean exit.

## [0.4.6] - 2026-01-09

### Fixed

- **TCP proxy: use raw byte parsing for HTTP headers and chunked encoding**: All line-based parsing in the TCP proxy now uses raw bytes instead of requiring valid UTF-8. This fixes "stream did not contain valid UTF-8" errors that occurred when DVB streamers or other servers sent responses with non-standard encoding in headers or chunked transfer framing.

## [0.4.5] - 2026-01-09

### Fixed

- **TCP proxy: default to binary pass-through when Content-Type is missing**: Responses without a Content-Type header are now passed through unchanged instead of attempting URL rewriting. This fixes streaming from DVB and other servers that don't send Content-Type headers for media streams.

## [0.4.4] - 2026-01-09

### Fixed

- **TCP proxy now handles binary content correctly**: URL rewriting is now only performed on text/XML responses (based on Content-Type header). Binary content like images, videos, and other media files are passed through unchanged. This fixes the "stream did not contain valid UTF-8" errors when clients requested media files through the proxy.

## [0.4.2] - 2025-01-08

### Fixed

- **Wait mode now works with non-existent servers**: With `-w/--wait`, dlna-proxy now starts immediately even if the origin server doesn't exist yet. It will keep retrying on each broadcast interval until the server appears. Previously it would fail at startup.

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
