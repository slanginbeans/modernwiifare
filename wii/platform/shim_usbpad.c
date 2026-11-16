/* Xbox 360 wired controllers over USB (libogc USBv5 "ven" devices, IOS58; the Homebrew Channel starts apps on it).
 *
 * Found by their interface (class 0xFF, subclass 0x5D, protocol 0x01), so Microsoft's pad (045E:028E) and the
 * third-party wired ones that use the same interface all work. Only devices from vendors that make 360-compatible pads
 * are opened (`PAD_VENDORS`): the USB LAN adapter system link uses is a vendor-class device too, and must never be
 * touched. The wireless receiver (protocol 0x81) is not handled.
 *
 * Reports come from the interrupt IN endpoint, read asynchronously: the callback (IOS IPC context) parses each
 * 20-byte input report (type 0x00, length 0x14) into `state` and queues the next read. A failed read marks the pad
 * lost; mwf_x360_scan (main loop) closes it and looks for pads again about once a second.
 *
 * Input report (public knowledge of the 360 pad's USB protocol, as Linux's xpad documents it):
 *   0: type 0x00, 1: length 0x14,
 *   2: D-pad up 0x01, down 0x02, left 0x04, right 0x08, start 0x10, back 0x20, left stick 0x40, right stick 0x80
 *   3: LB 0x01, RB 0x02, guide 0x04, A 0x10, B 0x20, X 0x40, Y 0x80
 *   4: left trigger, 5: right trigger (0..255), 6..13: LX, LY, RX, RY (int16 little-endian, up is positive).
 * LED command on the OUT endpoint: 01 03 nn, nn 6..9 = player 1..4 lit.
 * Rumble command on the OUT endpoint: 00 08 00 bb ss 00 00 00 (bb the big motor, ss the small one; xpad). */
#include <gccore.h>
#include <ogc/irq.h>
#include <ogc/usb.h>
#include <string.h>

#include "shim.h"

#define X360_CLASS 0xFF
#define X360_SUBCLASS 0x5D
#define X360_PROTOCOL 0x01
#define REPORT_LEN 32
/* Input scans between looks for a newly plugged pad (the scan runs once per frame). */
#define RESCAN_FRAMES 60
#define MAX_REJECTED 8

/* Vendors of 360-compatible wired pads (as Linux's xpad lists them): Microsoft, Logitech, Mad Catz, Thrustmaster,
 * PDP / Afterglow, Hori, Bigben, Razer, Harmonix, Razer Onza, PowerA, BDA / PowerA, Honey Bee, Joytech. */
static const u16 PAD_VENDORS[] = {0x045E, 0x046D, 0x0738, 0x044F, 0x0E6F, 0x0F0D, 0x146B,
                                  0x1532, 0x1BAD, 0x1689, 0x24C6, 0x20D6, 0x12AB, 0x162E};

static int pad_vendor(u16 vid) {
    for (unsigned i = 0; i < sizeof(PAD_VENDORS) / sizeof(PAD_VENDORS[0]); i++)
        if (PAD_VENDORS[i] == vid) return 1;
    return 0;
}

static int usb_ready = -1; /* -1: not tried, 0: failed, 1: ok */
static s32 fd = -1;
static u8 ep_in = 0x81, ep_out = 0x01;
static volatile int lost = 0;
static volatile mwf_x360 state;
static u8 report[REPORT_LEN] ATTRIBUTE_ALIGN(32);
static u8 led[32] ATTRIBUTE_ALIGN(32);
static int rescan = 0;
/* Devices already looked at that are not 360 pads (so they are not opened every second). */
static s32 rejected[MAX_REJECTED];
static int rejected_n = 0;

static s32 on_report(s32 result, void *usrdata);

static void queue_read(void) {
    if (USB_ReadIntrMsgAsync(fd, ep_in, REPORT_LEN, report, on_report, NULL) < 0) lost = 1;
}

static s16 le16(const u8 *p) {
    return (s16)(p[0] | (p[1] << 8));
}

static s32 on_report(s32 result, void *usrdata) {
    (void)usrdata;
    if (result < 0 || fd < 0) {
        lost = 1;
        return 0;
    }
    if (result >= 14 && report[0] == 0x00 && report[1] >= 0x14) {
        state.buttons = (uint16_t)(report[2] | (report[3] << 8));
        state.lt = report[4];
        state.rt = report[5];
        state.lx = le16(&report[6]);
        state.ly = le16(&report[8]);
        state.rx = le16(&report[10]);
        state.ry = le16(&report[12]);
    }
    queue_read();
    return 0;
}

