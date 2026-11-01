#!/usr/bin/env bash
# Local continuous integration: the same checks as .github/workflows/ci.yml and
# fuzz.yml, run on this machine instead of GitHub-hosted runners.
#
#   scripts/ci-local.sh                 # weights, fmt, clippy, tests, visual regression
#   scripts/ci-local.sh --skip-visual   # without the GUI screenshot comparison
#   scripts/ci-local.sh --fuzz 300      # also fuzz the RAW decoder for 300 s (nightly + cargo-fuzz)
#
# CARGO_TARGET_DIR is honoured, so exported trees can share build caches.
set -euo pipefail
VISUAL=1; FUZZ_SECONDS=0
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-visual) VISUAL=0 ;;
    --fuzz) FUZZ_SECONDS="${2:-}"; shift || true
      case "$FUZZ_SECONDS" in ''|*[!0-9]*) echo "--fuzz needs a duration in whole seconds" >&2; exit 2 ;; esac ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"; cd "$ROOT"
step() { printf '\n==> %s\n' "$1"; }
START=$(date +%s)

step "Reject bundled neural weights"
WEIGHT_RE='\.(mlmodelc|mlpackage|mlmodel|onnx|safetensors|pt|pth|ckpt)(/|$)|weight\.bin$|(^|/)(DenoiseModels|EnhancementModels|models)/'
if git rev-parse --is-inside-work-tree >/dev/null 2>&1 && [ "$(git rev-parse --show-toplevel)" = "$ROOT" ]; then
  FILES="$(git ls-files)"
else
  FILES="$(find . -path ./target -prune -o -type f -print | sed 's|^\./||')"
fi
if printf '%s\n' "$FILES" | grep -iE "$WEIGHT_RE"; then
  echo "Neural weights must be installed on demand, never committed" >&2; exit 1
fi

step "CMake for the default JPEG XL build"
cmake --version | head -1

step "cargo fmt --check"
cargo fmt --all --check

step "cargo clippy (-D warnings)"
cargo clippy --all-targets --workspace --quiet -- -D warnings

step "cargo test --workspace"
cargo test --workspace --quiet

if [ "$VISUAL" = 1 ]; then
  step "Visual regression (SDR welcome window)"
  SHOT="$(mktemp -d)/shopphoto-actual.png"
  cargo build --quiet -p sp-app --bin shopphoto --bin visual_compare
  BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug"
  # A throwaway home keeps the welcome window deterministic and the real
  # catalog untouched (only for the app; cargo needs the real home).
  VHOME="$(mktemp -d)"
  HOME="$VHOME" SHOPPHOTO_SDR=1 SP_VISUAL_REGRESSION=1 SP_SCREENSHOT="$SHOT" "$BIN/shopphoto"
  # The committed reference comes from a 1x 1600x952 runner. Other displays
  # (Retina, smaller screens) compare against a baseline recorded on this
  # machine for the same pixel size; the first run records it for review.
  SIZE="$(python3 -c 'import struct,sys; d=open(sys.argv[1],"rb").read(24); print("%dx%d" % struct.unpack(">II", d[16:24]))' "$SHOT")"
  REFERENCE=docs/research-2026-08-31/assets/ui-final-empty.png
  if [ "$SIZE" != 1600x952 ]; then
    REFERENCE="$ROOT/.ci-local/visual/ui-final-empty-$SIZE.png"
    if [ ! -f "$REFERENCE" ]; then
      mkdir -p "$(dirname "$REFERENCE")"; cp "$SHOT" "$REFERENCE"
      echo "Recorded a local visual baseline for $SIZE: $REFERENCE (review it once)."
    fi
  fi
  "$BIN/visual_compare" "$REFERENCE" "$SHOT" 18.0 0.20
fi

if [ "$FUZZ_SECONDS" -gt 0 ]; then
  step "Decoder fuzzing for ${FUZZ_SECONDS}s"
  command -v cargo-fuzz >/dev/null || { echo "Install with: cargo install cargo-fuzz --locked" >&2; exit 1; }
  (cd fuzz && cargo +nightly fuzz run raw_decode -- -max_total_time="$FUZZ_SECONDS" -rss_limit_mb=4096)
fi

printf '\nLocal CI passed in %ss.\n' "$(( $(date +%s) - START ))"
