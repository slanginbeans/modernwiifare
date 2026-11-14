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

enum { MODE_NONE, MODE_SKY, MODE_WORLD, MODE_HUD, MODE_VIEW, MODE_UI };

extern GXRModeObj *mwf_rmode;
/* Depth range split: the first-person model gets [0, VIEW_DEPTH], the world [VIEW_DEPTH, 1]. */
#define VIEW_DEPTH 0.02f
static int mode = MODE_NONE;
static const void *world_base = NULL;
/* The world's projection of this frame (boxes drawn in world space reuse it). */
static Mtx44 world_proj;

void mwf_frame_begin(const float view[3][4], float fovy_deg, float znear, float zfar, uint32_t clear_rgba) {
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
        GXColor k = {(u8)(s->tint >> 24), (u8)(s->tint >> 16), (u8)(s->tint >> 8), 0xff};
        GX_SetTevKColor(GX_KCOLOR0, k);
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

int32_t mwf_snd_play(const void *pcm, uint32_t bytes, uint32_t rate, uint8_t vol_l, uint8_t vol_r) {
    s32 voice = ASND_GetFirstUnusedVoice();
    if (voice < 0)
        return -1;
    if (ASND_SetVoice(voice, VOICE_MONO_16BIT, (s32)rate, 0, (void *)pcm, (s32)bytes, vol_l, vol_r, NULL) != SND_OK)
        return -1;
    return voice;
}

void mwf_snd_stop_all(void) {
    for (s32 v = 0; v < MAX_SND_VOICES; v++)
        ASND_StopVoice(v);
}

void mwf_view_begin(float fovy_deg, float znear, float zfar) {
    f32 aspect = (CONF_GetAspectRatio() == CONF_ASPECT_16_9) ? (16.0f / 9.0f) : (4.0f / 3.0f);
    Mtx44 proj;
    guPerspective(proj, fovy_deg, aspect, znear, zfar);
    GX_LoadProjectionMtx(proj, GX_PERSPECTIVE);
    GX_SetViewport(0, 0, mwf_rmode->fbWidth, mwf_rmode->efbHeight, 0.0f, VIEW_DEPTH);
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

/* ---- HUD ------------------------------------------------------------------------------------------------------ */

static void hud_mode(void) {
    if (mode == MODE_HUD) return;
    mode = MODE_HUD;
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

void mwf_ui_quads(int32_t tex, const mwf_ui_quad *q, uint32_t count) {
    if (count == 0 || tex < 0 || tex >= tex_count) return;
    ui_mode();
    GX_LoadTexObj(&tex_obj[tex], GX_TEXMAP0);
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

/* Solid boxes in world space (placeholder bodies): box i is mins b[i][0..3], maxs b[i][3..6], colour rgba[i];
   faces shaded by direction (top brightest). Depth tested against the world. Call after the world, before the
   first-person model. */
void mwf_draw_boxes(const float view[3][4], const float (*b)[6], const uint32_t *rgba, uint32_t count) {
    if (count == 0) return;
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
    GX_SetBlendMode(GX_BM_NONE, GX_BL_ONE, GX_BL_ZERO, GX_LO_CLEAR);
    GX_SetAlphaCompare(GX_ALWAYS, 0, GX_AOP_AND, GX_ALWAYS, 0);
    GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_TRUE);
    GX_SetCullMode(GX_CULL_NONE);
    /* Face shades: +z, -z, +x, -x, +y, -y. */
    static const u8 shade[6] = {255, 110, 200, 170, 185, 150};
    while (count > 0) {
        uint32_t n = count > 2000 ? 2000 : count;
        GX_Begin(GX_QUADS, GX_VTXFMT7, (u16)(n * 24));
        for (uint32_t i = 0; i < n; i++) {
            const float *m = b[i];
            f32 x0 = m[0], y0 = m[1], z0 = m[2], x1 = m[3], y1 = m[4], z1 = m[5];
            u32 c = rgba[i];
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
                    GX_Color4u8(r, g, bl, 0xff);
                }
            }
        }
        GX_End();
        b += n;
        rgba += n;
        count -= n;
    }
}

uint32_t mwf_frame_end(void) {
    /* mwf_gx_end_frame waits for the GPU (GX_DrawDone) and then for vsync; time the GPU part. */
    GX_SetZMode(GX_TRUE, GX_LEQUAL, GX_TRUE);
    GX_SetColorUpdate(GX_TRUE);
    uint32_t t0 = mwf_us();
    GX_DrawDone();
    uint32_t waited = mwf_us() - t0;
    mwf_gx_end_frame();
    return waited;
}
