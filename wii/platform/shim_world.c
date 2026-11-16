/* Game world rendering on GX: the C half of the pack renderer (Rust drives it; see shim.h).
 *
 * Data comes from a .wpk loaded into MEM2 and flushed once: texture data, interleaved world vertices and prebuilt
 * display lists are used where they lie. Recipes follow docs/wii-gx.md:
 *   stage 0: colour texture (x vertex colour when unlit), alpha = texture alpha x vertex alpha
 *   stage 1: x lightmap x 2                              (lit draws)
 *   stage 2: alpha from the I4 alpha plane x vertex alpha (split textures)
 */
#include <dirent.h>
#include <malloc.h>
#include <sys/stat.h>
#include <fat.h>
#include <strings.h>
#include <asndlib.h>
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

uint32_t mwf_mem2_mark(void) {
    mem2_init();
    return (uint32_t)mem2_lo;
}

void mwf_mem2_release(uint32_t mark) {
    /* The GPU may still be drawing from what is released (frames are pipelined). */
    mwf_gpu_idle();
    mem2_init();
    if (mark && (uintptr_t)mark <= mem2_lo) mem2_lo = (uintptr_t)mark;
}

static char app_dir[128] = "sd:/apps/mwf_game";

void mwf_set_app_dir(const char *argv0) {
    if (!argv0) return;
    const char *slash = strrchr(argv0, '/');
    /* Needs a device ("sd:" / "usb:") and a folder before the file name. */
    if (!slash || !strchr(argv0, ':') || slash == argv0) return;
    size_t n = (size_t)(slash - argv0);
    if (n >= sizeof app_dir) return;
    memcpy(app_dir, argv0, n);
    app_dir[n] = 0;
}

const char *mwf_app_dir(void) {
    return app_dir;
}

static int fat_state = -1; /* -1 not tried, 0 failed, 1 mounted */

int mwf_list_dir(const char *dir, const char *suffix, char *out, uint32_t cap) {
    if (fat_state < 0) fat_state = fatInitDefault() ? 1 : 0;
    if (!fat_state || cap == 0) return 0;
    DIR *d = opendir(dir);
    if (!d) return 0;
    size_t sl = strlen(suffix);
    uint32_t used = 0;
    int n = 0;
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        size_t l = strlen(e->d_name);
        if (l <= sl || strcasecmp(e->d_name + l - sl, suffix) != 0) continue;
        size_t stem = l - sl;
        if (used + stem + 1 >= cap) break;
        memcpy(out + used, e->d_name, stem);
        used += (uint32_t)stem;
        out[used++] = '\n';
        n++;
    }
    closedir(d);
    out[used < cap ? used : cap - 1] = 0;
    return n;
}

static int32_t file_error = 0;

int32_t mwf_file_error(void) {
    return file_error;
}

static FILE *open_at(const char *path, uint32_t offset) {
    if (fat_state < 0) fat_state = fatInitDefault() ? 1 : 0;
    if (!fat_state) {
        file_error = 1;
        return NULL;
    }
    FILE *f = fopen(path, "rb");
    if (!f) {
        file_error = 2;
        return NULL;
    }
    if (fseek(f, (long)offset, SEEK_SET) != 0) {
        fclose(f);
        file_error = 4;
        return NULL;
    }
    return f;
}

int32_t mwf_file_append(const char *path, const void *data, uint32_t len) {
    file_error = 0;
    FILE *f = fopen(path, "ab");
    if (!f) {
        file_error = 1;
        return -1;
    }
    size_t put = fwrite(data, 1, len, f);
    fclose(f);
    if (put != len) file_error = 4;
    return (int32_t)put;
}

int32_t mwf_mkdir(const char *path) {
    return mkdir(path, 0777);
}

/* Free MEM1 for the heap: newlib's free blocks plus the arena it hasn't claimed yet. */
uint32_t mwf_mem1_free_bytes(void) {
    struct mallinfo mi = mallinfo();
    uint32_t arena = (uint32_t)((uintptr_t)SYS_GetArena1Hi() - (uintptr_t)SYS_GetArena1Lo());
    return arena + (uint32_t)mi.fordblks;
}

int32_t mwf_file_write(const char *path, const void *data, uint32_t len) {
    file_error = 0;
    FILE *f = fopen(path, "wb");
    if (!f) {
        file_error = 1;
        return -1;
    }
    size_t put = fwrite(data, 1, len, f);
    fclose(f);
    if (put != len) file_error = 4;
    return (int32_t)put;
}

int32_t mwf_file_read(const char *path, uint32_t offset, uint32_t len, void *dst) {
    file_error = 0;
    FILE *f = open_at(path, offset);
    if (!f) return -1;
    size_t got = fread(dst, 1, len, f);
    fclose(f);
    if (got != len) file_error = 4;
    return (int32_t)got;
}

void *mwf_file_load_range(const char *path, uint32_t offset, uint32_t len) {
    file_error = 0;
    uint32_t padded = (len + 31u) & ~31u;
    if (padded == 0) return NULL;
    void *p = mwf_mem2_alloc(padded, 32);
    if (!p) {
        file_error = 3;
        return NULL;
    }
    FILE *f = open_at(path, offset);
    if (!f) return NULL;
    size_t got = fread(p, 1, len, f);
    fclose(f);
    if (got != len) {
        file_error = 4;
        return NULL;
    }
    memset((uint8_t *)p + len, 0, padded - len);
    DCFlushRange(p, padded);
    return p;
}

