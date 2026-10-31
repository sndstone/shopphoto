# Export, AI input and preview performance — 2026-10-10

This phase implements the follow-up [performance review](phase-performance-review.md)
and packages both these changes and the earlier [AI worker improvements](phase-ai-performance.md)
in the personal 0.4.3 release.

## Implementation

- Export resizing preserves the original linear-light Lanczos tap and sum order.
  Decoded transfer tables use at most 256 KiB; each axis cache is limited to
  8 MiB, with computed taps for extreme dimensions. No full-image float cache
  is allocated. The old resampler remains a test-only numerical reference.
- Exact default geometry preserves original samples instead of resampling
  and introducing floating-point roundoff. Unchanged size and sharpening
  bypass their work. Owned GPU-to-export handoffs retain both pixel buffers
  and consume them through preparation; the borrowed library APIs remain
  compatible. Required geometry, sharpening and colour transforms still run.
- Interactive AI keeps one canonical uncompressed RGBA8 file, within a
  default 512 MiB retained-file allowance. `SHOPPHOTO_MASK_INPUT_CACHE_MB`
  accepts 64–1024 MiB. Larger inputs run without retention. Keys include weak
  source identity, source generation, normalized render edits, capture metadata,
  resolved profiles/looks/lenses and applied camera denoise. Local masks,
  geometry and sample markers do not alter this canonical render. Weak
  identities retain no decoded RAW/RGB pixel vectors. Cache files are released
  on replacement, cancellation/failure, two minutes idle and shutdown.
- Export readbacks copy each tile's interior into a two-slot staging ring
  before its output texture is reused. Buffers are reused and bounded to
  two 1024-pixel RGBA tiles (at most 16 MiB total payload). Mapping uses
  nonblocking device polls with cancellation and a 30-second timeout.
  Row padding, high-precision codes, apron trimming and coverage checks
  remain explicit. `SHOPPHOTO_EXPORT_READBACK_SLOTS=1` permits serial QA.
- The viewport retains a pre-tone image, bounded to 64 MiB, after white balance,
  profile HueSat conversion and enabled RGB noise reduction. Downstream tone,
  colour, local edits, sharpening and display options reuse it. Source opens,
  source rebakes, white balance, negative/recovery, effective NR and resolved
  profile/look changes invalidate it. Larger proxies run without retention.
  Diagnostics include its texture bytes. A disabled-cache reference path
  remains available for equivalence checks.

The renderer still builds per-render colour buffers and lookup textures;
this phase does not claim to eliminate all CPU encoding overhead. Full CFA
development remains on the GPU.

## GPU and NPU placement

On the review Mac, the optional runtime reports `mps`: neural object/text/
semantic/depth/matting/super-resolution models run through PyTorch Metal on
the Apple GPU. Unsupported operators may fall back to CPU. OpenCV face/eye
detection, edge refinement and file processing use CPU work. See
[PyTorch's MPS documentation](https://docs.pytorch.org/docs/2.14/notes/mps.html).

Camera RAW denoise uses the separate Rust/Core ML bridge on macOS. Its
default `MLComputeUnitsAll` permits CPU, GPU and the Apple Neural Engine;
the OS selects/partitions supported work. Permission to use the Neural Engine
is not proof that a given prediction ran there. See
[Apple's compute-unit documentation](https://developer.apple.com/documentation/coreml/mlcomputeunits?language=objc).
`SHOPPHOTO_COREML_UNITS=cpu_ane` excludes the GPU; `cpu_gpu` excludes the Neural
Engine; `cpu` restricts processing to CPU. Changing this option needs timing
and quality checks. No compute-unit default or experimental public-depth backend was changed in
this release.

## Validation

The native-codec workspace passed 547 tests, zero failures and six explicit
ignores. All three real interactive-worker cases were additionally invoked
and passed: process reuse/error/cancel/idle/shutdown; PNG/RGBA transport parity;
and canonical-render reuse with edit invalidation and identical complete
16-bit masks. The last case observed one fresh render/staging at 47.3 ms and
a cache lookup at 0.089 ms for 1672 × 941 pixels; these are single observations
of input preparation, not whole-mask or large-RAW timing guarantees.

Resizer/reference tests cover 8/16-bit sRGB, Adobe RGB and PQ, translucent alpha,
odd sizes, one-dimensional inputs, severe reductions and upsampling. Owned
neutral preparation checks exact samples and allocation identity. GPU tests
check copied regions after texture reuse, unaligned rows, ring capacity,
cancellation/recovery, multi-tile spatial seams, 16-bit PQ export and disabled/
cached preview equivalence. Canonical-cache tests cover invalidation, no RAW
retention, oversize bypass, failure/cancel and idle/shutdown file cleanup.

Formatter, whitespace checks and strict workspace/all-target native-codec
Clippy passed. Performance measurements and final package acceptance are
recorded in [the release notes](release-0.4.3.md).

Python real-model validation passed 20 tests with two portrait-dependent skips.
Current logs are `/tmp/shopphoto-043-workspace-tests.log`,
`/tmp/shopphoto-043-clippy.log`, `/tmp/shopphoto-043-real-worker.log`,
`/tmp/shopphoto-043-python-tests.log`, `/tmp/shopphoto-043-performance-two-slots.log`
and `/tmp/shopphoto-043-performance-one-slot.log`.

Personal packaging completed and the mounted universal 0.4.3 artifact passed
codec/CLI/MCP checks. The final artifact and checksum are listed in the
release notes. Installed applications and previous DMGs were preserved.
