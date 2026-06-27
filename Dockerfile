# syntax=docker/dockerfile:1
# Build the fiducia-region CLI from the shared routing crate.
FROM rust:1-slim-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates
WORKDIR /build
ARG INTERFACES_REF=main
RUN git clone --depth 1 --branch "$INTERFACES_REF" \
    https://github.com/fiducia-cloud/fiducia-interfaces.git fiducia-interfaces
COPY . fiducia-routing.rs
WORKDIR /build/fiducia-routing.rs
RUN cargo build --release --bin fiducia-region && strip target/release/fiducia-region

FROM debian:bookworm-slim
RUN useradd --uid 10001 --user-group --home-dir /nonexistent --shell /usr/sbin/nologin fiducia
COPY --from=build --chown=10001:10001 /build/fiducia-routing.rs/target/release/fiducia-region /usr/local/bin/fiducia-region
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/fiducia-region"]
