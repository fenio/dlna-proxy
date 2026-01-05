# Build stage - builds natively for each target platform
FROM rust:latest AS builder

RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*

# Add musl target for current architecture
RUN case "$(uname -m)" in \
        "x86_64") rustup target add x86_64-unknown-linux-musl ;; \
        "aarch64") rustup target add aarch64-unknown-linux-musl ;; \
    esac

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build for current architecture
RUN case "$(uname -m)" in \
        "x86_64") cargo build --release --target x86_64-unknown-linux-musl && \
            cp target/x86_64-unknown-linux-musl/release/dlnaproxy /dlnaproxy ;; \
        "aarch64") cargo build --release --target aarch64-unknown-linux-musl && \
            cp target/aarch64-unknown-linux-musl/release/dlnaproxy /dlnaproxy ;; \
    esac

# Runtime stage - using scratch for minimal image size
FROM scratch

COPY --from=builder /dlnaproxy /dlnaproxy

# SSDP uses UDP multicast on port 1900
EXPOSE 1900/udp

ENTRYPOINT ["/dlnaproxy"]
