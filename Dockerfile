# Runtime image - just copies pre-built binary
FROM scratch

ARG TARGETARCH

COPY dlnaproxy-${TARGETARCH} /dlnaproxy

# SSDP uses UDP multicast on port 1900
EXPOSE 1900/udp

ENTRYPOINT ["/dlnaproxy"]
