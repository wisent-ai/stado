FROM rust:1.94-slim AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY backend/ backend/
COPY agent/Cargo.toml agent/Cargo.toml
RUN mkdir -p agent/src && echo "fn main(){}" > agent/src/main.rs
ENV SQLX_OFFLINE=true
RUN cargo build --release --package wisent-backend

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/wisent-backend /usr/local/bin/
EXPOSE 8080
CMD ["wisent-backend"]
