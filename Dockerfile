# ---- Build Stage ----
FROM rust:1.93-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3-dev pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

RUN CARGO_TARGET_DIR=/app/target cargo build --release

# Smoke test: verify the binary can evaluate Nulang code
RUN /app/target/release/nulang --eval 'perform IO.print("hello")'

# ---- Runtime Stage ----
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/nulang /usr/local/bin/nulang

ENTRYPOINT ["nulang"]
