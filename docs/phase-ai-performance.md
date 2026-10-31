# AI performance implementation — 2026-10-10

Interactive masks now reuse a Rust-owned serial worker and bounded model/image
caches. GUI input avoids intermediate PNG compression and decoding. The
implementation retains existing Float32 inference, full-resolution refinement,
16-bit alpha output, cancellation and source/edit validation.

## Changes

- `app/enhance_worker.rs`: app-owned executor with one pending action,
  cancellation of the active action on shutdown, process kill/reap on failures
  and cancellation, and release after 120 seconds idle. A stopped executor is
  recreated on the next mask action. Batch workers retain their existing
  per-thread ownership; models are never shared concurrently across threads.
- `app/enhance.rs`: mask rendering runs on that executor. Results check the
  open-source generation as well as path and edit snapshot, covering a reopen
  of the same filename. Uncompressed RGBA8 input is validated and memory-mapped
  by Python. The temporary renderer, render readback and source reference are
  released before inference. Temporary disk usage is higher than PNG input.
- `scripts/enhance.py`: retain at most four networks within a default 768 MiB
  parameter/buffer allowance; separate cheap face/eye detectors. Cache SAM
  image embeddings, depth maps and semantic logits within 64 MiB/four entries
  for the current proxy image. Keys use actual RGB pixels, dimensions, pinned
  model revision and device. Changed prompts/thresholds/classes reuse image
  inference; changed pixels invalidate it. Edge refinement uses original RGB.
- Small tile batches preserve the original tile/halo dimensions and group only
  consecutive tiles with equal sizes. Matting and SwinIR have numerical checks
  for batches 1/2/4; HAT/DRCT keep single-image calls. Default remains one tile
  because the local benchmarks did not establish a useful default gain.
- `scripts/benchmark-enhance.py` provides offline timings and parity checks.
  `scripts/prototype-coreml-depth.py` is a separate fixed-shape native-inference
  feasibility probe. Neither script downloads models.

Configuration and memory limits are described in [AI runtime](ai-runtime.md).
The weight allowance excludes activations, Python/driver overhead and allocator
caches, and is separate for each export worker. An oversized model executes
without being retained. `SHOPPHOTO_ENHANCE_CACHE_MB` is clamped to 128–2048 MiB;
`SHOPPHOTO_ENHANCE_TILE_BATCH` is clamped to 1–4, default 1.

## Measurements

M5 Pro, 24 GiB, Python 3.14.7, PyTorch 2.14.0, Metal/MPS. The fixture is the
1672 × 941 `docs/advanced-ai-features/ai-mask-scene.png`. Cache/transport results
are medians of three measured warm repetitions after an unmeasured warmup;
all compared 16-bit output masks matched exactly.

| Operation | Baseline | Improved | Measured ratio |
| --- | ---: | ---: | ---: |
| Point-mask image encoding reuse | 0.7799 s | 0.3512 s | 2.22× |
| Depth prediction reuse | 0.1602 s | 0.04470 s | 3.58× |
| Cached depth mask: image decoding vs mapped RGBA8 | 0.04471 s | 0.03087 s | 1.45× |
| Cached point mask: image decoding vs mapped RGBA8 | 0.3470 s | 0.3358 s | 1.03× |

These cover Python mask processing/refinement/output, excluding Rust rendering,
input staging/encoding and IPC. They are not whole-GUI or large-RAW guarantees.
The cache baseline retains warm models but clears image products before each
request, so the gain is from reusing image inference, not library/model startup.
The prior fresh-process text measurement is in [the original review](ai-performance-review.md).

Matting batches 2/4 were approximately neutral on this fixture. On a 640 × 360
SwinIR input, medians were 6.149 s (single), 6.199 s (batch 2), and 6.021 s
(batch 4), with identical quantized outputs. This is not enough evidence to
change default memory/throughput behavior. Numerical acceptance permits at
most two 16-bit codes of difference for batching; the measured cases were zero.

Raw benchmark results on the review machine are
`/tmp/shopphoto-ai-final-benchmarks.json` and
`/tmp/shopphoto-ai-production-benchmarks.json` (SwinIR comparison).
Run with the optional runtime's Python, for example:

```sh
python scripts/benchmark-enhance.py --output /tmp/ai-benchmarks.json
python scripts/benchmark-enhance.py --sr-model /path/to/SwinIR-M-x2.pth --output /tmp/ai-sr-benchmarks.json
```

## Native inference investigation

The Float32 Depth Anything V2 Small prototype exported a static
`[1,3,518,924]` Core ML model. Initial conversion failed on
`upsample_bicubic2d` in DinoV2's positional encoding. Precomputing that exact
shape-dependent encoding solved the export, with bit-identical CPU PyTorch
output before/after that transformation; bicubic was not replaced by bilinear.

The reusable probe measured 55.00 ms Core ML vs 60.86 ms warm PyTorch/MPS
forward (five-run medians), a 9.6% latency reduction. Maximum normalized depth
difference was `2.85e-5`. This comparison excludes preprocessing/refinement
and does not exercise a Rust FFI. Compute units were selected by Core ML;
Neural Engine placement was not established.

The probe used coremltools 9.1.dev1 in an isolated `/tmp` evaluation directory:
stable 9.0 had no Python 3.14 binary wheel on this host. The production runtime
and installer do not gain a coremltools dependency. Static shape coverage,
more photographs, accelerator placement and multi-device quality/throughput
validation are required before enabling a native enhancement backend. The
prototype is not bundled. No model training or quantization was introduced.

## Validation and scope

- Full native-codec workspace: 537 passed, zero failed, five explicit ignores
  (the three existing fixture-dependent cases plus two new runtime bridge cases).
- Strict all-target/workspace Clippy, formatter and whitespace checks passed.
- Both new ignored Rust acceptance cases were explicitly run and passed:
  separate actions retain one process; failures/cancellation/idle/shutdown kill
  and reap it; PNG and RGBA8 requests return identical 16-bit masks.
- Python suite: 20 passed, two portrait-dependent cases skipped. Real SlimSAM
  prompts, depth/semantic prediction reuse, changed-pixel invalidation,
  CLIPSeg, ViTMatte batches, SwinIR batches, PQ HDR preservation, input validation
  and byte/count eviction all passed. Semantic changes used the cached optional
  model and the local scene; no new model download occurred.
- The reusable native prototype exported, predicted and emitted its numerical
  report successfully. Results are in `/tmp/shopphoto-native-depth-reusable-results.json`.

This phase improves runtime lifetime and repeated image work. It does not
migrate every model into Rust, promise a particular accelerator speedup or
alter the trained weights. The local development executable is rebuilt; the
existing personal 0.4.2 DMG predates these changes and has not been rebuilt.
Native/Linux/Windows acceptance beyond this macOS host remains separate.
