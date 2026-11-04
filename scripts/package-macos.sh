#!/usr/bin/env bash
# Shared bundle assembly and ad-hoc signing for local install and release CI.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bin="${1:-target/release/fogu}"
app="${2:-target/macos/Fogu.app}"
version="$("$bin" --version)"
version="${version#fogu }"
[[ "$app" == */Fogu.app ]] || { echo 'Expected a Fogu.app destination' >&2; exit 1; }
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m755 "$bin" "$app/Contents/MacOS/fogu"
iconroot=$(mktemp -d)
trap 'rm -rf "$iconroot"' EXIT
mkdir "$iconroot/fogu.iconset"
for s in 16 32 128 256 512; do
    sips -z "$s" "$s" assets/fogu.png --out "$iconroot/fogu.iconset/icon_${s}x${s}.png" >/dev/null
    sips -z $((s * 2)) $((s * 2)) assets/fogu.png --out "$iconroot/fogu.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconroot/fogu.iconset" -o "$app/Contents/Resources/fogu.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Fogu</string>
<key>CFBundleIdentifier</key><string>io.github.revdep.fogu</string>
<key>CFBundleExecutable</key><string>fogu</string>
<key>CFBundleIconFile</key><string>fogu</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
cp LICENSE THIRD_PARTY_LICENSES "$app/Contents/Resources/"
[[ ! -f THIRD_PARTY_CRATES ]] || cp THIRD_PARTY_CRATES "$app/Contents/Resources/"
codesign -s - --force --deep "$app"
echo "Packaged $app ($version)"
