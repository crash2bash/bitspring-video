//! Sound output, and the clock the picture follows.
//!
//! The sound card is the timekeeper: it consumes samples at its own exact pace, so playback time
//! is "how many samples it has taken", refined with the instant each block reaches the speakers.
//! Video is then shown for that time, which keeps lips and voices together without ever
//! stretching or dropping sound.
//!
//! One thread feeds samples in ([`AudioOutput::write`]); the device's callback takes them out.
//! Between them is a lock-free ring, so the callback never waits for anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Samples kept ready for the device, in seconds.
const BUFFER_SECONDS: f64 = 1.0;
/// With no callback for this long the device is taken to have stopped.
const STALE_NS: u64 = 500_000_000;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// What the callback last did, published as one consistent snapshot.
#[derive(Clone, Copy, Default)]
struct Anchor {
    /// Frames consumed before the block the callback just filled.
    before: u64,
    /// Frames consumed including that block.
    total: u64,
    /// When that block's first frame reaches the speakers, in nanoseconds since `epoch`.
    heard_at: u64,
    /// Delay between the callback and the speakers.
    latency: u64,
    /// The block carried samples (or stood in for a track that has ended).
    running: bool,
}

/// State shared with the device callback. Everything here is safe to touch from a real-time thread.
struct Shared {
    epoch: Instant,
    rate: u32,

    playing: AtomicBool,
    flush: AtomicBool,
    ended: AtomicBool,
    /// Linear gain as `f32` bits.
    gain: AtomicU32,
    /// Media time of frame zero as `f64` bits; meaningful only while `has_base`.
    base: AtomicU64,
    has_base: AtomicBool,
    /// Seconds of media in each second of sound written since the base, as `f64` bits: more
    /// than 1 when the sound was made faster.
    speed: AtomicU64,
    /// Times the device asked for sound that had not arrived yet.
    underruns: AtomicU64,
    /// The device went away (unplugged, switched off): nothing more will be heard through it.
    lost: AtomicBool,

    // The anchor, guarded by a sequence counter: odd while the callback is writing.
    sequence: AtomicU64,
    before: AtomicU64,
    total: AtomicU64,
    heard_at: AtomicU64,
    latency: AtomicU64,
    running: AtomicBool,
}

impl Shared {
    fn publish(&self, a: Anchor) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.before.store(a.before, Ordering::Relaxed);
        self.total.store(a.total, Ordering::Relaxed);
        self.heard_at.store(a.heard_at, Ordering::Relaxed);
        self.latency.store(a.latency, Ordering::Relaxed);
        self.running.store(a.running, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::AcqRel);
    }

    fn anchor(&self) -> Anchor {
        loop {
            let start = self.sequence.load(Ordering::Acquire);
            if start % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let a = Anchor {
                before: self.before.load(Ordering::Relaxed),
                total: self.total.load(Ordering::Relaxed),
                heard_at: self.heard_at.load(Ordering::Relaxed),
                latency: self.latency.load(Ordering::Relaxed),
                running: self.running.load(Ordering::Relaxed),
            };
            if self.sequence.load(Ordering::Acquire) == start {
                return a;
            }
        }
    }
}

/// Reads playback time. Cheap, and usable from any thread.
#[derive(Clone)]
pub struct Clock {
    shared: Arc<Shared>,
}

impl Clock {
    /// Media time being heard right now, in seconds. `None` until sound for the current
    /// position has been queued (just after opening or seeking).
    pub fn position(&self) -> Option<f64> {
        let s = &self.shared;
        if !s.has_base.load(Ordering::Acquire) {
            return None;
        }
        let base = f64::from_bits(s.base.load(Ordering::Acquire));
        let speed = f64::from_bits(s.speed.load(Ordering::Acquire));
        let a = s.anchor();
        let rate = s.rate as f64;
        let now = s.epoch.elapsed().as_nanos() as u64;
        let handed_over = a.total as f64 / rate;

        let fresh = now.saturating_sub(a.heard_at.saturating_sub(a.latency)) < STALE_NS;
        if a.running && fresh {
            // Negative while the block is still on its way to the speakers.
            let since = (now as f64 - a.heard_at as f64) / 1e9;
            let heard = a.before as f64 / rate + since;
            Some(base + heard.min(handed_over) * speed)
        } else {
            Some(base + (handed_over - a.latency as f64 / 1e9) * speed)
        }
    }

