#!/bin/bash
# Synthetic clips for checking the pipeline. Needs ffmpeg with libx264 and libx265.
#
#   hdr10-steps-4k.mp4   grey patches at known luminance, then a ramp underneath
#   hdr10-4k.mp4         moving colour pattern tagged HDR10 (values are not meaningful light levels)
#   hlg-4k50.mp4         the same, tagged HLG, 50 fps
#   sdr-1080p.mp4        BT.709 SDR
#   sync-1080p60.mp4     a white flash and a beep at every full second: they must coincide
#   hdr10-surround.mkv   HDR10 with a 5.1 E-AC3 track at 48 kHz
#   tracks-hdr10.mkv     HDR10 with two sound tracks and three subtitle tracks (text, text in
#                        Russian, styled ASS)
#
# The patches in hdr10-steps-4k.mp4, left to right, in cd/m²:
#   0  50  100  203  400  600  800  1000  1300  1600  2000  3000  4000  10000
# On a display that reaches N cd/m², with the player mapping the picture, every patch up to the
# file's 4000 stays distinguishable; in passthrough the display decides where it clips.
set -euo pipefail
cd "$(dirname "$0")"

MASTER='G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)'
HDR10="-color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc -color_range tv"
X265="log-level=error:hdr-opt=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc"

# Ten-bit limited-range PQ code values for the patches, as a nested ffmpeg expression.
STEPS=$(python3 - <<'EOF'
m1, m2 = 2610 / 16384, 2523 / 4096 * 128
c1, c2, c3 = 3424 / 4096, 2413 / 4096 * 32, 2392 / 4096 * 32
def code(nits):
    y = (nits / 10000) ** m1
    return round(64 + 876 * ((c1 + c2 * y) / (1 + c3 * y)) ** m2)
levels = [0, 50, 100, 203, 400, 600, 800, 1000, 1300, 1600, 2000, 3000, 4000, 10000]
n = len(levels)
expr = str(code(levels[-1]))
for i in range(n - 2, -1, -1):
    expr = f"if(lt(X,W*{i + 1}/{n}),{code(levels[i])},{expr})"
# Lower 40%: a ramp, linear in PQ code, from black to 4000 cd/m².
ramp = f"64+({code(4000)}-64)*X/W"
print(f"if(lt(Y,H*0.6),{expr},{ramp})")
EOF
)

ffmpeg -hide_banner -loglevel error -y -f lavfi -i "color=black:size=3840x2160:rate=24:duration=1" \
  -vf "trim=end_frame=1,format=yuv420p10le,geq=lum='${STEPS}':cb=512:cr=512,loop=loop=239:size=1:start=0,setpts=N/24/TB" \
  -c:v libx265 -preset fast -crf 12 -tag:v hvc1 $HDR10 \
  -x265-params "${X265}:master-display=${MASTER}L(40000000,50):max-cll=4000,1000" \
  hdr10-steps-4k.mp4

ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc2=size=3840x2160:rate=24000/1001:duration=8" \
  -vf "format=yuv420p10le" -c:v libx265 -preset ultrafast -crf 20 -tag:v hvc1 $HDR10 \
  -x265-params "${X265}:master-display=${MASTER}L(40000000,50):max-cll=4000,400" \
  hdr10-4k.mp4

ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc2=size=3840x2160:rate=50:duration=6" \
  -vf "format=yuv420p10le" -c:v libx265 -preset ultrafast -crf 20 -tag:v hvc1 \
  -color_primaries bt2020 -color_trc arib-std-b67 -colorspace bt2020nc \
  -x265-params "log-level=error:repeat-headers=1:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc" \
  hlg-4k50.mp4

ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc2=size=1920x1080:rate=25:duration=8" \
  -vf "format=yuv420p,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv" \
  -c:v libx264 -preset ultrafast -crf 18 \
  sdr-1080p.mp4

ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i "color=c=black:size=1920x1080:rate=60:duration=20" \
  -f lavfi -i "sine=frequency=440:beep_factor=4:sample_rate=48000:duration=20" \
  -vf "drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='lt(mod(t,1),0.05)',format=yuv420p,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv" \
  -c:v libx264 -preset ultrafast -crf 18 -c:a aac -b:a 192k -ac 2 \
  sync-1080p60.mp4

ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i "testsrc2=size=3840x2160:rate=24000/1001:duration=12" \
  -f lavfi -i "sine=frequency=330:beep_factor=3:sample_rate=48000:duration=12" \
  -vf "format=yuv420p10le" -c:v libx265 -preset ultrafast -crf 20 $HDR10 \
  -x265-params "${X265}:master-display=${MASTER}L(10000000,50):max-cll=1000,400" \
  -c:a eac3 -b:a 640k -ac 6 -metadata:s:a:0 language=eng \
  hdr10-surround.mkv

