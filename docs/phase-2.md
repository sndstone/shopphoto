# Phase 2 — GPU Pipeline Foundation: completion note

Core completed 2026-07-20 (session 3). The app renders real Hasselblad
sensor data on the GPU: CFA upload → fused superpixel demosaic → proxy →
white balance + camera matrix → Linear Rec.2020 working space → sRGB output
texture displayed by egui. The exposure slider re-renders live.

## Acceptance status (BUILD.md Phase 2)

- [x] Color-correct image < 500 ms: X2D II 105 MP measured at ~160 ms total
      (78 ms decode + 83 ms GPU) in a dev build.
- [x] Golden tests: synthetic CFA through the real GPU vs CPU reference,
      tolerance ±2/255 across three colors; +1 EV linearity verified.
- [x] Shader CI gate: all 6 WGSL files validate on a headless adapter.
- [x] GPU memory: proxy design keeps working textures at 2048-long-edge
      (~25 MB); the only large allocations are the resident CFA (u16,
      ~210 MB for 105 MP) and the transient quarter-res superpixel image.
- [ ] Full-quality Malvar render + 1:1 tile zoom — deferred (shader written
      and validated; executor path not wired). See PROGRESS.md remainder.
- [ ] Pan/zoom input, histogram, dither — deferred to Phase 3 alongside the
      tone pipeline.

## Deviations / notable decisions

1. **Proxy strategy simplified vs BUILD.md §3.5**: the interactive proxy is
   built from the superpixel (quarter-res) image box-downsampled to a fixed
   2048-long-edge, rather than viewport-tracked 2× sizing. Correct trade-off
   for now; Phase 8 revisits.
2. **Output texture is `Rgba8Unorm` with in-shader sRGB encoding** because
   egui's `register_native_texture` requires Rgba8Unorm. This also
   pre-figures the Phase 6 composite design (encoding always in-shader).
3. **Texture-size limits**: eframe's default device caps 2D textures at
   8192 px; 100 MP Hasselblads need 11904. Both device-creation paths now
   request the adapter's real limit. GPUs limited to 8192 (some 2013-era
   cards) would need tiled CFA upload — recorded as a Phase 8 concern.
4. Superpixel shader fuses raw-prepare (black/white normalization) so the
   fast path touches each photosite exactly once and no full-res float
   intermediate exists on the fast path.
5. Tier-1 camera profile (embedded matrix, D65) is resolved through
   `CameraProfile::standard` — the Phase 3B profile abstraction is already
   load-bearing.
