//! The video engine: FFmpeg decodes, libplacebo renders, this crate keeps the two apart.
//!
//! A [`Source`] reads the file, a [`VideoDecoder`] and an [`AudioDecoder`] turn its packets into
//! pictures and sound, and a [`Gpu`] shows the pictures. Each belongs to one thread; [`Packet`]
//! and [`Frame`] values travel between them over whatever channel the caller likes. The engine
//! itself holds no locks.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::NonNull;

use bsv_display::policy::{self, Output, Primaries, Signal, SourceTransfer};

mod ffi;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn text(buf: &[c_char]) -> String {
    // SAFETY: the C side always NUL-terminates these fixed buffers.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

// ---- Logging --------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error = 1,
    Warning = 2,
    Info = 3,
    Debug = 4,
}

extern "C" fn log_to_stderr(_user: *mut c_void, level: c_int, message: *const c_char) {
    // SAFETY: the C side passes a NUL-terminated string valid for this call.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    let tag = match level {
        1 => "error",
        2 => "warn",
        3 => "info",
        _ => "debug",
    };
    eprintln!("[{tag}] {}", message.trim_end());
}

static LOG_FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

extern "C" fn log_to_file(_user: *mut c_void, level: c_int, message: *const c_char) {
    use std::io::Write;
    // SAFETY: the C side passes a NUL-terminated string valid for this call.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    let tag = match level {
        1 => "error",
        2 => "warn",
        3 => "info",
        _ => "debug",
    };
    if let Ok(mut file) = LOG_FILE.lock()
        && let Some(file) = file.as_mut()
    {
        let _ = writeln!(file, "[{tag}] {}", message.trim_end());
    }
}

/// Send FFmpeg's and libplacebo's messages to a file, up to `level`: for a program with no
/// console to print to. The file is started afresh.
pub fn log_to_file_up_to(path: &std::path::Path, level: LogLevel) -> std::io::Result<()> {
    *LOG_FILE.lock().unwrap() = Some(std::fs::File::create(path)?);
    // SAFETY: the callback is a plain function that lives for the whole program.
    unsafe { ffi::bsv_set_log(Some(log_to_file), std::ptr::null_mut(), level as c_int) };
    Ok(())
}

/// Add a line of the caller's own to the log file, if there is one.
pub fn log_line(text: &str) {
    use std::io::Write;
    if let Ok(mut file) = LOG_FILE.lock()
        && let Some(file) = file.as_mut()
    {
        let _ = writeln!(file, "{text}");
    }
}

/// Send FFmpeg's and libplacebo's messages to stderr, up to `level`.
pub fn log_to_stderr_up_to(level: LogLevel) {
    // SAFETY: the callback is a plain function that lives for the whole program.
    unsafe { ffi::bsv_set_log(Some(log_to_stderr), std::ptr::null_mut(), level as c_int) };
}

// ---- Media ----------------------------------------------------------------------------------

/// Dolby Vision as declared by the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DolbyVision {
    pub profile: u8,
    pub level: u8,
    /// 1: HDR10-compatible base layer, 2: SDR, 4: HLG, 0: none (profile 5).
    pub base_layer_compatibility: u8,
    /// A second video layer exists (profile 7). Only the base layer and the RPU are used.
    pub enhancement_layer: bool,
}

