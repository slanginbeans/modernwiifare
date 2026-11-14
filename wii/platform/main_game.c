#include <stdlib.h>

#include "shim.h"

extern int mwf_game_main(void);

int main(int argc, char **argv) {
    mwf_sys_init();
    mwf_video_init();
    /* The Homebrew Channel passes the .dol's own path ("sd:/apps/mwf_game/boot.dol"): packs sit beside it. */
    mwf_set_app_dir(argc > 0 && argv ? argv[0] : NULL);
    exit(mwf_game_main()); /* returns to the Homebrew Channel */
}
