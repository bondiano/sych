# Reviewer image for `sych serve`. The runtime keeps the pinned Rust toolchain: rust-analyzer
# reads reviewed Rust workspaces through offline `cargo metadata`.
FROM docker.io/library/rust:1.94.1-slim-trixie@sha256:cf09adf8c3ebaba10779e5c23ff7fe4df4cccdab8a91f199b0c142c53fef3e1a AS build
WORKDIR /src
# `.git` is not in the context: the build names its commit, which a self-review server checks.
ARG SYCH_BUILD_COMMIT
RUN test -n "$SYCH_BUILD_COMMIT"
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    SYCH_BUILD_COMMIT="$SYCH_BUILD_COMMIT" cargo build --release --locked -p sych-app --bin sych \
    && install -m 0755 target/release/sych /usr/local/bin/sych

FROM docker.io/library/rust:1.94.1-slim-trixie@sha256:cf09adf8c3ebaba10779e5c23ff7fe4df4cccdab8a91f199b0c142c53fef3e1a
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add rustfmt clippy \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin sych
COPY --from=build /usr/local/bin/sych /usr/local/bin/sych
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
