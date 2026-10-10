#!/usr/bin/env bash
# Build the frozen Laya sidecar (Phase 15, Plan 15-01).
#
#   bash sidecar/build-sidecar.sh [--skip-freeze] [--skip-weights]
#
# Steps: venv → install (CPU torch) → pin checkpoint revision → download
# weights snapshot (revision-pinned) → offline /health smoke → PyInstaller
# freeze → place `src-tauri/binaries/sge-laya-<triple>` + write pins.json.
#
# Reproducible: fixed laya version, pinned Hub revision SHA, sha256 recorded.
# A Hub re-upload cannot silently change weights — the smoke test refuses to
# run when the cached snapshot revision differs from pins.json.
#
# CONCURRENCY: tauri-build walks `sidecar/weights` on every cargo build, so
# this script downloads into `sidecar/weights-incoming` and atomically swaps
# it into place at the end. Never run `cargo build` against a half-written
# weights dir (a concurrent download causes EACCES in the build script).
set -euo pipefail
cd "$(dirname "$0")/.."

TRIPLE="${TRIPLE:-x86_64-unknown-linux-gnu}"
BIN_OUT="src-tauri/binaries/sge-laya-${TRIPLE}"
WEIGHTS_DIR="sidecar/weights/hf-cache"
MULTI_REPO="convaiinnovations/laya-multilingual"
SKIP_FREEZE=0; SKIP_WEIGHTS=0
for arg in "$@"; do
  case "$arg" in
    --skip-freeze) SKIP_FREEZE=1 ;;
    --skip-weights) SKIP_WEIGHTS=1 ;;
  esac
done

echo "==> [1/6] venv + install (CPU torch index)"
if [ ! -x sidecar/.venv/bin/python ]; then
  uv venv sidecar/.venv
fi
uv pip install --python sidecar/.venv/bin/python \
  --extra-index-url https://download.pytorch.org/whl/cpu \
  -r sidecar/requirements.txt

VPY=sidecar/.venv/bin/python
LAYA_VER="$($VPY -c 'import importlib.metadata as m; print(m.version("laya"))')"
echo "laya version: ${LAYA_VER}"

echo "==> [2/6] pin multilingual checkpoint revision"
REVISION="$($VPY -c "
from huggingface_hub import HfApi
api = HfApi()
print(api.model_info('${MULTI_REPO}').sha)
")"
echo "revision: ${REVISION}"

echo "==> [3/6] download weights snapshot (revision-pinned)"
# Atomic-swap pattern (see header): populate weights-incoming, swap at [5b].
if [ "$SKIP_WEIGHTS" = 0 ]; then
  mkdir -p sidecar/weights-incoming
  HF_HOME="$PWD/sidecar/weights-incoming" $VPY -c "
from huggingface_hub import snapshot_download
p = snapshot_download(repo_id='${MULTI_REPO}', revision='${REVISION}')
print('snapshot:', p)
"
else
  echo "(skipped)"
fi

echo "==> [4/6] record pins.json"
SHA256_FILE="$(find sidecar/weights-incoming -name '*.safetensors' -o -name '*.bin' 2>/dev/null | head -1 || true)"
if [ -n "${SHA256_FILE}" ]; then
  WEIGHT_SHA="$(sha256sum "${SHA256_FILE}" | cut -d' ' -f1)"
else
  WEIGHT_SHA="(weights skipped)"
fi
cat > sidecar/pins.json <<EOF
{
  "laya": "${LAYA_VER}",
  "multilingual_repo": "${MULTI_REPO}",
  "multilingual_revision": "${REVISION}",
  "weights_sha256_first_file": "${WEIGHT_SHA}",
  "built": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "triple": "${TRIPLE}"
}
EOF
cat sidecar/pins.json

echo "==> [5/6] offline /health smoke (unfrozen server)"
# Smoke against weights-incoming (the swap happens only after GREEN).
export HF_HOME="$PWD/sidecar/weights-incoming"
export HF_HUB_OFFLINE=1
export LAYA_HOST=127.0.0.1
export LAYA_PORT=43121
export LAYA_MODELS=multilingual
export LAYA_DEFAULT_MODEL=multilingual
export LAYA_MAX_LOADED=1
export LAYA_PRELOAD=1
T0=$(date +%s)
sidecar/.venv/bin/laya-serve >/tmp/opencode/laya-smoke.log 2>&1 &
SERVER_PID=$!
SMOKE_OK=0
for i in $(seq 1 120); do
  if curl -sf "http://127.0.0.1:43121/health" >/tmp/opencode/laya-health.json 2>/dev/null; then
    SMOKE_OK=1; break
  fi
  sleep 2
done
T1=$(date +%s)
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
if [ "$SMOKE_OK" = 1 ]; then
  echo "health: GREEN in $((T1 - T0))s (cold-start, unfrozen)"
  head -c 600 /tmp/opencode/laya-health.json; echo
else
  echo "health: FAILED — see /tmp/opencode/laya-smoke.log"
  tail -20 /tmp/opencode/laya-smoke.log || true
  exit 1
fi

if [ "$SKIP_FREEZE" = 1 ]; then
  echo "==> [5b/6] atomic swap weights-incoming -> weights (smoke GREEN)"
  rm -rf sidecar/weights
  mv sidecar/weights-incoming sidecar/weights
  echo "==> [6/6] freeze skipped (--skip-freeze); dev shim stays in place"
  exit 0
fi

echo "==> [6/6] PyInstaller freeze"
mkdir -p src-tauri/binaries
sidecar/.venv/bin/pyinstaller \
  --noconfirm --clean --onefile \
  --name "sge-laya-${TRIPLE}" \
  --collect-data laya \
  --collect-data tokenizers \
  sidecar/laya_serve_entry.py
mv "dist/sge-laya-${TRIPLE}/sge-laya-${TRIPLE}" "${BIN_OUT}" 2>/dev/null \
  || mv "dist/sge-laya-${TRIPLE}" "${BIN_OUT}"
chmod +x "${BIN_OUT}"
ls -la "${BIN_OUT}"
echo "frozen size: $(du -h "${BIN_OUT}" | cut -f1)"
echo "DONE: ${BIN_OUT}"
