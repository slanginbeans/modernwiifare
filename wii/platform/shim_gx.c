#include <gccore.h>
#include <malloc.h>
#include <string.h>

#include "shim.h"

#define FIFO_SIZE (256 * 1024)

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

void mwf_gx_init(uint32_t clear_rgba) {
    void *fifo = memalign(32, FIFO_SIZE);
    memset(fifo, 0, FIFO_SIZE);
    GX_Init(fifo, FIFO_SIZE);

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

void mwf_gx_end_frame(void) {
    GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_TRUE);
    GX_SetColorUpdate(GX_TRUE);
    /* The previous frame: wait until the GPU has passed its token, then show its XFB at the next retrace. */
    uint32_t t0 = mwf_us();
    if (queued_xfb >= 0) {
        while (GX_GetDrawSync() != issued) {
        }
        VIDEO_SetNextFramebuffer(mwf_xfb[queued_xfb]);
        VIDEO_Flush();
    }
    gpu_wait = mwf_us() - t0;
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
