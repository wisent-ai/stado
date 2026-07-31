# Builds the stado Rust binaries for linux/amd64 (fleet rollout).
# Used by the provider-neutral Stado release publisher; binaries land in /out.
FROM rust:slim-bookworm AS build
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY data ./data
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    CARGO_BUILD_JOBS="$(printf x | /usr/bin/wc -c)" cargo build --release --locked \
    && mkdir -p /out \
    && cp target/release/stado \
          target/release/stado-coverage \
          target/release/stado-fix \
          target/release/stado-watchdog \
          target/release/stado-mcp \
          /out/ \
    && cp target/release/stado /out/wc \
    && cd /out && sha256sum * > SHA256SUMS

FROM debian:bookworm-slim
COPY --from=build /out /out
COPY --from=build /app/data /app/data
ENTRYPOINT ["/bin/false"]
