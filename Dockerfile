# Runtime image - just copies pre-built binary
FROM scratch

ARG TARGETARCH

COPY --chmod=755 dlna-proxy-${TARGETARCH} /dlna-proxy

# SSDP uses UDP multicast on port 1900
EXPOSE 1900/udp

ENTRYPOINT ["/dlna-proxy"]
