# fldr-server image. Builds with the classic builder (no BuildKit features).
#
#   docker build --build-arg FLDR_GIT_SHA=$(git rev-parse --short=12 HEAD) -t fldr-server .
#
# FLDR_GIT_SHA is optional: when unset, build.rs reads the commit from the
# .git directory in the build context (Dokploy's clone includes it). If
# neither is available, /health simply omits gitSha.

FROM rust:1.99-bookworm AS builder
WORKDIR /src

# 1. Dependency layer: the manifests plus stub sources, so editing fldr code
#    does not rebuild the dependency graph.
COPY Cargo.toml Cargo.lock ./
COPY crates/fldr-core/Cargo.toml crates/fldr-core/
COPY crates/fldr-store/Cargo.toml crates/fldr-store/
COPY crates/fldr-server/Cargo.toml crates/fldr-server/
COPY crates/fldr-cli/Cargo.toml crates/fldr-cli/
RUN for c in fldr-core fldr-store; do \
        mkdir -p crates/$c/src && echo > crates/$c/src/lib.rs; \
    done \
 && for c in fldr-server fldr-cli; do \
        mkdir -p crates/$c/src && echo 'fn main() {}' > crates/$c/src/main.rs; \
    done \
 && cargo build --release --locked -p fldr-server \
 && rm -rf crates

# 2. The real build. Copy the full context (crates plus .git, for build.rs)
#    and touch sources so cargo never trusts stub artifacts whose mtimes
#    could be newer than the copied files.
COPY . .
ARG FLDR_GIT_SHA=
ENV FLDR_GIT_SHA=${FLDR_GIT_SHA}
RUN find crates -type f -exec touch {} + \
 && cargo build --release --locked -p fldr-server \
 && cp target/release/fldr-server /usr/local/bin/fldr-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10001 fldr \
 && useradd --system --uid 10001 --gid fldr --home-dir /data --no-create-home --shell /usr/sbin/nologin fldr \
 && mkdir -p /data \
 && chown fldr:fldr /data
COPY --from=builder /usr/local/bin/fldr-server /usr/local/bin/fldr-server

# Must follow the chown above so a fresh named volume inherits fldr ownership.
VOLUME /data
ENV DATA_DIR=/data PORT=8080 BIND=0.0.0.0 FLDR_LOG_FORMAT=json RUST_LOG=info
USER fldr
WORKDIR /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=90s --retries=3 \
    CMD ["/usr/local/bin/fldr-server", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/fldr-server"]
