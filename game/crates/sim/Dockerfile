# syntax=docker/dockerfile:1.4
FROM rust:1-bookworm AS builder
# System deps for Bevy/winit (wayland-sys via pkg-config). Without these the
# release build fails with "Package wayland-client was not found".
RUN apt-get update && apt-get install -y --no-install-recommends \
  pkg-config libwayland-dev libxkbcommon-dev libudev-dev libasound2-dev libx11-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# Cache workspace deps — copy manifests first.
COPY Cargo.toml ./Cargo.toml
COPY Cargo.lock ./Cargo.lock
COPY crates/game/Cargo.toml ./crates/game/Cargo.toml
COPY crates/mcp-server/Cargo.toml ./crates/mcp-server/Cargo.toml
COPY crates/server/Cargo.toml ./crates/server/Cargo.toml
COPY crates/server/clients/rust/Cargo.toml ./crates/server/clients/rust/Cargo.toml
COPY crates/net/Cargo.toml ./crates/net/Cargo.toml
COPY crates/sim/Cargo.toml ./crates/sim/Cargo.toml
RUN mkdir -p crates/game/src crates/mcp-server/src crates/server/src crates/server/clients/rust/src crates/net/src crates/sim/src \
 && echo "fn main(){}" > crates/mcp-server/src/main.rs \
 && echo "pub fn dummy(){}" > crates/game/src/lib.rs \
 && echo "pub fn dummy(){}" > crates/server/src/lib.rs \
 && echo "pub fn dummy(){}" > crates/server/clients/rust/src/lib.rs \
 && echo "pub fn dummy(){}" > crates/net/src/lib.rs \
 && echo "pub fn dummy(){}" > crates/sim/src/lib.rs \
 && echo "fn main(){}" > crates/sim/src/main.rs \
 && cargo build -p party-dungeon-sim --release 2>/dev/null || true

# Real build — copy the whole game crate (Bevy needs assets for the sim's collider builder).
COPY . ./game
# The sim only needs the `game` crate at compile time, not at runtime.
WORKDIR /app/game
ARG CARGO_FEATURES=""
RUN cargo build -p party-dungeon-sim --release ${CARGO_FEATURES:+--features "$CARGO_FEATURES"}

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/game/target/release/party-dungeon-sim /usr/local/bin/party-dungeon-sim
EXPOSE 6536 6565
ENTRYPOINT ["party-dungeon-sim"]
