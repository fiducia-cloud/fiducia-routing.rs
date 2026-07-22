# syntax=docker/dockerfile:1
# Build the fiducia-region CLI from the shared routing crate.
FROM rust:1.97.1-slim-bookworm@sha256:99e09cb2284e2ddbb73a995deee3e91783fd04d177602ccf6eab326d778ee777 AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates
WORKDIR /build
ARG INTERFACES_REF=487e470c45ab5851e8f6f3b1dc048fe067fbf408
RUN git init fiducia-interfaces \
    && git -C fiducia-interfaces remote add origin https://github.com/fiducia-cloud/fiducia-interfaces.git \
    && git -C fiducia-interfaces fetch --depth 1 origin "$INTERFACES_REF" \
    && test "$(git -C fiducia-interfaces rev-parse FETCH_HEAD)" = "$INTERFACES_REF" \
    && git -C fiducia-interfaces checkout --detach FETCH_HEAD \
    && test "$(git -C fiducia-interfaces rev-parse HEAD)" = "$INTERFACES_REF"
COPY . fiducia-routing.rs
WORKDIR /build/fiducia-routing.rs
RUN cargo build --locked --release --bin fiducia-region && strip target/release/fiducia-region

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e
COPY --from=build --chown=65532:65532 /build/fiducia-routing.rs/target/release/fiducia-region /usr/local/bin/fiducia-region
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/fiducia-region"]
