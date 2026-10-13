#!/usr/bin/env bash
# Build/test environment for this machine (2026-10-13).
# The Freedesktop SDK revision that cargo's cache was built against is gone;
# system -dev packages are not installed and there is no passwordless sudo.
# Source this file before ANY cargo build/test invocation:
#   source scripts/build-env.sh
SDK_REV=18fa1be304b09e42dd3606f6d933e2d6a27513dd3aeee1203b2b58d59ad3db71
SDK=/var/lib/flatpak/runtime/org.freedesktop.Sdk/x86_64/25.08/${SDK_REV}/files
SDK_LIB=${SDK}/lib/x86_64-linux-gnu
export PKG_CONFIG_PATH="/tmp/opencode/pkgconfig:${SDK_LIB}/pkgconfig"
# -L for the linker: dev .so symlinks are stubs in /tmp/opencode/syslibs
# pointing at HOST runtime libs (never the flatpak SDK: mixing SDK gtk with
# host webkit segfaults at runtime). One full rebuild, then incremental.
export RUSTFLAGS="-L /tmp/opencode/syslibs ${RUSTFLAGS:-}"
# LD_LIBRARY_PATH is ONLY for RUNNING test binaries/harness bins IF they were
# linked against SDK libs. With host-only linking it should be unnecessary.
# NEVER export it globally: it shadows cargo's own libssl/curl and segfaults cargo.
sge-run() {
  LD_LIBRARY_PATH="${SDK_LIB}:/tmp/opencode/syslibs:${LD_LIBRARY_PATH:-}" "$@"
}
