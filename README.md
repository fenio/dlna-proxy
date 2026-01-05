# dlnaproxy

`dlnaproxy` enables the use of a DLNA server (e.g., MiniDLNA) past the local network boundary.

## Use case

Let's say you're hosting a media library on a remote server. It might be because that remote server has more bandwidth, more storage, or both. It can also be your self-hosted NAS that you are trying to access from a remote location.

If you're able to connect to that server, either through a VPN or because the machine is routed directly on the Internet, `dlnaproxy` will attempt to connect to that server and if successful, it will announce it on your current LAN as if that server were there.

```
          Network boundary                 +------------------+
                ++          connect back   |                  |
     +----------++-------------------------+       you        |
     |          ||                         |                  |
     |          ||                         +---^--------------+
+----v-----+    ||   +------------+            |
| Remote   |    ||   |            +------------+
| DLNA     <----++---+ dlnaproxy  |    broadcast
| Server   | fetch info           |
|          |    ++   |            |
+----------+    ||   +------------+
                ||
                ||
                ++
```

## Installation

### Docker (recommended)

```bash
docker run --network host ghcr.io/fenio/dlnaproxy:main \
  -u http://REMOTE_SERVER:PORT/rootDesc.xml -vv
```

### Binary

Download the latest binary from [Releases](https://github.com/fenio/dlnaproxy/releases) or build from source:

```bash
cargo build --release
```

## Usage

### Command line

```bash
dlnaproxy -u http://192.168.1.100:8200/rootDesc.xml -vv
```

### With TCP proxy

If the remote DLNA server is not directly accessible from clients on your LAN, use the proxy option:

```bash
dlnaproxy -u http://REMOTE_SERVER:8200/rootDesc.xml -p LOCAL_IP:8200 -vv
```

This binds a local TCP proxy that forwards connections to the remote server.

### All options

```
Options:
  -c, --config <FILE>           TOML config file
  -u, --description-url <URL>   URL pointing to the remote DLNA server's root XML description
  -d, --interval <SECONDS>      Broadcast interval in seconds (default: 895)
  -p, --proxy <IP:PORT>         Local IP:PORT where to bind the TCP proxy
  -i, --iface <IFACE>           Network interface to broadcast on (requires root or CAP_NET_RAW)
  -v, --verbose...              Verbosity level (-v = info, -vv = debug, -vvv = trace)
  -h, --help                    Print help
  -V, --version                 Print version
```

### Config file

Instead of command line arguments, you can use a TOML config file:

```bash
dlnaproxy -c /path/to/config.toml
```

Example config (`config.toml.example`):

```toml
# URL pointing to the remote DLNA server's root XML description (required)
description_url = "http://192.168.1.100:8200/rootDesc.xml"

# Broadcast interval in seconds (default: 895)
period = 895

# Local IP:PORT for TCP proxy (optional)
#proxy = "192.168.1.50:8200"

# Network interface to broadcast on (optional, requires root or CAP_NET_RAW)
#iface = "eth0"

# Verbosity: 0=warn, 1=info, 2=debug, 3+=trace
verbose = 1
```

## Docker

### Pull the image

```bash
docker pull ghcr.io/fenio/dlnaproxy:main
```

### Run with command line arguments

```bash
docker run --network host ghcr.io/fenio/dlnaproxy:main \
  -u http://192.168.1.100:8200/rootDesc.xml \
  -p 192.168.1.50:8200 \
  -d 30 \
  -i eth0 \
  -vv
```

### Run with config file

```bash
docker run --network host \
  -v /path/to/config.toml:/config.toml \
  ghcr.io/fenio/dlnaproxy:main -c /config.toml
```

### Docker Compose

```yaml
services:
  dlnaproxy:
    image: ghcr.io/fenio/dlnaproxy:main
    network_mode: host
    restart: unless-stopped
    command: -u http://192.168.1.100:8200/rootDesc.xml -vv
```

### Notes

- `--network host` is required for SSDP multicast to work properly
- If using `-i` (interface binding), add `--cap-add=NET_RAW` or run privileged

## Building

### Native build

```bash
cargo build --release
```

### Cross-compile for Linux (static binary)

```bash
# For x86_64
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# For ARM64
rustup target add aarch64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

## License

See [LICENSE](LICENSE) file.
