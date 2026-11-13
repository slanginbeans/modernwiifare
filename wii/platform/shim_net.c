#include <gccore.h>
#include <network.h>
#include <poll.h> /* POLLIN: libogc network_wii.c takes it from here too */
#include <string.h>

#include "shim.h"

/* The Wii is big-endian, so network byte order == host order and the numeric a.b.c.d
 * value from Rust can be stored into sin_addr / sin_port directly. */

int mwf_net_init(char ip[16], char mask[16], char gw[16]) {
    memset(ip, 0, 16);
    memset(mask, 0, 16);
    memset(gw, 0, 16);
    /* DHCP, up to 20 retries of net_init. Works for the USB LAN adapter and Wi-Fi alike. */
    return if_config(ip, mask, gw, true, 20);
}

int mwf_udp_open(uint16_t port, int32_t *broadcast_opt_result, int32_t *bind_result) {
    s32 s = net_socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (s < 0) return s;

    u32 yes = 1;
    *broadcast_opt_result = net_setsockopt(s, SOL_SOCKET, SO_BROADCAST, &yes, sizeof(yes));

    struct sockaddr_in a;
    memset(&a, 0, sizeof(a));
    a.sin_len = sizeof(a);
    a.sin_family = AF_INET;
    a.sin_port = port;
    a.sin_addr.s_addr = INADDR_ANY;
    *bind_result = net_bind(s, (struct sockaddr *)&a, sizeof(a));
    return s;
}

int mwf_udp_send(int s, uint32_t ip, uint16_t port, const uint8_t *buf, uint32_t len) {
    struct sockaddr_in to;
    memset(&to, 0, sizeof(to));
    to.sin_len = sizeof(to);
    to.sin_family = AF_INET;
    to.sin_port = port;
    to.sin_addr.s_addr = ip;
    return net_sendto(s, buf, len, 0, (struct sockaddr *)&to, sizeof(to));
}

int mwf_udp_recv(int s, uint8_t *buf, uint32_t cap, uint32_t *from_ip, uint16_t *from_port) {
    struct pollsd pfd;
    pfd.socket = s;
    pfd.events = POLLIN;
    pfd.revents = 0;
    s32 r = net_poll(&pfd, 1, 0);
    if (r <= 0) return r; /* 0 = nothing waiting, <0 = error */
    if (!(pfd.revents & POLLIN)) return 0;

    struct sockaddr_in from;
    socklen_t from_len = sizeof(from);
    memset(&from, 0, sizeof(from));
    r = net_recvfrom(s, buf, cap, 0, (struct sockaddr *)&from, &from_len);
    if (r < 0) return r;
    *from_ip = from.sin_addr.s_addr;
    *from_port = from.sin_port;
    return r;
}
