/* Game world rendering on GX: the C half of the pack renderer (Rust drives it; see shim.h).
 *
 * Data comes from a .wpk loaded into MEM2 and flushed once: texture data, interleaved world vertices and prebuilt
 * display lists are used where they lie. Recipes follow docs/wii-gx.md:
 *   stage 0: colour texture (x vertex colour when unlit), alpha = texture alpha x vertex alpha
 *   stage 1: x lightmap x 2                              (lit draws)
 *   stage 2: alpha from the I4 alpha plane x vertex alpha (split textures)
 */
#include <fat.h>
#include <gccore.h>
#include <stdio.h>
#include <string.h>

#include "shim.h"

extern GXRModeObj *mwf_rmode;

/* ---- MEM2 bump allocator ------------------------------------------------------------------------------------- */

static uintptr_t mem2_lo = 0, mem2_hi = 0;

static void mem2_init(void) {
    if (mem2_lo == 0) {
        mem2_lo = (uintptr_t)SYS_GetArena2Lo();
        mem2_hi = (uintptr_t)SYS_GetArena2Hi();
    }
}

void *mwf_mem2_alloc(uint32_t size, uint32_t align) {
    mem2_init();
    if (align < 32) align = 32;
    uintptr_t p = (mem2_lo + align - 1) & ~(uintptr_t)(align - 1);
    if (p + size > mem2_hi) return NULL;
    mem2_lo = p + size;
    SYS_SetArena2Lo((void *)mem2_lo);
    return (void *)p;
}

uint32_t mwf_mem2_free_bytes(void) {
    mem2_init();
    return (uint32_t)(mem2_hi - mem2_lo);
}

/* ---- SD card ------------------------------------------------------------------------------------------------- */

static int fat_state = -1; /* -1 not tried, 0 failed, 1 mounted */

void *mwf_file_load(const char *path, uint32_t *size_out) {
    if (fat_state < 0) fat_state = fatInitDefault() ? 1 : 0;
    if (!fat_state) return NULL;
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (n <= 0) {
        fclose(f);
        return NULL;
    }
    uint32_t padded = ((uint32_t)n + 31u) & ~31u;
    void *p = mwf_mem2_alloc(padded, 32);
    if (!p) {
        fclose(f);
        return NULL;
    }
    size_t got = fread(p, 1, (size_t)n, f);
    fclose(f);
    if (got != (size_t)n) return NULL;
    memset((uint8_t *)p + n, 0, padded - (uint32_t)n);
    /* The CPU wrote the bytes; GX reads memory directly. Dolphin does not model this cache, hardware does. */
    DCFlushRange(p, padded);
    if (size_out) *size_out = (uint32_t)n;
    return p;
}

/* ---- textures ------------------------------------------------------------------------------------------------ */

#define MAX_TEX 1024
static GXTexObj tex_obj[MAX_TEX];
static GXTexObj tex_alpha[MAX_TEX];
static uint8_t tex_has_alpha[MAX_TEX];
static int32_t tex_count = 0;

static u8 gx_wrap(uint8_t w) {
    return w == 1 ? GX_CLAMP : (w == 2 ? GX_MIRROR : GX_REPEAT);
}

static void init_obj(GXTexObj *o, const void *data, const mwf_tex_desc *d, u8 fmt) {
    u8 wrap = gx_wrap(d->wrap);
    u8 mip = d->levels > 1 ? GX_TRUE : GX_FALSE;
    GX_InitTexObj(o, (void *)data, d->width, d->height, fmt, wrap, wrap, mip);
    if (mip) {
        GX_InitTexObjLOD(o, GX_LIN_MIP_LIN, GX_LINEAR, 0.0f, (f32)(d->levels - 1), 0.0f, GX_FALSE, GX_FALSE,
                         GX_ANISO_1);
    }
}

int32_t mwf_tex_create(const mwf_tex_desc *d) {
    if (tex_count >= MAX_TEX) return -1;
    int32_t h = tex_count++;
    init_obj(&tex_obj[h], d->data, d, d->format);
    tex_has_alpha[h] = d->alpha != NULL;
    if (d->alpha) init_obj(&tex_alpha[h], d->alpha, d, GX_TF_I4);
    return h;
}

/* ---- frame --------------------------------------------------------------------------------------------------- */

enum { MODE_NONE, MODE_SKY, MODE_WORLD };
static int mode = MODE_NONE;
static const void *world_base = NULL;

