# The release build runs here so the AppImage needs only glibc 2.35 (Ubuntu 22.04) and no newer.
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config curl ca-certificates git cmake patchelf file \
      libx11-dev libx11-xcb-dev libxcb1-dev libxcb-xkb-dev libxcursor-dev libxi-dev libxrandr-dev \
      libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig1-dev libfreetype-dev \
      libpam0g-dev libssl-dev libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*
# One shared toolchain location any uid can use; the build runs as the caller's uid.
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
    && mkdir -p /opt/cargo/registry && chmod -R a+rwX /opt/rustup /opt/cargo