impl DolbyVision {
    /// What the base layer is when shown without the RPU: "HDR10", "HLG" or "SDR". `None`
    /// when it cannot stand alone (profile 5, whose base layer is in Dolby's own colours).
    pub fn base_layer(&self) -> Option<&'static str> {
        match self.base_layer_compatibility {
            1 | 6 => Some("HDR10"),
            2 => Some("SDR"),
            4 => Some("HLG"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u32,
    pub fps: f64,
    /// Seconds, 0 when the container does not say.
    pub duration: f64,
    pub codec: String,
    pub pixel_format: String,
    /// The platform's video decoder is in use.
    pub hardware: bool,
    pub primaries: Primaries,
    pub transfer: SourceTransfer,
    /// HDR10 mastering display range, cd/m².
    pub mastering_nits: Option<(f32, f32)>,
    pub max_cll: Option<f32>,
    pub max_fall: Option<f32>,
    pub dolby_vision: Option<DolbyVision>,
}

impl MediaInfo {
    fn from_c(c: &ffi::bsv_media_info) -> MediaInfo {
        let dolby_vision = (c.dv_profile > 0).then_some(DolbyVision {
            profile: c.dv_profile as u8,
            level: c.dv_level as u8,
            base_layer_compatibility: c.dv_bl_compatibility as u8,
            enhancement_layer: c.dv_el_present != 0,
        });
        // FFmpeg's AVColorPrimaries / AVColorTransferCharacteristic, which follow ITU-T H.273.
        let mut primaries = match c.primaries {
            9 => Primaries::Bt2020,
            11 | 12 => Primaries::DisplayP3,
            _ => Primaries::Bt709,
        };
        let mut transfer = match c.transfer {
            16 => SourceTransfer::Pq,
            18 => SourceTransfer::Hlg,
            _ => SourceTransfer::Sdr,
        };
        if dolby_vision.is_some() {
            // Whatever the base layer is tagged as, applying the RPU yields BT.2020 PQ.
            primaries = Primaries::Bt2020;
            transfer = SourceTransfer::Pq;
        }
        MediaInfo {
            width: c.width.max(0) as u32,
            height: c.height.max(0) as u32,
            bit_depth: c.bit_depth.max(0) as u32,
            fps: c.fps,
            duration: c.duration,
            codec: text(&c.codec),
            pixel_format: text(&c.pixel_format),
            hardware: c.hardware != 0,
            primaries,
            transfer,
            mastering_nits: (c.mastering_max > 0.0).then_some((c.mastering_min, c.mastering_max)),
            max_cll: (c.max_cll > 0.0).then_some(c.max_cll),
            max_fall: (c.max_fall > 0.0).then_some(c.max_fall),
            dolby_vision,
        }
    }

    /// The video as the output policy sees it.
    pub fn source(&self) -> policy::Source {
        policy::Source {
            primaries: self.primaries,
            transfer: self.transfer,
            peak_nits: self.max_cll.or(self.mastering_nits.map(|(_, max)| max)),
            dolby_vision: self.dolby_vision.is_some(),
        }
    }
}

/// One decoded picture, in system memory, with all its metadata.
pub struct Frame {
    ptr: NonNull<c_void>,
    /// Seconds from the start of the file.
    pub pts: f64,
    pub duration: f64,
}

// SAFETY: an AVFrame is plain reference-counted data with no thread affinity.
unsafe impl Send for Frame {}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: we own the AVFrame until it is handed to a Gpu, which forgets the Frame.
        unsafe { ffi::bsv_frame_free(self.ptr.as_ptr()) };
    }
}

/// The sound track as declared by the file.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioInfo {
    pub codec: String,
    /// "DTS-HD MA", "Dolby TrueHD + Dolby Atmos"; empty for a plain codec.
    pub profile: String,
    /// "stereo", "5.1(side)".
    pub layout: String,
    pub language: String,
    pub sample_rate: u32,
    pub channels: u32,
}

/// A compressed packet on its way from the file to a decoder.
pub struct Packet {
    ptr: NonNull<c_void>,
    /// Seconds of content inside.
    pub duration: f64,
}

// SAFETY: an AVPacket is plain reference-counted data with no thread affinity.
unsafe impl Send for Packet {}

impl Packet {
    fn into_raw(self) -> *mut c_void {
        let ptr = self.ptr.as_ptr();
        std::mem::forget(self);
        ptr
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: we own the AVPacket until it is handed to a decoder.
        unsafe { ffi::bsv_packet_free(self.ptr.as_ptr()) };
    }
}

/// What [`Source::read`] found next in the file.
pub enum Read {
    Video(Packet),
    Audio(Packet),
    Subtitle(Packet),
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Subtitle,
}

/// A sound or subtitle stream the user can choose.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The stream's number in the file: what the select methods take.
    pub index: i32,
    pub kind: TrackKind,
    pub codec: String,
    pub profile: String,
    pub language: String,
    pub title: String,
    /// Sound only: "stereo", "5.1(side)".
    pub layout: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub is_default: bool,
    pub forced: bool,
    pub hearing_impaired: bool,
    /// Subtitles made of pictures (Blu-ray, DVD) rather than text.
    pub bitmap: bool,
}

/// The open file: hands out packets in the order they are stored.
pub struct Source {
    ptr: NonNull<ffi::bsv_source>,
}

// SAFETY: the source is only ever used from the one thread that owns it.
unsafe impl Send for Source {}

impl Source {
    pub fn open(path: &str) -> Result<Source, Error> {
        let c_path =
            CString::new(path).map_err(|_| Error("the path contains a NUL byte".into()))?;
        let mut error = [0 as c_char; 256];
        // SAFETY: both buffers outlive the call.
        let ptr = unsafe {
            ffi::bsv_source_open(c_path.as_ptr(), error.as_mut_ptr(), error.len() as c_int)
        };
        NonNull::new(ptr)
            .map(|ptr| Source { ptr })
            .ok_or_else(|| Error(text(&error)))
    }

