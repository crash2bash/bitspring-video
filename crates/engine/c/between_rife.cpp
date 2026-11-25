// Frames in between, made by a neural network: RIFE 4.6 run through ncnn on the GPU (Vulkan;
// over Metal on the Mac). The model takes two pictures as RGB and the moment wanted between
// them, and gives the picture for that moment.
//
// The engine's pictures are 8-bit 4:2:0: they are taken to RGB here and the answer is brought
// back, with the same figures both ways, so that a picture which did not change comes back as
// it went in.
//
// Measured on 2026-11-25 on an M4 Max with Homebrew's ncnn 20261020: 17 ms a picture at
// 960x540 and 51 ms at 1920x1080, the same away from the player, with nothing else on the GPU,
// and whichever of ncnn's options was changed. The model's own tool, built in 2022, takes 8 ms
// and 35 ms on the same machine; why this ncnn is the slower has not been found.
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

#include <allocator.h>
#include <gpu.h>
#include <layer.h>
#include <net.h>

extern "C" {
#include <libavutil/frame.h>
#include <libavutil/pixfmt.h>

#include "between.h"
}

#include "../rife/rife_ops.h"

// The model's own layer comes from before ncnn asked a GPU layer to say that it takes values
// packed in fours; unsaid, ncnn unpacks around each of the model's eight warps, which more
// costs time on some systems (none that could be measured on the Mac it was tried on). Said
// here, for the versions of ncnn that ask.
template <typename T> auto takes_packed(T *layer, int) -> decltype(layer->support_vulkan_packing = true, void())
{
    layer->support_vulkan_packing = true;
}
template <typename T> void takes_packed(T *, long) {}

static ncnn::Layer *new_warp(void *)
{
    ncnn::Layer *layer = new Warp;
    takes_packed(layer, 0);
    return layer;
}

namespace {

struct Maker {
    ncnn::Net net;
    int w = 0, h = 0;
    // The model wants sides that divide by 32: the pictures sit in the corner of larger ones.
    ncnn::Mat pictures[2];
    ncnn::Mat moment;
    int newer = 0;
    bool kept = false;
    int64_t kept_stamp = 0;
    // ncnn reads the model's numbers where they lie, and wants them on a 4-byte boundary.
    void *model = nullptr;
};

void say(char *error, int error_len, const char *text)
{
    if (error && error_len > 0)
        snprintf(error, (size_t) error_len, "%s", text);
}

// Luma weights of the picture's colour system; BT.709 unless it says BT.601.
void weights(const AVFrame *frame, float *kr, float *kb)
{
    bool sd = frame->colorspace == AVCOL_SPC_BT470BG || frame->colorspace == AVCOL_SPC_SMPTE170M;
    *kr = sd ? 0.299f : 0.2126f;
    *kb = sd ? 0.114f : 0.0722f;
}

float clamp01(float v)
{
    return v < 0.f ? 0.f : v > 1.f ? 1.f : v;
}

// A decoded picture (NV12 or planar 4:2:0, video range) into RGB between 0 and 1. Each colour
// sample serves its four pixels unchanged, which the way back undoes exactly.
void to_rgb(const AVFrame *frame, ncnn::Mat &rgb, int w, int h)
{
    float kr, kb;
    weights(frame, &kr, &kb);
    const float kg = 1.f - kr - kb;
    const bool nv12 = frame->format == AV_PIX_FMT_NV12;
    float *red = rgb.channel(0), *green = rgb.channel(1), *blue = rgb.channel(2);
    const int stride = rgb.w;
    for (int y = 0; y < h; y++) {
        const uint8_t *luma = frame->data[0] + (ptrdiff_t) y * frame->linesize[0];
        const uint8_t *u = frame->data[1] + (ptrdiff_t) (y / 2) * frame->linesize[1];
        const uint8_t *v = nv12 ? u + 1 : frame->data[2] + (ptrdiff_t) (y / 2) * frame->linesize[2];
        const int step = nv12 ? 2 : 1;
        float *r = red + (size_t) y * stride, *g = green + (size_t) y * stride, *b = blue + (size_t) y * stride;
        for (int x = 0; x < w; x++) {
            const float yy = (luma[x] - 16) * (1.f / 219.f);
            const float cb = (u[(x / 2) * step] - 128) * (1.f / 224.f);
            const float cr = (v[(x / 2) * step] - 128) * (1.f / 224.f);
            const float rr = yy + 2.f * (1.f - kr) * cr;
            const float bb = yy + 2.f * (1.f - kb) * cb;
            r[x] = clamp01(rr);
            b[x] = clamp01(bb);
            g[x] = clamp01((yy - kr * rr - kb * bb) / kg);
        }
    }
}

uint8_t code(float v, float low, float high)
{
    v = v < low ? low : v > high ? high : v;
    return (uint8_t) (v + 0.5f);
}

// The model's answer back into an NV12 picture; each colour sample is the mean of its four pixels.
void from_rgb(const ncnn::Mat &rgb, AVFrame *out, const AVFrame *like, int w, int h)
{
    float kr, kb;
    weights(like, &kr, &kb);
    const float kg = 1.f - kr - kb;
    const float *red = rgb.channel(0), *green = rgb.channel(1), *blue = rgb.channel(2);
    const int stride = rgb.w;
    std::vector<float> cb_row((size_t) (w + 1) / 2), cr_row((size_t) (w + 1) / 2);
    for (int y = 0; y < h; y++) {
        uint8_t *luma = out->data[0] + (ptrdiff_t) y * out->linesize[0];
        const float *r = red + (size_t) y * stride, *g = green + (size_t) y * stride, *b = blue + (size_t) y * stride;
        if (y % 2 == 0) {
            std::fill(cb_row.begin(), cb_row.end(), 0.f);
            std::fill(cr_row.begin(), cr_row.end(), 0.f);
        }
        for (int x = 0; x < w; x++) {
            const float yy = kr * r[x] + kg * g[x] + kb * b[x];
            luma[x] = code(16.f + 219.f * yy, 16.f, 235.f);
            cb_row[(size_t) x / 2] += (b[x] - yy) / (2.f * (1.f - kb));
            cr_row[(size_t) x / 2] += (r[x] - yy) / (2.f * (1.f - kr));
        }
        if (y % 2 == 1 || y == h - 1) {
            uint8_t *uv = out->data[1] + (ptrdiff_t) (y / 2) * out->linesize[1];
            const float rows = (y % 2 == 1) ? 2.f : 1.f;
            for (int x = 0; x < (w + 1) / 2; x++) {
                const float columns = (2 * x + 1 < w) ? 2.f : 1.f;
                const float share = 1.f / (rows * columns);
                uv[2 * x] = code(128.f + 224.f * cb_row[(size_t) x] * share, 16.f, 240.f);
                uv[2 * x + 1] = code(128.f + 224.f * cr_row[(size_t) x] * share, 16.f, 240.f);
            }
        }
    }
}

} // namespace

