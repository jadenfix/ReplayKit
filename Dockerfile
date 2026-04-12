FROM rust:1-bookworm AS build

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY examples ./examples
COPY apps ./apps

RUN cargo build -p replaykit-cli --release

FROM debian:bookworm-slim

WORKDIR /app

COPY --from=build /app/target/release/replaykit /usr/local/bin/replaykit

ENV REPLAYKIT_STORAGE=sqlite
ENV REPLAYKIT_DB_PATH=/data/replaykit.db

VOLUME ["/data"]

ENTRYPOINT ["replaykit"]
CMD ["demo-branch"]
