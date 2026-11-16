/* Paired-single kernels for the bodies' skinning (game_core::skin_kernels holds the scalar reference and its tests;
 * game/src/ps.rs checks these against it once at load). Broadway computes two floats per instruction in its
 * paired-single unit; libogc's ps_guMtxConcat / ps_guVecMultiply are written with it. Stable Rust has no PowerPC
 * inline assembly, so the kernels live here. */
#include <stdint.h>
#include <ogc/gu.h>
#include <ogc/gx.h>

#include "shim.h"

typedef struct {
    float pos[3];
    float weights[4];
    uint16_t bones[4];
    uint16_t out;
    uint16_t count;
} SkinVert; /* game_core::skin_kernels::SkinVert, 40 bytes */

void mwf_ps_concat_each(const float (*a)[3][4], const float (*b)[3][4], float (*out)[3][4], uint32_t n) {
    for (uint32_t i = 0; i < n; i++) ps_guMtxConcat((MtxP)a[i], (MtxP)b[i], (MtxP)out[i]);
}

/* Each vertex: its bones' skin matrices blended by weight (fused multiply-adds), then one paired-single transform of
 * the bind position. Vertices with a bone past `bones` or an `out` past `outs` are skipped. */
void mwf_ps_skin(const void *verts, uint32_t n, const float (*skin)[3][4], uint32_t bones, float (*out)[3],
                 uint32_t outs) {
    const SkinVert *v = (const SkinVert *)verts;
    for (uint32_t i = 0; i < n; i++, v++) {
        Mtx m = {{0}};
        uint32_t c = v->count > 4 ? 4 : v->count;
        int ok = v->out < outs;
        for (uint32_t k = 0; k < c && ok; k++) {
            if (v->bones[k] >= bones) {
                ok = 0;
                break;
            }
            const float *s = &skin[v->bones[k]][0][0];
            float w = v->weights[k];
            float *d = &m[0][0];
            for (int j = 0; j < 12; j++) d[j] += s[j] * w;
        }
        if (!ok) continue;
        ps_guVecMultiply(m, (const guVector *)v->pos, (guVector *)out[v->out]);
    }
}

/* A draw's position matrices: slot i gets eye x skin[keys[i]], or eye alone for 0xFFFF (vertices the CPU skinned)
 * and keys past `bones`. `loaded` (10 entries) holds what each slot has: slots already holding their key are left. */
void mwf_ps_load_slots(const float eye[3][4], const float (*skin)[3][4], uint32_t bones, const uint16_t *keys,
                       uint32_t n, uint16_t *loaded) {
    Mtx m;
    for (uint32_t i = 0; i < n && i < 10; i++) {
        if (loaded[i] == keys[i]) continue;
        loaded[i] = keys[i];
        if (keys[i] < bones) {
            ps_guMtxConcat((MtxP)eye, (MtxP)skin[keys[i]], m);
            GX_LoadPosMtxImm(m, GX_PNMTX0 + i * 3);
        } else {
            GX_LoadPosMtxImm((MtxP)eye, GX_PNMTX0 + i * 3);
        }
    }
}
