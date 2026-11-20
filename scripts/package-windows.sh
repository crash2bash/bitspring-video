#!/bin/bash
# Put the Windows build and the libraries it needs into one folder.
#
#   scripts/package-windows.sh        (in an MSYS2 UCRT64 shell, after `cargo build --release`)
#
# The libraries are MSYS2's. Its FFmpeg includes GPL parts, so a folder made this way is for
# trying the player out, not yet for handing to other people.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="target/bundle/BitSpring-Video-windows"
rm -rf "$OUT"
mkdir -p "$OUT"
cp target/release/bitspring-video.exe "$OUT/"

# Every library the program loads that comes from MSYS2 rather than from Windows itself.
ntldd -R "$OUT/bitspring-video.exe" |
  awk '{ print $3 }' | grep -i -E 'ucrt64|mingw64|clang64' | sort -u |
  while read -r dll; do
    cp "$(cygpath -u "$dll")" "$OUT/"
  done

echo "$(find "$OUT" -name '*.dll' | wc -l) libraries, $(du -sh "$OUT" | cut -f1) in $OUT"
