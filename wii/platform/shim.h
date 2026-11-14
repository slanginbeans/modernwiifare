/* Modern Wiifare: the thin C layer between the Rust runtime and libogc.
 *
 * Rust calls only these functions (declared again in wii/spikes/src/ffi.rs; keep both in sync).
 * Many libogc GX calls are static inline FIFO writes, so wrapping them in C is simpler and
 * faster than binding them from Rust. See ADR 0006. */
#ifndef MWF_SHIM_H
#define MWF_SHIM_H

#include <stddef.h>
#include <stdint.h>

/* Our own button bits (decoupled from libogc's PAD_BUTTON_* values). */
#define MWF_BTN_LEFT  0x0001u
#define MWF_BTN_RIGHT 0x0002u
#define MWF_BTN_DOWN  0x0004u
#define MWF_BTN_UP    0x0008u
#define MWF_BTN_Z     0x0010u
#define MWF_BTN_R     0x0020u
#define MWF_BTN_L     0x0040u
#define MWF_BTN_A     0x0100u
#define MWF_BTN_B     0x0200u
#define MWF_BTN_X     0x0400u
#define MWF_BTN_Y     0x0800u
#define MWF_BTN_START 0x1000u

typedef struct {
    uint16_t held, down;
    int8_t sx, sy, cx, cy; /* main stick, C-stick */
    uint8_t l, r;          /* analog triggers */
    uint8_t connected, _pad;
} mwf_pad; /* 12 bytes */

typedef struct {
    uint32_t arena1_lo, arena1_hi, arena2_lo, arena2_hi;
} mwf_mem;

typedef struct {
    float x, y, z;
    uint32_t rgba;
} mwf_vtx; /* 16 bytes */

/* ---- system (shim_common.c) ---- */
void mwf_sys_init(void);
int mwf_should_exit(void); /* reset button pressed */
uint32_t mwf_ms(void);     /* monotonic milliseconds, wraps */
uint32_t mwf_us(void);     /* monotonic microseconds, wraps */
void mwf_video_init(void); /* video + text console on framebuffer 0 */
void mwf_vsync(void);      /* flush stdout, wait for vertical sync */
void mwf_print(const char *s, size_t len);
void mwf_mem_info(mwf_mem *out);
int mwf_widescreen(void);     /* 1 when the console is set to 16:9 */
void mwf_input_scan(void);
void mwf_pad_read(int chan, mwf_pad *out);
double mwf_ffi_echo(float a, double b, int32_t c); /* returns a + b + c; float-ABI self-test */
void mwf_panic(const char *msg, size_t len) __attribute__((noreturn));

/* ---- GX (shim_gx.c) ---- */
void mwf_gx_init(uint32_t clear_rgba);
void mwf_gx_draw(const float mtx[3][4], const mwf_vtx *v, uint32_t n);
void mwf_gx_end_frame(void);

/* ---- game world (shim_world.c) ---- see docs/wii-gx.md and docs/pack.md */

/* Blend classes, same values as wpk::records::Blend. */
#define MWF_BLEND_OPAQUE 0
#define MWF_BLEND_ALPHA_TEST 1
#define MWF_BLEND_BLEND 2
#define MWF_BLEND_ADD 3
#define MWF_BLEND_MULTIPLY 4
/* Cull, same values as wpk::records::Cull. */
#define MWF_CULL_NONE 0
#define MWF_CULL_BACK 1
#define MWF_CULL_FRONT 2

/* Texture description, filled by Rust from a pack TextureRecord. Pointers are into the loaded pack (MEM2). */
typedef struct {
    const void *data;       /* GX texture data, 32-byte aligned */
    const void *alpha;      /* I4 alpha plane, or NULL */
    uint16_t width, height;
    uint8_t format;         /* GX texture format id */
    uint8_t levels;
    uint8_t wrap;           /* 0 repeat, 1 clamp, 2 mirror */
    uint8_t _pad;
} mwf_tex_desc; /* 16 bytes */

