#!/usr/bin/env bash
# Builds dist/Vessel-x86_64.AppImage: the Rust binary in an AppDir, with its own bsdtar for unpacking dropped
# archives (.zip/.rar/.7z/.tar). The binary only needs glibc: X11, Wayland, OpenGL and D-Bus are loaded from
# the desktop at run time, and HTTPS uses rustls with its own certificates.
# GitHub Actions runs this on every v* tag (.github/workflows/appimage.yml); locally it needs cargo, bsdtar and
# appimagetool (APPIMAGETOOL=/path/to/appimagetool, or on PATH).
#   ./build_appimage.sh            full build
#   ./build_appimage.sh --appdir   stop after the AppDir (no appimagetool needed)
set -euo pipefail
cd "$(dirname "$0")"

BUILD=build/appimage
APPDIR="$BUILD/Vessel.AppDir"
VERSION="${VESSEL_VERSION:-$(git describe --tags --always 2>/dev/null || echo dev)}"

cargo build --release --locked
BIN=target/release/vessel

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib/bsdtar" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps" "$APPDIR/usr/share/licenses/vessel"
cp "$BIN" "$APPDIR/usr/bin/vessel"
cp LICENSE "$APPDIR/usr/share/licenses/vessel/LICENSE"

# the icon, rendered from vessel.svg by Vessel itself
"$BIN" --render-icon 256 "$APPDIR/vessel.png"
cp "$APPDIR/vessel.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/vessel.png"
ln -sf vessel.png "$APPDIR/.DirIcon"

# bsdtar with the libraries it needs that a desktop may not have (not glibc's own)
BSDTAR="$(command -v bsdtar)"
cp "$BSDTAR" "$APPDIR/usr/lib/bsdtar/bsdtar"
ldd "$BSDTAR" | awk '/=> \//{print $3}' | while read -r so; do
    case "$(basename "$so")" in
        libc.so*|libm.so*|libpthread.so*|libdl.so*|librt.so*|ld-linux*) ;;
        *) cp -L "$so" "$APPDIR/usr/lib/bsdtar/" ;;
    esac
done
cat > "$APPDIR/usr/lib/bsdtar/run" <<'SH'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
LD_LIBRARY_PATH="$HERE${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" exec "$HERE/bsdtar" "$@"
SH
chmod +x "$APPDIR/usr/lib/bsdtar/run"

cat > "$APPDIR/vessel.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=Vessel
GenericName=Game launcher
Comment=Play games and run apps from outside Steam with Proton, Wine or natively
Exec=vessel %F
Icon=vessel
Terminal=false
Categories=Game;
Keywords=games;launcher;proton;wine;ge-proton;exe;
StartupWMClass=vessel
MimeType=application/x-ms-dos-executable;application/x-msdownload;application/x-msi;application/x-ms-shortcut;application/vnd.microsoft.portable-executable;application/x-dosexec;application/x-bat;application/x-msdos-program;
X-AppImage-Version=$VERSION
DESK
cp "$APPDIR/vessel.desktop" "$APPDIR/usr/share/applications/vessel.desktop"
cat > "$APPDIR/AppRun" <<'SH'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
# the bundled bsdtar, for unpacking dropped archives (kept off PATH so games don't see it)
VESSEL_BSDTAR="$HERE/usr/lib/bsdtar/run" exec "$HERE/usr/bin/vessel" "$@"
SH
chmod +x "$APPDIR/AppRun"

[ "${1:-}" = "--appdir" ] && { echo "AppDir ready: $APPDIR"; exit 0; }
TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
[ -n "$TOOL" ] || { echo "appimagetool not found (set APPIMAGETOOL)" >&2; exit 1; }
mkdir -p dist
ARCH=x86_64 VERSION="$VERSION" "$TOOL" --no-appstream "$APPDIR" dist/Vessel-x86_64.AppImage
echo "built dist/Vessel-x86_64.AppImage ($VERSION)"
