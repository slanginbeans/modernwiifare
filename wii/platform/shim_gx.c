#include <gccore.h>
#include <malloc.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "shim.h"

/* Room for two frames of the heaviest maps' commands (`mp_terminal` writes ~420 KB a frame): with less, the CPU stops
 * mid-frame whenever the FIFO reaches its high watermark and waits for the GPU (telemetry `full`), and frames can't
 * overlap as pipelining intends. MEM1. */
#define FIFO_SIZE (1024 * 1024)

extern GXRModeObj *mwf_rmode;
extern void *mwf_xfb[MWF_XFB_COUNT];
extern volatile u32 mwf_retraces;
/* Pipelined frames: the GPU draws frame N while the CPU builds frame N+1. Frame N's copy goes to XFB `fb`, then a
 * draw-sync token; at the end of frame N+1 the CPU waits for that token (not for its own frame), asks for frame N's XFB
 * at the next retrace, and queues frame N+1. Three XFBs: one on screen, one the GPU may be copying into, one free. */
static int fb = 0;
static u16 issued = 0;
static int queued_xfb = -1;
static u32 last_retrace = 0;
static uint32_t gpu_wait = 0;
/* Steady pacing: each frame stays on screen `interval` retraces (2: 30 fps at 60 Hz); `shown_at` is the retrace the
 * last flip took effect at. `pace_wait`: the time this frame waited for its turn (the frame's headroom). */
static u32 interval = 1;
static u32 shown_at = 0;
static uint32_t pace_wait = 0;

/* The GPU's own counters (GX_SetGPMetric): one pair at a time, rotated every ~second (`METRIC_FRAMES` frames), each
 * pair's last reading kept with the frames and microseconds it covered. */
#define METRIC_FRAMES 60
static const u32 METRICS[MWF_GPU_METRIC_SETS][2] = {
    {GX_PERF0_XF_WAIT_IN, GX_PERF1_CLOCKS},        /* busy = clocks - XF waiting for input */
    {GX_PERF0_XF_WAIT_OUT, GX_PERF1_TX_MEMSTALL},  /* transform held up by the pixel end; texturing stalled on memory */
    {GX_PERF0_QUAD_NON0CVG, GX_PERF1_TC_MISS},     /* 2x2 pixel quads drawn (fill); texture cache misses */
    {GX_PERF0_TRIANGLES_PASSED, GX_PERF1_VC_MISS_REQ}, /* triangles past culling; vertex cache misses (32 B each) */
};
static int metric_set = -1;
static u32 metric_frames = 0;
static uint32_t metric_t0 = 0;
static mwf_gpu_metric metric_last[MWF_GPU_METRIC_SETS];

/* The PI's view of the CPU FIFO: base, end and write pointer (bit 26 flags a wrap). */
#define PI_FIFO_BASE (*(volatile u32 *)0xCC00300C)
#define PI_FIFO_END (*(volatile u32 *)0xCC003010)
#define PI_FIFO_WP (*(volatile u32 *)0xCC003014)
static u32 fifo_last_wp = 0, fifo_base = 0, fifo_end = 0;
static uint32_t fifo_written = 0;

/* One uncached register read: cheap enough every 16th world draw. */
void mwf_fifo_sample(void) {
    u32 base = fifo_base, end = fifo_end;
    u32 wp = PI_FIFO_WP & 0x03FFFFE0;
    if (wp >= fifo_last_wp)
        fifo_written += wp - fifo_last_wp;
    else
        fifo_written += (end - fifo_last_wp) + (wp - base);
    fifo_last_wp = wp;
}

uint32_t mwf_fifo_bytes(void) {
    mwf_fifo_sample();
    return fifo_written;
}

uint32_t mwf_fifo_overflows(void) {
    return GX_GetOverflowCount();
}

