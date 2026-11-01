# One image for every backend binary (conventions: binaries at /usr/local/bin/{api,poller,admin}).
# Build context is backend/. A binary is built only if its crate exists, so the image builds from the
# 03 core milestone onward and grows as poller, admin and api land.
# Each binary is built in its own cargo invocation so cargo features cannot unify across them
# (poller enables core/decrypt; api must never see it).

ARG RUST_VERSION=1.95
FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /build
ENV CARGO_TERM_COLOR=never
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    set -eux; \
    mkdir -p /out; \
    cargo build --release --locked -p famweather-core; \
    for crate in api poller admin; do \
      if [ -d "crates/${crate}" ]; then \
        cargo build --release --locked -p "${crate}"; \
        cp "target/release/${crate}" "/out/${crate}"; \
      fi; \
    done

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /out/ /usr/local/bin/
# Supabase CA for sslmode=verify-full; backend/certs/.keep is committed so this works before the CA is added.
COPY certs/ /etc/ssl/app-certs/
USER 65532:65532
# No ENTRYPOINT: Compose and Fly choose the command (api, poller, admin).