    /// Open a subtitle file kept next to the video. Only its subtitles are read.
    pub fn open_sidecar(path: &str) -> Result<Source, Error> {
        let c_path =
            CString::new(path).map_err(|_| Error("the path contains a NUL byte".into()))?;
        let mut error = [0 as c_char; 256];
        // SAFETY: both buffers outlive the call.
        let ptr = unsafe {
            ffi::bsv_source_open_sidecar(c_path.as_ptr(), error.as_mut_ptr(), error.len() as c_int)
        };
        NonNull::new(ptr)
            .map(|ptr| Source { ptr })
            .ok_or_else(|| Error(text(&error)))
    }

    pub fn has_video(&self) -> bool {
        // SAFETY: plain call on an owned source.
        unsafe { ffi::bsv_source_has_video(self.ptr.as_ptr()) != 0 }
    }

    /// The sound track that will be played, if the file has one this build can decode.
    pub fn audio_info(&self) -> Option<AudioInfo> {
        let mut c = ffi::bsv_audio_info::default();
        // SAFETY: `c` is a valid out-pointer.
        if unsafe { ffi::bsv_source_audio_info(self.ptr.as_ptr(), &mut c) } == 0 {
            return None;
        }
        Some(AudioInfo {
            codec: text(&c.codec),
            profile: text(&c.profile),
            layout: text(&c.layout),
            language: text(&c.language),
            sample_rate: c.sample_rate.max(0) as u32,
            channels: c.channels.max(0) as u32,
        })
    }

    /// Every sound and subtitle track this build can decode, in file order.
    pub fn tracks(&self) -> Vec<Track> {
        // SAFETY: plain calls on an owned source; `c` is a valid out-pointer.
        let count = unsafe { ffi::bsv_source_track_count(self.ptr.as_ptr()) };
        (0..count)
            .filter_map(|n| {
                let mut c: ffi::bsv_track = unsafe { std::mem::zeroed() };
                if unsafe { ffi::bsv_source_track(self.ptr.as_ptr(), n, &mut c) } == 0 {
                    return None;
                }
                Some(Track {
                    index: c.index,
                    kind: if c.kind == 2 {
                        TrackKind::Audio
                    } else {
                        TrackKind::Subtitle
                    },
                    codec: text(&c.codec),
                    profile: text(&c.profile),
                    language: text(&c.language),
                    title: text(&c.title),
                    layout: text(&c.layout),
                    channels: c.channels.max(0) as u32,
                    sample_rate: c.sample_rate.max(0) as u32,
                    is_default: c.is_default != 0,
                    forced: c.forced != 0,
                    hearing_impaired: c.hearing_impaired != 0,
                    bitmap: c.bitmap != 0,
                })
            })
            .collect()
    }

    /// The sound track being read, if any.
    pub fn audio_index(&self) -> Option<i32> {
        // SAFETY: plain call on an owned source.
        let index = unsafe { ffi::bsv_source_audio_index(self.ptr.as_ptr()) };
        (index >= 0).then_some(index)
    }

    /// The subtitle track being read, if any.
    pub fn subtitle_index(&self) -> Option<i32> {
        // SAFETY: plain call on an owned source.
        let index = unsafe { ffi::bsv_source_subtitle_index(self.ptr.as_ptr()) };
        (index >= 0).then_some(index)
    }

    /// Read another sound track from now on. The sound decoder must be opened again.
    pub fn select_audio(&mut self, index: i32) -> Result<(), Error> {
        // SAFETY: plain call on an owned source.
        match unsafe { ffi::bsv_source_select_audio(self.ptr.as_ptr(), index) } {
            0 => Ok(()),
            _ => Err(Error(format!("stream {index} is not a sound track"))),
        }
    }

    /// Read this subtitle track from now on, or none.
    pub fn select_subtitle(&mut self, index: Option<i32>) -> Result<(), Error> {
        // SAFETY: plain call on an owned source.
        match unsafe { ffi::bsv_source_select_subtitle(self.ptr.as_ptr(), index.unwrap_or(-1)) } {
            0 => Ok(()),
            _ => Err(Error("that stream is not a subtitle track".into())),
        }
    }

    /// Fonts shipped inside the file, as (file name, contents).
    pub fn fonts(&self) -> Vec<(String, Vec<u8>)> {
        let mut fonts = Vec::new();
        loop {
            let mut name: *const c_char = std::ptr::null();
            let mut data: *const c_void = std::ptr::null();
            let mut len: c_int = 0;
            // SAFETY: all three are valid out-pointers; what they receive stays valid while
            // the source is open, and is copied before returning.
            let found = unsafe {
                ffi::bsv_source_font(
                    self.ptr.as_ptr(),
                    fonts.len() as c_int,
                    &mut name,
                    &mut data,
                    &mut len,
                )
            };
            if found == 0 || data.is_null() || len <= 0 {
                return fonts;
            }
            let name = unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned();
            let bytes =
                unsafe { std::slice::from_raw_parts(data as *const u8, len as usize) }.to_vec();
            fonts.push((name, bytes));
        }
    }