/* State for one world draw. tex/lightmap are texture handles from mwf_tex_create, or -1. */
typedef struct {
    int32_t tex;
    int32_t lightmap;
    uint8_t blend;
    uint8_t alpha_ref;
    uint8_t cull;
    uint8_t _pad;
    uint32_t tint;          /* RGBA8 light at half intensity, multiplied in x2 (static models); 0 = none */
} mwf_draw_state; /* 16 bytes */

/* Allocate from MEM2 (bump allocator over the MEM2 arena; never freed). NULL when out of memory. */
void *mwf_mem2_alloc(uint32_t size, uint32_t align);
uint32_t mwf_mem2_free_bytes(void);
/* Mark / release the MEM2 bump allocator (everything allocated after the mark is freed; for map changes). */
uint32_t mwf_mem2_mark(void);
void mwf_mem2_release(uint32_t mark);
/* File names in `dir` ending in `suffix` (suffix removed), '\n'-separated into `out`. Returns how many. */
int mwf_list_dir(const char *dir, const char *suffix, char *out, uint32_t cap);
/* Read a whole file from the SD card into MEM2 (32-byte aligned, flushed for GX). Returns NULL on failure. */
void *mwf_file_load(const char *path, uint32_t *size_out);
/* Read bytes [offset, offset + len) of a file into MEM2 (32-byte aligned, flushed for GX). NULL on failure. */
void *mwf_file_load_range(const char *path, uint32_t offset, uint32_t len);
/* Read bytes [offset, offset + len) of a file into `dst` (any memory). Returns the bytes read, or -1. */
int32_t mwf_file_read(const char *path, uint32_t offset, uint32_t len, void *dst);
/* Why the last file call failed: 0 none, 1 no SD card, 2 no such file, 3 out of MEM2, 4 read error. */
int32_t mwf_file_error(void);

/* Create a GX texture object; returns a handle >= 0, or -1 when the table is full. */
int32_t mwf_tex_create(const mwf_tex_desc *desc);
/* Forget every texture (before loading another map). */
void mwf_tex_reset(void);

/* Frame: clear, set the camera. view is a GX 3x4 world->eye matrix (row-major); fovy in degrees. */
void mwf_frame_begin(const float view[3][4], float fovy_deg, float znear, float zfar, uint32_t clear_rgba);
/* Point the world vertex arrays at an interleaved vertex block (stride 28: pos f32x3, rgba, uv f32x2, lm u16x2). */
void mwf_world_arrays(const void *vertices);
/* Set TEV, blend, alpha test, Z and cull for one world draw. */
void mwf_world_state(const mwf_draw_state *s);
/* Load the position matrix (GX_PNMTX0) for the following world / model draws: the view, or view x placement. */
void mwf_pos_matrix(const float m[3][4]);
/* Call a prebuilt display list (32-byte aligned, length a multiple of 32). */
void mwf_call_dl(const void *dl, uint32_t len);
/* Draw one skybox face: 4 corners (x,y,z relative to the eye) with texcoords (s,t), textured with `tex`. */
void mwf_sky_face(const float view_rot[3][4], const float corners[4][5], int32_t tex);
/* Solid screen-space rectangles (x, y, w, h in 640x480 pixels, origin top left), drawn over the frame. */
void mwf_hud_rects(const int16_t (*rects)[4], uint32_t count, uint32_t rgba);
/* Copy to the XFB and flip. Returns the microseconds the CPU waited for the GPU to finish the frame. */
uint32_t mwf_frame_end(void);

/* ---- network (shim_net.c) ---- all addresses are numeric a.b.c.d == network order on the Wii */
int mwf_net_init(char ip[16], char mask[16], char gw[16]);
int mwf_udp_open(uint16_t port, int32_t *broadcast_opt_result, int32_t *bind_result);
int mwf_udp_send(int s, uint32_t ip, uint16_t port, const uint8_t *buf, uint32_t len);
int mwf_udp_recv(int s, uint8_t *buf, uint32_t cap, uint32_t *from_ip, uint16_t *from_port);

#endif
