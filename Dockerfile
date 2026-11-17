# syntax=docker/dockerfile:1

# Cross-compiles on the build host's native platform (via tonistiigi/xx) instead of
# compiling under QEMU emulation, which is very slow for Rust.
FROM --platform=$BUILDPLATFORM tonistiigi/xx:1.10.0 AS xx

# Alpine so xx-cargo targets musl and produces a fully static binary for `scratch`.
# Keep the Rust version in step with .tool-versions.
FROM --platform=$BUILDPLATFORM rust:1.100-alpine AS build
COPY --from=xx / /
RUN apk add --no-cache clang lld ca-certificates
ARG TARGETPLATFORM
RUN xx-apk add --no-cache musl-dev gcc

WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/app/target,id=target-$TARGETPLATFORM \
    xx-cargo build --release --locked \
    && xx-verify --static target/$(xx-cargo --print-target-triple)/release/docker-monitor \
    && cp target/$(xx-cargo --print-target-triple)/release/docker-monitor /docker-monitor

FROM scratch

# The AWS SDK verifies TLS against the system CA bundle, which scratch doesn't have.
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ENV AWS_DEFAULT_REGION=us-east-1

COPY --from=build /docker-monitor /docker-monitor

CMD ["/docker-monitor"]
