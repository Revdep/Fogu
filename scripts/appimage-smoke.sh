#!/usr/bin/env bash
# Starts an AppImage on Ubuntu 22.04 under Xvfb, without network, and fails unless it opens a
# window and stays up: the check the AppImage catalog (appimage.github.io) runs, done before a
# release instead of after. Twice: on a bare system it must load its bundled X11/xkb libraries,
# on a desktop that has them, the system's (assets/appimage/AppRun).
# Usage: scripts/appimage-smoke.sh Fogu-x86_64.AppImage. Needs docker.
set -euo pipefail
appimage=$(realpath "$1")

# Base pinned by digest, like the build image: a changed upstream image cannot turn this green.
build() {
    docker build -q -t "$1" - <<EOF >/dev/null
FROM ubuntu@sha256:076e7bf82319619f28dd0aa9c02ab34b055ad37100395b4f49a2ffba04fb5e11
RUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
        --no-install-recommends xvfb x11-utils $2 >/dev/null && rm -rf /var/lib/apt/lists/*
EOF
}

# run IMAGE WANT: WANT is the LD_LIBRARY_PATH AppRun must hand Fogu (Fogu is not dumpable, so
# its /proc/PID/maps is closed even to root: AppRun runs again with a stub in Fogu's place).
run() {
    docker run --rm --network none -v "$appimage":/app.AppImage:ro -e WANT="$2" "$1" bash -c '
        Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
        sleep 2
        export DISPLAY=:99 APPIMAGE_EXTRACT_AND_RUN=1 HOME=/tmp/home
        mkdir -p "$HOME"
        /app.AppImage >/tmp/log.txt 2>&1 &
        app=$!
        sleep 12
        windows=$(xwininfo -root -children | grep -c "^ *0x.*[0-9]x[0-9]")
        if ! kill -0 "$app" 2>/dev/null || [ "$windows" -eq 0 ]; then
            echo "FAIL: no window after 12 s"
            tail -n 20 /tmp/log.txt
            exit 1
        fi
        cd /tmp && /app.AppImage --appimage-extract >/dev/null
        printf "#!/bin/sh\necho \"\${LD_LIBRARY_PATH-}\"\n" > squashfs-root/usr/bin/fogu
        got=$(squashfs-root/AppRun)
        if [ "$got" != "$WANT" ]; then
            echo "FAIL: AppRun gives LD_LIBRARY_PATH=\"$got\", want \"$WANT\""
            exit 1
        fi
        echo "ok: window open, LD_LIBRARY_PATH=\"$got\""
    '
}

build fogu-smoke:bare ""
build fogu-smoke:desktop "libxkbcommon-x11-0 libxcursor1 libxrandr2 libxi6"
echo "bare system:" && run fogu-smoke:bare /tmp/squashfs-root/usr/lib
echo "desktop:" && run fogu-smoke:desktop ""
