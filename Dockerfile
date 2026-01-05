# Build stage
FROM --platform=$BUILDPLATFORM rust:latest AS builder

ARG TARGETPLATFORM

RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*

# Add appropriate musl target based on target platform
RUN case "$TARGETPLATFORM" in \
        "linux/amd64") rustup target add x86_64-unknown-linux-musl ;; \
        "linux/arm64") rustup target add aarch64-unknown-linux-musl ;; \
        *) echo "Unsupported platform: $TARGETPLATFORM" && exit 1 ;; \
    esac

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build for the appropriate target
RUN case "$TARGETPLATFORM" in \
        "linux/amd64") cargo build --release --target x86_64-unknown-linux-musl && \
            cp target/x86_64-unknown-linux-musl/release/dlnaproxy /dlnaproxy ;; \
        "linux/arm64") cargo build --release --target aarch64-unknown-linux-musl && \
            cp target/aarch64-unknown-linux-musl/release/dlnaproxy /dlnaproxy ;; \
    esac

# Runtime stage - using scratch for minimal image size
FROM scratch

COPY --from=builder /dlnaproxy /dlnaproxy

# SSDP uses UDP multicast on port 1900
EXPOSE 1900/udp

ENTRYPOINT ["/dlnaproxy"]
