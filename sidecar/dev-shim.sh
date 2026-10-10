#!/usr/bin/env bash
# DEV SHIM (Phase 15) — stands in for the frozen PyInstaller binary until
# `sidecar/build-sidecar.sh` produces `sge-laya-<triple>`.
#
# Forwards to a `laya-serve` from a local venv when present; otherwise exits
# non-zero with a plain-language message so the Rust supervisor reports the
# sidecar honestly Down instead of hanging.
set -u
for venv in \
  "$(dirname "$0")/../../sidecar/.venv" \
  "/tmp/opencode/sidecar-spike/laya-env"; do
  if [ -x "$venv/bin/laya-serve" ]; then
    exec "$venv/bin/laya-serve" "$@"
  fi
done
echo "sge-laya dev shim: no laya-serve venv found (run sidecar/build-sidecar.sh)" >&2
exit 1
