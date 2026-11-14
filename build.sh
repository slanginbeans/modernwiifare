#!/bin/sh
# Builds the Wii programs inside the official devkitPro Docker image, so you don't need devkitPro or
# Rust installed. Output: dist/apps/mwf_game (copy that folder into the SD card's apps/).
# Usage: ./build.sh [make target]   (default: package; also: clean)
set -e
cd "$(dirname "$0")"
[ $# -eq 0 ] && set -- package
exec docker run --rm -v "$PWD":/src -w /src -u "$(id -u):$(id -g)" -e HOME=/tmp \
	devkitpro/devkitppc sh tools/docker-build.sh "$@"