    pub fn read(&mut self) -> Result<Read, Error> {
        let mut out = ffi::bsv_packet {
            avpacket: std::ptr::null_mut(),
            kind: 0,
            duration: 0.0,
        };
        // SAFETY: `out` is a valid out-pointer.
        match unsafe { ffi::bsv_source_read(self.ptr.as_ptr(), &mut out) } {
            1 => {
                let ptr = NonNull::new(out.avpacket)
                    .ok_or_else(|| Error("the file returned no packet".into()))?;
                let packet = Packet {
                    ptr,
                    duration: out.duration,
                };
                Ok(match out.kind {
                    1 => Read::Video(packet),
                    2 => Read::Audio(packet),
                    _ => Read::Subtitle(packet),
                })
            }
            0 => Ok(Read::End),
            code => Err(Error(format!(
                "reading the file failed (FFmpeg error {code})"
            ))),
        }
    }

    /// Jump to the keyframe at or before `seconds`. Both decoders must be flushed afterwards.
    pub fn seek(&mut self, seconds: f64) -> Result<(), Error> {
        // SAFETY: plain call on an owned source.
        match unsafe { ffi::bsv_source_seek(self.ptr.as_ptr(), seconds) } {
            code if code >= 0 => Ok(()),
            code => Err(Error(format!("seeking failed (FFmpeg error {code})"))),
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: we own the source.
        unsafe { ffi::bsv_source_close(self.ptr.as_ptr()) };
    }
}

pub struct VideoDecoder {
    ptr: NonNull<ffi::bsv_video>,
}

// SAFETY: the decoder is only ever used from the one thread that owns it.
unsafe impl Send for VideoDecoder {}

impl VideoDecoder {
    /// A decoder for the source's video. It keeps no reference to the source.
    pub fn open(source: &Source, hardware: bool) -> Result<VideoDecoder, Error> {
        let mut error = [0 as c_char; 256];
        // SAFETY: the source is alive for the call; the error buffer outlives it.
        let ptr = unsafe {
            ffi::bsv_video_open(
                source.ptr.as_ptr(),
                hardware as c_int,
                error.as_mut_ptr(),
                error.len() as c_int,
            )
        };
        NonNull::new(ptr)
            .map(|ptr| VideoDecoder { ptr })
            .ok_or_else(|| Error(text(&error)))
    }

    /// Stream properties. HDR metadata and `hardware` settle once the first frame is decoded.
    pub fn info(&self) -> MediaInfo {
        let mut c = ffi::bsv_media_info::default();
        // SAFETY: `c` is a valid out-pointer.
        unsafe { ffi::bsv_video_info(self.ptr.as_ptr(), &mut c) };
        MediaInfo::from_c(&c)
    }

    /// Feed one packet, or `None` at the end of the file; then call [`receive`](Self::receive)
    /// until it stops returning frames.
    pub fn send(&mut self, packet: Option<Packet>) -> Result<(), Error> {
        let raw = packet.map_or(std::ptr::null_mut(), Packet::into_raw);
        // SAFETY: ownership of the packet moves to the decoder.
        match unsafe { ffi::bsv_video_send(self.ptr.as_ptr(), raw) } {
            code if code >= 0 => Ok(()),
            code => Err(Error(format!(
                "video decoding failed (FFmpeg error {code})"
            ))),
        }
    }

    /// The next frame in presentation order, if one is ready.
    pub fn receive(&mut self) -> Result<Decoded<Frame>, Error> {
        let mut out = ffi::bsv_frame::default();
        // SAFETY: `out` is a valid out-pointer.
        match unsafe { ffi::bsv_video_receive(self.ptr.as_ptr(), &mut out) } {
            1 => {
                let ptr = NonNull::new(out.avframe)
                    .ok_or_else(|| Error("decoder returned no frame".into()))?;
                Ok(Decoded::Some(Frame {
                    ptr,
                    pts: out.pts,
                    duration: out.duration,
                }))
            }
            0 => Ok(Decoded::NeedsPacket),
            2 => Ok(Decoded::Finished),
            code => Err(Error(format!(
                "video decoding failed (FFmpeg error {code})"
            ))),
        }
    }