void mwf_gx_init(uint32_t clear_rgba) {
    void *fifo = memalign(32, FIFO_SIZE);
    memset(fifo, 0, FIFO_SIZE);
    GX_Init(fifo, FIFO_SIZE);
    fifo_base = PI_FIFO_BASE & 0x03FFFFE0;
    fifo_end = PI_FIFO_END & 0x03FFFFE0;
    fifo_last_wp = PI_FIFO_WP & 0x03FFFFE0;

    GXColor bg = {(u8)(clear_rgba >> 24), (u8)(clear_rgba >> 16), (u8)(clear_rgba >> 8), 0xff};
    GX_SetCopyClear(bg, 0x00ffffff);

    GX_SetViewport(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight, 0, 1);
    f32 yscale = GX_GetYScaleFactor(mwf_rmode->efbHeight, mwf_rmode->xfbHeight);
    u32 xfb_height = GX_SetDispCopyYScale(yscale);
    GX_SetScissor(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight);
    GX_SetDispCopySrc(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight);
    GX_SetDispCopyDst(mwf_rmode->fbWidth, xfb_height);
    GX_SetCopyFilter(mwf_rmode->aa, mwf_rmode->sample_pattern, GX_TRUE, mwf_rmode->vfilter);
    GX_SetFieldMode(mwf_rmode->field_rendering,
                    (mwf_rmode->viHeight == 2 * mwf_rmode->xfbHeight) ? GX_ENABLE : GX_DISABLE);
    GX_SetCullMode(GX_CULL_NONE);
    GX_SetDispCopyGamma(GX_GM_1_0);

    /* Direct float positions + RGBA8 vertex colours, one pass-through TEV stage. */
    GX_ClearVtxDesc();
    GX_SetVtxDesc(GX_VA_POS, GX_DIRECT);
    GX_SetVtxDesc(GX_VA_CLR0, GX_DIRECT);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_POS, GX_POS_XYZ, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
    GX_SetNumChans(1);
    GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    GX_SetNumTexGens(0);
    GX_SetNumTevStages(1);
    GX_SetTevOrder(GX_TEVSTAGE0, GX_TEXCOORDNULL, GX_TEXMAP_NULL, GX_COLOR0A0);
    GX_SetTevOp(GX_TEVSTAGE0, GX_PASSCLR);

    f32 aspect = (CONF_GetAspectRatio() == CONF_ASPECT_16_9) ? (16.0f / 9.0f) : (4.0f / 3.0f);
    Mtx44 proj;
    guPerspective(proj, 65.0f, aspect, 0.1f, 300.0f);
    GX_LoadProjectionMtx(proj, GX_PERSPECTIVE);
    GX_SetCurrentMtx(GX_PNMTX0);

    /* Show the first GX frame instead of the text console from now on. */
    fb = 1;
}

void mwf_gx_draw(const float mtx[3][4], const mwf_vtx *v, uint32_t n) {
    GX_LoadPosMtxImm((f32(*)[4])mtx, GX_PNMTX0);
    GX_Begin(GX_TRIANGLES, GX_VTXFMT0, (u16)n);
    for (uint32_t i = 0; i < n; i++) {
        GX_Position3f32(v[i].x, v[i].y, v[i].z);
        GX_Color1u32(v[i].rgba);
    }
    GX_End();
}

/* Wait for the GPU to finish everything queued (before memory it may still read is reused). */
void mwf_gpu_idle(void) {
    if (issued != 0) {
        while (GX_GetDrawSync() != issued) {
        }
    }
    GX_DrawDone();
}

uint32_t mwf_gpu_wait_us(void) {
    return gpu_wait;
}

void mwf_set_frame_interval(uint32_t retraces) {
    interval = retraces < 1 ? 1 : (retraces > 4 ? 4 : retraces);
}

uint32_t mwf_pace_wait_us(void) {
    return pace_wait;
}

void mwf_gpu_metrics(mwf_gpu_metric out[MWF_GPU_METRIC_SETS]) {
    memcpy(out, metric_last, sizeof metric_last);
}

/* After the previous frame's token: read the running pair (its commands are processed) and start the next one. */
static void metric_step(void) {
    uint32_t now = mwf_us();
    if (metric_set >= 0 && ++metric_frames < METRIC_FRAMES) return;
    if (metric_set >= 0) {
        u32 c0 = 0, c1 = 0;
        GX_ReadGPMetric(&c0, &c1);
        mwf_gpu_metric *m = &metric_last[metric_set];
        m->c0 = c0;
        m->c1 = c1;
        m->frames = metric_frames;
        m->us = now - metric_t0;
        m->taken += 1;
    }
    metric_set = (metric_set + 1) % MWF_GPU_METRIC_SETS;
    GX_SetGPMetric(METRICS[metric_set][0], METRICS[metric_set][1]);
    GX_ClearGPMetric();
    metric_frames = 0;
    metric_t0 = now;
}