    /// How many times the device ran out of sound while playing. Each one is an audible gap.
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }
}

/// The format the device was opened with.
#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
}

/// A sound output the system offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    /// Stays the same between runs and across plugging in again: what a choice is kept as.
    pub id: String,
    pub name: String,
}

fn name_of(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "output".into())
}

/// The outputs there are now, in the system's order, and the name of the one the system
/// itself sends sound to.
pub fn devices() -> (Vec<Device>, Option<String>) {
    let host = cpal::default_host();
    let all = host
        .output_devices()
        .map(|list| {
            list.filter_map(|d| {
                Some(Device {
                    id: d.id().ok()?.to_string(),
                    name: name_of(&d),
                })
            })
            .collect()
        })
        .unwrap_or_default();
    let system = host.default_output_device().map(|d| name_of(&d));
    (all, system)
}

pub struct AudioOutput {
    // Dropped first, which stops the callback.
    _stream: cpal::Stream,
    producer: rtrb::Producer<f32>,
    shared: Arc<Shared>,
    format: Format,
}

impl AudioOutput {
    /// Open the output `chosen` names by its [`Device::id`], or the one the system sends sound
    /// to when it is `None` or not connected now; at `preferred_rate` when it offers that rate.
    ///
    /// Asking for the track's own rate lets the device run at it, so nothing is resampled. The
    /// system's own output is followed when the system moves to another one; a chosen one is
    /// not, and reports [`lost`](Self::lost) when it goes away.
    pub fn open(preferred_rate: u32, chosen: Option<&str>) -> Result<AudioOutput, Error> {
        let err = |e: cpal::Error| Error(e.to_string());
        let host = cpal::default_host();
        let picked = chosen.and_then(|id| {
            let found = id.parse().ok().and_then(|id| host.device_by_id(&id));
            if found.is_none() {
                eprintln!("[warn] the chosen sound output is not connected: the system's plays");
            }
            found
        });
        let device = match picked {
            Some(device) => device,
            None => host
                .default_output_device()
                .ok_or(Error("no sound output device".into()))?,
        };
        let name = name_of(&device);
        let default = device.default_output_config().map_err(err)?;

        // Same channel count as the device's default; the track's rate if one of its modes has it.
        let config = device
            .supported_output_configs()
            .map_err(err)?
            .filter(|c| {
                c.channels() == default.channels() && c.sample_format() == cpal::SampleFormat::F32
            })
            .find_map(|c| c.try_with_sample_rate(preferred_rate))
            .unwrap_or(default);
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(Error(format!("{name} does not take 32-bit float samples")));
        }
        let config: cpal::StreamConfig = config.config();
        let channels = config.channels as usize;
        let rate = config.sample_rate;

        let shared = Arc::new(Shared {
            epoch: Instant::now(),
            rate,
            playing: AtomicBool::new(false),
            flush: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            gain: AtomicU32::new(1.0f32.to_bits()),
            base: AtomicU64::new(0),
            has_base: AtomicBool::new(false),
            speed: AtomicU64::new(1.0f64.to_bits()),
            underruns: AtomicU64::new(0),
            lost: AtomicBool::new(false),
            sequence: AtomicU64::new(0),
            before: AtomicU64::new(0),
            total: AtomicU64::new(0),
            heard_at: AtomicU64::new(0),
            latency: AtomicU64::new(0),
            running: AtomicBool::new(false),
        });

        let capacity = (rate as f64 * BUFFER_SECONDS) as usize * channels;
        let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(capacity);

        let state = shared.clone();
        let mut consumed = 0u64;
        let callback = move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
            let now = state.epoch.elapsed().as_nanos() as u64;
            let stamp = info.timestamp();
            let latency = stamp.playback.duration_since(stamp.callback).as_nanos() as u64;
            let frames = out.len() / channels;

            if state.flush.load(Ordering::Acquire) {
                let waiting = consumer.slots();
                if let Ok(chunk) = consumer.read_chunk(waiting) {
                    chunk.commit_all();
                }
                consumed = 0;
                state.flush.store(false, Ordering::Release);
            }

