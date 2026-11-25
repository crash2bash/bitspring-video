// The C side of the engine: FFmpeg decodes, libplacebo renders.
//
// Every object has one owning thread. The source and the decoders touch only FFmpeg; a gpu
// belongs to the render thread and touches only libplacebo. Packets and frames cross between
// threads as opaque pointers, so nothing here needs a lock.

#ifndef BSV_H
#define BSV_H

#include <stdint.h>

typedef struct bsv_gpu bsv_gpu;

// ---- Logging --------------------------------------------------------------------------------

// level: 1 error, 2 warning, 3 info, 4 debug.
typedef void (*bsv_log_fn)(void *user, int level, const char *message);
void bsv_set_log(bsv_log_fn fn, void *user, int max_level);

// ---- Reading and decoding ------------------------------------------------------------------
//
// Three objects, each used by one thread at a time:
//   bsv_source  reads packets from the file          (reading thread)
//   bsv_audio   decodes sound and converts it        (reading thread)
//   bsv_video   decodes pictures                     (video thread)

typedef struct bsv_source bsv_source;
typedef struct bsv_video bsv_video;
typedef struct bsv_audio bsv_audio;

struct bsv_media_info {
    int width, height;
    int bit_depth;
    double fps;
    double duration;        // seconds, 0 when unknown
    char codec[32];
    char pixel_format[32];
    int hardware;           // decoding on the GPU's video decoder

    // Colour as tagged in the stream: values of FFmpeg's AVColor* enums.
    int primaries, transfer, matrix, range;

    // HDR10 static metadata, 0 when absent. cd/m².
    float mastering_min, mastering_max;
    float max_cll, max_fall;

    // Dolby Vision configuration record, profile 0 when absent.
    int dv_profile, dv_level;
    int dv_bl_compatibility;    // 1 = HDR10 base, 2 = SDR, 4 = HLG, 0 = none (profile 5)
    int dv_el_present;          // an enhancement layer exists (profile 7); it is not decoded

    // How wide one pixel is shown against its height: 1 for square pixels, 64/45 for a 16:9 DVD.
    double pixel_aspect;
};

struct bsv_audio_info {
    char codec[32];
    char profile[32];       // "DTS-HD MA", "Dolby TrueHD + Dolby Atmos"; empty when plain
    char layout[32];        // "stereo", "5.1(side)"
    char language[16];
    int sample_rate;
    int channels;
};

enum bsv_packet_kind { BSV_PACKET_VIDEO = 1, BSV_PACKET_AUDIO = 2, BSV_PACKET_SUBTITLE = 3 };

// A sound or subtitle stream the user can choose.
struct bsv_track {
    int index;              // the stream's number in the file: what the select functions take
    int kind;               // BSV_PACKET_AUDIO or BSV_PACKET_SUBTITLE
    char codec[32];
    char profile[32];
    char language[16];
    char title[96];
    char layout[32];        // sound only
    int channels;
    int sample_rate;
    int is_default;
    int forced;
    int hearing_impaired;
    int bitmap;             // subtitles made of pictures (Blu-ray, DVD), not text
};

struct bsv_packet {
    void *avpacket;     // owned by the receiver: pass to a decoder or to bsv_packet_free
    int kind;           // enum bsv_packet_kind
    double duration;    // seconds of content in the packet
};

struct bsv_frame {
    void *avframe;      // owned by the receiver: pass to a gpu or to bsv_frame_free
    double pts;         // seconds from the start of the file
    double duration;
};

bsv_source *bsv_source_open(const char *url, char *error, int error_len);
// A subtitle file kept next to the video: only its subtitle stream is read.
bsv_source *bsv_source_open_sidecar(const char *url, char *error, int error_len);
// A sound file kept next to the video: only its sound is read, and no picture is looked for.
bsv_source *bsv_source_open_sound(const char *url, char *error, int error_len);
void bsv_source_close(bsv_source *s);
int bsv_source_has_video(const bsv_source *s);
// 1 and fills `out` when the file has a sound track this build can decode, else 0.
int bsv_source_audio_info(const bsv_source *s, struct bsv_audio_info *out);
// 1: a packet was written to out. 0: end of file. Negative: an FFmpeg error code.
int bsv_source_read(bsv_source *s, struct bsv_packet *out);
// Jump to the keyframe at or before `seconds`.
int bsv_source_seek(bsv_source *s, double seconds);
void bsv_packet_free(void *avpacket);

