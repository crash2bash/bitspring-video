// Frames in between, made by the system: VideoToolbox's low-latency frame interpolation
// (macOS 26). It takes two pictures and gives the one for the moment half-way.
//
// What it accepts was found by trying on 2026-11-25 (an M4 Max, macOS 27.2): 8-bit 4:2:0 in
// video range only, and no picture larger than 1920x1080. About 11 ms a picture at that size.
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <VideoToolbox/VideoToolbox.h>

#include <libavutil/frame.h>
#include <libavutil/pixfmt.h>

#include "between.h"

@interface BSVBetween : NSObject
@property(nonatomic, strong) VTFrameProcessor *processor;
@property(nonatomic, strong) NSDictionary *sourceAttributes;
@property(nonatomic, strong) NSDictionary *destinationAttributes;
@property(nonatomic) int width;
@property(nonatomic) int height;
@property(nonatomic) int64_t count;
// The second picture of the last pair stays in its buffer: it is the first of the next.
@property(nonatomic) int64_t keptStamp;
@property(nonatomic) BOOL kept;
@end

@implementation BSVBetween {
  @public
    CVPixelBufferRef _buffers[2];
    int _newer;
}

- (void)dealloc
{
    [_processor endSession];
    CVPixelBufferRelease(_buffers[0]);
    CVPixelBufferRelease(_buffers[1]);
}
@end

static void say(char *error, int error_len, NSString *text)
{
    if (error && error_len > 0)
        snprintf(error, error_len, "%s", text.UTF8String);
}

static CVPixelBufferRef new_buffer(NSDictionary *wanted, int w, int h)
{
    NSMutableDictionary *attributes = [wanted mutableCopy] ?: [NSMutableDictionary dictionary];
    attributes[(id) kCVPixelBufferIOSurfacePropertiesKey] = @{};
    CVPixelBufferRef buffer = NULL;
    CVPixelBufferCreate(kCFAllocatorDefault, w, h, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                        (__bridge CFDictionaryRef) attributes, &buffer);
    return buffer;
}

void *bsv_between_vt_open(int width, int height, char *error, int error_len)
{
    @autoreleasepool {
        if (!VTLowLatencyFrameInterpolationConfiguration.supported) {
            say(error, error_len, @"this Mac does not make frames in between");
            return NULL;
        }
        VTLowLatencyFrameInterpolationConfiguration *configuration =
            [[VTLowLatencyFrameInterpolationConfiguration alloc] initWithFrameWidth:width
                                                                        frameHeight:height
                                                         numberOfInterpolatedFrames:1];
        if (!configuration
            || ![configuration.frameSupportedPixelFormats containsObject:@(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)]) {
            say(error, error_len, @"the system makes no frames in between for a picture of this size");
            return NULL;
        }
        BSVBetween *b = [[BSVBetween alloc] init];
        b.processor = [[VTFrameProcessor alloc] init];
        NSError *failure = nil;
        // This loads the system's models: most of a second the first time.
        if (![b.processor startSessionWithConfiguration:configuration error:&failure]) {
            say(error, error_len, [NSString stringWithFormat:@"the system's frame interpolation did not start: %@",
                                                             failure.localizedDescription]);
            return NULL;
        }
        b.sourceAttributes = configuration.sourcePixelBufferAttributes;
        b.destinationAttributes = configuration.destinationPixelBufferAttributes;
        b.width = width;
        b.height = height;
        b->_buffers[0] = new_buffer(b.sourceAttributes, width, height);
        b->_buffers[1] = new_buffer(b.sourceAttributes, width, height);
        if (!b->_buffers[0] || !b->_buffers[1]) {
            say(error, error_len, @"no memory for the pictures");
            return NULL;
        }
        return (__bridge_retained void *) b;
    }
}

void bsv_between_vt_close(void *opaque)
{
    if (opaque)
        CFBridgingRelease(opaque);
}