static int was_rejected(s32 id) {
    for (int i = 0; i < rejected_n; i++)
        if (rejected[i] == id) return 1;
    return 0;
}

static void reject(s32 id) {
    if (rejected_n < MAX_REJECTED) rejected[rejected_n++] = id;
}

/* Open device `e` if its first interface is a 360 pad's; 1 when it is now the pad. */
static int try_open(const usb_device_entry *e) {
    s32 f = -1;
    if (USB_OpenDevice(e->device_id, e->vid, e->pid, &f) < 0 || f < 0) return 0;
    usb_devdesc d;
    memset(&d, 0, sizeof(d));
    int ok = 0;
    if (USB_GetDescriptors(f, &d) >= 0) {
        if (d.bNumConfigurations > 0 && d.configurations && d.configurations[0].bNumInterfaces > 0 &&
            d.configurations[0].interfaces) {
            usb_interfacedesc *i = &d.configurations[0].interfaces[0];
            if (i->bInterfaceClass == X360_CLASS && i->bInterfaceSubClass == X360_SUBCLASS &&
                i->bInterfaceProtocol == X360_PROTOCOL) {
                ok = 1;
                ep_in = 0x81;
                ep_out = 0x01;
                for (int k = 0; k < i->bNumEndpoints && i->endpoints; k++) {
                    usb_endpointdesc *ep = &i->endpoints[k];
                    if ((ep->bmAttributes & 3) != 3) continue; /* interrupt endpoints only */
                    if (ep->bEndpointAddress & 0x80)
                        ep_in = ep->bEndpointAddress;
                    else
                        ep_out = ep->bEndpointAddress;
                }
            }
        }
        USB_FreeDescriptors(&d);
    }
    if (!ok) {
        USB_CloseDevice(&f);
        return 0;
    }
    fd = f;
    lost = 0;
    memset((void *)&state, 0, sizeof(state));
    state.connected = 1;
    /* Player 1's light. */
    memset(led, 0, sizeof(led));
    led[0] = 0x01;
    led[1] = 0x03;
    led[2] = 0x06;
    USB_WriteIntrMsg(fd, ep_out, 3, led);
    queue_read();
    return 1;
}

static void look_for_pad(void) {
    usb_device_entry list[8];
    u8 n = 0;
    if (USB_GetDeviceList(list, 8, X360_CLASS, &n) < 0) return;
    for (int i = 0; i < n && fd < 0; i++) {
        if (was_rejected(list[i].device_id) || !pad_vendor(list[i].vid)) continue;
        if (!try_open(&list[i])) reject(list[i].device_id);
    }
}

void mwf_x360_scan(void) {
    if (usb_ready < 0) usb_ready = USB_Initialize() >= 0 ? 1 : 0;
    if (!usb_ready) return;
    if (fd >= 0 && lost) {
        u32 level = IRQ_Disable();
        s32 f = fd;
        fd = -1;
        memset((void *)&state, 0, sizeof(state));
        IRQ_Restore(level);
        USB_CloseDevice(&f);
        /* A replugged pad can come back with the same id. */
        rejected_n = 0;
    }
    if (fd < 0 && --rescan <= 0) {
        rescan = RESCAN_FRAMES;
        look_for_pad();
    }
}

static u8 rumble_msg[32] ATTRIBUTE_ALIGN(32);
static int rumble_sent = -1;

void mwf_x360_rumble(uint8_t big, uint8_t small) {
    int key = (big << 8) | small;
    if (fd < 0 || lost) {
        rumble_sent = -1;
        return;
    }
    if (key == rumble_sent) return;
    rumble_sent = key;
    memset(rumble_msg, 0, sizeof(rumble_msg));
    rumble_msg[1] = 0x08;
    rumble_msg[3] = big;
    rumble_msg[4] = small;
    USB_WriteIntrMsg(fd, ep_out, 8, rumble_msg);
}

void mwf_x360_read(mwf_x360 *out) {
    u32 level = IRQ_Disable();
    memcpy(out, (const void *)&state, sizeof(*out));
    IRQ_Restore(level);
}