int bsv_source_track_count(const bsv_source *s);
// Fills `out` with the n-th track (0-based); returns 0 when there is none.
int bsv_source_track(const bsv_source *s, int n, struct bsv_track *out);
// The stream now being read for each kind, or -1.
int bsv_source_audio_index(const bsv_source *s);
int bsv_source_subtitle_index(const bsv_source *s);
// Read another stream from now on. The matching decoder must be opened again afterwards.
int bsv_source_select_audio(bsv_source *s, int index);   // -1 for none
int bsv_source_select_subtitle(bsv_source *s, int index); // -1 for none
// A chapter of the file, on the same timeline as everything else: seconds from the start.
struct bsv_chapter {
    double start, end;      // end is 0 when the file does not say
    char title[128];        // empty when the chapter has none
};
// The chapters as the file lists them; returns 0 when there is no n-th (0-based).
int bsv_source_chapter_count(const bsv_source *s);
int bsv_source_chapter(const bsv_source *s, int n, struct bsv_chapter *out);

// The n-th font shipped inside the file; pointers are valid while the source is open.
int bsv_source_font(const bsv_source *s, int n, const char **name, const void **data, int *len);

// Subtitles: decoded on the reading thread into events, shown by a gpu on the render thread.
typedef struct bsv_subdec bsv_subdec;
typedef struct bsv_sub_event bsv_sub_event;

bsv_subdec *bsv_subdec_open(const bsv_source *s, char *error, int error_len);
void bsv_subdec_close(bsv_subdec *d);
int bsv_subdec_is_bitmap(const bsv_subdec *d);
// The stream's own ASS header, or NULL when the subtitles are plain text or pictures.
const char *bsv_subdec_header(const bsv_subdec *d, int *len);
// Consumes the packet. Returns an event owned by the caller, or NULL when the packet held none.
bsv_sub_event *bsv_subdec_decode(bsv_subdec *d, void *avpacket);
void bsv_sub_event_free(bsv_sub_event *e);

// Both decoders copy what they need from the source: create them before the threads start.
// hardware: 1 to use the platform's video decoder when the codec allows. If it later refuses part
// of the stream, decoding goes on in software from the same picture.
bsv_video *bsv_video_open(const bsv_source *s, int hardware, char *error, int error_len);
void bsv_video_close(bsv_video *v);
void bsv_video_info(const bsv_video *v, struct bsv_media_info *out);
// Whether the platform's video decoder is doing the decoding now: it can stop part-way through.
int bsv_video_is_hardware(const bsv_video *v);
// Feed one packet (consumed), or NULL at the end of the file. Then call receive until it returns 0.
int bsv_video_send(bsv_video *v, void *avpacket);
// 1: a frame was written to out. 0: wants another packet. 2: nothing left after the end. Negative: error.
int bsv_video_receive(bsv_video *v, struct bsv_frame *out);
// After a seek: forget everything in flight.
void bsv_video_flush(bsv_video *v, double seconds);
void bsv_frame_free(void *avframe);
// Another reference to the same picture, to be freed like any frame.
void *bsv_frame_share(void *avframe);

// ---- Frames in between ----------------------------------------------------------------------

// Between two neighbouring pictures of a video, the one for the moment half-way: 25 pictures
// a second become 50. A maker is opened for one size of picture.
typedef struct bsv_between bsv_between;
enum bsv_between_method {
    BSV_BETWEEN_SYSTEM = 1, // the system's own: VideoToolbox, on Apple systems only
    BSV_BETWEEN_NEURAL = 2, // RIFE
};
enum bsv_between_result {
    BSV_BETWEEN_MADE = 1,
    BSV_BETWEEN_NONE = 0,      // the two pictures are of different shots: nothing belongs between
    BSV_BETWEEN_UNSUITED = -2, // not a kind of picture this maker takes (10-bit, full range, another size)
    BSV_BETWEEN_FAILED = -1,
};
// `param`, `model`: the neural network's model, for BSV_BETWEEN_NEURAL; NULL otherwise.
bsv_between *bsv_between_open(int method, int width, int height, const char *param, const void *model,
                              size_t model_len, char *error, int error_len);
// Whether this build has the method at all.
int bsv_between_built(int method);
void bsv_between_close(bsv_between *m);
// `*out` receives the new frame (for bsv_frame_free or a gpu) when the result is MADE. `*moved`
// and `*relit` say how unlike the two pictures were found to be, which decides NONE.
int bsv_between_make(bsv_between *m, void *avframe_a, void *avframe_b, void **out, float *moved, float *relit);