const void *mwf_last_frame(void) {
    if (queued_xfb < 0) return NULL;
    mwf_gpu_idle();
    return mwf_xfb[queued_xfb];
}

static u8 clamp_u8(int v) {
    return (u8)(v < 0 ? 0 : (v > 255 ? 255 : v));
}

int mwf_screenshot(const char *path) {
    if (queued_xfb < 0 || mwf_rmode == NULL) return -1;
    mwf_gpu_idle();
    const u32 w = mwf_rmode->fbWidth, h = mwf_rmode->xfbHeight;
    const u8 *src = (const u8 *)mwf_xfb[queued_xfb];
    FILE *f = fopen(path, "wb");
    if (!f) return -1;
    u8 hdr[54] = {'B', 'M'};
    const u32 row = w * 3, size = 54 + row * h;
    const u32 vals[] = {size, 0, 54, 40, w, h};
    for (int i = 0; i < 6; i++) {
        u8 *p = hdr + 2 + i * 4;
        p[0] = (u8)vals[i];
        p[1] = (u8)(vals[i] >> 8);
        p[2] = (u8)(vals[i] >> 16);
        p[3] = (u8)(vals[i] >> 24);
    }
    hdr[26] = 1;  /* planes */
    hdr[28] = 24; /* bits */
    fwrite(hdr, 1, sizeof hdr, f);
    u8 *line = malloc(row);
    if (!line) {
        fclose(f);
        return -1;
    }
    /* BMP rows go bottom-up; the XFB is Y Cb Y Cr per pixel pair. */
    for (int y = (int)h - 1; y >= 0; y--) {
        const u8 *s = src + (u32)y * w * 2;
        for (u32 x = 0; x < w; x += 2) {
            int y0 = s[x * 2] - 16, cb = s[x * 2 + 1] - 128, y1 = s[x * 2 + 2] - 16, cr = s[x * 2 + 3] - 128;
            int ys[2] = {y0, y1};
            for (int k = 0; k < 2; k++) {
                int c = 298 * ys[k];
                u8 *d = line + (x + (u32)k) * 3;
                d[0] = clamp_u8((c + 516 * cb + 128) >> 8);
                d[1] = clamp_u8((c - 100 * cb - 208 * cr + 128) >> 8);
                d[2] = clamp_u8((c + 409 * cr + 128) >> 8);
            }
        }
        fwrite(line, 1, row, f);
    }
    free(line);
    fclose(f);
    return 0;
}

void mwf_gx_end_frame(void) {
    GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_TRUE);
    GX_SetColorUpdate(GX_TRUE);
    /* The previous frame: wait until the GPU has passed its token, then show its XFB at the next retrace. */
    uint32_t t0 = mwf_us();
    if (queued_xfb >= 0) {
        while (GX_GetDrawSync() != issued) {
        }
        gpu_wait = mwf_us() - t0;
        /* Its turn: the flip asked for now takes effect at the next retrace, so ask once the retrace before the one it
         * is due at has passed (a late frame goes at the next retrace). */
        uint32_t p0 = mwf_us();
        if (interval > 1) {
            while ((s32)(mwf_retraces - (shown_at + interval - 1)) < 0) VIDEO_WaitVSync();
        }
        pace_wait = mwf_us() - p0;
        VIDEO_SetNextFramebuffer(mwf_xfb[queued_xfb]);
        VIDEO_Flush();
        shown_at = mwf_retraces + 1;
        metric_step();
    } else {
        gpu_wait = mwf_us() - t0;
        pace_wait = 0;
    }
    /* At most one frame per retrace (so the XFB the next copy reuses has left the screen). */
    if (mwf_retraces == last_retrace) VIDEO_WaitVSync();
    last_retrace = mwf_retraces;
    /* This frame: its copy, then its token; the GPU works through it while the CPU builds the next frame. */
    GX_CopyDisp(mwf_xfb[fb], GX_TRUE);
    issued = (u16)(issued + 1 == 0 ? 1 : issued + 1);
    GX_SetDrawSync(issued);
    GX_Flush();
    queued_xfb = fb;
    fb = (fb + 1) % MWF_XFB_COUNT;
}
