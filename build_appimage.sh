#!/usr/bin/env bash
# Builds dist/Vessel-x86_64.AppImage: a PyInstaller bundle (Python + PyQt6 + certifi) in an AppDir, with
# its own bsdtar for unpacking dropped archives (.zip/.rar/.7z/.tar).
# GitHub Actions runs this on every v* tag (.github/workflows/appimage.yml); locally it needs
# python3, bsdtar and appimagetool (APPIMAGETOOL=/path/to/appimagetool, or on PATH).
#   ./build_appimage.sh            full build
#   ./build_appimage.sh --appdir   stop after the AppDir (no appimagetool needed)
set -euo pipefail
cd "$(dirname "$0")"

BUILD=build/appimage
APPDIR="$BUILD/Vessel.AppDir"
VENV="$BUILD/venv"
VERSION="${VESSEL_VERSION:-$(git describe --tags --always 2>/dev/null || echo dev)}"
PYTHON="${PYTHON:-python3}"

[ -x "$VENV/bin/python" ] || "$PYTHON" -m venv "$VENV"
"$VENV/bin/python" -m pip install -q --upgrade pip
"$VENV/bin/python" -m pip install -q pyinstaller PyQt6 certifi

# the icon, rendered from the SVG
QT_QPA_PLATFORM=offscreen "$VENV/bin/python" - vessel.svg "$BUILD/vessel.png" <<'PY'
import sys
from PyQt6.QtCore import Qt
from PyQt6.QtGui import QGuiApplication, QImage, QPainter
from PyQt6.QtSvg import QSvgRenderer
app = QGuiApplication(sys.argv)
img = QImage(256, 256, QImage.Format.Format_ARGB32)
img.fill(Qt.GlobalColor.transparent)
p = QPainter(img)
QSvgRenderer(sys.argv[1]).render(p)
p.end()
img.save(sys.argv[2])
PY

mkdir -p "$BUILD"
"$VENV/bin/pyinstaller" --noconfirm --clean --onedir --name vessel \
    --distpath "$BUILD/dist" --workpath "$BUILD/work" --specpath "$BUILD" \
    --add-data "$PWD/vessel.svg:." \
    --hidden-import certifi --collect-data certifi \
    --exclude-module tkinter \
    --exclude-module PyQt6.QtQml --exclude-module PyQt6.QtQuick --exclude-module PyQt6.QtPdf \
    --exclude-module PyQt6.QtMultimedia --exclude-module PyQt6.QtWebEngineCore \
    --exclude-module PyQt6.QtDesigner --exclude-module PyQt6.QtNetwork \
    vessel.py

INTERNAL="$BUILD/dist/vessel/_internal"
rm -rf "$INTERNAL/PyQt6/Qt6/translations"
rm -f "$INTERNAL/PyQt6/Qt6/plugins/platformthemes/libqgtk3.so"
# libraries AppImages take from the host (the AppImage excludelist)
for lib in libfontconfig.so.1 libfreetype.so.6 libexpat.so.1 libz.so.1 libuuid.so.1 \
           libX11.so.6 libX11-xcb.so.1 libxcb.so.1 libXau.so.6 libXdmcp.so.6 libXext.so.6 \
           libXrender.so.1 libstdc++.so.6 libgcc_s.so.1 libGL.so.1 libGLX.so.0 libGLdispatch.so.0 \
           libEGL.so.1 libgbm.so.1 libdrm.so.2 libasound.so.2; do
    rm -f "$INTERNAL/$lib"
done

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp -r "$BUILD/dist/vessel" "$APPDIR/usr/lib/vessel"
cp LICENSE "$APPDIR/usr/lib/vessel/LICENSE"

# bsdtar with the libraries it needs that a desktop may not have (not glibc's own)
BSDTAR="$(command -v bsdtar)"
mkdir -p "$APPDIR/usr/lib/bsdtar" "$APPDIR/usr/bin"
cp "$BSDTAR" "$APPDIR/usr/lib/bsdtar/bsdtar"
ldd "$BSDTAR" | awk '/=> \//{print $3}' | while read -r so; do
    case "$(basename "$so")" in
        libc.so*|libm.so*|libpthread.so*|libdl.so*|librt.so*|ld-linux*) ;;
        *) cp -L "$so" "$APPDIR/usr/lib/bsdtar/" ;;
    esac
done
cat > "$APPDIR/usr/bin/bsdtar" <<'SH'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")/../lib/bsdtar"
LD_LIBRARY_PATH="$HERE${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" exec "$HERE/bsdtar" "$@"
SH
chmod +x "$APPDIR/usr/bin/bsdtar"

cp "$BUILD/vessel.png" "$APPDIR/vessel.png"
cp "$BUILD/vessel.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/vessel.png"
ln -sf vessel.png "$APPDIR/.DirIcon"
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
VESSEL_BSDTAR="$HERE/usr/bin/bsdtar" exec "$HERE/usr/lib/vessel/vessel" "$@"
SH
chmod +x "$APPDIR/AppRun"

[ "${1:-}" = "--appdir" ] && { echo "AppDir ready: $APPDIR"; exit 0; }
TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
[ -n "$TOOL" ] || { echo "appimagetool not found (set APPIMAGETOOL)" >&2; exit 1; }
mkdir -p dist
ARCH=x86_64 VERSION="$VERSION" "$TOOL" --no-appstream "$APPDIR" dist/Vessel-x86_64.AppImage
echo "built dist/Vessel-x86_64.AppImage ($VERSION)"
