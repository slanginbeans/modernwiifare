#include <stdlib.h>

#include "shim.h"

extern int mwf_spike_net_main(void);

int main(void) {
    mwf_sys_init();
    mwf_video_init();
    exit(mwf_spike_net_main()); /* returns to the Homebrew Channel */
}