    /// After a seek to `seconds`: forget everything in flight.
    pub fn flush(&mut self, seconds: f64) {
        // SAFETY: plain call on an owned decoder.
        unsafe { ffi::bsv_video_flush(self.ptr.as_ptr(), seconds) };
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        // SAFETY: we own the decoder.
        unsafe { ffi::bsv_video_close(self.ptr.as_ptr()) };
    }
}

/// What a decoder has after being fed.
pub enum Decoded<T> {
    Some(T),
    /// Nothing more until another packet is sent.
    NeedsPacket,
    /// The end of the file was sent and everything has come out.
    Finished,
}

/// A run of decoded sound, borrowed from the decoder until its next call.
pub struct Samples<'a> {
    /// Interleaved 32-bit float, in the rate and channel count the decoder was opened with.
    pub data: &'a [f32],
    /// Media time of the first sample, in seconds.
    pub pts: f64,
}

pub struct AudioDecoder {
    ptr: NonNull<ffi::bsv_audio>,
    channels: usize,
}

// SAFETY: the decoder is only ever used from the one thread that owns it.
unsafe impl Send for AudioDecoder {}

impl AudioDecoder {
    /// A decoder for the source's sound, converting to `rate` Hz and `channels` channels.
    pub fn open(source: &Source, rate: u32, channels: u16) -> Result<AudioDecoder, Error> {
        let mut error = [0 as c_char; 256];
        // SAFETY: the source is alive for the call; the error buffer outlives it.
        let ptr = unsafe {
            ffi::bsv_audio_open(
                source.ptr.as_ptr(),
                rate as c_int,
                channels as c_int,
                error.as_mut_ptr(),
                error.len() as c_int,
            )
        };
        NonNull::new(ptr)
            .map(|ptr| AudioDecoder {
                ptr,
                channels: channels as usize,
            })
            .ok_or_else(|| Error(text(&error)))
    }

    pub fn send(&mut self, packet: Option<Packet>) -> Result<(), Error> {
        let raw = packet.map_or(std::ptr::null_mut(), Packet::into_raw);
        // SAFETY: ownership of the packet moves to the decoder.
        match unsafe { ffi::bsv_audio_send(self.ptr.as_ptr(), raw) } {
            code if code >= 0 => Ok(()),
            code => Err(Error(format!(
                "sound decoding failed (FFmpeg error {code})"
            ))),
        }
    }

    pub fn receive(&mut self) -> Result<Decoded<Samples<'_>>, Error> {
        let mut samples: *const f32 = std::ptr::null();
        let mut frames: c_int = 0;
        let mut pts = 0.0;
        // SAFETY: all three are valid out-pointers.
        match unsafe {
            ffi::bsv_audio_receive(self.ptr.as_ptr(), &mut samples, &mut frames, &mut pts)
        } {
            1 => {
                let len = frames.max(0) as usize * self.channels;
                // SAFETY: the decoder owns `len` samples at this address until it is called
                // again, which the returned borrow of `self` prevents.
                let data = if len == 0 {
                    &[][..]
                } else {
                    unsafe { std::slice::from_raw_parts(samples, len) }
                };
                Ok(Decoded::Some(Samples { data, pts }))
            }
            0 => Ok(Decoded::NeedsPacket),
            2 => Ok(Decoded::Finished),
            code => Err(Error(format!(
                "sound decoding failed (FFmpeg error {code})"
            ))),
        }
    }

    /// After a seek: forget everything in flight.
    pub fn flush(&mut self) {
        // SAFETY: plain call on an owned decoder.
        unsafe { ffi::bsv_audio_flush(self.ptr.as_ptr()) };
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        // SAFETY: we own the decoder.
        unsafe { ffi::bsv_audio_close(self.ptr.as_ptr()) };
    }
}

// ---- Subtitles ------------------------------------------------------------------------------

/// What a subtitle track is made of, which decides how it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum SubtitleKind {
    /// Text. `header` is the stream's own ASS styling; `None` for plain text formats, which
    /// get the renderer's built-in style.
    Text { header: Option<Vec<u8>> },
    /// Ready-made pictures (Blu-ray, DVD).
    Pictures,
}

/// One decoded subtitle: lines of text or a set of pictures, with when to show them.
pub struct SubtitleEvent {
    ptr: NonNull<ffi::bsv_sub_event>,
}

// SAFETY: an event is plain owned data with no thread affinity.
unsafe impl Send for SubtitleEvent {}

impl Drop for SubtitleEvent {
    fn drop(&mut self) {
        // SAFETY: we own the event until it is handed to a Gpu, which forgets this wrapper.
        unsafe { ffi::bsv_sub_event_free(self.ptr.as_ptr()) };
    }
}

pub struct SubtitleDecoder {
    ptr: NonNull<ffi::bsv_subdec>,
}

// SAFETY: the decoder is only ever used from the one thread that owns it.
unsafe impl Send for SubtitleDecoder {}

