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
    # ── X11 / D-Bus ──
    dbus-x11 \
    x11-xserver-utils \
    x11-utils \
    xfonts-base \
    xfonts-100dpi \
    xfonts-75dpi \
    # ── Python for fake-logind ──
    python3-gi \
    python3-gi-cairo \
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
    xdg-user-dirs \
    && locale-gen en_US.UTF-8 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

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

COPY fake-logind.py /usr/local/bin/fake-logind.py
RUN chmod +x /usr/local/bin/fake-logind.py

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 6901

USER vncuser
WORKDIR /home/vncuser

ENTRYPOINT ["/entrypoint.sh"]
