#!/usr/bin/env bash
# Installs Fogu for the current user: release binary, hicolor icons, .desktop entry.
#   scripts/install.sh              build + install (idempotent)
#   scripts/install.sh --uninstall  remove exactly what was installed
# The install root is $HOME (override HOME to install elsewhere, e.g. into a temp dir to test).
# Set FOGU_SKIP_BUILD=1 to reuse an existing target/release/fogu.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
sizes=(48 128 256 512)

# macOS: the binary only (no .desktop, no icon theme); BSD install has no -D, hence mkdir -p.
if [[ "$(uname)" == Darwin ]]; then
    if [[ "${1:-}" == "--uninstall" ]]; then
        rm -f "$bin_dir/fogu"
        echo "Fogu uninstalled from $bin_dir"
        exit 0
    fi
    [[ "${FOGU_SKIP_BUILD:-}" == 1 ]] || cargo build --release --locked -p fogu
    mkdir -p "$bin_dir"
    install -m 755 target/release/fogu "$bin_dir/fogu"
    echo "Installed Fogu to $bin_dir/fogu"
    [[ ":$PATH:" == *":$bin_dir:"* ]] || echo "$bin_dir is not on PATH: add it, or run $bin_dir/fogu"
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

[[ "${FOGU_SKIP_BUILD:-}" == 1 ]] || cargo build --release --locked -p fogu
install -Dm755 target/release/fogu "$bin_dir/fogu"
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
