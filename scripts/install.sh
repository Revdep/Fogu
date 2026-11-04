#!/usr/bin/env bash
# Installs Fogu for the current user: release binary, hicolor icons, .desktop entry (on macOS:
# ~/Applications/Fogu.app with its icon, and `fogu` in ~/.local/bin).
#   scripts/install.sh              build + install (idempotent)
#   scripts/install.sh --uninstall  remove exactly what was installed
# The install root is $HOME (override HOME to install elsewhere, e.g. into a temp dir to test).
# Set FOGU_SKIP_BUILD=1 to reuse an existing target/release/fogu (automatic in a release archive,
# which has no Cargo.toml).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
[[ "${PWD##*/}" != scripts ]] || cd ..

bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
sizes=(48 128 256 512)

# macOS: a minimal Fogu.app in ~/Applications (Dock icon, Finder and Launchpad launch) plus
# `fogu` on the command line. Unsigned: fine for a local build. BSD install has no -D.
if [[ "$(uname)" == Darwin ]]; then
    app="$HOME/Applications/Fogu.app"
    if [[ "${1:-}" == "--uninstall" ]]; then
        rm -rf "$app"
        rm -f "$bin_dir/fogu"
        echo "Fogu uninstalled ($app, $bin_dir/fogu)"
        exit 0
    fi
    [[ "${FOGU_SKIP_BUILD:-}" == 1 || ! -f Cargo.toml ]] || cargo build --release --locked -p fogu
    rm -rf "$app"
    mkdir -p "$bin_dir"
    scripts/package-macos.sh target/release/fogu "$app"
    # A wrapper, not a symlink: the full path makes macOS treat the process as the bundle.
    printf '#!/bin/sh\nexec "%s/Contents/MacOS/fogu" "$@"\n' "$app" > "$bin_dir/fogu"
    chmod 755 "$bin_dir/fogu"
    touch "$app"
    echo "Installed $app and $bin_dir/fogu"
    [[ ":$PATH:" == *":$bin_dir:"* ]] || echo "$bin_dir is not on PATH: add it, or open Fogu from Applications"
    exit 0
fi

refresh_caches() {
    command -v update-desktop-database >/dev/null && update-desktop-database "$data_dir/applications" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf "$data_dir/icons/hicolor" || true
}

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$bin_dir/fogu" "$data_dir/applications/fogu.desktop"
    for s in "${sizes[@]}"; do
        rm -f "$data_dir/icons/hicolor/${s}x${s}/apps/fogu.png"
    done
    refresh_caches
    echo "Fogu uninstalled from $HOME/.local"
    exit 0
fi

[[ "${FOGU_SKIP_BUILD:-}" == 1 || ! -f Cargo.toml ]] || cargo build --release --locked -p fogu
bin=target/release/fogu
[[ ! -f fogu ]] || bin=fogu
install -Dm755 "$bin" "$bin_dir/fogu"
for s in "${sizes[@]}"; do
    install -Dm644 "assets/icons/fogu-$s.png" "$data_dir/icons/hicolor/${s}x${s}/apps/fogu.png"
done
# Absolute Exec so the menu entry works even when ~/.local/bin is not on the session's PATH.
mkdir -p "$data_dir/applications"
sed "s|^Exec=.*|Exec=$bin_dir/fogu|" assets/fogu.desktop > "$data_dir/applications/fogu.desktop"
chmod 644 "$data_dir/applications/fogu.desktop"
refresh_caches
echo "Installed Fogu to $bin_dir/fogu (menu entry + icons under $data_dir)"
# Plasma's running panel keeps its icon lookups; a new icon shows only after it restarts.
if [[ "${XDG_CURRENT_DESKTOP:-}" == *KDE* ]]; then
    echo "KDE Plasma: if the menu/taskbar icon is generic, run: systemctl --user restart plasma-plasmashell"
fi