impl SubtitleDecoder {
    /// A decoder for the subtitle track the source has selected.
    pub fn open(source: &Source) -> Result<SubtitleDecoder, Error> {
        let mut error = [0 as c_char; 256];
        // SAFETY: the source is alive for the call; the error buffer outlives it.
        let ptr = unsafe {
            ffi::bsv_subdec_open(
                source.ptr.as_ptr(),
                error.as_mut_ptr(),
                error.len() as c_int,
            )
        };
        NonNull::new(ptr)
            .map(|ptr| SubtitleDecoder { ptr })
            .ok_or_else(|| Error(text(&error)))
    }

    pub fn kind(&self) -> SubtitleKind {
        // SAFETY: plain calls on an owned decoder; the header is copied before returning.
        unsafe {
            if ffi::bsv_subdec_is_bitmap(self.ptr.as_ptr()) != 0 {
                return SubtitleKind::Pictures;
            }
            let mut len: c_int = 0;
            let header = ffi::bsv_subdec_header(self.ptr.as_ptr(), &mut len);
            let header = (!header.is_null() && len > 0)
                .then(|| std::slice::from_raw_parts(header as *const u8, len as usize).to_vec());
            SubtitleKind::Text { header }
        }
    }

    /// Decode one packet. Many packets hold no event of their own.
    pub fn decode(&mut self, packet: Packet) -> Option<SubtitleEvent> {
        // SAFETY: ownership of the packet moves to the decoder.
        let event = unsafe { ffi::bsv_subdec_decode(self.ptr.as_ptr(), packet.into_raw()) };
        NonNull::new(event).map(|ptr| SubtitleEvent { ptr })
    }
}

impl Drop for SubtitleDecoder {
    fn drop(&mut self) {
        // SAFETY: we own the decoder.
        unsafe { ffi::bsv_subdec_close(self.ptr.as_ptr()) };
    }
}

// ---- Rendering ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Fast = 0,
    Balanced = 1,
    High = 2,
}

/// What the frame queue gets when it asks for another frame.
pub enum Pull {
    Frame(Frame),
    /// Nothing decoded yet; ask again on the next render.
    Pending,
    /// The file has ended.
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    /// A frame was shown.
    Shown,
    /// Nothing to show yet.
    Waiting,
    /// The last frame of the file has been shown.
    Ended,
}

/// How light is encoded on a surface or in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    Sdr,
    Pq,
    Hlg,
}

/// What one render call saw and produced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderStats {
    pub source_primaries: Option<Primaries>,
    pub source_transfer: Option<Transfer>,
    /// Mastering range of the frame, cd/m².
    pub source_nits: (f32, f32),
    pub source_max_cll: f32,
    /// Brightness of the current scene from dynamic metadata, 0 when there is none.
    pub scene_nits: f32,
    pub dolby_vision: bool,
    pub hdr10_plus: bool,
    /// A subtitle was drawn over this frame.
    pub subtitle_shown: bool,
    /// The surface the system actually gave us.
    pub output_primaries: Option<Primaries>,
    pub output_transfer: Option<Transfer>,
    pub output_nits: (f32, f32),
    pub output_bits: u32,
    pub fps: f64,
}

fn primaries_from_c(v: c_int) -> Option<Primaries> {
    match v {
        1 => Some(Primaries::Bt709),
        2 => Some(Primaries::DisplayP3),
        3 => Some(Primaries::Bt2020),
        _ => None,
    }
}

fn transfer_from_c(v: c_int) -> Option<Transfer> {
    match v {
        1 | 4 => Some(Transfer::Sdr),
        2 => Some(Transfer::Pq),
        3 => Some(Transfer::Hlg),
        _ => None,
    }
}

fn target_to_c(output: &Output) -> ffi::bsv_target {
    let primaries = |p: Primaries| match p {
        Primaries::Bt709 => 1,
        Primaries::DisplayP3 => 2,
        Primaries::Bt2020 => 3,
    };
    match output.signal {
        Signal::Sdr(p) => ffi::bsv_target {
            primaries: primaries(p),
            transfer: 1,
            max_nits: 0.0,
            min_nits: 0.0,
            passthrough: 0,
        },
        Signal::Hdr10 => ffi::bsv_target {
            primaries: 3,
            transfer: 2,
            max_nits: output.peak_nits,
            min_nits: output.min_nits.unwrap_or(0.0),
            passthrough: output.passthrough as c_int,
        },
    }
}

