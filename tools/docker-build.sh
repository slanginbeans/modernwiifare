#!/bin/sh
# Runs inside the devkitpro/devkitppc container (build.bat / build.sh use it when devkitPro isn't
# installed). The image has no Rust, so rustup is installed into .toolchain/ in the repo, which is
# git-ignored and survives between runs. Usage: tools/docker-build.sh [make target]
set -e
cd "$(dirname "$0")/.."
export RUSTUP_HOME="$PWD/.toolchain/rustup"
export CARGO_HOME="$PWD/.toolchain/cargo"
export PATH="$CARGO_HOME/bin:$PATH"
if [ "${1:-}" != "clean" ] && ! command -v rustup >/dev/null 2>&1; then
	echo "[rust] Installing Rust into .toolchain/ (first run only)..."
	curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
		sh -s -- -y --profile minimal --default-toolchain none --no-modify-path
fi
git config --global --add safe.directory "$PWD" 2>/dev/null || true
make -C wii "$@"