// Sound comes out as interleaved 32-bit float at the rate and channel count asked for here.
bsv_audio *bsv_audio_open(const bsv_source *s, int out_rate, int out_channels, char *error, int error_len);
void bsv_audio_close(bsv_audio *a);
int bsv_audio_send(bsv_audio *a, void *avpacket);
// 1: `*samples` holds `*frames` frames starting at media time `*pts`, valid until the next call.
// 0: wants another packet. 2: nothing left after the end. Negative: error.
int bsv_audio_receive(bsv_audio *a, const float **samples, int *frames, double *pts);
void bsv_audio_flush(bsv_audio *a);

// ---- Still pictures -------------------------------------------------------------------------

// Pictures of a video taken apart from playback, with a reader of their own: one thread at a
// time. HDR is brought down to SDR for them.
typedef struct bsv_still bsv_still;
bsv_still *bsv_still_open(const char *path, char *error, int error_len);
void bsv_still_close(bsv_still *s);
// Write the picture at `seconds` as a PNG at `path`, `width` pixels wide (0: as large as it is
// shown). exact: the very picture on screen at that time; otherwise the nearest keyframe before
// it, which is quick. Returns 0, or -1 with `error` filled.
int bsv_still_save(bsv_still *s, double seconds, int width, int exact, const char *path, char *error,
                   int error_len);

// ---- Sound at another speed --------------------------------------------------------------

// Interleaved 32-bit float in and out, at one rate and channel count, made `speed` times as
// fast (0.5 to 4) without changing its pitch. What comes out lags what goes in by a few
// tens of milliseconds; send 0 frames at the end to have the rest.
typedef struct bsv_tempo bsv_tempo;
bsv_tempo *bsv_tempo_open(int rate, int channels, double speed, char *error, int error_len);
void bsv_tempo_close(bsv_tempo *t);
int bsv_tempo_send(bsv_tempo *t, const float *samples, int frames);
// 1: `*samples` holds `*frames` frames, valid until the next call. 0: wants more. 2: all out.
int bsv_tempo_receive(bsv_tempo *t, const float **samples, int *frames);

// ---- Rendering ------------------------------------------------------------------------------

enum bsv_primaries { BSV_PRIM_BT709 = 1, BSV_PRIM_DISPLAY_P3 = 2, BSV_PRIM_BT2020 = 3 };
enum bsv_transfer { BSV_TRC_SRGB = 1, BSV_TRC_PQ = 2, BSV_TRC_HLG = 3, BSV_TRC_SDR_OTHER = 4 };

// What to send to the display.
struct bsv_target {
    int primaries;      // enum bsv_primaries
    int transfer;       // BSV_TRC_SRGB or BSV_TRC_PQ
    float max_nits;     // luminance range to map into; 0 lets libplacebo infer it
    float min_nits;
    int passthrough;    // HDR only: forward the source's metadata, do not tone-map
};

enum bsv_pull { BSV_PULL_FRAME = 1, BSV_PULL_EOF = 0, BSV_PULL_NONE = 2 };
// Called on the render thread whenever the frame queue wants another frame.
typedef int (*bsv_pull_fn)(void *user, struct bsv_frame *out);

enum bsv_render_status {
    BSV_RENDER_OK = 0,
    BSV_RENDER_WAIT = 1,    // nothing to show yet (or the window is hidden)
    BSV_RENDER_EOF = 2,
    BSV_RENDER_ERROR = -1,
};

struct bsv_render_stats {
    // The frame on screen, as libplacebo understood it.
    int src_primaries, src_transfer;        // enum bsv_primaries / bsv_transfer, 0 unknown
    float src_max_nits, src_min_nits;       // mastering range
    float src_max_cll;
    float src_scene_max_nits;               // dynamic metadata for this scene, 0 when absent
    int src_dolby_vision;                   // the frame carries a Dolby Vision RPU that is applied
    int src_hdr10_plus;
    int subtitle_shown;                     // a subtitle was drawn over this frame

    // The surface actually obtained from the system.
    int out_primaries, out_transfer;
    float out_max_nits, out_min_nits;
    int out_bits;

    int upscaled;           // the picture on screen was doubled by the upscaling shader
    int interlaced;         // the picture is made of two fields, made whole before it is shown
    int new_picture;        // this call drew a picture, not only showed one drawn before
    float gpu_ms;           // what this call's work took on the GPU, as last measured

    double fps;             // estimated from the frames' timestamps
};