extern "C" fn pull_trampoline(user: *mut c_void, out: *mut ffi::bsv_frame) -> c_int {
    // SAFETY: `user` is the `&mut &mut dyn FnMut` that `Gpu::render` passed for this call only.
    let pull = unsafe { &mut *(user as *mut &mut dyn FnMut() -> Pull) };
    match pull() {
        Pull::Frame(frame) => {
            // SAFETY: `out` is valid; ownership of the AVFrame moves to the queue.
            unsafe {
                (*out).avframe = frame.ptr.as_ptr();
                (*out).pts = frame.pts;
                (*out).duration = frame.duration;
            }
            std::mem::forget(frame);
            1
        }
        Pull::End => 0,
        Pull::Pending => 2,
    }
}

pub struct Gpu {
    ptr: NonNull<ffi::bsv_gpu>,
}

// SAFETY: the gpu is only ever used from the one thread that owns it.
unsafe impl Send for Gpu {}

impl Gpu {
    /// Render into a window by its handle (`HWND`), through Direct3D 11.
    ///
    /// # Safety
    /// `hwnd` must be a live window that outlives the returned `Gpu`.
    #[cfg(windows)]
    pub unsafe fn for_window(hwnd: *mut c_void) -> Result<Gpu, Error> {
        let mut error = [0 as c_char; 256];
        // SAFETY: upheld by the caller; the error buffer outlives the call.
        let ptr =
            unsafe { ffi::bsv_gpu_create_win32(hwnd, error.as_mut_ptr(), error.len() as c_int) };
        NonNull::new(ptr)
            .map(|ptr| Gpu { ptr })
            .ok_or_else(|| Error(text(&error)))
    }

    /// Render into a swapchain that belongs to no window, for an application that composes
    /// its own (WinUI). Returns the gpu and the swapchain (an `IDXGISwapChain1*`) for the
    /// application to put into one of its panels; the gpu owns it.
    #[cfg(windows)]
    pub fn for_composition(width: u32, height: u32) -> Result<(Gpu, *mut c_void), Error> {
        let mut error = [0 as c_char; 256];
        let mut swapchain = std::ptr::null_mut();
        // SAFETY: both out-pointers are valid for the call.
        let ptr = unsafe {
            ffi::bsv_gpu_create_composition(
                width as c_int,
                height as c_int,
                &mut swapchain,
                error.as_mut_ptr(),
                error.len() as c_int,
            )
        };
        NonNull::new(ptr)
            .map(|ptr| (Gpu { ptr }, swapchain))
            .ok_or_else(|| Error(text(&error)))
    }

