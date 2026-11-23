#if defined(__APPLE__)
#define VK_USE_PLATFORM_METAL_EXT
#endif
#if defined(_WIN32)
// COM from C: method macros, and the interface ids defined here rather than looked for in a library.
#define COBJMACROS
#define INITGUID
#include <initguid.h>
#endif

#include <float.h>
#include <limits.h>
#include <math.h>
#include <stdbool.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/dovi_meta.h>
#include <libavutil/hwcontext.h>
#include <libavutil/avstring.h>
#include <libavutil/channel_layout.h>
#include <libavutil/mastering_display_metadata.h>
#include <libavutil/opt.h>
#include <libavutil/pixdesc.h>
#include <libavutil/time.h>
#include <libswresample/swresample.h>

#include <ass/ass.h>

#include <libplacebo/log.h>
#include <libplacebo/options.h>
#include <libplacebo/renderer.h>
#include <libplacebo/utils/frame_queue.h>
#if defined(_WIN32)
#include <libplacebo/d3d11.h>
#include <dxgi1_3.h>
#else
#include <libplacebo/vulkan.h>
#endif

#define PL_LIBAV_IMPLEMENTATION 1
#include <libplacebo/utils/libav.h>

#include "bsv.h"

// ---- Logging --------------------------------------------------------------------------------

static bsv_log_fn g_log_fn;
static void *g_log_user;
static int g_log_level = 2;

static void emit(int level, const char *message)
{
    if (!g_log_fn || level > g_log_level)
        return;
    g_log_fn(g_log_user, level, message);
}

static void emitf(int level, const char *fmt, ...)
{
    if (!g_log_fn || level > g_log_level)
        return;
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    g_log_fn(g_log_user, level, buf);
}

static void log_from_ffmpeg(void *avcl, int level, const char *fmt, va_list vl)
{
    int ours = level <= AV_LOG_ERROR ? 1 : level <= AV_LOG_WARNING ? 2 : level <= AV_LOG_INFO ? 3 : 4;
    if (!g_log_fn || ours > g_log_level || level > AV_LOG_VERBOSE)
        return;
    char buf[1024];
    int prefix = 1;
    av_log_format_line2(avcl, level, fmt, vl, buf, sizeof(buf), &prefix);
    size_t len = strlen(buf);
    while (len && (buf[len - 1] == '\n' || buf[len - 1] == '\r'))
        buf[--len] = 0;
    if (len)
        g_log_fn(g_log_user, ours, buf);
}

static void log_from_placebo(void *priv, enum pl_log_level level, const char *message)
{
    emit(level <= PL_LOG_ERR ? 1 : level == PL_LOG_WARN ? 2 : level == PL_LOG_INFO ? 3 : 4, message);
}

static enum pl_log_level placebo_level(void)
{
    switch (g_log_level) {
    case 1: return PL_LOG_ERR;
    case 2: return PL_LOG_WARN;
    case 3: return PL_LOG_INFO;
    default: return PL_LOG_DEBUG;
    }
}

void bsv_set_log(bsv_log_fn fn, void *user, int max_level)
{
    g_log_fn = fn;
    g_log_user = user;
    g_log_level = max_level;
    av_log_set_callback(log_from_ffmpeg);
}

static void set_error(char *error, int error_len, const char *what, int averr)
{
    if (!error || error_len <= 0)
        return;
    if (averr) {
        char detail[128];
        av_strerror(averr, detail, sizeof(detail));
        snprintf(error, error_len, "%s: %s", what, detail);
    } else {
        snprintf(error, error_len, "%s", what);
    }
}

// ---- Reading --------------------------------------------------------------------------------

#if defined(__APPLE__)
#define BSV_HW_TYPE AV_HWDEVICE_TYPE_VIDEOTOOLBOX
#elif defined(_WIN32)
#define BSV_HW_TYPE AV_HWDEVICE_TYPE_D3D11VA
#else
#define BSV_HW_TYPE AV_HWDEVICE_TYPE_VAAPI
#endif

struct bsv_source {
    AVFormatContext *fmt;
    AVStream *video;
    AVStream *audio;
    const AVCodec *audio_codec;
    AVStream *subtitle;
    // Time zero for every stream, so picture and sound share one timeline.
    double origin;
    double video_frame_time;
};

// Open a file on this machine, and nothing else.
//
// A file handed to the player is not to be trusted: something named like a film can be a
// playlist that points FFmpeg at other files or at the network. So only the "file" protocol
// is allowed (whatever the file references, no connection is ever made), the path is marked
// as a path (a name like "concat:a|b" is not read as a protocol), and formats that are lists
// of other files are refused. `formats`, when given, limits what the file may be at all.
static int open_local(AVFormatContext **fmt, const char *path, const char *formats)
{
    char *url = strncmp(path, "file:", 5) == 0 ? av_strdup(path) : av_asprintf("file:%s", path);
    if (!url)
        return AVERROR(ENOMEM);
    AVDictionary *options = NULL;
    av_dict_set(&options, "protocol_whitelist", "file", 0);
    if (formats)
        av_dict_set(&options, "format_whitelist", formats, 0);
    int ret = avformat_open_input(fmt, url, NULL, &options);
    av_dict_free(&options);
    av_free(url);
    if (ret < 0)
        return ret;

    static const char *const lists[] = { "hls", "concat", "dash", "imf", "lavfi" };
    for (size_t i = 0; i < sizeof(lists) / sizeof(lists[0]); i++) {
        if (av_match_name(lists[i], (*fmt)->iformat->name)) {
            avformat_close_input(fmt);
            return AVERROR_INVALIDDATA;
        }
    }
    return 0;
}

// Tell the demuxer which streams are wanted; packets of the others are never read.
static void choose_streams(bsv_source *s)
{
    for (unsigned i = 0; i < s->fmt->nb_streams; i++) {
        AVStream *st = s->fmt->streams[i];
        bool wanted = st == s->video || st == s->audio || st == s->subtitle;
        st->discard = wanted ? AVDISCARD_DEFAULT : AVDISCARD_ALL;
    }
}

void bsv_source_close(bsv_source *s)
{
    if (!s)
        return;
    avformat_close_input(&s->fmt);
    av_free(s);
}