void mwf_frame_begin(const float view[3][4], float fovy_deg, float znear, float zfar, uint32_t clear_rgba) {
    GXColor bg = {(u8)(clear_rgba >> 24), (u8)(clear_rgba >> 16), (u8)(clear_rgba >> 8), 0xff};
    GX_SetCopyClear(bg, GX_MAX_Z24);
    f32 aspect = (CONF_GetAspectRatio() == CONF_ASPECT_16_9) ? (16.0f / 9.0f) : (4.0f / 3.0f);
    Mtx44 proj;
    guPerspective(proj, fovy_deg, aspect, znear, zfar);
    GX_LoadProjectionMtx(proj, GX_PERSPECTIVE);
    GX_LoadPosMtxImm((f32(*)[4])view, GX_PNMTX0);
    GX_SetCurrentMtx(GX_PNMTX0);
    GX_InvalidateTexAll();
    GX_SetColorUpdate(GX_TRUE);
    GX_SetAlphaUpdate(GX_FALSE);
    mode = MODE_NONE;
    world_base = NULL;
}

/* ---- world --------------------------------------------------------------------------------------------------- */

#define WORLD_STRIDE 28

static void world_mode(void) {
    if (mode == MODE_WORLD) return;
    mode = MODE_WORLD;
    GX_ClearVtxDesc();
    GX_SetVtxDesc(GX_VA_POS, GX_INDEX16);
    GX_SetVtxDesc(GX_VA_CLR0, GX_INDEX16);
    GX_SetVtxDesc(GX_VA_TEX0, GX_INDEX16);
    GX_SetVtxDesc(GX_VA_TEX1, GX_INDEX16);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_POS, GX_POS_XYZ, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_TEX0, GX_TEX_ST, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_TEX1, GX_TEX_ST, GX_U16, 15);
    GX_SetNumChans(1);
    GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    GX_SetNumTexGens(2);
    GX_SetTexCoordGen(GX_TEXCOORD0, GX_TG_MTX2x4, GX_TG_TEX0, GX_IDENTITY);
    GX_SetTexCoordGen(GX_TEXCOORD1, GX_TG_MTX2x4, GX_TG_TEX1, GX_IDENTITY);
}

void mwf_world_arrays(const void *v) {
    world_mode();
    if (v == world_base) return;
    world_base = v;
    const uint8_t *b = (const uint8_t *)v;
    GX_SetArray(GX_VA_POS, (void *)(b + 0), WORLD_STRIDE);
    GX_SetArray(GX_VA_CLR0, (void *)(b + 12), WORLD_STRIDE);
    GX_SetArray(GX_VA_TEX0, (void *)(b + 16), WORLD_STRIDE);
    GX_SetArray(GX_VA_TEX1, (void *)(b + 24), WORLD_STRIDE);
    GX_InvVtxCache();
}

static void stage_ops(u8 stage, u8 scale) {
    GX_SetTevColorOp(stage, GX_TEV_ADD, GX_TB_ZERO, scale, GX_TRUE, GX_TEVPREV);
    GX_SetTevAlphaOp(stage, GX_TEV_ADD, GX_TB_ZERO, GX_CS_SCALE_1, GX_TRUE, GX_TEVPREV);
}

/* IW4 front faces are clockwise on screen (D3D). VERIFY on Dolphin/hardware: if surfaces vanish and their insides
 * show, swap these two. */
#define MWF_GX_CULL_BACK GX_CULL_BACK
#define MWF_GX_CULL_FRONT GX_CULL_FRONT

