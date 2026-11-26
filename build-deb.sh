#!/bin/bash
set -e

# Get metadata from Cargo.toml (filter workspace for the applet crate)
META=$(cargo metadata --no-deps --format-version 1 | jq '.packages[] | select(.name == "cosmic-ext-connected")')
NAME=$(echo "$META" | jq -r '.name')
VERSION=$(echo "$META" | jq -r '.version')
DESC=$(echo "$META" | jq -r '.description')
ARCH=amd64
APPID=io.github.nwxnw.cosmic-ext-connected

# Build the project
cargo build --release

# Prepare directories
rm -rf deb_build
mkdir -p deb_build/DEBIAN \
         deb_build/usr/bin \
         deb_build/usr/share/applications \
         deb_build/usr/share/metainfo \
         deb_build/usr/share/icons/hicolor/scalable/apps

# Copy files
cp "${CARGO_TARGET_DIR:-target}/release/$NAME" deb_build/usr/bin/
chmod 755 deb_build/usr/bin/$NAME
cp data/$APPID.desktop deb_build/usr/share/applications/$APPID.desktop
cp data/$APPID.metainfo.xml deb_build/usr/share/metainfo/$APPID.metainfo.xml
cp data/icons/hicolor/scalable/apps/$APPID.svg deb_build/usr/share/icons/hicolor/scalable/apps/$APPID.svg
cp data/icons/hicolor/scalable/apps/$APPID-symbolic.svg deb_build/usr/share/icons/hicolor/scalable/apps/$APPID-symbolic.svg
cp data/icons/hicolor/scalable/apps/$APPID-disconnected-symbolic.svg deb_build/usr/share/icons/hicolor/scalable/apps/$APPID-disconnected-symbolic.svg
cp data/icons/hicolor/scalable/apps/$APPID-merged-symbolic.svg deb_build/usr/share/icons/hicolor/scalable/apps/$APPID-merged-symbolic.svg
cp data/icons/hicolor/scalable/apps/$APPID-split-symbolic.svg deb_build/usr/share/icons/hicolor/scalable/apps/$APPID-split-symbolic.svg

# Create control file
cat > deb_build/DEBIAN/control <<EOL
Package: $NAME
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: nwxnw <https://github.com/nwxnw>
Description: $DESC
EOL

# Build the .deb
DEB_NAME="${NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --build deb_build "$DEB_NAME"
echo "Created $DEB_NAME"
