"""Read a capture made by capture.swift: when each flash was shown, when each beep sounded."""
import struct
import sys

prefix = sys.argv[1]
skip = float(sys.argv[2]) if len(sys.argv) > 2 else 0.0  # seconds of the capture to leave out

frames = [tuple(map(float, l.split())) for l in open(prefix + ".video.txt") if l.strip()]
# shown, stamped, brightness
use_shown = sum(1 for f in frames if f[0] > 0) > len(frames) / 2
times = [(f[0] if use_shown else f[1], f[2]) for f in frames]
peak = max(b for _, b in times) if times else 0
floor = min(b for _, b in times) if times else 0
level = floor + (peak - floor) * 0.5
flashes = []
dark = True
for t, b in times:
    if b >= level and dark:
        flashes.append(t)
    dark = b < level

index = [l.split() for l in open(prefix + ".audio.txt") if l.strip()]
raw = open(prefix + ".audio.raw", "rb").read()
samples = struct.unpack("<%df" % (len(raw) // 4), raw)
loudest = max((abs(s) for s in samples), default=0.0)
beeps = []
quiet_until = -1.0
threshold = loudest * 0.25
for stamped, count, rate, offset in index:
    stamped, count, rate, offset = float(stamped), int(count), float(rate), int(offset)
    for i in range(count):
        if abs(samples[offset + i]) > threshold:
            t = stamped + i / rate
            if t > quiet_until:
                beeps.append(t)
            quiet_until = t + 0.5

start = min(times[0][0] if times else 0, float(index[0][0]) if index else 0)
print(f"frames {len(frames)} (times from {'display' if use_shown else 'stamp'}), brightness {floor:.0f}..{peak:.0f}; "
      f"flashes {len(flashes)}; sound peak {loudest:.2e}; beeps {len(beeps)}")
offsets = []
for f in flashes:
    if f - start < skip or not beeps:
        continue
    b = min(beeps, key=lambda x: abs(x - f))
    if abs(b - f) < 0.45:
        offsets.append((f - start, (b - f) * 1000))
if offsets:
    values = sorted(o for _, o in offsets)
    median = values[len(values) // 2]
    print(f"beep against flash, ms (plus = sound after the picture): median {median:+.1f}, "
          f"from {values[0]:+.1f} to {values[-1]:+.1f}, pairs {len(values)}")
    print("   " + " ".join(f"{o:+.0f}" for _, o in offsets))
