#!/bin/bash
# Put what was built in MSYS2 for Windows, and the libraries it needs, into one folder:
# the player's core (bitspring_video_core.dll), which the Windows application loads, and
# bitspring-video.exe, the same player without the application around it (for `--displays`,
# `--info` and scripted runs).
#
#   scripts/package-windows.sh        (in an MSYS2 UCRT64 shell, after
#                                      `cargo build --release -p bitspring-video -p bsv-winhost`)
#
# The application itself (windows/BitSpringVideo) is built separately with `dotnet build`,
# and its output goes into the same folder: see .github/workflows/windows.yml.
#
# The libraries are MSYS2's. Its FFmpeg includes GPL parts, so a folder made this way is for
# trying the player out, not yet for handing to other people.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="target/bundle/BitSpring-Video-windows"
rm -rf "$OUT"
mkdir -p "$OUT"
cp target/release/bitspring-video.exe target/release/bitspring_video_core.dll "$OUT/"

# Every library the two load that comes from MSYS2 rather than from Windows itself.
ntldd -R "$OUT/bitspring-video.exe" "$OUT/bitspring_video_core.dll" |
  awk '{ print $3 }' | grep -i -E 'ucrt64|mingw64|clang64' | sort -u |
  while read -r dll; do
    cp "$(cygpath -u "$dll")" "$OUT/"
  done

echo "$(find "$OUT" -name '*.dll' | wc -l) libraries, $(du -sh "$OUT" | cut -f1) in $OUT"