    /// How many pixels one unit of the application's panel is (its display scaling).
    #[cfg(windows)]
    pub fn set_scale(&mut self, scale: f32) {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_set_scale(self.ptr.as_ptr(), scale) };
    }

    /// Render into a `CAMetalLayer`.
    ///
    /// # Safety
    /// `layer` must point to a live `CAMetalLayer` that outlives the returned `Gpu`.
    #[cfg(target_os = "macos")]
    pub unsafe fn for_metal_layer(layer: *mut c_void) -> Result<Gpu, Error> {
        let mut error = [0 as c_char; 256];
        // SAFETY: upheld by the caller; the error buffer outlives the call.
        let ptr =
            unsafe { ffi::bsv_gpu_create_metal(layer, error.as_mut_ptr(), error.len() as c_int) };
        NonNull::new(ptr)
            .map(|ptr| Gpu { ptr })
            .ok_or_else(|| Error(text(&error)))
    }

    /// The window's size in physical pixels changed.
    pub fn resize(&mut self, width: u32, height: u32) {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_resize(self.ptr.as_ptr(), width as c_int, height as c_int) };
    }

    /// Choose a rendering preset, then apply libplacebo options on top ("key=value,key=value").
    pub fn set_options(&mut self, quality: Quality, options: &str) -> Result<(), Error> {
        let c_options =
            CString::new(options).map_err(|_| Error("options contain a NUL byte".into()))?;
        // SAFETY: the string outlives the call.
        match unsafe {
            ffi::bsv_gpu_set_options(self.ptr.as_ptr(), quality as c_int, c_options.as_ptr())
        } {
            0 => Ok(()),
            _ => Err(Error(format!(
                "the renderer does not accept the options \"{options}\""
            ))),
        }
    }

    /// Make a font from inside the file available to styled subtitles. Call before
    /// [`begin_subtitles`](Self::begin_subtitles).
    pub fn add_font(&mut self, name: &str, data: &[u8]) {
        let Ok(c_name) = CString::new(name) else {
            return;
        };
        // SAFETY: both buffers outlive the call, which copies them.
        unsafe {
            ffi::bsv_gpu_subs_add_font(
                self.ptr.as_ptr(),
                c_name.as_ptr(),
                data.as_ptr() as *const c_void,
                data.len() as c_int,
            )
        };
    }

    /// Start showing a subtitle track of this kind, or none. `video_size` is the picture's
    /// size in the file, which subtitle styles are scaled against.
    pub fn begin_subtitles(&mut self, kind: Option<&SubtitleKind>, video_size: (u32, u32)) {
        let (code, header): (c_int, Option<&[u8]>) = match kind {
            None => (0, None),
            Some(SubtitleKind::Text { header }) => (1, header.as_deref()),
            Some(SubtitleKind::Pictures) => (2, None),
        };
        // SAFETY: the header outlives the call, which copies it.
        unsafe {
            ffi::bsv_gpu_subs_begin(
                self.ptr.as_ptr(),
                code,
                header.map_or(std::ptr::null(), |h| h.as_ptr() as *const c_char),
                header.map_or(0, |h| h.len() as c_int),
                video_size.0 as c_int,
                video_size.1 as c_int,
            )
        };
    }

    /// How subtitles look: `scale` is the text size (1.0 as styled), `nits` how bright their
    /// white is in cd/m² (0.0 for the usual SDR white).
    pub fn set_subtitle_look(&mut self, scale: f32, nits: f32) {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_subs_set_look(self.ptr.as_ptr(), scale, nits) };
    }

    /// Keep subtitles clear of this many pixels at the bottom of the window.
    pub fn set_subtitle_lift(&mut self, pixels: f32) {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_subs_set_lift(self.ptr.as_ptr(), pixels) };
    }

    /// After a seek: forget the subtitle events received so far.
    pub fn clear_subtitles(&mut self) {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_subs_clear(self.ptr.as_ptr()) };
    }

    /// Queue a subtitle for showing at its time.
    pub fn push_subtitle(&mut self, event: SubtitleEvent) {
        // SAFETY: ownership of the event moves to the gpu.
        unsafe { ffi::bsv_gpu_subs_push(self.ptr.as_ptr(), event.ptr.as_ptr()) };
        std::mem::forget(event);
    }

    /// Apply a frame's Dolby Vision RPU (the default), or leave it aside and show the base
    /// layer as the plain signal it also is. Holds for frames taken from now on.
    pub fn set_dolby_vision(&mut self, apply: bool) {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_set_dolby_vision(self.ptr.as_ptr(), apply as c_int) };
    }

    /// How many refreshes of the display a picture waits in line between being drawn and
    /// being shown.
    pub fn frames_in_line(&self) -> u32 {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_frames_in_line(self.ptr.as_ptr()) }.max(0) as u32
    }

    /// Forget every queued frame: after a seek, or before another file.
    pub fn reset(&mut self) {
        // SAFETY: plain call on an owned gpu.
        unsafe { ffi::bsv_gpu_reset(self.ptr.as_ptr()) };
    }

    /// Show the picture for time `pts` and wait for the display's next refresh.
    ///
    /// `pull` is called, possibly several times, whenever the queue wants more frames.
    pub fn render(
        &mut self,
        pts: f64,
        vsync: f64,
        output: &Output,
        mut pull: &mut dyn FnMut() -> Pull,
    ) -> Result<(RenderStatus, RenderStats), Error> {
        let target = target_to_c(output);
        let mut c = ffi::bsv_render_stats::default();
        let user = &mut pull as *mut &mut dyn FnMut() -> Pull as *mut c_void;
        // SAFETY: every pointer is valid for the duration of the call, and `pull` is only
        // invoked inside it.
        let status = unsafe {
            ffi::bsv_gpu_render(
                self.ptr.as_ptr(),
                pts,
                vsync,
                &target,
                Some(pull_trampoline),
                user,
                &mut c,
            )
        };
        let stats = RenderStats {
            source_primaries: primaries_from_c(c.src_primaries),
            source_transfer: transfer_from_c(c.src_transfer),
            source_nits: (c.src_min_nits, c.src_max_nits),
            source_max_cll: c.src_max_cll,
            scene_nits: c.src_scene_max_nits,
            dolby_vision: c.src_dolby_vision != 0,
            hdr10_plus: c.src_hdr10_plus != 0,
            subtitle_shown: c.subtitle_shown != 0,
            output_primaries: primaries_from_c(c.out_primaries),
            output_transfer: transfer_from_c(c.out_transfer),
            output_nits: (c.out_min_nits, c.out_max_nits),
            output_bits: c.out_bits.max(0) as u32,
            fps: c.fps,
        };
        match status {
            0 => Ok((RenderStatus::Shown, stats)),
            1 => Ok((RenderStatus::Waiting, stats)),
            2 => Ok((RenderStatus::Ended, stats)),
            _ => Err(Error("rendering failed".into())),
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: we own the gpu.
        unsafe { ffi::bsv_gpu_destroy(self.ptr.as_ptr()) };
    }
}