void mwf_world_state(const mwf_draw_state *s) {
    int has_tex = s->tex >= 0 && s->tex < tex_count;
    int lit = s->lightmap >= 0 && s->lightmap < tex_count;
    int has_plane = has_tex && tex_has_alpha[s->tex];
    u8 st = GX_TEVSTAGE0;

    if (has_tex) {
        GX_LoadTexObj(&tex_obj[s->tex], GX_TEXMAP0);
        GX_SetTevOrder(st, GX_TEXCOORD0, GX_TEXMAP0, GX_COLOR0A0);
        if (lit)
            GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_ZERO, GX_CC_ZERO, GX_CC_TEXC);
        else
            GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_TEXC, GX_CC_RASC, GX_CC_ZERO);
        GX_SetTevAlphaIn(st, GX_CA_ZERO, GX_CA_TEXA, GX_CA_RASA, GX_CA_ZERO);
    } else {
        GX_SetTevOrder(st, GX_TEXCOORDNULL, GX_TEXMAP_NULL, GX_COLOR0A0);
        GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_ZERO, GX_CC_ZERO, lit ? GX_CC_ONE : GX_CC_RASC);
        GX_SetTevAlphaIn(st, GX_CA_ZERO, GX_CA_ZERO, GX_CA_ZERO, GX_CA_RASA);
    }
    stage_ops(st, GX_CS_SCALE_1);
    st++;

    if (lit) {
        GX_LoadTexObj(&tex_obj[s->lightmap], GX_TEXMAP1);
        GX_SetTevOrder(st, GX_TEXCOORD1, GX_TEXMAP1, GX_COLORNULL);
        GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_CPREV, GX_CC_TEXC, GX_CC_ZERO);
        GX_SetTevAlphaIn(st, GX_CA_ZERO, GX_CA_ZERO, GX_CA_ZERO, GX_CA_APREV);
        stage_ops(st, GX_CS_SCALE_2);
        st++;
    }

    if (has_plane) {
        GX_LoadTexObj(&tex_alpha[s->tex], GX_TEXMAP2);
        GX_SetTevOrder(st, GX_TEXCOORD0, GX_TEXMAP2, GX_COLOR0A0);
        GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_ZERO, GX_CC_ZERO, GX_CC_CPREV);
        GX_SetTevAlphaIn(st, GX_CA_ZERO, GX_CA_TEXA, GX_CA_RASA, GX_CA_ZERO);
        stage_ops(st, GX_CS_SCALE_1);
        st++;
    }
    GX_SetNumTevStages(st);

    switch (s->blend) {
    case MWF_BLEND_ALPHA_TEST:
        GX_SetBlendMode(GX_BM_NONE, GX_BL_ONE, GX_BL_ZERO, GX_LO_CLEAR);
        GX_SetAlphaCompare(GX_GEQUAL, s->alpha_ref, GX_AOP_AND, GX_ALWAYS, 0);
        GX_SetZCompLoc(GX_FALSE); /* Z after texturing, so cut-out texels do not write depth */
        GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_TRUE);
        break;
    case MWF_BLEND_BLEND:
        GX_SetBlendMode(GX_BM_BLEND, GX_BL_SRCALPHA, GX_BL_INVSRCALPHA, GX_LO_CLEAR);
        GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
        GX_SetZCompLoc(GX_TRUE);
        GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_FALSE);
        break;
    case MWF_BLEND_ADD:
        GX_SetBlendMode(GX_BM_BLEND, GX_BL_ONE, GX_BL_ONE, GX_LO_CLEAR);
        GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
        GX_SetZCompLoc(GX_TRUE);
        GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_FALSE);
        break;
    case MWF_BLEND_MULTIPLY:
        GX_SetBlendMode(GX_BM_BLEND, GX_BL_DSTCLR, GX_BL_ZERO, GX_LO_CLEAR);
        GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
        GX_SetZCompLoc(GX_TRUE);
        GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_FALSE);
        break;
    default:
        GX_SetBlendMode(GX_BM_NONE, GX_BL_ONE, GX_BL_ZERO, GX_LO_CLEAR);
        GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
        GX_SetZCompLoc(GX_TRUE);
        GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_TRUE);
        break;
    }
    GX_SetCullMode(s->cull == MWF_CULL_BACK    ? MWF_GX_CULL_BACK
                   : s->cull == MWF_CULL_FRONT ? MWF_GX_CULL_FRONT
                                               : GX_CULL_NONE);
}

void mwf_call_dl(const void *dl, uint32_t len) {
    GX_CallDispList((void *)dl, len);
}

/* ---- sky ----------------------------------------------------------------------------------------------------- */

void mwf_sky_face(const float view_rot[3][4], const float c[4][5], int32_t tex) {
    if (tex < 0 || tex >= tex_count) return;
    if (mode != MODE_SKY) {
        mode = MODE_SKY;
        world_base = NULL;
        GX_ClearVtxDesc();
        GX_SetVtxDesc(GX_VA_POS, GX_DIRECT);
        GX_SetVtxDesc(GX_VA_TEX0, GX_DIRECT);
        GX_SetVtxAttrFmt(GX_VTXFMT1, GX_VA_POS, GX_POS_XYZ, GX_F32, 0);
        GX_SetVtxAttrFmt(GX_VTXFMT1, GX_VA_TEX0, GX_TEX_ST, GX_F32, 0);
        GX_SetNumChans(0);
        GX_SetNumTexGens(1);
        GX_SetTexCoordGen(GX_TEXCOORD0, GX_TG_MTX2x4, GX_TG_TEX0, GX_IDENTITY);
        GX_SetNumTevStages(1);
        GX_SetTevOrder(GX_TEVSTAGE0, GX_TEXCOORD0, GX_TEXMAP0, GX_COLORNULL);
        GX_SetTevOp(GX_TEVSTAGE0, GX_REPLACE);
        GX_SetBlendMode(GX_BM_NONE, GX_BL_ONE, GX_BL_ZERO, GX_LO_CLEAR);
        GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
        GX_SetZMode(GX_FALSE, GX_ALWAYS, GX_FALSE);
        GX_SetCullMode(GX_CULL_NONE);
        GX_LoadPosMtxImm((f32(*)[4])view_rot, GX_PNMTX1);
    }
    GX_SetCurrentMtx(GX_PNMTX1);
    GX_LoadTexObj(&tex_obj[tex], GX_TEXMAP0);
    GX_Begin(GX_QUADS, GX_VTXFMT1, 4);
    for (int i = 0; i < 4; i++) {
        GX_Position3f32(c[i][0], c[i][1], c[i][2]);
        GX_TexCoord2f32(c[i][3], c[i][4]);
    }
    GX_End();
    GX_SetCurrentMtx(GX_PNMTX0);
}

void mwf_frame_end(void) {
    mwf_gx_end_frame();
}
