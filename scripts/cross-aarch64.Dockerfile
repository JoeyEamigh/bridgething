FROM rust:1.94-bookworm

RUN apt-get update && \
    apt-get install --assume-yes --no-install-recommends chromium && \
    rm -rf /var/lib/apt/lists/*

COPY scripts/cross-aarch64-deps.sh scripts/libswupdate-stub.sh /tmp/scripts/
COPY crates/swupdate-sys/vendor/swupdate /tmp/swupdate
RUN /tmp/scripts/cross-aarch64-deps.sh /tmp/swupdate && rm -rf /tmp/swupdate /tmp/scripts

RUN rustup target add aarch64-unknown-linux-gnu && rustup component add clippy

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig
