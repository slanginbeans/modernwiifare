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
void mwf_video_init(void); /* video + text console on framebuffer 0 */
void mwf_vsync(void);      /* flush stdout, wait for vertical sync */
void mwf_print(const char *s, size_t len);
void mwf_mem_info(mwf_mem *out);
void mwf_input_scan(void);
void mwf_pad_read(int chan, mwf_pad *out);
double mwf_ffi_echo(float a, double b, int32_t c); /* returns a + b + c; float-ABI self-test */
void mwf_panic(const char *msg, size_t len) __attribute__((noreturn));

/* ---- GX (shim_gx.c) ---- */
void mwf_gx_init(uint32_t clear_rgba);
void mwf_gx_draw(const float mtx[3][4], const mwf_vtx *v, uint32_t n);
void mwf_gx_end_frame(void);

/* ---- network (shim_net.c) ---- all addresses are numeric a.b.c.d == network order on the Wii */
int mwf_net_init(char ip[16], char mask[16], char gw[16]);
int mwf_udp_open(uint16_t port, int32_t *broadcast_opt_result, int32_t *bind_result);
int mwf_udp_send(int s, uint32_t ip, uint16_t port, const uint8_t *buf, uint32_t len);
int mwf_udp_recv(int s, uint8_t *buf, uint32_t cap, uint32_t *from_ip, uint16_t *from_port);

#endif
