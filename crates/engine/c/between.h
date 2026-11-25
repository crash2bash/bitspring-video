// The makers of frames in between, one to a file, behind bsv_between in bsv.c.
#ifndef BSV_BETWEEN_H
#define BSV_BETWEEN_H

struct AVFrame;

// The system's own (VideoToolbox, Apple systems). `a` and `b` are 8-bit 4:2:0 in video range,
// NV12 or planar, of the size the maker was opened for; `out` is an NV12 frame of that size
// with its buffers allocated. make returns 0 when `out` holds the picture half-way.
void *bsv_between_vt_open(int width, int height, char *error, int error_len);
void bsv_between_vt_close(void *maker);
int bsv_between_vt_make(void *maker, const struct AVFrame *a, const struct AVFrame *b, struct AVFrame *out);

#endif