# Two sound tracks and three subtitle tracks, for trying the track menus and subtitle drawing.
SUBS=$(mktemp -d)
python3 - "$SUBS" <<'PY'
import sys
out = sys.argv[1]
def stamp(t):
    return "%02d:%02d:%02d,%03d" % (t // 3600, t // 60 % 60, t % 60, 0)
lines = {
    "en": ["A line of plain text.", "Two lines of text,\nthe second one longer than the first.",
           "<i>Italics for a voice off screen.</i>", "White here sits at SDR white, not at the display's peak."],
    "ru": ["Строка обычного текста.", "Две строки текста,\nвторая длиннее первой.",
           "<i>Курсив для голоса за кадром.</i>", "Белый здесь на уровне SDR-белого, а не пика дисплея."],
}
for lang, texts in lines.items():
    with open(f"{out}/{lang}.srt", "w") as f:
        for i in range(8):
            f.write(f"{i + 1}\n{stamp(1 + i * 3)} --> {stamp(3 + i * 3)}\n{texts[i % 4]}\n\n")
with open(f"{out}/styled.ass", "w") as f:
    f.write("""[Script Info]
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Georgia,64,&H0080E0FF,&H000000FF,&H00301000,&H80000000,-1,0,0,0,100,100,0,0,1,3,2,2,60,60,70,1
Style: Sign,Helvetica Neue,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,0,8,60,60,60,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:12.00,Default,,0,0,0,,Styled subtitles keep their own font and colour
Dialogue: 0,0:00:02.00,0:00:12.00,Sign,,0,0,0,,{\\an8}A sign at the top of the picture
Dialogue: 0,0:00:13.00,0:00:24.00,Default,,0,0,0,,{\\pos(480,300)}Placed by coordinates
""")
PY
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i "testsrc2=size=3840x2160:rate=24000/1001:duration=26" \
  -f lavfi -i "sine=frequency=330:beep_factor=3:sample_rate=48000:duration=26" \
  -f lavfi -i "sine=frequency=660:beep_factor=2:sample_rate=44100:duration=26" \
  -i "$SUBS/en.srt" -i "$SUBS/ru.srt" -i "$SUBS/styled.ass" \
  -map 0:v -map 1:a -map 2:a -map 3:s -map 4:s -map 5:s \
  -vf "format=yuv420p10le" -c:v libx265 -preset ultrafast -crf 24 $HDR10 \
  -x265-params "${X265}:master-display=${MASTER}L(10000000,50):max-cll=1000,400:keyint=48" \
  -c:a:0 eac3 -b:a:0 640k -ac:a:0 6 -c:a:1 aac -b:a:1 160k -ac:a:1 2 \
  -c:s:0 srt -c:s:1 srt -c:s:2 ass \
  -metadata:s:a:0 language=eng -metadata:s:a:0 title="Surround" \
  -metadata:s:a:1 language=rus -metadata:s:a:1 title="Стерео" \
  -metadata:s:s:0 language=eng -metadata:s:s:1 language=rus \
  -metadata:s:s:2 language=eng -metadata:s:s:2 title="Styled" \
  -disposition:s:0 0 -disposition:s:1 0 -disposition:s:2 0 \
  tracks-hdr10.mkv
rm -rf "$SUBS"

ls -lh ./*.mp4 ./*.mkv
