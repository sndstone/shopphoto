# Live zoomed previews and Camera RAW compute preferences

Implemented after the verified 0.4.3 personal DMG, on 2026-10-10. Included in
the [0.4.4 personal release](release-0.4.4.md). Earlier DMGs and the installed
application are unchanged.

## Preview behavior

Full-resolution tiles previously waited whenever the primary pointer was held,
including on adjustment sliders. They now refresh during image-control, preset,
before/after and view-option changes. Pure pan/zoom gestures still defer new
tile coverage until idle, retaining overscan reuse. Before/after comparison
pipelines follow the same policy so either displayed side remains current.

## Camera RAW denoise setting

Open **Tools → Settings… → Camera RAW denoise**. Choices are:

- Automatic (recommended): Core ML may choose CPU, GPU and Neural Engine.
- GPU + CPU: excludes the Neural Engine.
- Neural Engine (NPU) + CPU: excludes the GPU; enabled on Apple Silicon only.
- CPU only: useful for comparison and troubleshooting.

The policy is saved in preferences, defaults to Automatic for older files, and
applies to interactive Camera RAW denoise, batch Auto and GUI exports. Loading
preferences for an unavailable device restores Automatic. Windows/Linux RAW
inference currently uses the existing CPU ONNX runtime; hardware choices are
disabled there. Other optional Python AI tools retain their existing backend.

Selection is a policy, not an assertion about actual per-layer placement.
Core ML can fall back to CPU for unsupported layers, and allowing Neural Engine
does not guarantee that every layer runs there. The setting does not affect
Metal viewport rendering or manual noise reduction.

A policy change clears the current unblended inference and queues a new run if
camera denoise is active. Generation checks reject stale worker results. Each
worker request carries its own policy; no process environment is mutated. Both
retained model sessions and disk inference keys include the policy, preventing
reuse of another device mode's output. GUI exports only reuse a current RAW
result if its policy matches; otherwise they start from untouched source CFA.
The existing `SHOPPHOTO_COREML_UNITS` override remains for CLI calls, while GUI
requests use the saved setting explicitly.

Apple references:
[Neural Engine + CPU](https://developer.apple.com/documentation/coreml/mlcomputeunits/cpuandneuralengine),
[GPU + CPU](https://developer.apple.com/documentation/coreml/mlcomputeunits/cpuandgpu),
[Core ML placement](https://developer-mdn.apple.com/videos/play/wwdc2021/10038/).

## Verification

- Workspace tests: 550 passed, 0 failed, 6 explicitly ignored.
- A real GPU regression exercises three consecutive held-pointer egui frames:
  full-resolution tile pixels change for every exposure adjustment before release.
- Preference compatibility/roundtrip, retained-session replacement and disk-cache
  separation tests pass; changing policy keeps already borrowed sessions valid.
- Settings UI rendered offscreen in English and Dutch; both layouts reviewed.
- Native strict all-target workspace Clippy, formatting and diff checks pass.
- Intel macOS workspace compilation passes; no physical Intel runtime test.

The initial hardware acceptance fixture also exposed a pre-existing unsigned
subtraction panic for undersized sensor images. RAW inference now checks sensor
buffer/active-area dimensions and rejects inputs too small for the selected
model's 512×512 Bayer-quad layout before loading the model. A regression covers
undersized, truncated and overflowing active areas. Hardware acceptance uses
the established 1024×1024 sensor fixture; normal sensor tiling is unchanged.
