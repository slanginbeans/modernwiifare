#!/bin/sh
# Run by build.bat inside devkitPro's own shell (msys2) on Windows:
# builds from the repo root. Usage: winbuild.sh [make target]
set -e
cd "$(dirname "$0")/.."
# A login shell resets PATH; add the Windows Rust install (rustup's default location).
if [ -n "$USERPROFILE" ]; then
	PATH="$(cygpath -u "$USERPROFILE")/.cargo/bin:$PATH"
	export PATH
fi
make -C wii "$@"