void *mwf_file_load(const char *path, uint32_t *size_out) {
    file_error = 0;
    if (fat_state < 0) fat_state = fatInitDefault() ? 1 : 0;
    if (!fat_state) {
        file_error = 1;
        return NULL;
    }
    FILE *f = fopen(path, "rb");
    if (!f) {
        file_error = 2;
        return NULL;
    }
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
        file_error = 3;
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

void mwf_tex_reset(void) {
    tex_count = 0;
    GX_InvalidateTexAll();
}

int32_t mwf_tex_mark(void) {
    return tex_count;
}

void mwf_tex_release(int32_t mark) {
    mwf_gpu_idle();
    if (mark >= 0 && mark <= tex_count) tex_count = mark;
    GX_InvalidateTexAll();
}

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

enum { MODE_NONE, MODE_SKY, MODE_WORLD, MODE_HUD, MODE_VIEW, MODE_UI, MODE_VIEW_MTX };

extern GXRModeObj *mwf_rmode;
/* Depth range split: the first-person model gets [0, VIEW_DEPTH], the world [VIEW_DEPTH, 1]. */
#define VIEW_DEPTH 0.02f
static int mode = MODE_NONE;
static const void *world_base = NULL;
/* The world's projection of this frame (boxes drawn in world space reuse it). */
static Mtx44 world_proj;

/* Distance fog for world draws (linear, in eye depth), set by mwf_fog; off for the sky, the first-person model, the HUD
 * and menus. */
static int fog_on = 0;
static float fog_start = 0.0f, fog_end = 1.0f, frame_near = 1.0f, frame_far = 1000.0f;
static GXColor fog_colour = {0, 0, 0, 0xff};

static void fog_world(int on) {
    if (on && fog_on) {
        /* GX rebuilds eye depth from screen z as A / (B - z) assuming the depth range [0, 1]; the world draws in
         * [VIEW_DEPTH, 1], so pass the near and far that give the same A and B for that range:
         * B = d + (1 - d) f / (f - n), A = (1 - d) f n / (f - n). */
        float n = frame_near, f = frame_far, d = VIEW_DEPTH;
        float b = d + (1.0f - d) * f / (f - n);
        float n2 = (1.0f - d) * f * n / ((f - n) * b);
        float f2 = b * n2 / (b - 1.0f);
        GX_SetFog(GX_FOG_LIN, fog_start, fog_end, n2, f2, fog_colour);
    } else {
        GX_SetFog(GX_FOG_NONE, 0.0f, 1.0f, 0.1f, 1.0f, fog_colour);
    }
}

void mwf_fog(int32_t on, float start, float end, uint32_t rgba) {
    fog_on = on != 0 && end > start;
    fog_start = start;
    fog_end = end;
    fog_colour = (GXColor){(u8)(rgba >> 24), (u8)(rgba >> 16), (u8)(rgba >> 8), 0xff};
}

void mwf_frame_begin(const float view[3][4], float fovy_deg, float znear, float zfar, uint32_t clear_rgba) {
    frame_near = znear;
    frame_far = zfar;
    /* Fog is per frame: whoever draws a world sets it after this (World::draw does). */
    fog_on = 0;
    GXColor bg = {(u8)(clear_rgba >> 24), (u8)(clear_rgba >> 16), (u8)(clear_rgba >> 8), 0xff};
    GX_SetCopyClear(bg, GX_MAX_Z24);
    f32 aspect = (CONF_GetAspectRatio() == CONF_ASPECT_16_9) ? (16.0f / 9.0f) : (4.0f / 3.0f);
    Mtx44 proj;
    guPerspective(proj, fovy_deg, aspect, znear, zfar);
    GX_LoadProjectionMtx(proj, GX_PERSPECTIVE);
    memcpy(world_proj, proj, sizeof(Mtx44));
    GX_LoadPosMtxImm((f32(*)[4])view, GX_PNMTX0);
    GX_SetCurrentMtx(GX_PNMTX0);
    GX_SetViewport(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight, VIEW_DEPTH, 1.0f);
    GX_InvalidateTexAll();
    GX_SetColorUpdate(GX_TRUE);
    GX_SetAlphaUpdate(GX_FALSE);
    mode = MODE_NONE;
    world_base = NULL;
}

/* ---- world --------------------------------------------------------------------------------------------------- */

#define WORLD_STRIDE 28

/* The world display-list layout set now (wpk::quant::dl_flags; world_mode sets the default, 0). */
#define MWF_DL_IDX8 1
#define MWF_DL_NO_COLOR 2
static u8 world_dl_flags = 0;

static void world_mode(void) {
    if (mode == MODE_WORLD) return;
    mode = MODE_WORLD;
    world_dl_flags = 0;
    fog_world(1);
    GX_ClearVtxDesc();
    GX_SetVtxDesc(GX_VA_POS, GX_INDEX16);
    GX_SetVtxDesc(GX_VA_CLR0, GX_INDEX16);
    GX_SetVtxDesc(GX_VA_TEX0, GX_INDEX16);
    GX_SetVtxDesc(GX_VA_TEX1, GX_INDEX16);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_POS, GX_POS_XYZ, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_TEX0, GX_TEX_ST, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT0, GX_VA_TEX1, GX_TEX_ST, GX_U16, 15);
    /* Compact vertices (wpk::quant): positions s16 at 1/8 unit, UVs s16 with 12 / 10 / 8 fraction bits. */
    static const u8 uv_frac[3] = {12, 10, 8};
    for (int i = 0; i < 3; i++) {
        u8 fmt = (u8)(GX_VTXFMT3 + i);
        GX_SetVtxAttrFmt(fmt, GX_VA_POS, GX_POS_XYZ, GX_S16, 3);
        GX_SetVtxAttrFmt(fmt, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
        GX_SetVtxAttrFmt(fmt, GX_VA_TEX0, GX_TEX_ST, GX_S16, uv_frac[i]);
        GX_SetVtxAttrFmt(fmt, GX_VA_TEX1, GX_TEX_ST, GX_U16, 15);
    }
    GX_SetNumChans(1);
    GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    GX_SetNumTexGens(2);
    GX_SetTexCoordGen(GX_TEXCOORD0, GX_TG_MTX2x4, GX_TG_TEX0, GX_IDENTITY);
    GX_SetTexCoordGen(GX_TEXCOORD1, GX_TG_MTX2x4, GX_TG_TEX1, GX_IDENTITY);
}

void mwf_world_arrays(const void *v, uint32_t stride) {
    world_mode();
    if (v == world_base) return;
    world_base = v;
    const uint8_t *b = (const uint8_t *)v;
    if (stride == WORLD_STRIDE) {
        GX_SetArray(GX_VA_POS, (void *)(b + 0), WORLD_STRIDE);
        GX_SetArray(GX_VA_CLR0, (void *)(b + 12), WORLD_STRIDE);
        GX_SetArray(GX_VA_TEX0, (void *)(b + 16), WORLD_STRIDE);
        GX_SetArray(GX_VA_TEX1, (void *)(b + 24), WORLD_STRIDE);
    } else {
        /* 18-byte compact vertex: pos 0, rgba 6, uv 10, lm 14. */
        GX_SetArray(GX_VA_POS, (void *)(b + 0), (u8)stride);
        GX_SetArray(GX_VA_CLR0, (void *)(b + 6), (u8)stride);
        GX_SetArray(GX_VA_TEX0, (void *)(b + 10), (u8)stride);
        GX_SetArray(GX_VA_TEX1, (void *)(b + 14), (u8)stride);
    }
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

void mwf_world_tint(uint32_t rgba) {
    GXColor k = {(u8)(rgba >> 24), (u8)(rgba >> 16), (u8)(rgba >> 8), 0xff};
    GX_SetTevKColor(GX_KCOLOR0, k);
}

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

    if (s->tint != 0) {
        /* Static model light: colour x konst x 2 (the konst holds half the light, like lightmaps). */
        mwf_world_tint(s->tint);
        GX_SetTevKColorSel(st, GX_TEV_KCSEL_K0);
        GX_SetTevOrder(st, GX_TEXCOORDNULL, GX_TEXMAP_NULL, GX_COLORNULL);
        GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_CPREV, GX_CC_KONST, GX_CC_ZERO);
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

/* How the next world display lists index their vertices: u8 or u16 indices, with or without a colour index. Without
 * one, the colour channel's material colour comes from a white register (the batch's vertices were all white), so
 * TEV sees the same colour either way. Skipped when unchanged. */
void mwf_world_desc(uint8_t flags) {
    world_mode();
    if (flags == world_dl_flags) return;
    world_dl_flags = flags;
    u8 idx = (flags & MWF_DL_IDX8) ? GX_INDEX8 : GX_INDEX16;
    GX_SetVtxDesc(GX_VA_POS, idx);
    GX_SetVtxDesc(GX_VA_CLR0, (flags & MWF_DL_NO_COLOR) ? GX_NONE : idx);
    GX_SetVtxDesc(GX_VA_TEX0, idx);
    GX_SetVtxDesc(GX_VA_TEX1, idx);
    if (flags & MWF_DL_NO_COLOR) {
        GXColor white = {255, 255, 255, 255};
        GX_SetChanMatColor(GX_COLOR0A0, white);
        GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_REG, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    } else {
        GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    }
}

void mwf_call_dl(const void *dl, uint32_t len) {
    GX_CallDispList((void *)dl, len);
}

/* ---- sky ----------------------------------------------------------------------------------------------------- */

void mwf_sky_face(const float view_rot[3][4], const float c[4][5], int32_t tex) {
    if (tex < 0 || tex >= tex_count) return;
    if (mode != MODE_SKY) {
        mode = MODE_SKY;
        fog_world(0);
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

void mwf_pos_matrix(const float m[3][4]) {
    GX_LoadPosMtxImm((f32(*)[4])m, GX_PNMTX0);
}

/* ---- first-person model ---------------------------------------------------------------------------------------- */

void mwf_flush(const void *p, uint32_t len) {
    DCFlushRange((void *)p, len);
}

/* ---- Sound (libogc ASND: 16 voices mixed by the DSP) ---- */

void mwf_snd_init(void) {
    ASND_Init();
    ASND_Pause(0);
}

/* A voice for a new sound: a free one, else the one playing longest (voice stealing: footsteps of six players and
   full-auto fire with long tails can keep all 16 busy, and a dropped gunshot is worse than a cut-off tail). ASND hands
   out voice 0 last; stealing picks among 1..15. */
static s32 claim_voice(void);
static u32 voice_steals = 0;

uint32_t mwf_snd_steals(void) {
    return voice_steals;
}

int32_t mwf_snd_play(const void *pcm, uint32_t bytes, uint32_t rate, uint8_t vol_l, uint8_t vol_r) {
    s32 voice = claim_voice();
    if (voice < 0)
        return -1;
    if (ASND_SetVoice(voice, VOICE_MONO_16BIT, (s32)rate, 0, (void *)pcm, (s32)bytes, vol_l, vol_r, NULL) != SND_OK)
        return -1;
    return voice;
}

/* DSP ADPCM clips (`--adpcm`): ASND only mixes PCM, so each voice playing one gets two PCM buffers that the voice
   callback refills (decoding with the game's `mwf_adpcm_decode`, `wpk::adpcm`) while the other plays. The callback
   runs in the audio interrupt: decoding is integer-only, about 2048 samples per call. */
#define ADPCM_BUF_SAMPLES 2048
typedef struct {
    const uint8_t *clip; /* NULL: not an ADPCM voice, or nothing left to queue */
    uint32_t len;
    uint32_t pos; /* next sample to decode */
    int16_t hist[2];
    uint8_t next; /* buffer the next decode goes to */
} adpcm_voice;
static adpcm_voice adpcm_voices[MAX_SND_VOICES];
static int16_t adpcm_pcm[MAX_SND_VOICES][2][ADPCM_BUF_SAMPLES] ATTRIBUTE_ALIGN(32);

/* Decode the next piece of voice `v`'s clip into buffer `b`; its byte length (padded to 32), or 0 at the end. */
static uint32_t adpcm_fill(s32 v, uint8_t b) {
    adpcm_voice *a = &adpcm_voices[v];
    int16_t *out = adpcm_pcm[v][b];
    uint32_t n = mwf_adpcm_decode(a->clip, a->len, a->pos, a->hist, out, ADPCM_BUF_SAMPLES);
    if (n == 0)
        return 0;
    a->pos += n;
    uint32_t bytes = (n * 2 + 31) & ~31u;
    if (bytes > n * 2)
        memset((uint8_t *)out + n * 2, 0, bytes - n * 2);
    DCFlushRange(out, bytes);
    return bytes;
}

/* ASND voice callback: queue the next piece while the current one plays. */
static void adpcm_more(s32 v) {
    if (v < 0 || v >= MAX_SND_VOICES)
        return;
    adpcm_voice *a = &adpcm_voices[v];
    if (!a->clip || !ASND_TestVoiceBufferReady(v))
        return;
    uint32_t bytes = adpcm_fill(v, a->next);
    if (bytes == 0) {
        a->clip = NULL;
        return;
    }
    ASND_AddVoice(v, adpcm_pcm[v][a->next], (s32)bytes);
    a->next ^= 1;
}

static u32 voice_started[MAX_SND_VOICES];
static u32 voice_clock = 0;

static s32 claim_voice(void) {
    s32 v = ASND_GetFirstUnusedVoice();
    if (v < 0 || v >= MAX_SND_VOICES) {
        u32 oldest = 0xffffffffu;
        v = -1;
        for (s32 i = 1; i < MAX_SND_VOICES; i++)
            if (voice_started[i] < oldest) {
                oldest = voice_started[i];
                v = i;
            }
        if (v < 0)
            return -1;
        u32 level = IRQ_Disable();
        adpcm_voices[v].clip = NULL;
        IRQ_Restore(level);
        ASND_StopVoice(v);
        voice_steals++;
    }
    voice_started[v] = ++voice_clock;
    return v;
}

uint32_t mwf_snd_voice_clock(int32_t voice) {
    return (voice >= 0 && voice < MAX_SND_VOICES) ? voice_started[voice] : 0;
}

void mwf_snd_stop_if(int32_t voice, uint32_t clock) {
    if (voice < 0 || voice >= MAX_SND_VOICES || clock == 0 || voice_started[voice] != clock)
        return;
    u32 level = IRQ_Disable();
    adpcm_voices[voice].clip = NULL;
    IRQ_Restore(level);
    ASND_StopVoice(voice);
}

int32_t mwf_snd_play_adpcm(const void *clip, uint32_t bytes, uint32_t rate, uint8_t vol_l, uint8_t vol_r) {
    s32 voice = claim_voice();
    if (voice < 0 || voice >= MAX_SND_VOICES)
        return -1;
    u32 level = IRQ_Disable();
    adpcm_voice *a = &adpcm_voices[voice];
    a->clip = (const uint8_t *)clip;
    a->len = bytes;
    a->pos = 0;
    a->hist[0] = a->hist[1] = 0;
    a->next = 0;
    uint32_t first = adpcm_fill(voice, 0);
    a->next = 1;
    s32 ok = first ? ASND_SetVoice(voice, VOICE_MONO_16BIT, (s32)rate, 0, adpcm_pcm[voice][0], (s32)first, vol_l, vol_r,
                                   adpcm_more)
                   : SND_INVALID;
    if (ok != SND_OK)
        a->clip = NULL;
    IRQ_Restore(level);
    return ok == SND_OK ? voice : -1;
}

void mwf_snd_stop_all(void) {
    u32 level = IRQ_Disable();
    for (s32 v = 0; v < MAX_SND_VOICES; v++)
        adpcm_voices[v].clip = NULL;
    IRQ_Restore(level);
    for (s32 v = 0; v < MAX_SND_VOICES; v++)
        ASND_StopVoice(v);
}

void mwf_view_begin(float fovy_deg, float znear, float zfar) {
    fog_world(0);
    f32 aspect = (CONF_GetAspectRatio() == CONF_ASPECT_16_9) ? (16.0f / 9.0f) : (4.0f / 3.0f);
    Mtx44 proj;
    guPerspective(proj, fovy_deg, aspect, znear, zfar);
    GX_LoadProjectionMtx(proj, GX_PERSPECTIVE);
    GX_SetViewport(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight, 0.0f, VIEW_DEPTH);
    GX_SetCurrentMtx(GX_PNMTX0);
}

void mwf_world_view(void) {
    fog_world(1);
    GX_LoadProjectionMtx(world_proj, GX_PERSPECTIVE);
    GX_SetViewport(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight, VIEW_DEPTH, 1.0f);
    GX_SetCurrentMtx(GX_PNMTX0);
}

void mwf_view_arrays(const void *pos, const void *attr) {
    if (mode != MODE_VIEW) {
        mode = MODE_VIEW;
        GX_ClearVtxDesc();
        GX_SetVtxDesc(GX_VA_POS, GX_INDEX16);
        GX_SetVtxDesc(GX_VA_CLR0, GX_INDEX16);
        GX_SetVtxDesc(GX_VA_TEX0, GX_INDEX16);
        GX_SetVtxAttrFmt(GX_VTXFMT6, GX_VA_POS, GX_POS_XYZ, GX_F32, 0);
        GX_SetVtxAttrFmt(GX_VTXFMT6, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
        GX_SetVtxAttrFmt(GX_VTXFMT6, GX_VA_TEX0, GX_TEX_ST, GX_F32, 0);
        GX_SetNumChans(1);
        GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
        GX_SetNumTexGens(1);
        GX_SetTexCoordGen(GX_TEXCOORD0, GX_TG_MTX2x4, GX_TG_TEX0, GX_IDENTITY);
    }
    world_base = NULL;
    GX_SetArray(GX_VA_POS, (void *)pos, 12);
    GX_SetArray(GX_VA_CLR0, (void *)attr, 12);
    GX_SetArray(GX_VA_TEX0, (void *)((const uint8_t *)attr + 4), 12);
    GX_InvVtxCache();
}

/* As mwf_view_arrays, for display lists whose vertices start with a position-matrix index (u8, direct: the GX
 * matrix slot x 3), so GX places rigid parts with their bone's matrix (mwf_pos_matrix_slot). */
void mwf_view_arrays_indexed(const void *pos, const void *attr) {
    if (mode != MODE_VIEW_MTX) {
        mwf_view_arrays(pos, attr);
        mode = MODE_VIEW_MTX;
        GX_SetVtxDesc(GX_VA_PTNMTXIDX, GX_DIRECT);
    }
    world_base = NULL;
    GX_SetArray(GX_VA_POS, (void *)pos, 12);
    GX_SetArray(GX_VA_CLR0, (void *)attr, 12);
    GX_SetArray(GX_VA_TEX0, (void *)((const uint8_t *)attr + 4), 12);
    GX_InvVtxCache();
}

/* Load `m` into position matrix slot `slot` (0..9). */
void mwf_pos_matrix_slot(const float m[3][4], uint32_t slot) {
    if (slot < 10) GX_LoadPosMtxImm((f32(*)[4])m, GX_PNMTX0 + slot * 3);
}

/* ---- HUD ------------------------------------------------------------------------------------------------------ */

static void hud_mode(void) {
    if (mode == MODE_HUD) return;
    mode = MODE_HUD;
    fog_world(0);
    world_base = NULL;
    Mtx44 ortho;
    guOrtho(ortho, 0, 480, 0, 640, -1, 1);
    GX_LoadProjectionMtx(ortho, GX_ORTHOGRAPHIC);
    Mtx id;
    guMtxIdentity(id);
    GX_LoadPosMtxImm(id, GX_PNMTX2);
    GX_SetCurrentMtx(GX_PNMTX2);
    GX_ClearVtxDesc();
    GX_SetVtxDesc(GX_VA_POS, GX_DIRECT);
    GX_SetVtxAttrFmt(GX_VTXFMT2, GX_VA_POS, GX_POS_XY, GX_S16, 0);
    GX_SetNumChans(1);
    GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_REG, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    GX_SetNumTexGens(0);
    GX_SetNumTevStages(1);
    GX_SetTevOrder(GX_TEVSTAGE0, GX_TEXCOORDNULL, GX_TEXMAP_NULL, GX_COLOR0A0);
    GX_SetTevOp(GX_TEVSTAGE0, GX_PASSCLR);
    GX_SetBlendMode(GX_BM_BLEND, GX_BL_SRCALPHA, GX_BL_INVSRCALPHA, GX_LO_CLEAR);
    GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
    GX_SetZMode(GX_FALSE, GX_ALWAYS, GX_FALSE);
    GX_SetCullMode(GX_CULL_NONE);
}

/* Textured 2D quads on the 640x480 screen (menus, MW2 text): texture x vertex colour, alpha blended. */
static void ui_mode(void) {
    if (mode == MODE_UI) return;
    mode = MODE_UI;
    fog_world(0);
    world_base = NULL;
    Mtx44 ortho;
    guOrtho(ortho, 0, 480, 0, 640, -1, 1);
    GX_LoadProjectionMtx(ortho, GX_ORTHOGRAPHIC);
    Mtx id;
    guMtxIdentity(id);
    GX_LoadPosMtxImm(id, GX_PNMTX2);
    GX_SetCurrentMtx(GX_PNMTX2);
    GX_ClearVtxDesc();
    GX_SetVtxDesc(GX_VA_POS, GX_DIRECT);
    GX_SetVtxDesc(GX_VA_CLR0, GX_DIRECT);
    GX_SetVtxDesc(GX_VA_TEX0, GX_DIRECT);
    GX_SetVtxAttrFmt(GX_VTXFMT2, GX_VA_POS, GX_POS_XY, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT2, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT2, GX_VA_TEX0, GX_TEX_ST, GX_F32, 0);
    GX_SetNumChans(1);
    GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    GX_SetNumTexGens(1);
    GX_SetTexCoordGen(GX_TEXCOORD0, GX_TG_MTX2x4, GX_TG_TEX0, GX_IDENTITY);
    GX_SetNumTevStages(1);
    GX_SetTevOrder(GX_TEVSTAGE0, GX_TEXCOORD0, GX_TEXMAP0, GX_COLOR0A0);
    GX_SetTevOp(GX_TEVSTAGE0, GX_MODULATE);
    GX_SetBlendMode(GX_BM_BLEND, GX_BL_SRCALPHA, GX_BL_INVSRCALPHA, GX_LO_CLEAR);
    GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
    GX_SetZMode(GX_FALSE, GX_ALWAYS, GX_FALSE);
    GX_SetCullMode(GX_CULL_NONE);
}

/* The last frame shown, at half size (RGB565), for the in-game menus to draw behind them (`MWF_TEX_GRAB`). */
#define GRAB_W 320
#define GRAB_H 240
static u16 *grab_texels = NULL;
static GXTexObj grab_obj;
static int grab_ok = 0;

static u16 rgb565(int y, int cb, int cr) {
    int c = 298 * (y - 16);
    int r = (c + 409 * cr + 128) >> 8, g = (c - 100 * cb - 208 * cr + 128) >> 8, b = (c + 516 * cb + 128) >> 8;
    r = r < 0 ? 0 : (r > 255 ? 255 : r);
    g = g < 0 ? 0 : (g > 255 ? 255 : g);
    b = b < 0 ? 0 : (b > 255 ? 255 : b);
    return (u16)(((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3));
}

int32_t mwf_grab_screen(void) {
    const u8 *src = (const u8 *)mwf_last_frame();
    if (!src) return 0;
    if (!grab_texels) {
        grab_texels = memalign(32, GRAB_W * GRAB_H * 2);
        if (!grab_texels) return 0;
    }
    const u32 fw = mwf_rmode->fbWidth, fh = mwf_rmode->xfbHeight;
    /* GX RGB565 is 4 x 4 texel tiles, row by row. Each texel samples one pixel pair of the YUYV frame. */
    for (u32 ty = 0; ty < GRAB_H; ty += 4) {
        for (u32 tx = 0; tx < GRAB_W; tx += 4) {
            u16 *tile = grab_texels + (ty * GRAB_W + tx * 4);
            for (u32 y = 0; y < 4; y++) {
                const u32 sy = (ty + y) * fh / GRAB_H;
                for (u32 x = 0; x < 4; x++) {
                    const u32 sx = ((tx + x) * fw / GRAB_W) & ~1u;
                    const u8 *p = src + (sy * fw + sx) * 2;
                    tile[y * 4 + x] = rgb565(p[0], p[1] - 128, p[3] - 128);
                }
            }
        }
    }
    DCFlushRange(grab_texels, GRAB_W * GRAB_H * 2);
    GX_InitTexObj(&grab_obj, grab_texels, GRAB_W, GRAB_H, GX_TF_RGB565, GX_CLAMP, GX_CLAMP, GX_FALSE);
    GX_InvalidateTexAll();
    grab_ok = 1;
    return 1;
}

/* Bind a UI texture: colour times the vertex colour, and when the texture keeps its smooth alpha in a separate I4
 * plane (`.gxt` flag bit 0), a second stage takes the alpha from that plane. */
static void ui_bind(const GXTexObj *colour, const GXTexObj *alpha) {
    GX_LoadTexObj((GXTexObj *)colour, GX_TEXMAP0);
    if (alpha) {
        GX_LoadTexObj((GXTexObj *)alpha, GX_TEXMAP1);
        GX_SetTevOrder(GX_TEVSTAGE1, GX_TEXCOORD0, GX_TEXMAP1, GX_COLOR0A0);
        GX_SetTevColorIn(GX_TEVSTAGE1, GX_CC_ZERO, GX_CC_ZERO, GX_CC_ZERO, GX_CC_CPREV);
        GX_SetTevAlphaIn(GX_TEVSTAGE1, GX_CA_ZERO, GX_CA_TEXA, GX_CA_RASA, GX_CA_ZERO);
        stage_ops(GX_TEVSTAGE1, GX_CS_SCALE_1);
        GX_SetNumTevStages(2);
    } else {
        GX_SetNumTevStages(1);
    }
}

void mwf_ui_quads(int32_t tex, const mwf_ui_quad *q, uint32_t count) {
    if (count == 0) return;
    if (tex == MWF_TEX_GRAB) {
        if (!grab_ok) return;
        ui_mode();
        ui_bind(&grab_obj, NULL);
    } else {
        if (tex < 0 || tex >= tex_count) return;
        ui_mode();
        ui_bind(&tex_obj[tex], tex_has_alpha[tex] ? &tex_alpha[tex] : NULL);
    }
    while (count > 0) {
        uint32_t n = count > 16383 ? 16383 : count;
        GX_Begin(GX_QUADS, GX_VTXFMT2, (u16)(n * 4));
        for (uint32_t i = 0; i < n; i++) {
            const mwf_ui_quad *a = &q[i];
            u8 r = (u8)(a->rgba >> 24), g = (u8)(a->rgba >> 16), b = (u8)(a->rgba >> 8), al = (u8)a->rgba;
            GX_Position2f32(a->x, a->y);
            GX_Color4u8(r, g, b, al);
            GX_TexCoord2f32(a->s0, a->t0);
            GX_Position2f32(a->x + a->w, a->y);
            GX_Color4u8(r, g, b, al);
            GX_TexCoord2f32(a->s1, a->t0);
            GX_Position2f32(a->x + a->w, a->y + a->h);
            GX_Color4u8(r, g, b, al);
            GX_TexCoord2f32(a->s1, a->t1);
            GX_Position2f32(a->x, a->y + a->h);
            GX_Color4u8(r, g, b, al);
            GX_TexCoord2f32(a->s0, a->t1);
        }
        GX_End();
        q += n;
        count -= n;
    }
}

void mwf_ui_verts(int32_t tex, const mwf_ui_vert *v, uint32_t quads) {
    if (quads == 0 || tex < 0 || tex >= tex_count) return;
    ui_mode();
    ui_bind(&tex_obj[tex], tex_has_alpha[tex] ? &tex_alpha[tex] : NULL);
    while (quads > 0) {
        uint32_t n = quads > 16383 ? 16383 : quads;
        GX_Begin(GX_QUADS, GX_VTXFMT2, (u16)(n * 4));
        for (uint32_t i = 0; i < n * 4; i++) {
            const mwf_ui_vert *a = &v[i];
            GX_Position2f32(a->x, a->y);
            GX_Color4u8((u8)(a->rgba >> 24), (u8)(a->rgba >> 16), (u8)(a->rgba >> 8), (u8)a->rgba);
            GX_TexCoord2f32(a->s, a->t);
        }
        GX_End();
        v += n * 4;
        quads -= n;
    }
}
void mwf_hud_rects(const int16_t (*r)[4], uint32_t count, uint32_t rgba) {
    if (count == 0) return;
    hud_mode();
    GXColor c = {(u8)(rgba >> 24), (u8)(rgba >> 16), (u8)(rgba >> 8), (u8)rgba};
    GX_SetChanMatColor(GX_COLOR0A0, c);
    /* One draw of up to 16383 quads at a time (vertex count is a u16). */
    while (count > 0) {
        uint32_t n = count > 16383 ? 16383 : count;
        GX_Begin(GX_QUADS, GX_VTXFMT2, (u16)(n * 4));
        for (uint32_t i = 0; i < n; i++) {
            s16 x = r[i][0], y = r[i][1], w = r[i][2], h = r[i][3];
            GX_Position2s16(x, y);
            GX_Position2s16(x + w, y);
            GX_Position2s16(x + w, y + h);
            GX_Position2s16(x, y + h);
        }
        GX_End();
        r += n;
        count -= n;
    }
}

/* ---- effect sprites ---------------------------------------------------------------------------------------- */

/* How far decals are pulled towards the eye in window depth (a polygon offset through the viewport's depth range).
   With the marks' own 0.25-unit lift off their surface this keeps them from depth fighting the world. */
#define DECAL_DEPTH_BIAS 4e-6f

static void sprite_quads(const float view[3][4], int32_t tex, uint32_t blend, const mwf_sprite_vert *v,
                         uint32_t quads, int32_t world, int decal);

void mwf_sprites(const float view[3][4], int32_t tex, uint32_t blend, const mwf_sprite_vert *v, uint32_t quads,
                 int32_t world) {
    sprite_quads(view, tex, blend, v, quads, world, 0);
}

void mwf_decals(const float view[3][4], int32_t tex, uint32_t blend, const mwf_sprite_vert *v, uint32_t quads) {
    sprite_quads(view, tex, blend, v, quads, 1, 1);
}

static void sprite_quads(const float view[3][4], int32_t tex, uint32_t blend, const mwf_sprite_vert *v,
                         uint32_t quads, int32_t world, int decal) {
    if (quads == 0) return;
    int has_tex = tex >= 0 && tex < tex_count;
    int has_plane = has_tex && tex_has_alpha[tex];
    mode = MODE_NONE; /* the next world, view or HUD draw sets its own state up again */
    world_base = NULL;
    if (world) {
        /* Additive sprites aren't fogged: fog would add its colour. */
        fog_world(blend != MWF_BLEND_ADD);
        GX_LoadProjectionMtx(world_proj, GX_PERSPECTIVE);
        f32 bias = decal ? DECAL_DEPTH_BIAS : 0.0f;
        GX_SetViewport(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight, VIEW_DEPTH - bias, 1.0f - bias);
    } else {
        fog_world(0);
    }
    GX_LoadPosMtxImm((f32(*)[4])view, GX_PNMTX0);
    GX_SetCurrentMtx(GX_PNMTX0);
    GX_ClearVtxDesc();
    GX_SetVtxDesc(GX_VA_POS, GX_DIRECT);
    GX_SetVtxDesc(GX_VA_CLR0, GX_DIRECT);
    GX_SetVtxDesc(GX_VA_TEX0, GX_DIRECT);
    GX_SetVtxAttrFmt(GX_VTXFMT7, GX_VA_POS, GX_POS_XYZ, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT7, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT7, GX_VA_TEX0, GX_TEX_ST, GX_F32, 0);
    GX_SetNumChans(1);
    GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    GX_SetNumTexGens(1);
    GX_SetTexCoordGen(GX_TEXCOORD0, GX_TG_MTX2x4, GX_TG_TEX0, GX_IDENTITY);
    u8 st = GX_TEVSTAGE0;
    if (has_tex) {
        GX_LoadTexObj(&tex_obj[tex], GX_TEXMAP0);
        GX_SetTevOrder(st, GX_TEXCOORD0, GX_TEXMAP0, GX_COLOR0A0);
        GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_TEXC, GX_CC_RASC, GX_CC_ZERO);
        GX_SetTevAlphaIn(st, GX_CA_ZERO, GX_CA_TEXA, GX_CA_RASA, GX_CA_ZERO);
    } else {
        GX_SetTevOrder(st, GX_TEXCOORDNULL, GX_TEXMAP_NULL, GX_COLOR0A0);
        GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_ZERO, GX_CC_ZERO, GX_CC_RASC);
        GX_SetTevAlphaIn(st, GX_CA_ZERO, GX_CA_ZERO, GX_CA_ZERO, GX_CA_RASA);
    }
    stage_ops(st, GX_CS_SCALE_1);
    st++;
    if (has_plane) {
        GX_LoadTexObj(&tex_alpha[tex], GX_TEXMAP2);
        GX_SetTevOrder(st, GX_TEXCOORD0, GX_TEXMAP2, GX_COLOR0A0);
        GX_SetTevColorIn(st, GX_CC_ZERO, GX_CC_ZERO, GX_CC_ZERO, GX_CC_CPREV);
        GX_SetTevAlphaIn(st, GX_CA_ZERO, GX_CA_TEXA, GX_CA_RASA, GX_CA_ZERO);
        stage_ops(st, GX_CS_SCALE_1);
        st++;
    }
    GX_SetNumTevStages(st);
    if (blend == MWF_BLEND_ADD)
        GX_SetBlendMode(GX_BM_BLEND, GX_BL_SRCALPHA, GX_BL_ONE, GX_LO_CLEAR);
    else if (blend == MWF_BLEND_MULTIPLY)
        GX_SetBlendMode(GX_BM_BLEND, GX_BL_DSTCLR, GX_BL_ZERO, GX_LO_CLEAR);
    else
        GX_SetBlendMode(GX_BM_BLEND, GX_BL_SRCALPHA, GX_BL_INVSRCALPHA, GX_LO_CLEAR);
    GX_SetAlphaCompare(GX_GREATER, 0, GX_AOP_AND, GX_ALWAYS, 0);
    GX_SetZCompLoc(GX_TRUE);
    GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_FALSE);
    GX_SetCullMode(GX_CULL_NONE);
    while (quads > 0) {
        uint32_t n = quads > 16383 ? 16383 : quads;
        GX_Begin(GX_QUADS, GX_VTXFMT7, (u16)(n * 4));
        for (uint32_t i = 0; i < n * 4; i++) {
            const mwf_sprite_vert *a = &v[i];
            GX_Position3f32(a->x, a->y, a->z);
            GX_Color4u8((u8)(a->rgba >> 24), (u8)(a->rgba >> 16), (u8)(a->rgba >> 8), (u8)a->rgba);
            GX_TexCoord2f32(a->s, a->t);
        }
        GX_End();
        v += n * 4;
        quads -= n;
    }
}

/* Solid boxes in world space (placeholder bodies): box i is mins b[i][0..3], maxs b[i][3..6], colour rgba[i];
   faces shaded by direction (top brightest). Depth tested against the world. Call after the world, before the
   first-person model. */
void mwf_draw_boxes(const float view[3][4], const float (*b)[6], const uint32_t *rgba, uint32_t count) {
    if (count == 0) return;
    fog_world(1);
    mode = MODE_NONE; /* the next world, view or HUD draw sets its own state up again */
    world_base = NULL;
    GX_LoadProjectionMtx(world_proj, GX_PERSPECTIVE);
    GX_LoadPosMtxImm((f32(*)[4])view, GX_PNMTX0);
    GX_SetCurrentMtx(GX_PNMTX0);
    GX_ClearVtxDesc();
    GX_SetVtxDesc(GX_VA_POS, GX_DIRECT);
    GX_SetVtxDesc(GX_VA_CLR0, GX_DIRECT);
    GX_SetVtxAttrFmt(GX_VTXFMT7, GX_VA_POS, GX_POS_XYZ, GX_F32, 0);
    GX_SetVtxAttrFmt(GX_VTXFMT7, GX_VA_CLR0, GX_CLR_RGBA, GX_RGBA8, 0);
    GX_SetNumChans(1);
    GX_SetChanCtrl(GX_COLOR0A0, GX_DISABLE, GX_SRC_REG, GX_SRC_VTX, GX_LIGHTNULL, GX_DF_NONE, GX_AF_NONE);
    GX_SetNumTexGens(0);
    GX_SetNumTevStages(1);
    GX_SetTevOrder(GX_TEVSTAGE0, GX_TEXCOORDNULL, GX_TEXMAP_NULL, GX_COLOR0A0);
    GX_SetTevOp(GX_TEVSTAGE0, GX_PASSCLR);
    /* Alpha from the colour's low byte: opaque boxes first with depth writes, then see-through ones (smoke)
       blended over them without. */
    GX_SetBlendMode(GX_BM_BLEND, GX_BL_SRCALPHA, GX_BL_INVSRCALPHA, GX_LO_CLEAR);
    GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
    GX_SetCullMode(GX_CULL_NONE);
    /* Face shades: +z, -z, +x, -x, +y, -y. */
    static const u8 shade[6] = {255, 110, 200, 170, 185, 150};
    const float (*b0)[6] = b;
    const uint32_t *rgba0 = rgba;
    uint32_t count0 = count;
    for (int pass = 0; pass < 2; pass++) {
    b = b0;
    rgba = rgba0;
    count = count0;
    GX_SetZMode(GX_TRUE, GX_LEQUAL, pass == 0 ? GX_TRUE : GX_FALSE);
    while (count > 0) {
        uint32_t n = count > 2000 ? 2000 : count;
        uint32_t here = 0;
        for (uint32_t i = 0; i < n; i++)
            if (((rgba[i] & 0xff) == 0xff) == (pass == 0)) here++;
        if (here == 0) {
            b += n;
            rgba += n;
            count -= n;
            continue;
        }
        GX_Begin(GX_QUADS, GX_VTXFMT7, (u16)(here * 24));
        for (uint32_t i = 0; i < n; i++) {
            const float *m = b[i];
            f32 x0 = m[0], y0 = m[1], z0 = m[2], x1 = m[3], y1 = m[4], z1 = m[5];
            u32 c = rgba[i];
            if (((c & 0xff) == 0xff) != (pass == 0)) continue;
            f32 q[6][4][3] = {
                {{x0, y0, z1}, {x1, y0, z1}, {x1, y1, z1}, {x0, y1, z1}},
                {{x0, y0, z0}, {x0, y1, z0}, {x1, y1, z0}, {x1, y0, z0}},
                {{x1, y0, z0}, {x1, y1, z0}, {x1, y1, z1}, {x1, y0, z1}},
                {{x0, y0, z0}, {x0, y0, z1}, {x0, y1, z1}, {x0, y1, z0}},
                {{x0, y1, z0}, {x0, y1, z1}, {x1, y1, z1}, {x1, y1, z0}},
                {{x0, y0, z0}, {x1, y0, z0}, {x1, y0, z1}, {x0, y0, z1}},
            };
            for (int f = 0; f < 6; f++) {
                u8 r = (u8)(((c >> 24) & 0xff) * shade[f] / 255);
                u8 g = (u8)(((c >> 16) & 0xff) * shade[f] / 255);
                u8 bl = (u8)(((c >> 8) & 0xff) * shade[f] / 255);
                for (int v = 0; v < 4; v++) {
                    GX_Position3f32(q[f][v][0], q[f][v][1], q[f][v][2]);
                    GX_Color4u8(r, g, bl, (u8)(c & 0xff));
                }
            }
        }
        GX_End();
        b += n;
        rgba += n;
        count -= n;
    }
    }
    GX_SetBlendMode(GX_BM_NONE, GX_BL_ONE, GX_BL_ZERO, GX_LO_CLEAR);
    GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_TRUE);
}

uint32_t mwf_frame_end(void) {
    /* Pipelined: mwf_gx_end_frame waits for the previous frame's GPU work, not this one's; that wait is returned. */
    mwf_gx_end_frame();
    return mwf_gpu_wait_us();
}
