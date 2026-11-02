# The Linux computer-use live test's desktop: Xvfb, a window manager, the accessibility bus and
# real GTK apps, plus what builds brigadier-cu. run.sh builds the helper inside and drives it.
FROM ubuntu:24.04
ARG RUST_TOOLCHAIN=1.98.1
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl build-essential pkg-config \
      libx11-dev libxi-dev libxtst-dev libxext-dev libwayland-dev libxkbcommon-dev libdbus-1-dev \
      xvfb mutter dbus dbus-x11 at-spi2-core libatk-adaptor libgtk-3-0 \
      mousepad thunar zenity xterm xdotool x11-utils python3 \
    && rm -rf /var/lib/apt/lists/* \
    # run.sh names the caller's uid here: dbus refuses a user without a passwd entry.
    && chmod a+w /etc/passwd
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN" -c rustfmt,clippy \
    && chmod -R a+w /opt/cargo /opt/rustup