// Copy a decoded picture (NV12, or planar 4:2:0 whose two colour planes are woven together
// here) into a buffer the system can work on.
static void put(const AVFrame *frame, CVPixelBufferRef buffer, int w, int h)
{
    CVPixelBufferLockBaseAddress(buffer, 0);
    uint8_t *y = CVPixelBufferGetBaseAddressOfPlane(buffer, 0);
    size_t y_stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0);
    for (int row = 0; row < h; row++)
        memcpy(y + row * y_stride, frame->data[0] + row * frame->linesize[0], w);
    uint8_t *uv = CVPixelBufferGetBaseAddressOfPlane(buffer, 1);
    size_t uv_stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 1);
    int chroma_w = (w + 1) / 2, chroma_h = (h + 1) / 2;
    if (frame->format == AV_PIX_FMT_NV12) {
        for (int row = 0; row < chroma_h; row++)
            memcpy(uv + row * uv_stride, frame->data[1] + row * frame->linesize[1], chroma_w * 2);
    } else {
        for (int row = 0; row < chroma_h; row++) {
            const uint8_t *u = frame->data[1] + row * frame->linesize[1];
            const uint8_t *v = frame->data[2] + row * frame->linesize[2];
            uint8_t *to = uv + row * uv_stride;
            for (int x = 0; x < chroma_w; x++) {
                to[2 * x] = u[x];
                to[2 * x + 1] = v[x];
            }
        }
    }
    CVPixelBufferUnlockBaseAddress(buffer, 0);
}

int bsv_between_vt_make(void *opaque, const AVFrame *a, const AVFrame *b, AVFrame *out)
{
    @autoreleasepool {
        BSVBetween *s = (__bridge BSVBetween *) opaque;
        int w = s.width, h = s.height;
        int older = s->_newer;
        if (!s.kept || s.keptStamp != a->pts)
            put(a, s->_buffers[older], w, h);
        int newer = 1 - older;
        put(b, s->_buffers[newer], w, h);
        s->_newer = newer;
        s.kept = YES;
        s.keptStamp = b->pts;

        CVPixelBufferRef made = new_buffer(s.destinationAttributes, w, h);
        if (!made)
            return -1;
        int64_t n = s.count++;
        VTFrameProcessorFrame *first = [[VTFrameProcessorFrame alloc] initWithBuffer:s->_buffers[older]
                                                               presentationTimeStamp:CMTimeMake(n * 2, 1000)];
        VTFrameProcessorFrame *second = [[VTFrameProcessorFrame alloc] initWithBuffer:s->_buffers[newer]
                                                                presentationTimeStamp:CMTimeMake(n * 2 + 2, 1000)];
        VTFrameProcessorFrame *middle = [[VTFrameProcessorFrame alloc] initWithBuffer:made
                                                                presentationTimeStamp:CMTimeMake(n * 2 + 1, 1000)];
        VTLowLatencyFrameInterpolationParameters *parameters = nil;
        if (first && second && middle) {
            parameters = [[VTLowLatencyFrameInterpolationParameters alloc] initWithSourceFrame:second
                                                                                previousFrame:first
                                                                           interpolationPhase:@[ @0.5f ]
                                                                            destinationFrames:@[ middle ]];
        }
        NSError *failure = nil;
        BOOL ok = parameters && [s.processor processWithParameters:parameters error:&failure];
        if (ok) {
            CVPixelBufferLockBaseAddress(made, kCVPixelBufferLock_ReadOnly);
            for (int plane = 0; plane < 2; plane++) {
                const uint8_t *from = CVPixelBufferGetBaseAddressOfPlane(made, plane);
                size_t stride = CVPixelBufferGetBytesPerRowOfPlane(made, plane);
                int rows = plane ? (h + 1) / 2 : h;
                int bytes = plane ? ((w + 1) / 2) * 2 : w;
                for (int row = 0; row < rows; row++)
                    memcpy(out->data[plane] + row * out->linesize[plane], from + row * stride, bytes);
            }
            CVPixelBufferUnlockBaseAddress(made, kCVPixelBufferLock_ReadOnly);
        }
        CVPixelBufferRelease(made);
        return ok ? 0 : -1;
    }
}
