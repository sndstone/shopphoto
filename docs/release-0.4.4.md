# ShopPhoto 0.4.4 — live zoomed previews and RAW denoise device choices

Zoomed full-resolution previews now update while adjusting image controls,
including before/after comparisons. Pure pan/zoom gestures retain deferred tile
coverage and overscan reuse.

Open **Tools → Settings… → Camera RAW denoise** to select a saved policy:

- **Automatic (recommended)** — the default, allowing CPU, GPU and Neural Engine.
- **GPU + CPU** — excludes the Neural Engine.
- **Neural Engine (NPU) + CPU** — excludes GPU, available on Apple Silicon.
- **CPU only** — for comparison and troubleshooting.

Core ML can use CPU fallback for unsupported layers. Neural Engine selection
allows NPU execution without guaranteeing every layer runs there. The setting
applies to interactive camera RAW denoise, batch Auto and GUI exports; other
Python AI tools keep their existing backend. Changing the choice invalidates
inference/session caches and reruns active RAW denoise. Older preference files
retain Automatic. Undersized/malformed sensor input now returns an error before
model loading rather than overflowing the tiler's dimensions.

The [implementation record](phase-live-preview-compute.md) describes behavior,
cache isolation and hardware acceptance. This release also retains all
[0.4.3 performance improvements](release-0.4.3.md).

## Validation and package

The live-update GPU regression, preference compatibility, session replacement,
disk-cache separation and malformed RAW checks passed. Settings layouts were reviewed
in English and Dutch. Actual per-layer NPU placement was not profiled.

The native-codec workspace passed 554 tests, zero failures and six explicit
ignores. Strict all-target workspace Clippy, formatting and whitespace checks
passed. Final mounted acceptance and checksum are recorded in the repository
handoff and the checksum file alongside the DMG.

The package contains a universal arm64/x86_64 app. It is ad-hoc signed and not notarized.
Apple Silicon native codecs require macOS 27.0; Intel retains standard codecs
and macOS 12.0. Intel compilation does not establish physical Intel acceptance.

Install by dragging ShopPhoto.app into Applications. Building this release does
not replace the installed application or earlier DMGs.

## Final artifact verification

- Artifact: `dist/ShopPhoto-0.4.4-personal.dmg`, 703,131,024 bytes (670.6 MiB).
- SHA-256: `af7aaf6106aa6a994e81a528942ae6a1bd29e4101534f2835d25d240362c337d`; adjacent basename-based checksum verified.
- DMG integrity, mounted version 0.4.4, both arm64/x86_64 slices and deep strict
  signature passed. All six relocated native libraries loaded from the bundle.
- Mounted JPEG/HEIC/AVIF rendering, resized 16-bit PNG, JSONL edits/export/undo and
  MCP preview passed with original hashes and sidecars preserved.
- The mounted GUI launched, rendered a disposable image at 100% zoom with edited
  controls, produced a reviewed screenshot and exited normally. The temporary
  image hash stayed unchanged and no sidecar was written.
- The installed application remains version 0.4.1; earlier DMGs remain.

The DMG's enclosed notes predate these final integrity checks; the repository
handoff and adjacent checksum record final artifact verification.
