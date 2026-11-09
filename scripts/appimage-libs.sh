#!/bin/sh
# Runs in the glibc 2.28 build container (scripts/package-linux.sh): copies the X11/xkb client
# libraries winit opens at run time, which a bare system may lack, and their licences into $1
# for the AppImage (assets/appimage/AppRun decides at start whether to use them).
set -eu
out=$1
# With docs: most of these packages carry their licence as a COPYING doc, which the image's
# nodocs drops; reinstall for the ones the image already has.
pkgs="libxkbcommon libxkbcommon-x11 libxcb libXcursor libXrender libXfixes libXrandr libXext libXi"
# shellcheck disable=SC2086 # a list of packages
dnf install -y -q $pkgs >/dev/null
# shellcheck disable=SC2086
dnf reinstall -y -q --setopt=tsflags= $pkgs >/dev/null
rm -rf "$out"
mkdir -p "$out/licenses"
for lib in libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb-xkb.so.1 libXcursor.so.1 \
    libXrender.so.1 libXfixes.so.3 libXrandr.so.2 libXext.so.6 libXi.so.6; do
    cp -L "/usr/lib64/$lib" "$out/"
    pkg=$(rpm -qf --qf '%{NAME}' "/usr/lib64/$lib")
    # libxkbcommon-x11 is covered by libxkbcommon's; el8's libxcb ships none, so it is kept in
    # assets/appimage/licenses (from the el8 source package).
    [ "$pkg" = libxkbcommon-x11 ] && pkg=libxkbcommon
    found=
    for licence in $(rpm -qL "$pkg"; rpm -qd "$pkg" | grep -E '/(COPYING|LICENSE)[^/]*$' || :; \
        ls assets/appimage/licenses/"$pkg"-* 2>/dev/null || :); do
        name=${licence##*/}
        cp "$licence" "$out/licenses/$pkg-${name#"$pkg"-}"
        found=1
    done
    [ -n "$found" ] || { echo "error: no licence file for $pkg ($lib)" >&2; exit 1; }
done
