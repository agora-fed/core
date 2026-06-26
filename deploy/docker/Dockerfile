# Minimal runtime image for the dsoc-gateway binary. The binary is built externally
# (cargo --release on an x86_64 Debian-13 host with the same libc as the runtime), so this stage
# only stages the binary + the static web bundle into a small runtime layer. See
# docs/ops/DEPLOYMENT-k3s-vm.md for the full build/deploy procedure.
FROM debian:trixie-slim

# Runtime deps: TLS roots only — the binary statically links rustls and links libc/libm/libgcc
# dynamically (the base image provides those). No package manager remains in the final image.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Drop privileges. The gateway needs nothing root-owned at runtime.
RUN useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin dsoc
USER 10001:10001

COPY --chown=10001:10001 target/release/dsoc-gateway /usr/local/bin/dsoc-gateway
COPY --chown=10001:10001 web/dist /srv/web

ENV WEB_ROOT=/srv/web GATEWAY_PORT=8080 RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/dsoc-gateway"]
