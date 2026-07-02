# Production gateway image (docs/ops/DEPLOYMENT-k3s-vm.md). Build context is deploy/docker/ with
# the two artifacts staged in first:
#   cp target-docker/release/dsoc-gateway deploy/docker/dsoc-gateway   (built in rust:1-bookworm —
#     glibc 2.36 forward-compatible with trixie; a binary from a rolling-release host is not)
#   cp -r web/dist deploy/docker/web
# Then: docker build -t dsoc-gateway:<version> deploy/docker
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && apt-get clean
COPY dsoc-gateway /usr/local/bin/dsoc-gateway
COPY web /srv/web
ENV GATEWAY_PORT=8080 WEB_ROOT=/srv/web
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/dsoc-gateway"]
