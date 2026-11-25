//! Declarations matching `c/bsv.h`. Keep the two files in step by hand: the surface is small.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_double, c_float, c_int, c_void};

#[repr(C)]
pub struct bsv_source {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct bsv_video {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct bsv_audio {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct bsv_still {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct bsv_tempo {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct bsv_between {
    _opaque: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
pub struct bsv_audio_info {
    pub codec: [c_char; 32],
    pub profile: [c_char; 32],
    pub layout: [c_char; 32],
    pub language: [c_char; 16],
    pub sample_rate: c_int,
    pub channels: c_int,
}

#[repr(C)]
pub struct bsv_subdec {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct bsv_sub_event {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct bsv_track {
    pub index: c_int,
    pub kind: c_int,
    pub codec: [c_char; 32],
    pub profile: [c_char; 32],
    pub language: [c_char; 16],
    pub title: [c_char; 96],
    pub layout: [c_char; 32],
    pub channels: c_int,
    pub sample_rate: c_int,
    pub is_default: c_int,
    pub forced: c_int,
    pub hearing_impaired: c_int,
    pub bitmap: c_int,
}

#[repr(C)]
pub struct bsv_chapter {
    pub start: c_double,
    pub end: c_double,
    pub title: [c_char; 128],
}

#[repr(C)]
pub struct bsv_packet {
    pub avpacket: *mut c_void,
    pub kind: c_int,
    pub duration: c_double,
}

#[repr(C)]
pub struct bsv_gpu {
    _opaque: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
pub struct bsv_media_info {
    pub width: c_int,
    pub height: c_int,
    pub bit_depth: c_int,
    pub fps: c_double,
    pub duration: c_double,
    pub codec: [c_char; 32],
    pub pixel_format: [c_char; 32],
    pub hardware: c_int,
    pub primaries: c_int,
    pub transfer: c_int,
    pub matrix: c_int,
    pub range: c_int,
    pub mastering_min: c_float,
    pub mastering_max: c_float,
    pub max_cll: c_float,
    pub max_fall: c_float,
    pub dv_profile: c_int,
    pub dv_level: c_int,
    pub dv_bl_compatibility: c_int,
    pub dv_el_present: c_int,
    pub pixel_aspect: c_double,
}

#[repr(C)]
pub struct bsv_frame {
    pub avframe: *mut c_void,
    pub pts: c_double,
    pub duration: c_double,
}

impl Default for bsv_frame {
    fn default() -> Self {
        bsv_frame {
            avframe: std::ptr::null_mut(),
            pts: 0.0,
            duration: 0.0,
        }
    }
}

#[repr(C)]
pub struct bsv_target {
    pub primaries: c_int,
    pub transfer: c_int,
    pub max_nits: c_float,
    pub min_nits: c_float,
    pub passthrough: c_int,
}

#[repr(C)]
#[derive(Default)]
pub struct bsv_render_stats {
    pub src_primaries: c_int,
    pub src_transfer: c_int,
    pub src_max_nits: c_float,
    pub src_min_nits: c_float,
    pub src_max_cll: c_float,
    pub src_scene_max_nits: c_float,
    pub src_dolby_vision: c_int,
    pub src_hdr10_plus: c_int,
    pub subtitle_shown: c_int,
    pub out_primaries: c_int,
    pub out_transfer: c_int,
    pub out_max_nits: c_float,
    pub out_min_nits: c_float,
    pub out_bits: c_int,
    pub upscaled: c_int,
    pub interlaced: c_int,
    pub new_picture: c_int,
    pub gpu_ms: c_float,
    pub fps: c_double,
}

pub type bsv_log_fn =
    Option<extern "C" fn(user: *mut c_void, level: c_int, message: *const c_char)>;
pub type bsv_pull_fn = Option<extern "C" fn(user: *mut c_void, out: *mut bsv_frame) -> c_int>;

unsafe extern "C" {
    pub fn bsv_set_log(f: bsv_log_fn, user: *mut c_void, max_level: c_int);

    pub fn bsv_source_open(
        url: *const c_char,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_source;
    pub fn bsv_source_open_sidecar(
        url: *const c_char,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_source;
    pub fn bsv_source_open_sound(
        url: *const c_char,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_source;
    pub fn bsv_source_close(s: *mut bsv_source);
    pub fn bsv_source_has_video(s: *const bsv_source) -> c_int;
    pub fn bsv_source_audio_info(s: *const bsv_source, out: *mut bsv_audio_info) -> c_int;
    pub fn bsv_source_read(s: *mut bsv_source, out: *mut bsv_packet) -> c_int;
    pub fn bsv_source_seek(s: *mut bsv_source, seconds: c_double) -> c_int;
    pub fn bsv_packet_free(avpacket: *mut c_void);

    pub fn bsv_source_track_count(s: *const bsv_source) -> c_int;
    pub fn bsv_source_track(s: *const bsv_source, n: c_int, out: *mut bsv_track) -> c_int;
    pub fn bsv_source_chapter_count(s: *const bsv_source) -> c_int;
    pub fn bsv_source_chapter(s: *const bsv_source, n: c_int, out: *mut bsv_chapter) -> c_int;
    pub fn bsv_source_audio_index(s: *const bsv_source) -> c_int;
    pub fn bsv_source_subtitle_index(s: *const bsv_source) -> c_int;
    pub fn bsv_source_select_audio(s: *mut bsv_source, index: c_int) -> c_int;
    pub fn bsv_source_select_subtitle(s: *mut bsv_source, index: c_int) -> c_int;
    pub fn bsv_source_font(
        s: *const bsv_source,
        n: c_int,
        name: *mut *const c_char,
        data: *mut *const c_void,
        len: *mut c_int,
    ) -> c_int;

    pub fn bsv_subdec_open(
        s: *const bsv_source,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_subdec;
    pub fn bsv_subdec_close(d: *mut bsv_subdec);
    pub fn bsv_subdec_is_bitmap(d: *const bsv_subdec) -> c_int;
    pub fn bsv_subdec_header(d: *const bsv_subdec, len: *mut c_int) -> *const c_char;
    pub fn bsv_subdec_decode(d: *mut bsv_subdec, avpacket: *mut c_void) -> *mut bsv_sub_event;
    pub fn bsv_sub_event_free(e: *mut bsv_sub_event);

    pub fn bsv_video_open(
        s: *const bsv_source,
        hardware: c_int,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_video;
    pub fn bsv_video_close(v: *mut bsv_video);
    pub fn bsv_video_info(v: *const bsv_video, out: *mut bsv_media_info);
    pub fn bsv_video_is_hardware(v: *const bsv_video) -> c_int;
    pub fn bsv_video_send(v: *mut bsv_video, avpacket: *mut c_void) -> c_int;
    pub fn bsv_video_receive(v: *mut bsv_video, out: *mut bsv_frame) -> c_int;
    pub fn bsv_video_flush(v: *mut bsv_video, seconds: c_double);
    pub fn bsv_frame_free(avframe: *mut c_void);
    pub fn bsv_frame_share(avframe: *mut c_void) -> *mut c_void;
    pub fn bsv_between_open(
        method: c_int,
        width: c_int,
        height: c_int,
        param: *const c_char,
        model: *const c_void,
        model_len: usize,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_between;
    pub fn bsv_between_built(method: c_int) -> c_int;
    pub fn bsv_between_close(m: *mut bsv_between);
    pub fn bsv_between_make(
        m: *mut bsv_between,
        a: *mut c_void,
        b: *mut c_void,
        out: *mut *mut c_void,
        moved: *mut c_float,
        relit: *mut c_float,
    ) -> c_int;

    pub fn bsv_audio_open(
        s: *const bsv_source,
        out_rate: c_int,
        out_channels: c_int,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_audio;
    pub fn bsv_audio_close(a: *mut bsv_audio);
    pub fn bsv_audio_send(a: *mut bsv_audio, avpacket: *mut c_void) -> c_int;
    pub fn bsv_audio_receive(
        a: *mut bsv_audio,
        samples: *mut *const c_float,
        frames: *mut c_int,
        pts: *mut c_double,
    ) -> c_int;
    pub fn bsv_audio_flush(a: *mut bsv_audio);

    #[cfg(target_os = "macos")]
    pub fn bsv_gpu_create_metal(
        layer: *mut c_void,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_gpu;
    #[cfg(windows)]
    pub fn bsv_gpu_create_win32(
        hwnd: *mut c_void,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_gpu;
    #[cfg(windows)]
    pub fn bsv_gpu_create_composition(
        width: c_int,
        height: c_int,
        swapchain: *mut *mut c_void,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_gpu;
    #[cfg(windows)]
    pub fn bsv_gpu_set_scale(g: *mut bsv_gpu, scale: c_float);
    pub fn bsv_gpu_destroy(g: *mut bsv_gpu);
    pub fn bsv_gpu_resize(g: *mut bsv_gpu, width: c_int, height: c_int);
    pub fn bsv_gpu_set_options(g: *mut bsv_gpu, preset: c_int, options: *const c_char) -> c_int;
    pub fn bsv_gpu_set_upscaler(g: *mut bsv_gpu, shader: *const c_char, len: usize) -> c_int;
    pub fn bsv_gpu_cache_load(g: *mut bsv_gpu, data: *const c_void, len: usize) -> c_int;
    pub fn bsv_gpu_cache_save(g: *mut bsv_gpu, data: *mut c_void, size: usize) -> usize;
    pub fn bsv_gpu_cache_signature(g: *mut bsv_gpu) -> u64;
    pub fn bsv_gpu_reset(g: *mut bsv_gpu);
    pub fn bsv_gpu_set_dolby_vision(g: *mut bsv_gpu, use_it: c_int);
    pub fn bsv_gpu_frames_in_line(g: *const bsv_gpu) -> c_int;
    pub fn bsv_gpu_subs_add_font(
        g: *mut bsv_gpu,
        name: *const c_char,
        data: *const c_void,
        len: c_int,
    );
    pub fn bsv_gpu_subs_begin(
        g: *mut bsv_gpu,
        kind: c_int,
        header: *const c_char,
        header_len: c_int,
        video_w: c_int,
        video_h: c_int,
    );
    pub fn bsv_gpu_subs_clear(g: *mut bsv_gpu);
    pub fn bsv_gpu_subs_set_delay(g: *mut bsv_gpu, seconds: c_double);
    pub fn bsv_gpu_set_picture(g: *mut bsv_gpu, shape: c_double, cut: c_double, fill: c_int);
    pub fn bsv_still_open(path: *const c_char, error: *mut c_char, error_len: c_int) -> *mut bsv_still;
    pub fn bsv_still_close(s: *mut bsv_still);
    pub fn bsv_still_save(
        s: *mut bsv_still,
        seconds: c_double,
        width: c_int,
        exact: c_int,
        path: *const c_char,
        error: *mut c_char,
        error_len: c_int,
    ) -> c_int;
    pub fn bsv_tempo_open(
        rate: c_int,
        channels: c_int,
        speed: c_double,
        night: c_int,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut bsv_tempo;
    pub fn bsv_tempo_close(t: *mut bsv_tempo);
    pub fn bsv_tempo_send(t: *mut bsv_tempo, samples: *const f32, frames: c_int) -> c_int;
    pub fn bsv_tempo_receive(t: *mut bsv_tempo, samples: *mut *const f32, frames: *mut c_int) -> c_int;
    pub fn bsv_gpu_subs_set_lift(g: *mut bsv_gpu, pixels: c_float);
    pub fn bsv_gpu_subs_set_look(g: *mut bsv_gpu, scale: c_float, nits: c_float);
    pub fn bsv_gpu_subs_push(g: *mut bsv_gpu, e: *mut bsv_sub_event);
    pub fn bsv_gpu_render(
        g: *mut bsv_gpu,
        pts: c_double,
        vsync: c_double,
        target: *const bsv_target,
        pull: bsv_pull_fn,
        user: *mut c_void,
        stats: *mut bsv_render_stats,
    ) -> c_int;
}