extern "C" void *bsv_between_rife_open(int width, int height, const char *param, const void *model, size_t model_len,
                                       char *error, int error_len)
{
    if (!param || !model || !model_len) {
        say(error, error_len, "the neural network's model is not in this build");
        return nullptr;
    }
    // Once for the whole program, and left standing: other makers may be using it.
    ncnn::create_gpu_instance();
    if (ncnn::get_gpu_count() <= 0) {
        say(error, error_len, "no graphics card here runs the neural network (Vulkan is needed)");
        return nullptr;
    }
    ncnn::VulkanDevice *device = ncnn::get_gpu_device(ncnn::get_default_gpu_index());
    if (!device) {
        say(error, error_len, "the graphics card did not open for the neural network");
        return nullptr;
    }

    Maker *m = new Maker();
    m->w = width;
    m->h = height;
    ncnn::Option opt;
    opt.num_threads = 2;
    opt.use_vulkan_compute = true;
    opt.use_fp16_packed = true;
    opt.use_fp16_storage = true;
    opt.use_fp16_arithmetic = false;
    opt.use_int8_storage = true;
    m->net.opt = opt;
    m->net.set_vulkan_device(device);
    m->net.register_custom_layer("rife.Warp", new_warp);

    const size_t rounded = (model_len + 15) & ~(size_t) 15;
    m->model = ncnn::fastMalloc(rounded);
    bool ok = m->model != nullptr;
    if (ok) {
        memset(m->model, 0, rounded);
        memcpy(m->model, model, model_len);
        ok = m->net.load_param_mem(param) == 0 && m->net.load_model((const unsigned char *) m->model) > 0;
    }
    if (!ok) {
        say(error, error_len, "the neural network's model could not be read");
        ncnn::fastFree(m->model);
        delete m;
        return nullptr;
    }

    const int wide = (width + 31) / 32 * 32, tall = (height + 31) / 32 * 32;
    for (ncnn::Mat &picture : m->pictures) {
        picture.create(wide, tall, 3);
        picture.fill(0.f);
    }
    m->moment.create(wide, tall, 1);
    m->moment.fill(0.5f);
    if (m->pictures[0].empty() || m->pictures[1].empty() || m->moment.empty()) {
        say(error, error_len, "no memory for the pictures");
        ncnn::fastFree(m->model);
        delete m;
        return nullptr;
    }
    return m;
}

extern "C" void bsv_between_rife_close(void *maker)
{
    Maker *m = (Maker *) maker;
    if (!m)
        return;
    m->net.clear();
    ncnn::fastFree(m->model);
    delete m;
}

extern "C" int bsv_between_rife_make(void *maker, const AVFrame *a, const AVFrame *b, AVFrame *out)
{
    Maker *m = (Maker *) maker;
    const int older = m->newer;
    if (!m->kept || m->kept_stamp != a->pts)
        to_rgb(a, m->pictures[older], m->w, m->h);
    const int newer = 1 - older;
    to_rgb(b, m->pictures[newer], m->w, m->h);
    m->newer = newer;
    m->kept = true;
    m->kept_stamp = b->pts;

    ncnn::Mat made;
    {
        ncnn::Extractor run = m->net.create_extractor();
        run.input("in0", m->pictures[older]);
        run.input("in1", m->pictures[newer]);
        run.input("in2", m->moment);
        if (run.extract("out0", made) != 0 || made.empty() || made.c < 3 || made.w < m->w || made.h < m->h)
            return -1;
    }
    from_rgb(made, out, b, m->w, m->h);
    return 0;
}