            let playing = state.playing.load(Ordering::Acquire);
            let mut got = 0;
            if playing {
                let want = consumer.slots().min(frames * channels) / channels * channels;
                if let Ok(chunk) = consumer.read_chunk(want) {
                    let gain = f32::from_bits(state.gain.load(Ordering::Relaxed));
                    let (first, second) = chunk.as_slices();
                    for (o, s) in out.iter_mut().zip(first.iter().chain(second)) {
                        *o = s * gain;
                    }
                    chunk.commit_all();
                    got = want / channels;
                }
            }
            out[got * channels..].fill(0.0);

            // Once the track is over, time keeps moving at the device's pace over the silence.
            let over = playing && got < frames && state.ended.load(Ordering::Acquire);
            if playing && got < frames && !over && state.has_base.load(Ordering::Acquire) {
                state.underruns.fetch_add(1, Ordering::Relaxed);
            }
            let advanced = if over { frames } else { got } as u64;
            state.publish(Anchor {
                before: consumed,
                total: consumed + advanced,
                heard_at: now + latency,
                latency,
                running: advanced > 0,
            });
            consumed += advanced;
        };

        let watch = shared.clone();
        let on_error = move |e: cpal::Error| {
            use cpal::ErrorKind;
            if matches!(
                e.kind(),
                ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated
            ) {
                watch.lost.store(true, Ordering::Release);
            }
            eprintln!("[warn] sound output: {e}");
        };
        let stream = device
            .build_output_stream(config, callback, on_error, None)
            .map_err(err)?;
        stream.play().map_err(err)?;

        let format = Format {
            device: name,
            sample_rate: rate,
            channels: channels as u16,
        };
        Ok(AudioOutput {
            _stream: stream,
            producer,
            shared,
            format,
        })
    }

    pub fn format(&self) -> &Format {
        &self.format
    }

    /// The device has gone away: sound has to be opened again elsewhere.
    pub fn lost(&self) -> bool {
        self.shared.lost.load(Ordering::Acquire)
    }

    pub fn clock(&self) -> Clock {
        Clock {
            shared: self.shared.clone(),
        }
    }

    pub fn controls(&self) -> Controls {
        Controls {
            shared: self.shared.clone(),
        }
    }

    /// Queue interleaved samples; returns how many were taken. Fewer than given means the
    /// buffer is full: try again shortly with the rest.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        let channels = self.format.channels as usize;
        let n = self.producer.slots().min(samples.len()) / channels * channels;
        if n == 0 {
            return 0;
        }
        match self.producer.write_chunk(n) {
            Ok(mut chunk) => {
                let (first, second) = chunk.as_mut_slices();
                let split = first.len();
                first.copy_from_slice(&samples[..split]);
                second.copy_from_slice(&samples[split..n]);
                chunk.commit_all();
                n
            }
            Err(_) => 0,
        }
    }

    /// Throw away everything queued and forget the time base: the step before sound from a
    /// new position is written. Waits for the device callback to do it.
    pub fn flush(&mut self) {
        let s = &self.shared;
        s.has_base.store(false, Ordering::Release);
        s.ended.store(false, Ordering::Release);
        s.flush.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_millis(300);
        while s.flush.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Declare the media time of the next sample written after a [`flush`](Self::flush), and
    /// how many seconds of media each second of the sound from there on holds (1 at normal
    /// speed, 1.5 for sound made half as fast again).
    pub fn set_base(&mut self, seconds: f64, speed: f64) {
        self.shared.speed.store(speed.to_bits(), Ordering::Release);
        self.shared.base.store(seconds.to_bits(), Ordering::Release);
        self.shared.has_base.store(true, Ordering::Release);
    }

    /// No more samples will come for this track.
    pub fn set_ended(&mut self) {
        self.shared.ended.store(true, Ordering::Release);
    }
}

impl Drop for AudioOutput {
    /// A clock left over from this output tells no time once it is gone.
    fn drop(&mut self) {
        self.shared.has_base.store(false, Ordering::Release);
    }
}

/// Start, stop and volume: the part of the output another thread may drive.
#[derive(Clone)]
pub struct Controls {
    shared: Arc<Shared>,
}

impl Controls {
    /// While not playing the device gets silence and time stands still.
    pub fn set_playing(&self, playing: bool) {
        self.shared.playing.store(playing, Ordering::Release);
    }

    /// Linear gain, 1.0 for unchanged.
    pub fn set_gain(&self, gain: f32) {
        self.shared
            .gain
            .store(gain.max(0.0).to_bits(), Ordering::Relaxed);
    }
}
