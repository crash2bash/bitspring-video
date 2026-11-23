#!/bin/bash
# Build "BitSpring Video.app" into target/bundle, with everything it needs inside.
#
#   scripts/bundle-macos.sh
#
# The libraries come from Homebrew as installed on this machine. Homebrew's FFmpeg includes
# GPL parts (x264, x265), so an app built this way is for your own use; one to hand to other
# people needs an FFmpeg built with decoders only.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME="BitSpring Video"
IDENTIFIER="app.bitspring.video"
VERSION=$(cargo metadata --no-deps --format-version 1 |
  python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "bitspring-video"))')
APP="target/bundle/$NAME.app"
MOLTENVK="$(brew --prefix molten-vk)/lib/libMoltenVK.dylib"

echo "Building $NAME $VERSION"
cargo build --release -p bitspring-video

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/vulkan/icd.d" "$APP/Contents/Frameworks"
cp target/release/bitspring-video "$APP/Contents/MacOS/bitspring-video"

# The icons, at every size the system asks for: the app's own, and the one for its files.
make_icns() { # picture name
  local iconset
  iconset="$(mktemp -d)/$2.iconset"
  mkdir -p "$iconset"
  for size in 16 32 128 256 512; do
    sips -z $size $size "$1" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$1" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$APP/Contents/Resources/$2.icns"
  rm -rf "$(dirname "$iconset")"
}
make_icns assets/icon/icon.png AppIcon
make_icns assets/icon/document.png Document

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$IDENTIFIER</string>
  <key>CFBundleExecutable</key><string>bitspring-video</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- Shown by macOS the first time a video is opened from one of these places. -->
  <key>NSDocumentsFolderUsageDescription</key><string>To find subtitle files kept next to the video.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>To find subtitle files kept next to the video.</string>
  <key>NSDesktopFolderUsageDescription</key><string>To find subtitle files kept next to the video.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>To play videos from this disk and find their subtitle files.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>To play videos from this network disk and find their subtitle files.</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Video</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <!-- Without a picture of its own, a video file shows the system's camcorder page even
           where this player is the one chosen to open it. -->
      <key>CFBundleTypeIconFile</key><string>Document</string>
      <!-- Offered in "Open With"; never takes a file type away from the player already chosen. -->
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.movie</string>
        <string>public.video</string>
        <string>public.mpeg-4</string>
        <string>com.apple.quicktime-movie</string>
        <string>public.mpeg-2-transport-stream</string>
        <string>public.avchd-mpeg-2-transport-stream</string>
        <string>org.matroska.mkv</string>
        <string>org.webmproject.webm</string>
      </array>
    </dict>
  </array>
  <!-- Types macOS does not know by itself. -->
  <key>UTImportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>org.matroska.mkv</string>
      <key>UTTypeDescription</key><string>Matroska video</string>
      <key>UTTypeConformsTo</key><array><string>public.movie</string></array>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>mkv</string></array></dict>
    </dict>
    <dict>
      <key>UTTypeIdentifier</key><string>org.webmproject.webm</string>
      <key>UTTypeDescription</key><string>WebM video</string>
      <key>UTTypeConformsTo</key><array><string>public.movie</string></array>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>webm</string></array></dict>
    </dict>
  </array>
</dict>
</plist>
PLIST

# Libraries. MoltenVK is not linked: the Vulkan loader opens it at run time through the
# driver description written below, so it is named here by hand.
python3 scripts/gather-libraries.py "$APP" "$MOLTENVK"
cat > "$APP/Contents/Resources/vulkan/icd.d/MoltenVK_icd.json" <<JSON
{
    "file_format_version": "1.0.0",
    "ICD": {
        "library_path": "../../../Frameworks/libMoltenVK.dylib",
        "api_version": "1.4.0",
        "is_portability_driver": true
    }
}
JSON

# Changing a library's references breaks its signature, and Apple silicon runs nothing
# unsigned. An ad hoc signature (no certificate) is enough on the machine that built it.
find "$APP/Contents/Frameworks" -name '*.dylib' -exec codesign --force --sign - {} + 2>/dev/null
codesign --force --sign - "$APP"
codesign --verify --deep --strict "$APP"

du -sh "$APP" | awk '{print "Built: '"$APP"' (" $1 ")"}'
