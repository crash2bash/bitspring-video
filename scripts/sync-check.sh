#!/bin/bash
# Measure how far the picture is from its sound, in milliseconds, on this Mac.
#
#   scripts/sync-check.sh [seconds] [player arguments...]
#
# Plays a clip that flashes white for one frame at the instant it beeps (HEVC 10-bit at 23.976
# frames, E-AC3 5.1 and AC-3 stereo, in Matroska: a film's formats), records the player's window
# together with the sound it makes, both stamped by the same clock, and prints the distance
# between each flash and its beep. The recording's sound stamps are the moments the device says
# the sound is heard, so the figures are the real distance, short of what the display and the
# sound hardware add on their own. A film at 23.976 on a 120 Hz display cannot be nearer than one
# refresh (8 ms) either way.
#
# With SOUND=beside the beeps are in a sound file next to the video ("Sound/sync.mka") and the
# video's own sound is silence, so what is measured is a sound file kept in step with the
# picture. With SOUND=alone the video has no sound of its own and the file is put on unasked.
#
# The player runs at a volume too low to hear. The terminal needs the Screen Recording
# permission. Build the app first: scripts/bundle-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

SECONDS_LONG="${1:-60}"
shift || true
APP="$PWD/target/bundle/BitSpring Video.app"
WORK="$PWD/target/sync-check"
mkdir -p "$WORK"

if [ ! -f "$WORK/sync.mkv" ]; then
  echo "Making the clip"
  BEEP="0.05*sin(2*PI*1000*t)*lt(mod(t\,1.001)\,0.03)"
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "color=c=black:s=3840x2160:r=24000/1001:d=90,drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='eq(mod(n\,24)\,0)',format=yuv420p10le" \
    -f lavfi -i "aevalsrc=exprs='$BEEP|$BEEP|$BEEP|0|0|0':c=5.1:s=48000:d=90" \
    -f lavfi -i "aevalsrc=exprs='$BEEP|$BEEP':c=stereo:s=48000:d=90" \
    -map 0:v -map 1:a -map 2:a \
    -c:v libx265 -preset ultrafast -x265-params "bframes=4:keyint=48:log-level=error" -tag:v hvc1 \
    -c:a:0 eac3 -b:a:0 640k -c:a:1 ac3 -b:a:1 192k \
    -metadata:s:a:0 language=eng -metadata:s:a:1 language=rus "$WORK/sync.mkv"
fi
CLIP="$WORK/sync.mkv"
CHOICE=()
if [ -n "${SOUND:-}" ]; then
  CLIP="$WORK/$SOUND/sync.mkv"
  if [ ! -f "$CLIP" ]; then
    echo "Making the clip with its sound beside it"
    mkdir -p "$WORK/$SOUND/Sound"
    ffmpeg -hide_banner -loglevel error -y -i "$WORK/sync.mkv" -map 0:a:1 -c copy "$WORK/$SOUND/Sound/sync.mka"
    if [ "$SOUND" = beside ]; then
      ffmpeg -hide_banner -loglevel error -y -i "$WORK/sync.mkv" -f lavfi -i "anullsrc=r=48000:cl=stereo" \
        -map 0:v -map 1:a -c:v copy -c:a ac3 -b:a 192k -shortest "$CLIP"
    else
      ffmpeg -hide_banner -loglevel error -y -i "$WORK/sync.mkv" -map 0:v -c copy "$CLIP"
    fi
  fi
  # The sound file is the first track counted from 2000; it is known a moment after the start.
  if [ "$SOUND" = beside ]; then CHOICE=(--script "2:audio 2000"); fi
fi
if [ ! -x "$WORK/capture" ] || [ scripts/sync-check/main.swift -nt "$WORK/capture" ]; then
  echo "Building the recorder"
  swiftc -O scripts/sync-check/main.swift -o "$WORK/capture" 2>/dev/null
fi

# Its own settings, so that nothing is picked up where an earlier run stopped.
printf '{"volume":0.03}' > "$WORK/settings.json"
rm -f "$WORK/history.json"
# The system will not start an application with its output on an external disk, where this
# folder may be: what the player prints goes to a temporary folder.
PRINTED="$(mktemp -d)"
open -n -g -a "$APP" --stdout "$PRINTED/player.out" --stderr "$PRINTED/player.err" "$CLIP" \
  --args --verbose --settings "$WORK/settings.json" ${CHOICE[@]+"${CHOICE[@]}"} "$@"
sleep 4
PID=$(pgrep -f "$APP/Contents/MacOS/bitspring-video" | head -1)
"$WORK/capture" "$PID" "$SECONDS_LONG" "$WORK/recording" || true
pkill -f "$APP/Contents/MacOS/bitspring-video" || true
python3 scripts/sync-check/analyse.py "$WORK/recording" 2
echo "What the player printed: $PRINTED"
