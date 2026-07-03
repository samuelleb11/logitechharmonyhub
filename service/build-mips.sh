#!/bin/bash
# Reproducible cross-build of a Rust binary for the Harmony Hub:
#   big-endian MIPS32, static musl, runs on Linux 2.6.31.
#
# ⚠️ The kernel has NO CONFIG_FUTEX, so keep the program SINGLE-THREADED
#    (no std::thread::spawn; uncontended std locks are fine). Verified:
#    a single-threaded probe runs; Go cannot run at all (futex).
# ⚠️ UPX does NOT work on this kernel (its stub SIGTRAPs) — DEPLOY THE NATIVE
#    binary. For the slow UART upload, `zip -9` it (device has BusyBox unzip).
#
# One-time host setup (macOS):
#   brew install rustup upx zip
#   rustup toolchain install 1.74.0 --profile minimal   # last Tier-2 for mips-musl:
#   rustup target add mips-unknown-linux-musl --toolchain 1.74.0   # ships prebuilt std
#                                                                   # incl. self-contained musl
# Rust 1.75+ demoted mips* to Tier 3 (no prebuilt std), so pin 1.74.
#
# Per-crate: .cargo/config.toml points the linker at the bundled rust-lld and uses
# link-self-contained=yes + relocation-model=static + -no-pie. A one-time shim aliases
# libgcc_s -> libunwind.a (panic=abort means it's unused) to satisfy the -lgcc_s ref.
set -e
CRATE="${1:-.}"
TC=$(echo ~/.rustup/toolchains/1.74.0-*)
SC="$TC/lib/rustlib/mips-unknown-linux-musl/lib/self-contained"
mkdir -p /tmp/mipslibs && cp -f "$SC/libunwind.a" /tmp/mipslibs/libgcc_s.a
export PATH="$TC/bin:$PATH"
cd "$CRATE"
cargo build --release --target mips-unknown-linux-musl
BIN=$(ls -1 target/mips-unknown-linux-musl/release/ | grep -vE '\.|^build$|^deps$|^incremental$|^examples$' | head -1)
P="target/mips-unknown-linux-musl/release/$BIN"
echo "=== $P ==="
file "$P"
echo "size: $(stat -f%z "$P") bytes"
