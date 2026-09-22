# ═══════════════════════════════════════════════════════════════════════════
# Stage 1 — build the "Mount Manager" app (Rust/GTK4) into a .deb + apt repo
# ═══════════════════════════════════════════════════════════════════════════
FROM ubuntu:24.04 AS mount-manager-build

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        curl \
        ca-certificates \
        dpkg-dev \
        debhelper \
        apt-utils \
        gzip \
        python3 \
        libgtk-4-dev \
        libadwaita-1-dev \
    && rm -rf /var/lib/apt/lists/*

# Ubuntu 24.04 ships rustc 1.75, which is older than what gtk4-rs 0.9 and its
# dependencies expect, so the toolchain comes from rustup.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain 1.85.0

WORKDIR /src/mount-manager
COPY mount-manager/ /src/mount-manager/
RUN cargo generate-lockfile
RUN ./packaging/build-deb.sh
# Turn dist/*.deb into a file:// APT repository so the package can be installed
# (and re-installed) with plain `apt-get install mount-manager`.
RUN ./packaging/make-apt-repo.sh /src/mount-manager/dist /src/mount-manager/dist/repo

# ═══════════════════════════════════════════════════════════════════════════
# Stage 2 — the GNOME webtop image
# ═══════════════════════════════════════════════════════════════════════════
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
ENV LANG=en_US.UTF-8
ENV LANGUAGE=en_US:en
ENV LC_ALL=en_US.UTF-8

RUN apt-get update && apt-get install -y --no-install-recommends \
    # ── GNOME core desktop ──
    gnome-shell \
    gnome-session \
    gnome-terminal \
    nautilus-extension-gnome-terminal \
    gnome-control-center \
    gnome-settings-daemon \
    gnome-tweaks \
    mutter \
    nautilus \
    # ── Ubuntu Yaru theme ──
    yaru-theme-gnome-shell \
    yaru-theme-gtk \
    yaru-theme-icon \
    yaru-theme-sound \
    # ── Icon themes (critical — provides actual app icons) ──
    adwaita-icon-theme \
    adwaita-icon-theme-full \
    hicolor-icon-theme \
    ubuntu-mono \
    dmz-cursor-theme \
    librsvg2-common \
    libgdk-pixbuf2.0-bin \
    # ── Desktop experience: dock, icons, backgrounds ──
    gnome-shell-extension-ubuntu-dock \
    gnome-shell-extension-desktop-icons-ng \
    gnome-backgrounds \
    ubuntu-wallpapers \
    # ── GNOME Flashback fallback ──
    gnome-flashback \
    gnome-panel \
    metacity \
    # ── Mesa software rendering (llvmpipe) ──
    mesa-utils \
    libgl1-mesa-dri \
    libglx-mesa0 \
    libgl1 \
    libegl-mesa0 \
    libegl1 \
    libgles2 \
    libglvnd0 \
    # ── X virtual framebuffer + VNC ──
    xvfb \
    x11vnc \
    novnc \
    websockify \
    # ── Audio (PulseAudio virtual sink → browser) ──
    pulseaudio \
    pulseaudio-utils \
    libpulse0 \
    # ── X11 / D-Bus ──
    dbus-x11 \
    x11-xserver-utils \
    x11-utils \
    xfonts-base \
    xfonts-100dpi \
    xfonts-75dpi \
    # ── Python for fake-logind + audio bridge ──
    python3 \
    python3-gi \
    python3-gi-cairo \
    python3-websockets \
    gir1.2-gio-2.0 \
    # ── Utilities ──
    sudo \
    wget \
    curl \
    net-tools \
    locales \
    fonts-dejavu-core \
    fonts-liberation \
    fonts-noto-color-emoji \
    at-spi2-core \
    python3-nautilus \
    xdg-user-dirs \
    # ── SMB/CIFS client (Nautilus network shares + mount + CLI) ──
    gvfs-backends \
    gvfs-fuse \
    cifs-utils \
    smbclient \
    && locale-gen en_US.UTF-8 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# ── Mount Manager: publish the local APT repo, then install the app ────────
# The .deb is built in the stage above. Publishing the repository means the
# container can also run `apt-get install --reinstall mount-manager` later.
COPY --from=mount-manager-build /src/mount-manager/dist/repo /opt/mount-manager/repo
RUN echo "deb [trusted=yes] file:/opt/mount-manager/repo stable main" \
       > /etc/apt/sources.list.d/mount-manager.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        mount-manager \
        policykit-1 \
        sshfs \
        nfs-common \
        davfs2 \
        avahi-utils \
        libsecret-tools \
        libglib2.0-bin \
        fuse3 \
        psmisc \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && dpkg -l mount-manager \
    && mount-manager --version

# Install Firefox from Mozilla official APT repo (Ubuntu 24.04 firefox pkg is a snap stub)
RUN install -d -m 0755 /etc/apt/keyrings \
    && wget -q https://packages.mozilla.org/apt/repo-signing-key.gpg -O /etc/apt/keyrings/packages.mozilla.org.asc \
    && echo "deb [signed-by=/etc/apt/keyrings/packages.mozilla.org.asc] https://packages.mozilla.org/apt mozilla main" \
       > /etc/apt/sources.list.d/mozilla.list \
    && printf "Package: *\nPin: origin packages.mozilla.org\nPin-Priority: 1000\n" \
       > /etc/apt/preferences.d/mozilla \
    && apt-get update \
    && apt-get install -y --no-install-recommends firefox \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Register SVG pixbuf loader (fixes red-square icons) + rebuild icon caches
RUN /usr/lib/*/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders --update-cache

# Rebuild icon caches so icons render at correct sizes (fixes blurriness)
RUN gtk-update-icon-cache -f -t /usr/share/icons/Yaru 2>/dev/null || true \
    && gtk-update-icon-cache -f -t /usr/share/icons/Adwaita 2>/dev/null || true \
    && gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true \
    && gtk-update-icon-cache -f -t /usr/share/icons/ubuntu-mono-dark 2>/dev/null || true \
    && gtk-update-icon-cache -f -t /usr/share/icons/ubuntu-mono-light 2>/dev/null || true

# Remove D-Bus activation for real logind (we use fake-logind instead)
RUN rm -f /usr/share/dbus-1/system-services/org.freedesktop.login1.service \
    && rm -f /usr/share/dbus-1/system-services/org.freedesktop.hostname1.service \
    && rm -f /usr/share/dbus-1/system-services/org.freedesktop.timedate1.service

# Non-root user with passwordless sudo
RUN useradd -m -s /bin/bash vncuser \
    && echo "vncuser:vncpass" | chpasswd \
    && usermod -aG sudo vncuser \
    && echo "vncuser ALL=(ALL) NOPASSWD: ALL" >> /etc/sudoers

RUN mkdir -p /home/vncuser/.vnc \
    && chown -R vncuser:vncuser /home/vncuser

# Install custom webtop landing page (noVNC + audio player)
COPY webtop.html /usr/share/novnc/webtop.html
COPY novnc-audio.js /usr/share/novnc/novnc-audio.js
RUN sed -i "s#</body>#<script src=\"novnc-audio.js\"></script></body>#" /usr/share/novnc/vnc.html

COPY fake-logind.py /usr/local/bin/fake-logind.py
COPY audio-bridge.py /usr/local/bin/audio-bridge.py
RUN chmod +x /usr/local/bin/fake-logind.py /usr/local/bin/audio-bridge.py

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# noVNC web + audio WebSocket
EXPOSE 6901 6902

USER vncuser
WORKDIR /home/vncuser

ENTRYPOINT ["/entrypoint.sh"]
