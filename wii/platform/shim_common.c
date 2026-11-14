#include <gccore.h>
#include <ogc/lwp_watchdog.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "shim.h"

/* Shared with shim_gx.c. */
GXRModeObj *mwf_rmode = NULL;
void *mwf_xfb[2] = {NULL, NULL};

static u32 pads_connected = 0;

void mwf_sys_init(void) {
    PAD_Init();
}

int mwf_should_exit(void) {
    return SYS_ResetButtonDown() ? 1 : 0;
}

uint32_t mwf_us(void) {
    return (uint32_t)ticks_to_microsecs(gettime());
}

uint32_t mwf_ms(void) {
    return (uint32_t)ticks_to_millisecs(gettime());
}

void mwf_video_init(void) {
    VIDEO_Init();
    mwf_rmode = VIDEO_GetPreferredMode(NULL);
    mwf_xfb[0] = MEM_K0_TO_K1(SYS_AllocateFramebuffer(mwf_rmode));
    mwf_xfb[1] = MEM_K0_TO_K1(SYS_AllocateFramebuffer(mwf_rmode));
    console_init(mwf_xfb[0], 20, 20, mwf_rmode->fbWidth, mwf_rmode->xfbHeight,
                 mwf_rmode->fbWidth * VI_DISPLAY_PIX_SZ);
    VIDEO_Configure(mwf_rmode);
    VIDEO_SetNextFramebuffer(mwf_xfb[0]);
    VIDEO_SetBlack(false);
    VIDEO_Flush();
    VIDEO_WaitVSync();
    if (mwf_rmode->viTVMode & VI_NON_INTERLACE) VIDEO_WaitVSync();
    printf("\x1b[2J");
}

void mwf_vsync(void) {
    fflush(stdout);
    VIDEO_WaitVSync();
}

void mwf_print(const char *s, size_t len) {
    fwrite(s, 1, len, stdout);
}

void mwf_mem_info(mwf_mem *out) {
    out->arena1_lo = (uint32_t)SYS_GetArena1Lo();
    out->arena1_hi = (uint32_t)SYS_GetArena1Hi();
    out->arena2_lo = (uint32_t)SYS_GetArena2Lo();
    out->arena2_hi = (uint32_t)SYS_GetArena2Hi();
}

int mwf_widescreen(void) {
    return CONF_GetAspectRatio() == CONF_ASPECT_16_9;
}

void mwf_input_scan(void) {
    pads_connected = PAD_ScanPads();
}

static uint16_t map_buttons(u16 b) {
    uint16_t o = 0;
    if (b & PAD_BUTTON_LEFT) o |= MWF_BTN_LEFT;
    if (b & PAD_BUTTON_RIGHT) o |= MWF_BTN_RIGHT;
    if (b & PAD_BUTTON_DOWN) o |= MWF_BTN_DOWN;
    if (b & PAD_BUTTON_UP) o |= MWF_BTN_UP;
    if (b & PAD_TRIGGER_Z) o |= MWF_BTN_Z;
    if (b & PAD_TRIGGER_R) o |= MWF_BTN_R;
    if (b & PAD_TRIGGER_L) o |= MWF_BTN_L;
    if (b & PAD_BUTTON_A) o |= MWF_BTN_A;
    if (b & PAD_BUTTON_B) o |= MWF_BTN_B;
    if (b & PAD_BUTTON_X) o |= MWF_BTN_X;
    if (b & PAD_BUTTON_Y) o |= MWF_BTN_Y;
    if (b & PAD_BUTTON_START) o |= MWF_BTN_START;
    return o;
}

void mwf_pad_read(int chan, mwf_pad *out) {
    memset(out, 0, sizeof(*out));
    if (chan < 0 || chan > 3) return;
    out->connected = (pads_connected >> chan) & 1;
    out->held = map_buttons(PAD_ButtonsHeld(chan));
    out->down = map_buttons(PAD_ButtonsDown(chan));
    out->sx = PAD_StickX(chan);
    out->sy = PAD_StickY(chan);
    out->cx = PAD_SubStickX(chan);
    out->cy = PAD_SubStickY(chan);
    out->l = PAD_TriggerL(chan);
    out->r = PAD_TriggerR(chan);
}

double mwf_ffi_echo(float a, double b, int32_t c) {
    return (double)a + b + (double)c;
}

void mwf_panic(const char *msg, size_t len) {
    if (mwf_rmode == NULL) mwf_video_init();
    VIDEO_SetNextFramebuffer(mwf_xfb[0]);
    VIDEO_Flush();
    printf("\x1b[2J\x1b[2;0H\x1b[31;1mRUST PANIC\x1b[37;1m\n\n%.*s\n\nPress START or RESET to exit.\n",
           (int)len, msg);
    fflush(stdout);
    for (;;) {
        VIDEO_WaitVSync();
        PAD_ScanPads();
        if ((PAD_ButtonsDown(0) & PAD_BUTTON_START) || SYS_ResetButtonDown()) exit(1);
    }
}

/* The stock Rust core for powerpc-unknown-linux-gnu is built with unwind landing pads.
 * We build with panic=abort, so these are never reached; weak so a real definition wins. */
__attribute__((weak)) void _Unwind_Resume(void *e) {
    (void)e;
    abort();
}
__attribute__((weak)) int rust_eh_personality(void) {
    abort();
}
