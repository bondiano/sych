# Reviewer image for `sych serve`. The runtime keeps the pinned Rust toolchain: rust-analyzer
# reads reviewed Rust workspaces through offline `cargo metadata`.
# The web UI's static build, served by `sych serve` at /ui (`http.ui_dir`).
FROM docker.io/library/node:24.22.2-slim@sha256:c86ecead9e55a5210ac1727de38a4ca840bf03dcaf34dc9749addbf383b1920c AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
COPY schemas/openapi.json /src/schemas/openapi.json
RUN pnpm gen:api && pnpm build

FROM docker.io/library/rust:1.100.0-slim-trixie@sha256:e9d95b74296f7126a83ae3d041aeaba1ff4c478f13abc5cc034fcf7c131c3d4a AS build
WORKDIR /src
# `.git` is not in the context: the build names its commit, which a self-review server checks.
ARG SYCH_BUILD_COMMIT
RUN test -n "$SYCH_BUILD_COMMIT"
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    SYCH_BUILD_COMMIT="$SYCH_BUILD_COMMIT" cargo build --release --locked -p sych-app --bin sych \
    && install -m 0755 target/release/sych /usr/local/bin/sych

FROM docker.io/library/rust:1.100.0-slim-trixie@sha256:e9d95b74296f7126a83ae3d041aeaba1ff4c478f13abc5cc034fcf7c131c3d4a
# Base image is digest-pinned; Debian security updates supersede exact apt package versions.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add rustfmt clippy \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin sych
COPY --from=build /usr/local/bin/sych /usr/local/bin/sych
COPY --from=web /src/web/dist /usr/share/sych/ui
LABEL org.opencontainers.image.title="Sych" \
      org.opencontainers.image.description="Deterministic code review: a Rust core with bounded model calls" \
      org.opencontainers.image.source="https://github.com/bondiano/sych" \
      org.opencontainers.image.documentation="https://github.com/bondiano/sych/tree/main/docs"
USER 10001
WORKDIR /data
ENV XDG_CACHE_HOME=/data/cache CARGO_HOME=/data/cargo CARGO_NET_OFFLINE=true
EXPOSE 8080
ENTRYPOINT ["sych"]
CMD ["serve", "--config", "/etc/sych/sych.yaml"]