bsv_source *bsv_source_open(const char *url, char *error, int error_len)
{
    bsv_source *s = av_mallocz(sizeof(*s));
    if (!s)
        return NULL;

    int ret = open_local(&s->fmt, url, NULL);
    if (ret < 0) {
        set_error(error, error_len, "cannot open the file", ret);
        goto fail;
    }
    if ((ret = avformat_find_stream_info(s->fmt, NULL)) < 0) {
        set_error(error, error_len, "cannot read the file's streams", ret);
        goto fail;
    }
    int video = av_find_best_stream(s->fmt, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (video >= 0)
        s->video = s->fmt->streams[video];
    int audio = av_find_best_stream(s->fmt, AVMEDIA_TYPE_AUDIO, -1, video, &s->audio_codec, 0);
    if (audio >= 0 && s->audio_codec)
        s->audio = s->fmt->streams[audio];
    if (!s->video && !s->audio) {
        set_error(error, error_len, "nothing in the file this build can play", 0);
        goto fail;
    }
    choose_streams(s);

    if (s->fmt->start_time != AV_NOPTS_VALUE)
        s->origin = s->fmt->start_time / (double) AV_TIME_BASE;
    if (s->video) {
        AVRational rate = av_guess_frame_rate(s->fmt, s->video, NULL);
        s->video_frame_time = rate.num > 0 && rate.den > 0 ? av_q2d(av_inv_q(rate)) : 0.04;
    }
    return s;

fail:
    bsv_source_close(s);
    return NULL;
}

// A subtitle file that sits next to the video. Its times already count from the video's start.
bsv_source *bsv_source_open_sidecar(const char *url, char *error, int error_len)
{
    bsv_source *s = av_mallocz(sizeof(*s));
    if (!s)
        return NULL;
    int ret = open_local(&s->fmt, url, "srt,ass,webvtt");
    if (ret < 0) {
        set_error(error, error_len, "cannot open the subtitle file", ret);
        goto fail;
    }
    if ((ret = avformat_find_stream_info(s->fmt, NULL)) < 0) {
        set_error(error, error_len, "cannot read the subtitle file", ret);
        goto fail;
    }
    int index = av_find_best_stream(s->fmt, AVMEDIA_TYPE_SUBTITLE, -1, -1, NULL, 0);
    if (index < 0 || !avcodec_find_decoder(s->fmt->streams[index]->codecpar->codec_id)) {
        set_error(error, error_len, "no subtitles in the file", 0);
        goto fail;
    }
    s->subtitle = s->fmt->streams[index];
    choose_streams(s);
    return s;

fail:
    bsv_source_close(s);
    return NULL;
}

int bsv_source_has_video(const bsv_source *s)
{
    return s->video != NULL;
}

int bsv_source_audio_info(const bsv_source *s, struct bsv_audio_info *out)
{
    memset(out, 0, sizeof(*out));
    if (!s->audio)
        return 0;
    const AVCodecParameters *par = s->audio->codecpar;
    snprintf(out->codec, sizeof(out->codec), "%s", avcodec_get_name(par->codec_id));
    const char *profile = avcodec_profile_name(par->codec_id, par->profile);
    if (profile)
        snprintf(out->profile, sizeof(out->profile), "%s", profile);
    av_channel_layout_describe(&par->ch_layout, out->layout, sizeof(out->layout));
    const AVDictionaryEntry *language = av_dict_get(s->audio->metadata, "language", NULL, 0);
    if (language)
        snprintf(out->language, sizeof(out->language), "%s", language->value);
    out->sample_rate = par->sample_rate;
    out->channels = par->ch_layout.nb_channels;
    return 1;
}

void bsv_packet_free(void *avpacket)
{
    AVPacket *pkt = avpacket;
    av_packet_free(&pkt);
}

int bsv_source_read(bsv_source *s, struct bsv_packet *out)
{
    AVPacket *pkt = av_packet_alloc();
    if (!pkt)
        return AVERROR(ENOMEM);
    for (;;) {
        int ret = av_read_frame(s->fmt, pkt);
        if (ret < 0) {
            av_packet_free(&pkt);
            return ret == AVERROR_EOF ? 0 : ret;
        }
        const AVStream *st = NULL;
        if (s->video && pkt->stream_index == s->video->index)
            st = s->video;
        else if (s->audio && pkt->stream_index == s->audio->index)
            st = s->audio;
        else if (s->subtitle && pkt->stream_index == s->subtitle->index)
            st = s->subtitle;
        if (!st) {
            av_packet_unref(pkt);
            continue;
        }
        out->avpacket = pkt;
        out->kind = st == s->video ? BSV_PACKET_VIDEO : st == s->audio ? BSV_PACKET_AUDIO : BSV_PACKET_SUBTITLE;
        out->duration = pkt->duration > 0 ? pkt->duration * av_q2d(st->time_base)
                                          : st == s->video ? s->video_frame_time : 0.02;
        return 1;
    }
}

int bsv_source_seek(bsv_source *s, double seconds)
{
    if (seconds < 0)
        seconds = 0;
    // Seeking by the video stream lands on a picture that can be decoded on its own.
    const AVStream *st = s->video ? s->video : s->audio;
    int64_t ts = llround((seconds + s->origin) / av_q2d(st->time_base));
    return avformat_seek_file(s->fmt, st->index, INT64_MIN, ts, ts, 0);
}

// ---- Tracks ---------------------------------------------------------------------------------

// A stream the user can choose: sound or subtitles this build can decode.
static bool is_track(const AVStream *st)
{
    enum AVMediaType type = st->codecpar->codec_type;
    if (type != AVMEDIA_TYPE_AUDIO && type != AVMEDIA_TYPE_SUBTITLE)
        return false;
    return avcodec_find_decoder(st->codecpar->codec_id) != NULL;
}

int bsv_source_track_count(const bsv_source *s)
{
    int count = 0;
    for (unsigned i = 0; i < s->fmt->nb_streams; i++)
        count += is_track(s->fmt->streams[i]);
    return count;
}

int bsv_source_track(const bsv_source *s, int n, struct bsv_track *out)
{
    memset(out, 0, sizeof(*out));
    for (unsigned i = 0; i < s->fmt->nb_streams; i++) {
        const AVStream *st = s->fmt->streams[i];
        if (!is_track(st) || n-- > 0)
            continue;
        const AVCodecParameters *par = st->codecpar;
        out->index = st->index;
        out->kind = par->codec_type == AVMEDIA_TYPE_AUDIO ? BSV_PACKET_AUDIO : BSV_PACKET_SUBTITLE;
        snprintf(out->codec, sizeof(out->codec), "%s", avcodec_get_name(par->codec_id));
        const char *profile = avcodec_profile_name(par->codec_id, par->profile);
        if (profile)
            snprintf(out->profile, sizeof(out->profile), "%s", profile);
        const AVDictionaryEntry *entry;
        if ((entry = av_dict_get(st->metadata, "language", NULL, 0)))
            snprintf(out->language, sizeof(out->language), "%s", entry->value);
        if ((entry = av_dict_get(st->metadata, "title", NULL, 0)))
            snprintf(out->title, sizeof(out->title), "%s", entry->value);
        if (out->kind == BSV_PACKET_AUDIO) {
            av_channel_layout_describe(&par->ch_layout, out->layout, sizeof(out->layout));
            out->channels = par->ch_layout.nb_channels;
            out->sample_rate = par->sample_rate;
        } else {
            const AVCodecDescriptor *desc = avcodec_descriptor_get(par->codec_id);
            out->bitmap = desc && (desc->props & AV_CODEC_PROP_BITMAP_SUB);
        }
        out->is_default = !!(st->disposition & AV_DISPOSITION_DEFAULT);
        out->forced = !!(st->disposition & AV_DISPOSITION_FORCED);
        out->hearing_impaired = !!(st->disposition & AV_DISPOSITION_HEARING_IMPAIRED);
        return 1;
    }
    return 0;
}

int bsv_source_audio_index(const bsv_source *s)
{
    return s->audio ? s->audio->index : -1;
}

int bsv_source_subtitle_index(const bsv_source *s)
{
    return s->subtitle ? s->subtitle->index : -1;
}

static AVStream *stream_of(const bsv_source *s, int index, enum AVMediaType type)
{
    if (index < 0 || (unsigned) index >= s->fmt->nb_streams)
        return NULL;
    AVStream *st = s->fmt->streams[index];
    return st->codecpar->codec_type == type && is_track(st) ? st : NULL;
}

int bsv_source_select_audio(bsv_source *s, int index)
{
    AVStream *st = stream_of(s, index, AVMEDIA_TYPE_AUDIO);
    if (!st)
        return -1;
    s->audio = st;
    s->audio_codec = avcodec_find_decoder(st->codecpar->codec_id);
    choose_streams(s);
    return 0;
}

int bsv_source_select_subtitle(bsv_source *s, int index)
{
    AVStream *st = index < 0 ? NULL : stream_of(s, index, AVMEDIA_TYPE_SUBTITLE);
    if (index >= 0 && !st)
        return -1;
    s->subtitle = st;
    choose_streams(s);
    return 0;
}

// Fonts shipped inside the file (Matroska attachments), which styled subtitles rely on.
int bsv_source_font(const bsv_source *s, int n, const char **name, const void **data, int *len)
{
    for (unsigned i = 0; i < s->fmt->nb_streams; i++) {
        const AVStream *st = s->fmt->streams[i];
        const AVCodecParameters *par = st->codecpar;
        if (par->codec_type != AVMEDIA_TYPE_ATTACHMENT || par->extradata_size <= 0)
            continue;
        const AVDictionaryEntry *mime = av_dict_get(st->metadata, "mimetype", NULL, 0);
        bool font = par->codec_id == AV_CODEC_ID_TTF || par->codec_id == AV_CODEC_ID_OTF ||
                    (mime && (strstr(mime->value, "font") || strstr(mime->value, "truetype")));
        if (!font || n-- > 0)
            continue;
        const AVDictionaryEntry *file = av_dict_get(st->metadata, "filename", NULL, 0);
        *name = file ? file->value : "font";
        *data = par->extradata;
        *len = par->extradata_size;
        return 1;
    }
    return 0;
}

// ---- Subtitle decoding ----------------------------------------------------------------------

struct sub_picture {
    int x, y, w, h;
    uint8_t *rgba; // premultiplied, 4 bytes per pixel
};

struct bsv_sub_event {
    double start;
    double duration;    // negative: until the next event
    char **texts;       // ASS event lines
    int text_count;
    struct sub_picture *pictures;
    int picture_count;
    int canvas_w, canvas_h;
};

void bsv_sub_event_free(bsv_sub_event *e)
{
    if (!e)
        return;
    for (int i = 0; i < e->text_count; i++)
        av_free(e->texts[i]);
    av_free(e->texts);
    for (int i = 0; i < e->picture_count; i++)
        av_free(e->pictures[i].rgba);
    av_free(e->pictures);
    av_free(e);
}

struct bsv_subdec {
    AVCodecContext *dec;
    double time_base;
    double origin;
    int bitmap;
    int styled; // the stream is ASS/SSA: it brings its own styles
    int video_w, video_h;
};

void bsv_subdec_close(bsv_subdec *d)
{
    if (!d)
        return;
    avcodec_free_context(&d->dec);
    av_free(d);
}

bsv_subdec *bsv_subdec_open(const bsv_source *s, char *error, int error_len)
{
    if (!s->subtitle) {
        set_error(error, error_len, "no subtitle track is selected", 0);
        return NULL;
    }
    bsv_subdec *d = av_mallocz(sizeof(*d));
    if (!d)
        return NULL;
    const AVCodecParameters *par = s->subtitle->codecpar;
    const AVCodec *codec = avcodec_find_decoder(par->codec_id);
    const AVCodecDescriptor *desc = avcodec_descriptor_get(par->codec_id);
    d->time_base = av_q2d(s->subtitle->time_base);
    d->origin = s->origin;
    d->bitmap = desc && (desc->props & AV_CODEC_PROP_BITMAP_SUB);
    d->styled = par->codec_id == AV_CODEC_ID_ASS || par->codec_id == AV_CODEC_ID_SSA;
    if (s->video) {
        d->video_w = s->video->codecpar->width;
        d->video_h = s->video->codecpar->height;
    }

    int ret = 0;
    d->dec = codec ? avcodec_alloc_context3(codec) : NULL;
    if (!d->dec || (ret = avcodec_parameters_to_context(d->dec, par)) < 0) {
        set_error(error, error_len, "cannot set up the subtitle decoder", ret);
        goto fail;
    }
    d->dec->pkt_timebase = s->subtitle->time_base;
    if ((ret = avcodec_open2(d->dec, codec, NULL)) < 0) {
        set_error(error, error_len, "cannot start the subtitle decoder", ret);
        goto fail;
    }
    return d;

fail:
    bsv_subdec_close(d);
    return NULL;
}

int bsv_subdec_is_bitmap(const bsv_subdec *d)
{
    return d->bitmap;
}

const char *bsv_subdec_header(const bsv_subdec *d, int *len)
{
    // Only a real ASS stream's header is worth keeping; for plain text formats FFmpeg invents
    // one in a 1990s style, and the renderer's own default looks better.
    if (!d->styled || !d->dec->subtitle_header) {
        *len = 0;
        return NULL;
    }
    *len = d->dec->subtitle_header_size;
    return (const char *) d->dec->subtitle_header;
}

// A palette picture as premultiplied RGBA.
static uint8_t *picture_rgba(const AVSubtitleRect *rect)
{
    uint8_t *out = av_malloc((size_t) rect->w * rect->h * 4);
    if (!out)
        return NULL;
    const uint32_t *palette = (const uint32_t *) rect->data[1];
    for (int y = 0; y < rect->h; y++) {
        const uint8_t *src = rect->data[0] + y * rect->linesize[0];
        uint8_t *dst = out + (size_t) y * rect->w * 4;
        for (int x = 0; x < rect->w; x++) {
            uint32_t argb = palette[src[x]];
            unsigned a = argb >> 24;
            dst[x * 4 + 0] = (uint8_t) (((argb >> 16) & 0xFF) * a / 255);
            dst[x * 4 + 1] = (uint8_t) (((argb >> 8) & 0xFF) * a / 255);
            dst[x * 4 + 2] = (uint8_t) ((argb & 0xFF) * a / 255);
            dst[x * 4 + 3] = (uint8_t) a;
        }
    }
    return out;
}

bsv_sub_event *bsv_subdec_decode(bsv_subdec *d, void *avpacket)
{
    AVPacket *pkt = avpacket;
    AVSubtitle sub = { 0 };
    int got = 0;
    int ret = avcodec_decode_subtitle2(d->dec, &sub, &got, pkt);
    double start = pkt->pts != AV_NOPTS_VALUE ? pkt->pts * d->time_base - d->origin : 0;
    double packet_duration = pkt->duration > 0 ? pkt->duration * d->time_base : -1;
    av_packet_free(&pkt);
    if (ret < 0 || !got)
        return NULL;

    bsv_sub_event *e = av_mallocz(sizeof(*e));
    if (!e) {
        avsubtitle_free(&sub);
        return NULL;
    }
    e->start = start + sub.start_display_time / 1000.0;
    bool timed = sub.end_display_time != 0 && sub.end_display_time != UINT32_MAX &&
                 sub.end_display_time > sub.start_display_time;
    if (d->bitmap) {
        // Picture subtitles stay up until the next one replaces or clears them.
        e->duration = timed ? (sub.end_display_time - sub.start_display_time) / 1000.0 : -1;
        e->canvas_w = d->dec->width > 0 ? d->dec->width : d->video_w;
        e->canvas_h = d->dec->height > 0 ? d->dec->height : d->video_h;
        e->pictures = av_calloc(sub.num_rects ? sub.num_rects : 1, sizeof(*e->pictures));
    } else {
        e->duration = timed ? (sub.end_display_time - sub.start_display_time) / 1000.0
                            : packet_duration > 0 ? packet_duration : 5.0;
        e->texts = av_calloc(sub.num_rects ? sub.num_rects : 1, sizeof(*e->texts));
    }
    for (unsigned i = 0; i < sub.num_rects; i++) {
        const AVSubtitleRect *rect = sub.rects[i];
        if (rect->type == SUBTITLE_ASS && rect->ass && e->texts) {
            e->texts[e->text_count++] = av_strdup(rect->ass);
        } else if (rect->type == SUBTITLE_BITMAP && rect->w > 0 && rect->h > 0 && e->pictures) {
            uint8_t *rgba = picture_rgba(rect);
            if (rgba)
                e->pictures[e->picture_count++] = (struct sub_picture) { rect->x, rect->y, rect->w, rect->h, rgba };
        }
    }
    avsubtitle_free(&sub);
    // A text event with nothing in it says nothing; an empty picture event clears the screen.
    if (!d->bitmap && e->text_count == 0) {
        bsv_sub_event_free(e);
        return NULL;
    }
    return e;
}

// ---- Video decoding -------------------------------------------------------------------------

struct bsv_video {
    AVCodecContext *dec;
    AVCodecParameters *par; // what the decoder was made from, to make another
    AVRational stream_time_base;
    AVBufferRef *hw_device;
    enum AVPixelFormat hw_format;
    AVFrame *frame;
    double time_base;
    double origin;
    double next_pts;    // where a frame without a timestamp goes
    double skip_before; // after a seek: frames that end before this are not wanted
    struct bsv_media_info info;

    // While the hardware decodes, the packets it was given since the keyframe before the last
    // one are kept, so that software can take over from there if the hardware gives up.
    AVPacket **recent;
    int recent_len, recent_cap;
    int recent_key;    // where in `recent` the last keyframe is
    int replay;        // software is catching up: the next of `recent` to give it; else -1
    int end_waiting;   // the file ended while it was catching up
    double handed_out; // the time of the last frame handed out
    double not_again;  // frames up to this time were handed out by the hardware already
    int64_t fell_at;   // when the hardware gave up, on the system's clock
};

// The most packets kept: a stream that marks no keyframes would otherwise keep them all.
#define BSV_RECENT_MAX 4096

static enum AVPixelFormat pick_format(AVCodecContext *ctx, const enum AVPixelFormat *formats)
{
    bsv_video *v = ctx->opaque;
    for (const enum AVPixelFormat *p = formats; *p != AV_PIX_FMT_NONE; p++) {
        if (*p == v->hw_format)
            return *p;
    }
    // The hardware decoder does not take this stream: decode in software.
    v->info.hardware = 0;
    for (const enum AVPixelFormat *p = formats; *p != AV_PIX_FMT_NONE; p++) {
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(*p);
        if (desc && !(desc->flags & AV_PIX_FMT_FLAG_HWACCEL))
            return *p;
    }
    return AV_PIX_FMT_NONE;
}

static void enable_hardware(bsv_video *v, const AVCodec *codec)
{
    for (int i = 0;; i++) {
        const AVCodecHWConfig *config = avcodec_get_hw_config(codec, i);
        if (!config)
            return;
        if (!(config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX) || config->device_type != BSV_HW_TYPE)
            continue;
        if (av_hwdevice_ctx_create(&v->hw_device, BSV_HW_TYPE, NULL, NULL, 0) < 0)
            return;
        v->dec->hw_device_ctx = av_buffer_ref(v->hw_device);
        v->dec->get_format = pick_format;
        v->hw_format = config->pix_fmt;
        v->info.hardware = 1;
        return;
    }
}

static void fill_info(bsv_video *v, const bsv_source *s)
{
    struct bsv_media_info *info = &v->info;
    const AVCodecParameters *par = s->video->codecpar;
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(par->format);

    info->width = par->width;
    info->height = par->height;
    info->bit_depth = desc ? desc->comp[0].depth : 8;
    AVRational rate = av_guess_frame_rate(s->fmt, s->video, NULL);
    info->fps = rate.num > 0 && rate.den > 0 ? av_q2d(rate) : 0;
    info->duration = s->fmt->duration != AV_NOPTS_VALUE ? s->fmt->duration / (double) AV_TIME_BASE : 0;
    snprintf(info->codec, sizeof(info->codec), "%s", avcodec_get_name(par->codec_id));
    snprintf(info->pixel_format, sizeof(info->pixel_format), "%s", desc ? desc->name : "unknown");
    info->primaries = par->color_primaries;
    info->transfer = par->color_trc;
    info->matrix = par->color_space;
    info->range = par->color_range;

    const AVPacketSideData *sd;
    sd = av_packet_side_data_get(par->coded_side_data, par->nb_coded_side_data,
                                 AV_PKT_DATA_MASTERING_DISPLAY_METADATA);
    if (sd) {
        const AVMasteringDisplayMetadata *m = (const AVMasteringDisplayMetadata *) sd->data;
        if (m->has_luminance) {
            info->mastering_min = (float) av_q2d(m->min_luminance);
            info->mastering_max = (float) av_q2d(m->max_luminance);
        }
    }
    sd = av_packet_side_data_get(par->coded_side_data, par->nb_coded_side_data,
                                 AV_PKT_DATA_CONTENT_LIGHT_LEVEL);
    if (sd) {
        const AVContentLightMetadata *c = (const AVContentLightMetadata *) sd->data;
        info->max_cll = (float) c->MaxCLL;
        info->max_fall = (float) c->MaxFALL;
    }
    sd = av_packet_side_data_get(par->coded_side_data, par->nb_coded_side_data, AV_PKT_DATA_DOVI_CONF);
    if (sd) {
        const AVDOVIDecoderConfigurationRecord *dv = (const AVDOVIDecoderConfigurationRecord *) sd->data;
        info->dv_profile = dv->dv_profile;
        info->dv_level = dv->dv_level;
        info->dv_bl_compatibility = dv->dv_bl_signal_compatibility_id;
        info->dv_el_present = dv->el_present_flag;
    }
}

static void forget_recent(bsv_video *v)
{
    for (int i = 0; i < v->recent_len; i++)
        av_packet_free(&v->recent[i]);
    v->recent_len = 0;
    v->recent_key = 0;
    v->replay = -1;
    v->end_waiting = 0;
}

void bsv_video_close(bsv_video *v)
{
    if (!v)
        return;
    forget_recent(v);
    av_free(v->recent);
    av_frame_free(&v->frame);
    avcodec_free_context(&v->dec);
    avcodec_parameters_free(&v->par);
    av_buffer_unref(&v->hw_device);
    av_free(v);
}

// Make the decoder, on the hardware or not. Returns what went wrong in words, or NULL.
static const char *start_decoder(bsv_video *v, int hardware, int *code)
{
    const AVCodec *codec = avcodec_find_decoder(v->par->codec_id);
    *code = 0;
    if (!codec)
        return "no decoder for this video in this build";
    v->dec = avcodec_alloc_context3(codec);
    if (!v->dec)
        return "cannot set up the video decoder";
    if ((*code = avcodec_parameters_to_context(v->dec, v->par)) < 0)
        return "cannot set up the video decoder";
    v->dec->pkt_timebase = v->stream_time_base;
    v->dec->opaque = v;
    v->dec->thread_count = 0;
    // Film grain is synthesised on the GPU at output resolution rather than baked in by the decoder.
    v->dec->export_side_data |= AV_CODEC_EXPORT_DATA_FILM_GRAIN;
    v->info.hardware = 0;
    if (hardware)
        enable_hardware(v, codec);
    if ((*code = avcodec_open2(v->dec, codec, NULL)) < 0)
        return "cannot start the video decoder";
    return NULL;
}

bsv_video *bsv_video_open(const bsv_source *s, int hardware, char *error, int error_len)
{
    if (!s->video) {
        set_error(error, error_len, "the file has no video", 0);
        return NULL;
    }
    bsv_video *v = av_mallocz(sizeof(*v));
    if (!v)
        return NULL;
    v->hw_format = AV_PIX_FMT_NONE;
    v->stream_time_base = s->video->time_base;
    v->time_base = av_q2d(s->video->time_base);
    v->origin = s->origin;
    v->replay = -1;
    v->handed_out = v->not_again = -DBL_MAX;

    if (!(v->par = avcodec_parameters_alloc()) || avcodec_parameters_copy(v->par, s->video->codecpar) < 0) {
        set_error(error, error_len, "cannot set up the video decoder", 0);
        goto fail;
    }
    int code;
    const char *problem = start_decoder(v, hardware, &code);
    if (problem) {
        set_error(error, error_len, problem, code);
        goto fail;
    }
    if (!(v->frame = av_frame_alloc()))
        goto fail;
    fill_info(v, s);
    return v;

fail:
    bsv_video_close(v);
    return NULL;
}

void bsv_video_info(const bsv_video *v, struct bsv_media_info *out)
{
    *out = v->info;
}

void bsv_frame_free(void *avframe)
{
    AVFrame *frame = avframe;
    av_frame_free(&frame);
}

// Some files carry their HDR metadata only inside the video bitstream, not in the container.
// The first decoded frame then completes what `fill_info` could not find.
static void learn_from_frame(bsv_video *v, const AVFrame *frame)
{
    struct bsv_media_info *info = &v->info;
    // Colour tags inside the bitstream are what the decoder acts on; containers often omit them.
    if (frame->color_primaries != AVCOL_PRI_UNSPECIFIED)
        info->primaries = frame->color_primaries;
    if (frame->color_trc != AVCOL_TRC_UNSPECIFIED)
        info->transfer = frame->color_trc;
    if (frame->colorspace != AVCOL_SPC_UNSPECIFIED)
        info->matrix = frame->colorspace;
    if (frame->color_range != AVCOL_RANGE_UNSPECIFIED)
        info->range = frame->color_range;

    const AVFrameSideData *sd;
    if (info->mastering_max <= 0 &&
        (sd = av_frame_get_side_data(frame, AV_FRAME_DATA_MASTERING_DISPLAY_METADATA))) {
        const AVMasteringDisplayMetadata *m = (const AVMasteringDisplayMetadata *) sd->data;
        if (m->has_luminance) {
            info->mastering_min = (float) av_q2d(m->min_luminance);
            info->mastering_max = (float) av_q2d(m->max_luminance);
        }
    }
    if (info->max_cll <= 0 && (sd = av_frame_get_side_data(frame, AV_FRAME_DATA_CONTENT_LIGHT_LEVEL))) {
        const AVContentLightMetadata *c = (const AVContentLightMetadata *) sd->data;
        info->max_cll = (float) c->MaxCLL;
        info->max_fall = (float) c->MaxFALL;
    }
}

// Hand the decoded frame to the caller, copied out of the video decoder's memory if needed.
// Returns 1, or 3 when the frame lies before the seek target and was dropped without copying.
static int produce(bsv_video *v, struct bsv_frame *out)
{
    double duration = v->frame->duration > 0 ? v->frame->duration * v->time_base
                                             : v->info.fps > 0 ? 1.0 / v->info.fps : 0.04;
    double pts = v->frame->best_effort_timestamp != AV_NOPTS_VALUE
                     ? v->frame->best_effort_timestamp * v->time_base - v->origin
                     : v->next_pts;
    v->next_pts = pts + duration;
    if (pts + duration <= v->skip_before || pts <= v->not_again) {
        av_frame_unref(v->frame);
        return 3;
    }

    AVFrame *frame = av_frame_alloc();
    if (!frame)
        return AVERROR(ENOMEM);

    if (v->frame->hw_frames_ctx) {
        int ret = av_hwframe_transfer_data(frame, v->frame, 0);
        if (ret >= 0)
            ret = av_frame_copy_props(frame, v->frame);
        av_frame_unref(v->frame);
        if (ret < 0) {
            av_frame_free(&frame);
            return ret;
        }
    } else {
        av_frame_move_ref(frame, v->frame);
    }
    learn_from_frame(v, frame);

    out->avframe = frame;
    out->pts = pts;
    out->duration = duration;
    v->handed_out = pts;
    return 1;
}

// Add a packet to those kept. Takes the packet.
static void keep(bsv_video *v, AVPacket *pkt)
{
    if (v->recent_len == v->recent_cap) {
        int cap = v->recent_cap ? v->recent_cap * 2 : 256;
        AVPacket **grown = av_realloc_array(v->recent, cap, sizeof(*grown));
        if (!grown) {
            av_packet_free(&pkt);
            return;
        }
        v->recent = grown;
        v->recent_cap = cap;
    }
    v->recent[v->recent_len++] = pkt;
}

// Keep a packet the hardware is about to be given. A picture may lean on the pictures of the
// stretch before its own keyframe, so two stretches are kept: from the keyframe before last.
static void remember(bsv_video *v, const AVPacket *pkt)
{
    if (v->recent_len >= BSV_RECENT_MAX)
        forget_recent(v);
    if (pkt->flags & AV_PKT_FLAG_KEY) {
        int gone = v->recent_key;
        for (int i = 0; i < gone; i++)
            av_packet_free(&v->recent[i]);
        memmove(v->recent, v->recent + gone, (v->recent_len - gone) * sizeof(*v->recent));
        v->recent_len -= gone;
        v->recent_key = v->recent_len;
    }
    AVPacket *copy = av_packet_clone(pkt);
    if (copy)
        keep(v, copy);
}

// The hardware decoder has refused part of the stream. It does that for whole stretches a
// software decoder reads without complaint, and a film should not stop for it: decode the rest
// in software, starting again from the packets kept, and hand out only what was not handed out
// before. Returns 0 when software has taken over.
static int fall_back(bsv_video *v, int reason)
{
    char why[AV_ERROR_MAX_STRING_SIZE];
    av_strerror(reason, why, sizeof(why));
    if (v->handed_out > -DBL_MAX)
        av_log(NULL, AV_LOG_WARNING,
               "the hardware decoder gave up after the picture at %.3f s (%s): decoding in software from here\n",
               v->handed_out, why);
    else
        av_log(NULL, AV_LOG_WARNING,
               "the hardware decoder gave up before its first picture (%s): decoding in software\n", why);

    avcodec_free_context(&v->dec);
    av_buffer_unref(&v->hw_device);
    v->hw_format = AV_PIX_FMT_NONE;
    int code;
    if (start_decoder(v, 0, &code)) {
        // No decoder at all now: the calls below answer with an error from here on.
        avcodec_free_context(&v->dec);
        forget_recent(v);
        return -1;
    }
    v->not_again = v->handed_out;
    // From the keyframe before last, unless the last one has itself been handed out: then so
    // has every picture that leans on the stretch before it, and it is enough to start there.
    v->replay = 0;
    if (v->recent_key < v->recent_len) {
        const AVPacket *key = v->recent[v->recent_key];
        if ((key->flags & AV_PKT_FLAG_KEY) && key->pts != AV_NOPTS_VALUE &&
            key->pts * v->time_base - v->origin <= v->handed_out)
            v->replay = v->recent_key;
    }
    v->fell_at = av_gettime_relative();
    return 0;
}

int bsv_video_send(bsv_video *v, void *avpacket)
{
    AVPacket *pkt = avpacket;
    if (!v->dec) {
        av_packet_free(&pkt);
        return AVERROR_DECODER_NOT_FOUND;
    }
    if (v->replay >= 0) {
        // Software is still going through the packets kept; this one waits its turn behind them.
        if (pkt)
            keep(v, pkt);
        else
            v->end_waiting = 1;
        return 0;
    }
    if (v->info.hardware && pkt)
        remember(v, pkt);
    int ret = avcodec_send_packet(v->dec, pkt);
    av_packet_free(&pkt);
    // A damaged packet is skipped; the decoder recovers at the next keyframe.
    if (ret == AVERROR_INVALIDDATA || ret == AVERROR(EAGAIN) || ret == AVERROR_EOF)
        return 0;
    if (ret < 0 && v->info.hardware && fall_back(v, ret) == 0)
        return 0;
    return ret;
}

// While software catches up: give it the next packet kept. Returns 0 when there are no more.
static int replay_one(bsv_video *v)
{
    if (v->replay < v->recent_len) {
        if (avcodec_send_packet(v->dec, v->recent[v->replay]) != AVERROR(EAGAIN))
            v->replay++;
        return 1;
    }
    int ended = v->end_waiting;
    av_log(NULL, AV_LOG_INFO, "software caught up with the film %.0f ms after the hardware gave up\n",
           (av_gettime_relative() - v->fell_at) / 1000.0);
    forget_recent(v);
    if (ended)
        avcodec_send_packet(v->dec, NULL);
    return ended;
}

int bsv_video_receive(bsv_video *v, struct bsv_frame *out)
{
    if (!v->dec)
        return AVERROR_DECODER_NOT_FOUND;
    for (;;) {
        int ret = avcodec_receive_frame(v->dec, v->frame);
        if (ret == AVERROR(EAGAIN)) {
            if (v->replay >= 0 && replay_one(v))
                continue;
            return 0;
        }
        if (ret == AVERROR_EOF)
            return 2;
        if (ret < 0) {
            if (v->info.hardware && fall_back(v, ret) == 0)
                continue;
            return ret;
        }
        ret = produce(v, out);
        if (ret != 3)
            return ret;
    }
}

int bsv_video_is_hardware(const bsv_video *v)
{
    return v->info.hardware;
}

void bsv_video_flush(bsv_video *v, double seconds)
{
    if (v->dec)
        avcodec_flush_buffers(v->dec);
    forget_recent(v);
    v->handed_out = v->not_again = -DBL_MAX;
    v->next_pts = seconds;
    v->skip_before = seconds;
}

// ---- Sound decoding -------------------------------------------------------------------------

struct bsv_audio {
    AVCodecContext *dec;
    SwrContext *swr;
    AVFrame *frame;
    double time_base;
    double origin;
    int out_rate;
    AVChannelLayout out_layout;

    // What the converter is currently set up for.
    AVChannelLayout in_layout;
    enum AVSampleFormat in_format;
    int in_rate;

    float *buffer;
    int buffer_frames;
    double next_pts;
    int draining;
};

void bsv_audio_close(bsv_audio *a)
{
    if (!a)
        return;
    av_frame_free(&a->frame);
    swr_free(&a->swr);
    avcodec_free_context(&a->dec);
    av_channel_layout_uninit(&a->in_layout);
    av_channel_layout_uninit(&a->out_layout);
    av_free(a->buffer);
    av_free(a);
}

bsv_audio *bsv_audio_open(const bsv_source *s, int out_rate, int out_channels, char *error, int error_len)
{
    if (!s->audio) {
        set_error(error, error_len, "the file has no sound", 0);
        return NULL;
    }
    bsv_audio *a = av_mallocz(sizeof(*a));
    if (!a)
        return NULL;
    a->time_base = av_q2d(s->audio->time_base);
    a->origin = s->origin;
    a->out_rate = out_rate;
    a->in_format = AV_SAMPLE_FMT_NONE;
    av_channel_layout_default(&a->out_layout, out_channels);

    int ret = 0;
    a->dec = avcodec_alloc_context3(s->audio_codec);
    if (!a->dec || (ret = avcodec_parameters_to_context(a->dec, s->audio->codecpar)) < 0) {
        set_error(error, error_len, "cannot set up the sound decoder", ret);
        goto fail;
    }
    a->dec->pkt_timebase = s->audio->time_base;
    if ((ret = avcodec_open2(a->dec, s->audio_codec, NULL)) < 0) {
        set_error(error, error_len, "cannot start the sound decoder", ret);
        goto fail;
    }
    if (!(a->frame = av_frame_alloc()))
        goto fail;
    return a;

fail:
    bsv_audio_close(a);
    return NULL;
}

int bsv_audio_send(bsv_audio *a, void *avpacket)
{
    AVPacket *pkt = avpacket;
    if (!pkt)
        a->draining = 1;
    int ret = avcodec_send_packet(a->dec, pkt);
    av_packet_free(&pkt);
    if (ret == AVERROR_INVALIDDATA || ret == AVERROR(EAGAIN) || ret == AVERROR_EOF)
        return 0;
    return ret;
}

// (Re)build the converter when the decoder's output format is new or has changed mid-stream.
static int prepare_converter(bsv_audio *a, const AVFrame *frame)
{
    AVChannelLayout layout = { 0 };
    if (frame->ch_layout.order == AV_CHANNEL_ORDER_UNSPEC)
        av_channel_layout_default(&layout, frame->ch_layout.nb_channels);
    else
        av_channel_layout_copy(&layout, &frame->ch_layout);

    if (a->swr && a->in_format == frame->format && a->in_rate == frame->sample_rate &&
        av_channel_layout_compare(&a->in_layout, &layout) == 0) {
        av_channel_layout_uninit(&layout);
        return 0;
    }
    swr_free(&a->swr);
    int ret = swr_alloc_set_opts2(&a->swr, &a->out_layout, AV_SAMPLE_FMT_FLT, a->out_rate,
                                  &layout, frame->format, frame->sample_rate, 0, NULL);
    if (ret >= 0) {
        // When channels are folded down, scale so the sum cannot clip.
        av_opt_set_double(a->swr, "rematrix_maxval", 1.0, 0);
        ret = swr_init(a->swr);
    }
    av_channel_layout_uninit(&a->in_layout);
    a->in_layout = layout;
    a->in_format = frame->format;
    a->in_rate = frame->sample_rate;
    return ret;
}

static int ensure_buffer(bsv_audio *a, int frames)
{
    if (frames <= a->buffer_frames)
        return 0;
    av_free(a->buffer);
    a->buffer = av_malloc_array((size_t) frames * a->out_layout.nb_channels, sizeof(float));
    a->buffer_frames = a->buffer ? frames : 0;
    return a->buffer ? 0 : AVERROR(ENOMEM);
}

int bsv_audio_receive(bsv_audio *a, const float **samples, int *frames, double *pts)
{
    int ret = avcodec_receive_frame(a->dec, a->frame);
    if (ret == AVERROR(EAGAIN))
        return 0;
    if (ret == AVERROR_EOF) {
        // The converter may still hold a tail of samples.
        if (!a->swr || !a->draining)
            return 2;
        a->draining = 0;
        int left = (int) swr_get_delay(a->swr, a->out_rate) + 32;
        if (ensure_buffer(a, left) < 0)
            return AVERROR(ENOMEM);
        uint8_t *out = (uint8_t *) a->buffer;
        int n = swr_convert(a->swr, &out, left, NULL, 0);
        if (n <= 0)
            return 2;
        *samples = a->buffer;
        *frames = n;
        *pts = a->next_pts;
        a->next_pts += n / (double) a->out_rate;
        return 1;
    }
    if (ret < 0)
        return ret;

    if ((ret = prepare_converter(a, a->frame)) < 0) {
        av_frame_unref(a->frame);
        return ret;
    }
    // Samples still inside the converter belong before this frame.
    double delay = swr_get_delay(a->swr, a->in_rate) / (double) a->in_rate;
    double frame_pts = a->frame->best_effort_timestamp != AV_NOPTS_VALUE
                           ? a->frame->best_effort_timestamp * a->time_base - a->origin - delay
                           : a->next_pts;

    int capacity = swr_get_out_samples(a->swr, a->frame->nb_samples);
    if (ensure_buffer(a, capacity) < 0) {
        av_frame_unref(a->frame);
        return AVERROR(ENOMEM);
    }
    uint8_t *out = (uint8_t *) a->buffer;
    int n = swr_convert(a->swr, &out, capacity, (const uint8_t **) a->frame->extended_data, a->frame->nb_samples);
    av_frame_unref(a->frame);
    if (n < 0)
        return n;

    *samples = a->buffer;
    *frames = n;
    *pts = frame_pts;
    a->next_pts = frame_pts + n / (double) a->out_rate;
    return 1;
}

void bsv_audio_flush(bsv_audio *a)
{
    avcodec_flush_buffers(a->dec);
    a->draining = 0;
    // Dropping the converter also drops the samples it was holding from before the seek.
    swr_free(&a->swr);
    a->in_format = AV_SAMPLE_FMT_NONE;
}

// ---- Rendering ------------------------------------------------------------------------------

#define BSV_SUB_EVENTS 32

struct bsv_gpu {
    pl_log log;
    // The graphics API underneath: Direct3D 11 on Windows, Vulkan elsewhere (over Metal on the
    // Mac). Everything after `gpu` is the same for both.
#if defined(_WIN32)
    pl_d3d11 d3d;
    IDXGISwapChain1 *composition; // our own swapchain, when drawing into an application's panel
#else
    pl_vk_inst inst;
    VkSurfaceKHR surface;
    pl_vulkan vk;
#endif
    pl_gpu gpu;
    pl_swapchain sw;
    pl_renderer rr;
    pl_queue queue;
    pl_options opts;
    bool ignore_dovi; // show a Dolby Vision file's base layer as it is, without its RPU

    // Subtitles. Text is drawn by libass; pictures come ready-made from the decoder.
    int subs_kind; // 0 off, 1 text, 2 pictures
    int subs_video_w, subs_video_h;
    ASS_Library *ass;
    ASS_Renderer *ass_renderer;
    ASS_Track *ass_track;
    bsv_sub_event *sub_events[BSV_SUB_EVENTS]; // picture events, in the order they start
    int sub_event_count;
    const bsv_sub_event *sub_shown; // the picture event now in the texture
    pl_tex sub_tex;
    uint8_t *sub_buffer;
    size_t sub_buffer_size;
    int sub_x, sub_y, sub_w, sub_h; // where the texture's contents go
    int sub_frame_w, sub_frame_h;   // the size text was last laid out for
    bool sub_ready;
    float sub_lift; // pixels at the bottom of the window that subtitles keep clear of
    float sub_scale; // text size, 1 = as styled
    float sub_nits;  // how bright subtitle white is, cd/m²; 0 = SDR white
};

// ---- Subtitles on the GPU's side -------------------------------------------------------------

// Plain-text subtitles (SRT, WebVTT, MP4 text) carry no styling of their own. This is theirs:
// laid out for a 1080-line picture and scaled with it, slightly off-white so a line of text is
// not the brightest thing in a dark scene.
// A plain, well-made face that every installation of the system has.
#if defined(_WIN32)
#define BSV_SUBTITLE_FONT "Segoe UI"
#else
#define BSV_SUBTITLE_FONT "Helvetica Neue"
#endif

static const char DEFAULT_ASS_HEADER[] =
    "[Script Info]\n"
    "ScriptType: v4.00+\n"
    "PlayResX: 1920\n"
    "PlayResY: 1080\n"
    "WrapStyle: 0\n"
    "ScaledBorderAndShadow: yes\n"
    "\n"
    "[V4+ Styles]\n"
    "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, "
    "Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, "
    "Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n"
    "Style: Default," BSV_SUBTITLE_FONT ",60,&H00E6E6E6,&H00E6E6E6,&H00101010,&H78000000,"
    "0,0,0,0,100,100,0,0,1,2.6,1.2,2,140,140,62,1\n"
    "\n"
    "[Events]\n"
    "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

static void quiet_ass(int level, const char *fmt, va_list args, void *data)
{
    if (level > 2 || !g_log_fn)
        return;
    char buf[512];
    vsnprintf(buf, sizeof(buf), fmt, args);
    emit(2, buf);
}

static bool ensure_ass(bsv_gpu *g)
{
    if (g->ass)
        return true;
    g->ass = ass_library_init();
    if (!g->ass)
        return false;
    ass_set_message_cb(g->ass, quiet_ass, NULL);
    ass_set_extract_fonts(g->ass, 1);
    return true;
}

static void subs_drop_events(bsv_gpu *g)
{
    for (int i = 0; i < g->sub_event_count; i++)
        bsv_sub_event_free(g->sub_events[i]);
    g->sub_event_count = 0;
    g->sub_shown = NULL;
    g->sub_ready = false;
}

static void subs_free(bsv_gpu *g)
{
    subs_drop_events(g);
    if (g->ass_track)
        ass_free_track(g->ass_track);
    if (g->ass_renderer)
        ass_renderer_done(g->ass_renderer);
    if (g->ass)
        ass_library_done(g->ass);
    g->ass_track = NULL;
    g->ass_renderer = NULL;
    g->ass = NULL;
    av_freep(&g->sub_buffer);
    g->sub_buffer_size = 0;
}

void bsv_gpu_subs_add_font(bsv_gpu *g, const char *name, const void *data, int len)
{
    if (ensure_ass(g))
        ass_add_font(g->ass, name, data, len);
}

void bsv_gpu_subs_begin(bsv_gpu *g, int kind, const char *header, int header_len, int video_w, int video_h)
{
    subs_drop_events(g);
    if (g->ass_track) {
        ass_free_track(g->ass_track);
        g->ass_track = NULL;
    }
    g->subs_kind = kind;
    g->subs_video_w = video_w;
    g->subs_video_h = video_h;
    if (kind != 1 || !ensure_ass(g))
        return;
    if (!g->ass_renderer) {
        g->ass_renderer = ass_renderer_init(g->ass);
        if (!g->ass_renderer) {
            g->subs_kind = 0;
            return;
        }
        ass_set_fonts(g->ass_renderer, NULL, BSV_SUBTITLE_FONT, ASS_FONTPROVIDER_AUTODETECT, NULL, 1);
    }
    g->ass_track = ass_new_track(g->ass);
    if (!g->ass_track) {
        g->subs_kind = 0;
        return;
    }
    if (header && header_len > 0)
        ass_process_codec_private(g->ass_track, header, header_len);
    else
        ass_process_codec_private(g->ass_track, DEFAULT_ASS_HEADER, (int) strlen(DEFAULT_ASS_HEADER));
    // The same line can arrive twice after a seek; libass then keeps one.
    ass_set_check_readorder(g->ass_track, 1);
}

void bsv_gpu_subs_set_look(bsv_gpu *g, float scale, float nits)
{
    g->sub_scale = scale;
    g->sub_nits = nits;
}

void bsv_gpu_subs_set_lift(bsv_gpu *g, float pixels)
{
    g->sub_lift = pixels > 0 ? pixels : 0;
}

void bsv_gpu_subs_clear(bsv_gpu *g)
{
    subs_drop_events(g);
    if (g->ass_track)
        ass_flush_events(g->ass_track);
}

void bsv_gpu_subs_push(bsv_gpu *g, bsv_sub_event *e)
{
    if (g->subs_kind == 1 && g->ass_track) {
        long long start = llround(e->start * 1000);
        long long duration = llround(e->duration * 1000);
        for (int i = 0; i < e->text_count; i++)
            ass_process_chunk(g->ass_track, e->texts[i], (int) strlen(e->texts[i]), start, duration);
        g->sub_ready = false;
        bsv_sub_event_free(e);
    } else if (g->subs_kind == 2) {
        if (g->sub_event_count == BSV_SUB_EVENTS) {
            if (g->sub_shown == g->sub_events[0])
                g->sub_shown = NULL;
            bsv_sub_event_free(g->sub_events[0]);
            memmove(g->sub_events, g->sub_events + 1, (BSV_SUB_EVENTS - 1) * sizeof(g->sub_events[0]));
            g->sub_event_count--;
        }
        g->sub_events[g->sub_event_count++] = e;
    } else {
        bsv_sub_event_free(e);
    }
}

static bool subs_buffer(bsv_gpu *g, int w, int h)
{
    size_t need = (size_t) w * h * 4;
    if (need > g->sub_buffer_size) {
        av_freep(&g->sub_buffer);
        g->sub_buffer = av_malloc(need);
        g->sub_buffer_size = g->sub_buffer ? need : 0;
    }
    if (!g->sub_buffer)
        return false;
    memset(g->sub_buffer, 0, need);
    g->sub_w = w;
    g->sub_h = h;
    return true;
}

static bool subs_upload(bsv_gpu *g)
{
    pl_gpu gpu = g->gpu;
    pl_fmt fmt = pl_find_named_fmt(gpu, "rgba8");
    if (!fmt)
        return false;
    bool ok = pl_tex_recreate(gpu, &g->sub_tex, pl_tex_params(
        .w = g->sub_w,
        .h = g->sub_h,
        .format = fmt,
        .sampleable = true,
        .host_writable = true,
    ));
    return ok && pl_tex_upload(gpu, pl_tex_transfer_params(
        .tex = g->sub_tex,
        .ptr = g->sub_buffer,
        .row_pitch = (size_t) g->sub_w * 4,
    ));
}

// Blend libass's list of single-colour alpha bitmaps into one premultiplied RGBA picture.
static bool subs_draw_text(bsv_gpu *g, const ASS_Image *images)
{
    int x0 = INT_MAX, y0 = INT_MAX, x1 = INT_MIN, y1 = INT_MIN;
    for (const ASS_Image *im = images; im; im = im->next) {
        if (im->w <= 0 || im->h <= 0)
            continue;
        x0 = FFMIN(x0, im->dst_x);
        y0 = FFMIN(y0, im->dst_y);
        x1 = FFMAX(x1, im->dst_x + im->w);
        y1 = FFMAX(y1, im->dst_y + im->h);
    }
    if (x1 <= x0 || y1 <= y0 || !subs_buffer(g, x1 - x0, y1 - y0))
        return false;
    g->sub_x = x0;
    g->sub_y = y0;

    for (const ASS_Image *im = images; im; im = im->next) {
        if (im->w <= 0 || im->h <= 0)
            continue;
        unsigned r = im->color >> 24, gr = (im->color >> 16) & 0xFF, b = (im->color >> 8) & 0xFF;
        unsigned opacity = 255 - (im->color & 0xFF);
        for (int y = 0; y < im->h; y++) {
            const uint8_t *src = im->bitmap + (size_t) y * im->stride;
            uint8_t *dst = g->sub_buffer + ((size_t) (im->dst_y - y0 + y) * g->sub_w + (im->dst_x - x0)) * 4;
            for (int x = 0; x < im->w; x++, dst += 4) {
                unsigned a = src[x] * opacity / 255;
                if (!a)
                    continue;
                unsigned keep = 255 - a;
                dst[0] = (uint8_t) ((r * a + dst[0] * keep) / 255);
                dst[1] = (uint8_t) ((gr * a + dst[1] * keep) / 255);
                dst[2] = (uint8_t) ((b * a + dst[2] * keep) / 255);
                dst[3] = (uint8_t) (a + dst[3] * keep / 255);
            }
        }
    }
    return subs_upload(g);
}

static bool subs_draw_pictures(bsv_gpu *g, const bsv_sub_event *e)
{
    int x0 = INT_MAX, y0 = INT_MAX, x1 = INT_MIN, y1 = INT_MIN;
    for (int i = 0; i < e->picture_count; i++) {
        const struct sub_picture *p = &e->pictures[i];
        x0 = FFMIN(x0, p->x);
        y0 = FFMIN(y0, p->y);
        x1 = FFMAX(x1, p->x + p->w);
        y1 = FFMAX(y1, p->y + p->h);
    }
    if (x1 <= x0 || y1 <= y0 || !subs_buffer(g, x1 - x0, y1 - y0))
        return false;
    g->sub_x = x0;
    g->sub_y = y0;
    for (int i = 0; i < e->picture_count; i++) {
        const struct sub_picture *p = &e->pictures[i];
        for (int y = 0; y < p->h; y++) {
            memcpy(g->sub_buffer + ((size_t) (p->y - y0 + y) * g->sub_w + (p->x - x0)) * 4,
                   p->rgba + (size_t) y * p->w * 4, (size_t) p->w * 4);
        }
    }
    return subs_upload(g);
}

// Fill in the overlay for time `pts`, if a subtitle is due. `out` is the frame being drawn to.
static bool subs_prepare(bsv_gpu *g, double pts, const struct pl_frame *out, struct pl_overlay *overlay,
                         struct pl_overlay_part *part)
{
    float left = fminf(out->crop.x0, out->crop.x1), top = fminf(out->crop.y0, out->crop.y1);
    float width = fabsf(out->crop.x1 - out->crop.x0), height = fabsf(out->crop.y1 - out->crop.y0);
    if (width < 16 || height < 16)
        return false;
    float scale = 1;

    if (g->subs_kind == 1 && g->ass_track && g->ass_renderer) {
        // Text is laid out at the size the picture has on screen, so it is as sharp as the display.
        int w = (int) lroundf(width), h = (int) lroundf(height);
        ass_set_frame_size(g->ass_renderer, w, h);
        ass_set_storage_size(g->ass_renderer, g->subs_video_w, g->subs_video_h);
        ass_set_font_scale(g->ass_renderer, g->sub_scale > 0 ? g->sub_scale : 1.0);
        int changed = 0;
        ASS_Image *images = ass_render_frame(g->ass_renderer, g->ass_track, llround(pts * 1000), &changed);
        if (!images) {
            g->sub_ready = false;
            return false;
        }
        if (changed || !g->sub_ready || w != g->sub_frame_w || h != g->sub_frame_h) {
            g->sub_frame_w = w;
            g->sub_frame_h = h;
            g->sub_ready = subs_draw_text(g, images);
        }
    } else if (g->subs_kind == 2) {
        // The event on screen is the last one that has started.
        while (g->sub_event_count >= 2 && g->sub_events[1]->start <= pts) {
            if (g->sub_shown == g->sub_events[0])
                g->sub_shown = NULL;
            bsv_sub_event_free(g->sub_events[0]);
            memmove(g->sub_events, g->sub_events + 1, (g->sub_event_count - 1) * sizeof(g->sub_events[0]));
            g->sub_event_count--;
        }
        const bsv_sub_event *e = g->sub_event_count ? g->sub_events[0] : NULL;
        if (!e || e->start > pts || e->picture_count == 0 || (e->duration > 0 && pts > e->start + e->duration))
            return false;
        if (e != g->sub_shown) {
            g->sub_ready = subs_draw_pictures(g, e);
            g->sub_shown = e;
        }
        // Picture subtitles are drawn for their own canvas, usually the full 16:9 frame even
        // when the video has had its black bars cut off: fit by width, centre vertically.
        if (e->canvas_w > 0 && e->canvas_h > 0) {
            scale = width / e->canvas_w;
            top += (height - e->canvas_h * scale) / 2;
        }
    } else {
        return false;
    }
    if (!g->sub_ready || !g->sub_tex)
        return false;

    float x = left + g->sub_x * scale, y = top + g->sub_y * scale;
    float w = g->sub_w * scale, h = g->sub_h * scale;
    // Never below the bottom of the window, nor under the controls while they are showing.
    float bottom = (float) out->planes[0].texture->params.h - g->sub_lift;
    if (y + h > bottom)
        y = fmaxf(bottom - h, 0);
    *part = (struct pl_overlay_part) {
        .src = { 0, 0, (float) g->sub_w, (float) g->sub_h },
        .dst = { x, y, x + w, y + h },
    };
    *overlay = (struct pl_overlay) {
        .tex = g->sub_tex,
        .mode = PL_OVERLAY_NORMAL,
        .coords = PL_OVERLAY_COORDS_DST_FRAME,
        .repr = pl_color_repr_rgb,
        // Tagged sRGB: on an HDR surface libplacebo then places subtitle white at SDR white,
        // not at the display's peak.
        .color = pl_color_space_srgb,
        .parts = part,
        .num_parts = 1,
    };
    overlay->repr.alpha = PL_ALPHA_PREMULTIPLIED;
    // A different white level for the subtitles alone, leaving the picture untouched.
    if (g->sub_nits > 0)
        overlay->color.hdr.max_luma = g->sub_nits;
    return true;
}

void bsv_gpu_destroy(bsv_gpu *g)
{
    if (!g)
        return;
    subs_free(g);
    if (g->gpu)
        pl_tex_destroy(g->gpu, &g->sub_tex);
    pl_queue_destroy(&g->queue);
    pl_renderer_destroy(&g->rr);
    pl_options_free(&g->opts);
    pl_swapchain_destroy(&g->sw);
#if defined(_WIN32)
    if (g->composition)
        IDXGISwapChain1_Release(g->composition);
    pl_d3d11_destroy(&g->d3d);
#else
    pl_vulkan_destroy(&g->vk);
    if (g->surface && g->inst) {
        PFN_vkDestroySurfaceKHR destroy =
            (PFN_vkDestroySurfaceKHR) g->inst->get_proc_addr(g->inst->instance, "vkDestroySurfaceKHR");
        if (destroy)
            destroy(g->inst->instance, g->surface, NULL);
    }
    pl_vk_inst_destroy(&g->inst);
#endif
    pl_log_destroy(&g->log);
    free(g);
}

static bsv_gpu *new_gpu(void)
{
    bsv_gpu *g = calloc(1, sizeof(*g));
    if (g) {
        g->log = pl_log_create(PL_API_VER, pl_log_params(
            .log_cb = log_from_placebo,
            .log_level = placebo_level(),
        ));
    }
    return g;
}

// Everything after the device and its swapchain exist: the same on every system.
static bsv_gpu *finish_gpu(bsv_gpu *g, const char *api, char *error, int error_len)
{
    if (!g->sw) {
        set_error(error, error_len, "cannot create the swapchain", 0);
        goto fail;
    }
    g->rr = pl_renderer_create(g->log, g->gpu);
    g->queue = pl_queue_create(g->gpu);
    g->opts = pl_options_alloc(g->log);
    if (!g->rr || !g->queue || !g->opts) {
        set_error(error, error_len, "cannot create the renderer", 0);
        goto fail;
    }
    pl_options_reset(g->opts, &pl_render_high_quality_params);
    emitf(3, "renderer: libplacebo %s on %s%s", pl_version(), api, g->gpu->glsl.compute ? " (compute)" : "");
    return g;

fail:
    bsv_gpu_destroy(g);
    return NULL;
}

#if defined(__APPLE__)
bsv_gpu *bsv_gpu_create_metal(void *layer, char *error, int error_len)
{
    bsv_gpu *g = new_gpu();
    if (!g)
        return NULL;

    const char *extensions[] = { VK_KHR_SURFACE_EXTENSION_NAME, VK_EXT_METAL_SURFACE_EXTENSION_NAME };
    g->inst = pl_vk_inst_create(g->log, pl_vk_inst_params(
        .extensions = extensions,
        .num_extensions = 2,
    ));
    if (!g->inst) {
        set_error(error, error_len, "cannot start Vulkan (is MoltenVK installed?)", 0);
        bsv_gpu_destroy(g);
        return NULL;
    }
    PFN_vkCreateMetalSurfaceEXT create =
        (PFN_vkCreateMetalSurfaceEXT) g->inst->get_proc_addr(g->inst->instance, "vkCreateMetalSurfaceEXT");
    VkMetalSurfaceCreateInfoEXT info = {
        .sType = VK_STRUCTURE_TYPE_METAL_SURFACE_CREATE_INFO_EXT,
        .pLayer = layer,
    };
    if (!create || create(g->inst->instance, &info, NULL, &g->surface) != VK_SUCCESS) {
        set_error(error, error_len, "cannot attach Vulkan to the window", 0);
        bsv_gpu_destroy(g);
        return NULL;
    }
    g->vk = pl_vulkan_create(g->log, pl_vulkan_params(
        .instance = g->inst->instance,
        .get_proc_addr = g->inst->get_proc_addr,
        .surface = g->surface,
    ));
    if (!g->vk) {
        set_error(error, error_len, "no usable Vulkan device", 0);
        bsv_gpu_destroy(g);
        return NULL;
    }
    g->gpu = g->vk->gpu;
    g->sw = pl_vulkan_create_swapchain(g->vk, pl_vulkan_swapchain_params(
        .surface = g->surface,
        .present_mode = VK_PRESENT_MODE_FIFO_KHR,
    ));
    return finish_gpu(g, "Vulkan", error, error_len);
}
#endif

#if defined(_WIN32)
// Direct3D 11 is the dependable way to an HDR surface on Windows: the swapchain is told the
// colour space (BT.2020 PQ) and the HDR10 metadata, and the system's compositor takes it from
// there. A machine without a GPU gets the software device, so the player still starts.
bsv_gpu *bsv_gpu_create_win32(void *hwnd, char *error, int error_len)
{
    bsv_gpu *g = new_gpu();
    if (!g)
        return NULL;
    g->d3d = pl_d3d11_create(g->log, pl_d3d11_params(
        .allow_software = true,
    ));
    if (!g->d3d) {
        set_error(error, error_len, "cannot start Direct3D 11", 0);
        bsv_gpu_destroy(g);
        return NULL;
    }
    g->gpu = g->d3d->gpu;
    g->sw = pl_d3d11_create_swapchain(g->d3d, pl_d3d11_swapchain_params(
        .window = (HWND) hwnd,
    ));
    return finish_gpu(g, g->d3d->software ? "Direct3D 11 (software)" : "Direct3D 11", error, error_len);
}

// For an application that composes its own window (WinUI): a swapchain that belongs to no
// window. The application puts it into one of its panels and draws its controls above it;
// we render into it exactly as into a window's. `*swapchain` receives an IDXGISwapChain1*,
// owned by the gpu: the application takes its own reference when it adopts it.
bsv_gpu *bsv_gpu_create_composition(int width, int height, void **swapchain, char *error, int error_len)
{
    bsv_gpu *g = new_gpu();
    if (!g)
        return NULL;
    g->d3d = pl_d3d11_create(g->log, pl_d3d11_params(
        .allow_software = true,
    ));
    if (!g->d3d) {
        set_error(error, error_len, "cannot start Direct3D 11", 0);
        bsv_gpu_destroy(g);
        return NULL;
    }
    g->gpu = g->d3d->gpu;

    IDXGIDevice *device = NULL;
    IDXGIAdapter *adapter = NULL;
    IDXGIFactory2 *factory = NULL;
    HRESULT hr = ID3D11Device_QueryInterface(g->d3d->device, &IID_IDXGIDevice, (void **) &device);
    if (SUCCEEDED(hr))
        hr = IDXGIDevice_GetAdapter(device, &adapter);
    if (SUCCEEDED(hr))
        hr = IDXGIAdapter_GetParent(adapter, &IID_IDXGIFactory2, (void **) &factory);
    if (SUCCEEDED(hr)) {
        DXGI_SWAP_CHAIN_DESC1 desc = {
            .Width = width > 8 ? (UINT) width : 8,
            .Height = height > 8 ? (UINT) height : 8,
            // Ten bits: enough for HDR10, and for SDR without banding.
            .Format = DXGI_FORMAT_R10G10B10A2_UNORM,
            .SampleDesc = { .Count = 1 },
            // What libplacebo asks of the swapchains it makes itself. Without the last of
            // these it cannot clear the bars beside a picture narrower than the panel.
            .BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT | DXGI_USAGE_SHADER_INPUT | DXGI_USAGE_UNORDERED_ACCESS,
            .BufferCount = 2,
            .Scaling = DXGI_SCALING_STRETCH,
            .SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD,
            .AlphaMode = DXGI_ALPHA_MODE_IGNORE,
        };
        if (ID3D11Device_GetFeatureLevel(g->d3d->device) < D3D_FEATURE_LEVEL_11_0)
            desc.BufferUsage &= ~DXGI_USAGE_UNORDERED_ACCESS;
        hr = IDXGIFactory2_CreateSwapChainForComposition(factory, (IUnknown *) g->d3d->device, &desc, NULL,
                                                         &g->composition);
        if (FAILED(hr) && (desc.BufferUsage & DXGI_USAGE_UNORDERED_ACCESS)) {
            desc.BufferUsage &= ~DXGI_USAGE_UNORDERED_ACCESS;
            hr = IDXGIFactory2_CreateSwapChainForComposition(factory, (IUnknown *) g->d3d->device, &desc, NULL,
                                                             &g->composition);
        }
    }
    if (factory)
        IDXGIFactory2_Release(factory);
    if (adapter)
        IDXGIAdapter_Release(adapter);
    if (device)
        IDXGIDevice_Release(device);
    if (FAILED(hr) || !g->composition) {
        set_error(error, error_len, "cannot create a swapchain for the application's panel", 0);
        bsv_gpu_destroy(g);
        return NULL;
    }
    g->sw = pl_d3d11_create_swapchain(g->d3d, pl_d3d11_swapchain_params(
        .swapchain = (IDXGISwapChain *) g->composition,
    ));
    *swapchain = g->composition;
    return finish_gpu(g, g->d3d->software ? "Direct3D 11 (software)" : "Direct3D 11", error, error_len);
}

// A panel lays its swapchain out in its own units; tell it how many pixels one unit is, so a
// swapchain sized in pixels is shown one to one on a scaled display.
void bsv_gpu_set_scale(bsv_gpu *g, float scale)
{
    if (!g->composition || scale <= 0)
        return;
    IDXGISwapChain2 *sc = NULL;
    if (SUCCEEDED(IDXGISwapChain1_QueryInterface(g->composition, &IID_IDXGISwapChain2, (void **) &sc))) {
        DXGI_MATRIX_3X2_F inverse = { ._11 = 1.0f / scale, ._22 = 1.0f / scale };
        IDXGISwapChain2_SetMatrixTransform(sc, &inverse);
        IDXGISwapChain2_Release(sc);
    }
}
#endif

void bsv_gpu_resize(bsv_gpu *g, int width, int height)
{
    pl_swapchain_resize(g->sw, &width, &height);
}

int bsv_gpu_set_options(bsv_gpu *g, int preset, const char *options)
{
    const struct pl_render_params *base = preset == 0   ? &pl_render_fast_params
                                          : preset == 2 ? &pl_render_high_quality_params
                                                        : &pl_render_default_params;
    pl_options_reset(g->opts, base);
    if (options && *options && !pl_options_load(g->opts, options))
        return -1;
    return 0;
}

void bsv_gpu_set_dolby_vision(bsv_gpu *g, int use)
{
    g->ignore_dovi = !use;
}

int bsv_gpu_frames_in_line(const bsv_gpu *g)
{
    return pl_swapchain_latency(g->sw);
}

void bsv_gpu_reset(bsv_gpu *g)
{
    pl_queue_reset(g->queue);
    pl_renderer_flush_cache(g->rr);
}

static bool map_frame(pl_gpu gpu, pl_tex *tex, const struct pl_source_frame *src, struct pl_frame *out)
{
    AVFrame *frame = src->frame_data;
    // Dolby Vision, HDR10+ and mastering metadata on the frame all become part of `out` here.
    bool ok = pl_map_avframe_ex(gpu, out, pl_avframe_params(
        .frame = frame,
        .tex = tex,
    ));
    av_frame_free(&frame); // the mapped frame holds its own reference
    if (!ok)
        emit(1, "cannot upload a video frame to the GPU");
    return ok;
}

static void unmap_frame(pl_gpu gpu, struct pl_frame *frame, const struct pl_source_frame *src)
{
    pl_unmap_avframe(gpu, frame);
}

static void discard_frame(const struct pl_source_frame *src)
{
    AVFrame *frame = src->frame_data;
    av_frame_free(&frame);
}

struct pull_ctx {
    bsv_pull_fn pull;
    void *user;
    bool ignore_dovi;
};

static enum pl_queue_status pull_frame(struct pl_source_frame *out, const struct pl_queue_params *params)
{
    struct pull_ctx *ctx = params->priv;
    struct bsv_frame f = { 0 };
    switch (ctx->pull(ctx->user, &f)) {
    case BSV_PULL_FRAME:
        // Without its RPU the frame is what its base layer says: HDR10 with the stream's own
        // mastering metadata, HLG or SDR.
        if (ctx->ignore_dovi) {
            av_frame_remove_side_data(f.avframe, AV_FRAME_DATA_DOVI_METADATA);
            av_frame_remove_side_data(f.avframe, AV_FRAME_DATA_DOVI_RPU_BUFFER);
        }
        *out = (struct pl_source_frame) {
            .pts = f.pts,
            .duration = f.duration,
            .first_field = pl_field_from_avframe(f.avframe),
            .frame_data = f.avframe,
            .map = map_frame,
            .unmap = unmap_frame,
            .discard = discard_frame,
        };
        return PL_QUEUE_OK;
    case BSV_PULL_EOF:
        return PL_QUEUE_EOF;
    default:
        return PL_QUEUE_MORE;
    }
}

static int primaries_out(enum pl_color_primaries p)
{
    switch (p) {
    case PL_COLOR_PRIM_BT_709: return BSV_PRIM_BT709;
    case PL_COLOR_PRIM_DISPLAY_P3: return BSV_PRIM_DISPLAY_P3;
    case PL_COLOR_PRIM_BT_2020: return BSV_PRIM_BT2020;
    default: return 0;
    }
}

static int transfer_out(enum pl_color_transfer t)
{
    switch (t) {
    case PL_COLOR_TRC_UNKNOWN: return 0;
    case PL_COLOR_TRC_SRGB: return BSV_TRC_SRGB;
    case PL_COLOR_TRC_PQ: return BSV_TRC_PQ;
    case PL_COLOR_TRC_HLG: return BSV_TRC_HLG;
    default: return BSV_TRC_SDR_OTHER;
    }
}

static enum pl_color_primaries primaries_in(int p)
{
    switch (p) {
    case BSV_PRIM_DISPLAY_P3: return PL_COLOR_PRIM_DISPLAY_P3;
    case BSV_PRIM_BT2020: return PL_COLOR_PRIM_BT_2020;
    default: return PL_COLOR_PRIM_BT_709;
    }
}

static void describe_source(const struct pl_frame *ref, struct bsv_render_stats *stats)
{
    const struct pl_hdr_metadata *hdr = &ref->color.hdr;
    stats->src_primaries = primaries_out(ref->color.primaries);
    stats->src_transfer = transfer_out(ref->color.transfer);
    stats->src_max_nits = hdr->max_luma;
    stats->src_min_nits = hdr->min_luma;
    stats->src_max_cll = hdr->max_cll;
    stats->src_dolby_vision = ref->repr.sys == PL_COLOR_SYSTEM_DOLBYVISION || ref->repr.dovi != NULL;
    stats->src_hdr10_plus = hdr->scene_max[0] > 0 || hdr->ootf.num_anchors > 0;
    if (hdr->max_pq_y > 0) {
        stats->src_scene_max_nits = pl_hdr_rescale(PL_HDR_PQ, PL_HDR_NITS, hdr->max_pq_y);
    } else if (stats->src_hdr10_plus) {
        stats->src_scene_max_nits = fmaxf(hdr->scene_max[0], fmaxf(hdr->scene_max[1], hdr->scene_max[2]));
    }
}

// The signal to ask the system for, given the frame about to be shown.
static struct pl_color_space choose_signal(const struct bsv_target *target, const struct pl_frame *ref)
{
    struct pl_color_space signal = { 0 };
    if (target->transfer != BSV_TRC_PQ) {
        signal.primaries = primaries_in(target->primaries);
        signal.transfer = PL_COLOR_TRC_SRGB;
        return signal;
    }
    if (target->passthrough && pl_color_space_is_hdr(&ref->color)) {
        // The source's own primaries, transfer and static metadata; the display maps it.
        signal.primaries = ref->color.primaries;
        signal.transfer = ref->color.transfer;
        signal.hdr.prim = ref->color.hdr.prim;
        signal.hdr.min_luma = ref->color.hdr.min_luma;
        signal.hdr.max_luma = ref->color.hdr.max_luma;
        signal.hdr.max_cll = ref->color.hdr.max_cll;
        signal.hdr.max_fall = ref->color.hdr.max_fall;
        return signal;
    }
    // Mapped here: tell the display the picture already fits its range.
    signal.primaries = PL_COLOR_PRIM_BT_2020;
    signal.transfer = PL_COLOR_TRC_PQ;
    signal.hdr.prim = ref->color.hdr.prim.red.x > 0 ? ref->color.hdr.prim
                                                    : *pl_raw_primaries_get(PL_COLOR_PRIM_DISPLAY_P3);
    signal.hdr.max_luma = target->max_nits;
    signal.hdr.min_luma = target->min_nits;
    // After mapping nothing is brighter than the display's peak, or than the content was to begin with.
    // The light levels must not be left at zero: macOS reads zero as "no highlights" and gives no headroom.
    const struct pl_hdr_metadata *src = &ref->color.hdr;
    signal.hdr.max_cll = src->max_cll > 0 ? fminf(src->max_cll, target->max_nits) : target->max_nits;
    signal.hdr.max_fall = src->max_fall > 0 ? fminf(src->max_fall, signal.hdr.max_cll) : signal.hdr.max_cll;
    return signal;
}

int bsv_gpu_render(bsv_gpu *g, double pts, double vsync, const struct bsv_target *target,
                   bsv_pull_fn pull, void *user, struct bsv_render_stats *stats)
{
    memset(stats, 0, sizeof(*stats));
    const struct pl_render_params *params = &g->opts->params;

    struct pull_ctx ctx = { pull, user, g->ignore_dovi };
    struct pl_frame_mix mix = { 0 };
    enum pl_queue_status status = pl_queue_update(g->queue, &mix, pl_queue_params(
        .pts = pts,
        .radius = pl_frame_mix_radius(params),
        .vsync_duration = vsync,
        // The time asked for is the sound's, and is to be taken as given. Left at its default
        // the queue moves that time onto a frame's own whenever the two come within a
        // millisecond, and keeps every move it has made: at 23.976 frames on a 120 Hz display
        // they meet on every frame, so the picture ran at 24 and left the sound behind by a
        // millisecond a second.
        .drift_compensation = 0,
        .get_frame = pull_frame,
        .priv = &ctx,
    ));
    if (status == PL_QUEUE_ERR)
        return BSV_RENDER_ERROR;
    if (!mix.num_frames)
        return status == PL_QUEUE_EOF ? BSV_RENDER_EOF : BSV_RENDER_WAIT;

    const struct pl_frame *ref = pl_frame_mix_current(&mix);
    if (!ref)
        ref = mix.frames[0];
    describe_source(ref, stats);
    stats->fps = pl_queue_estimate_fps(g->queue);

    // The system switches the surface between SDR and HDR on the next frame after a new request.
    struct pl_color_space signal = choose_signal(target, ref);
    pl_swapchain_colorspace_hint(g->sw, &signal);

    struct pl_swapchain_frame surface;
    if (!pl_swapchain_start_frame(g->sw, &surface))
        return BSV_RENDER_WAIT;

    struct pl_frame out;
    pl_frame_from_swapchain(&out, &surface);
    if (pl_color_space_is_hdr(&out.color)) {
        // The range libplacebo maps into. For passthrough it equals the source's, so nothing moves.
        out.color.hdr = signal.hdr;
    }
    pl_rect2df_aspect_copy_rot(&out.crop, &ref->crop, 0.0, ref->rotation);

    struct pl_overlay subtitle;
    struct pl_overlay_part subtitle_part;
    if (g->subs_kind && subs_prepare(g, pts, &out, &subtitle, &subtitle_part)) {
        out.overlays = &subtitle;
        out.num_overlays = 1;
        stats->subtitle_shown = 1;
    }

    stats->out_primaries = primaries_out(out.color.primaries);
    stats->out_transfer = transfer_out(out.color.transfer);
    stats->out_max_nits = out.color.hdr.max_luma;
    stats->out_min_nits = out.color.hdr.min_luma;
    stats->out_bits = out.repr.bits.color_depth;

    bool ok = pl_render_image_mix(g->rr, &mix, &out, params);
    if (!pl_swapchain_submit_frame(g->sw))
        ok = false;
    pl_swapchain_swap_buffers(g->sw);

    if (!ok)
        return BSV_RENDER_ERROR;
    return status == PL_QUEUE_EOF ? BSV_RENDER_EOF : BSV_RENDER_OK;
}