// One of these exists, according to the system the bridge was built for. The caller keeps
// the layer or window alive for the gpu's lifetime.
//   layer: a CAMetalLayer*         (Apple systems)
//   hwnd:  a window handle, HWND   (Windows)
bsv_gpu *bsv_gpu_create_metal(void *layer, char *error, int error_len);
bsv_gpu *bsv_gpu_create_win32(void *hwnd, char *error, int error_len);
// Windows, for an application that composes its own window: `*swapchain` receives an
// IDXGISwapChain1* to put into one of its panels. bsv_gpu_set_scale says how many pixels one
// of the panel's units is.
bsv_gpu *bsv_gpu_create_composition(int width, int height, void **swapchain, char *error, int error_len);
void bsv_gpu_set_scale(bsv_gpu *g, float scale);
void bsv_gpu_destroy(bsv_gpu *g);
void bsv_gpu_resize(bsv_gpu *g, int width, int height);

// preset: 0 fast, 1 balanced, 2 high quality. options: libplacebo "key=value,key=value", or NULL.
int bsv_gpu_set_options(bsv_gpu *g, int preset, const char *options);

// Upscaling by a neural network: `shader` is a shader in mpv's format (the text of a .glsl
// file, `len` bytes) that enlarges the picture before it is fitted to the window; such a shader
// says itself when it has anything to do. NULL: none. Returns -1 when the text is not a shader.
int bsv_gpu_set_upscaler(bsv_gpu *g, const char *shader, size_t len);

// Shaders compiled in an earlier run, as bsv_gpu_cache_save gave them: with these the renderer
// does not compile them again. Give them before the first picture. Returns how many were
// taken, negative when the data is not a cache.
int bsv_gpu_cache_load(bsv_gpu *g, const void *data, size_t len);
// Writes what has been compiled so far into `data`, as far as `size` allows, and returns the
// size needed (call with size 0 to learn it). The signature changes when the contents do.
size_t bsv_gpu_cache_save(bsv_gpu *g, void *data, size_t size);
uint64_t bsv_gpu_cache_signature(bsv_gpu *g);

// Drop every queued frame (after a seek or when the file changes).
void bsv_gpu_reset(bsv_gpu *g);

// How the picture is placed in the window: `shape` is the width over the height to show it at
// (0: its own, with pixels that are not square made so), `cut` a shape to cut it down to (0:
// none; 2.39 takes away the bars above and below a 16:9 picture of a scope film), and `fill`
// fills the window, cutting what does not fit.
void bsv_gpu_set_picture(bsv_gpu *g, double shape, double cut, int fill);

// Whether a Dolby Vision RPU on a frame is applied (the default) or left aside, which shows
// the base layer as the plain signal it also is (HDR10 for profile 8.1). It holds for frames
// taken from now on: drop the queued ones and decode again from the current position.
void bsv_gpu_set_dolby_vision(bsv_gpu *g, int use);

// How many refreshes of the display a picture waits in line between being drawn and being
// shown: the pictures the surface holds ahead of the one on screen.
int bsv_gpu_frames_in_line(const bsv_gpu *g);

// Subtitles. kind: 0 off, 1 text (header: the stream's ASS header, or NULL for the built-in
// style), 2 pictures. Fonts must be added before the first text track begins.
void bsv_gpu_subs_add_font(bsv_gpu *g, const char *name, const void *data, int len);
void bsv_gpu_subs_begin(bsv_gpu *g, int kind, const char *header, int header_len, int video_w, int video_h);
// After a seek: forget the events received so far.
void bsv_gpu_subs_clear(bsv_gpu *g);
// Show each subtitle this many seconds after the moment it is timed for (negative: before).
void bsv_gpu_subs_set_delay(bsv_gpu *g, double seconds);
// scale: text size, 1 as styled. nits: how bright subtitle white is, 0 for the usual SDR white.
void bsv_gpu_subs_set_look(bsv_gpu *g, float scale, float nits);
// Keep subtitles above this many pixels at the bottom of the window (where the controls are).
void bsv_gpu_subs_set_lift(bsv_gpu *g, float pixels);
// Takes ownership of the event.
void bsv_gpu_subs_push(bsv_gpu *g, bsv_sub_event *e);

// Show the frame for time `pts` and wait for the display. vsync: the display's frame time in seconds.
int bsv_gpu_render(bsv_gpu *g, double pts, double vsync, const struct bsv_target *target,
                   bsv_pull_fn pull, void *user, struct bsv_render_stats *stats);

#endif
