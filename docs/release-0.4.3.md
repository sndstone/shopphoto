# ShopPhoto 0.4.3 — faster exports, AI selections and previews

This personal release includes persistent interactive AI workers and bounded
model/image caches, retained canonical AI input, faster linear-light export
resizing, fewer image copies, overlapped GPU tile transfers and a bounded
viewport prefix cache. It retains the Presets/AI/Workflow sidebar.

## Measured improvements

Isolated M5 Pro, 24 GiB, optimized ARM64 Rust; synthetic export/RAW fixtures.

| Operation | Before/reference | New | Scope |
| --- | ---: | ---: | --- |
| 1200 × 800 → 600 × 400 preparation | 1.726 s | 0.0940 s | Same earlier preparation fixture; about 18× faster; excludes GPU rendering/encoding |
| Neutral 12 MP preparation | 0.761 s | 0.000702 s | Borrowed API, warm repeated synthetic buffer; no geometry resampling, one required output copy |
| NR-enabled proxy tone change | 11.59 ms | 1.45 ms | Median five runs, CPU encoding + GPU completion, histogram disabled; about 8× faster |
| 12 MP tiled 8-bit development/readback | 84.61 ms | 52.72 ms | Serial vs two-slot transfers, median five runs; about 38% lower latency |
| 12 MP tiled 16-bit development/readback | 92.25 ms | 56.07 ms | Same comparison; about 39% lower latency |

The 45× resizer from the earlier review was an unbounded full-linear-image
prototype. The production version uses small transfer/axis caches instead
and the measured complete preparation above is about 18× faster. These
measurements do not establish equivalent gains for total file exports,
different edits, 50–100 MP photographs or other computers.

The earlier Python benchmarks also remain relevant: warm point mask processing
improved from 0.780 to 0.351 s and depth from 0.160 to 0.0447 s on the 1672 × 941
scene after image-inference reuse. Canonical Rust input reuse now additionally
avoids repeated GPU development and staging. The actual-worker acceptance case
confirmed identical complete 16-bit masks and recomputation after a changed edit.

Run `cargo run -p sp-pipeline --release --example performance` alone to reproduce
the Rust measurements; use `SHOPPHOTO_EXPORT_READBACK_SLOTS=1` for serial
transfer comparison. Details, limits and output contracts are in
[the implementation record](phase-performance-implementation.md).

## GPU or NPU?

Object/text/semantic/depth masks, matting and super-resolution use PyTorch's
Apple GPU/MPS backend on Apple Silicon. Unsupported operators and OpenCV
detectors/refinement can use CPU. These enhancement models do not use the
Apple Neural Engine in this release.

Camera RAW denoise uses Core ML on macOS, allowing CPU, GPU and Neural Engine.
Core ML chooses supported placement; the current configuration does not
establish actual NPU use by every model/layer. The optional Python runtime remains separate.

## Validation and package

The native-codec workspace passed 547 tests, zero failures and six explicit
ignores; all three real interactive-worker cases additionally passed. Python
passed 20 tests with two portrait-dependent skips. Resizer/reference parity,
pixel/allocation preservation, canonical-cache invalidation/cleanup, multi-tile
spatial seams, 16-bit PQ, staging reuse/cancellation and cached/uncached preview
equivalence passed. Strict workspace/all-target Clippy, formatting and whitespace
checks passed on macOS.

The DMG contains a universal arm64/x86_64 app. It is ad-hoc signed and
not notarized. Apple Silicon native codecs require macOS 27.0; the Intel slice
retains standard codecs and macOS 12.0. Intel compilation does not establish
physical Intel acceptance.

Final DMG integrity, signatures, mounted smoke tests
and its SHA-256 checksum are recorded in the repository handoff and the
checksum file alongside the artifact. Install by dragging ShopPhoto.app into
Applications. The existing installed app and earlier DMGs remain available.

## Final artifact verification

- Artifact: `dist/ShopPhoto-0.4.3-personal.dmg`, 703,002,354 bytes (670.4 MiB).
- SHA-256: `6b1197e0218a7fb54d6c0379e81fb5511dc29b6a4b23740ae06a6600033d919e`.
  The adjacent checksum file uses the DMG basename.
- DMG integrity, mounted deep strict signature, version 0.4.3 and both universal
  slices passed.
- All six relocated native libraries loaded from the bundle. Mounted
  JPEG/HEIC/AVIF rendering, resized 16-bit PNG, JSONL edits/export/undo and MCP
  preview passed with original hashes and sidecars preserved.
- The installed app remains version 0.4.1. This release did not install over it.
  ARM native codec and Intel runtime limitations remain as described above.
