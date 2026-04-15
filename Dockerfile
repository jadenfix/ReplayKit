FROM rust:1-bookworm AS build

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY examples ./examples
COPY fixtures ./fixtures
COPY apps ./apps

RUN cargo build --release -p replaykit-cli -p replaykit-collector

FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/replaykit /usr/local/bin/replaykit
COPY --from=build /app/target/release/replaykit-collector /usr/local/bin/replaykit-collector

ENV REPLAYKIT_STORAGE=sqlite
ENV REPLAYKIT_DATA_ROOT=/data

VOLUME ["/data"]
