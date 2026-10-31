# Apple Silicon optimizations (BUILD.md §10) — implemented

Status: complete (2026-07-26). These were specified in §10 during Phase 2
planning and verified in none of the phases that followed; this is the pass
that closed them, plus the one Phase 4 shader that was never written.

## §10.1 Mapped staging ring

`queue.write_texture` allocates an internal staging belt and memcpys the
whole slice into it. For a 105 MP CFA that is a second ~200 MB copy plus a
200 MB transient allocation, which §10.1 names explicitly as the thing not
to do.

`sp_gpu::upload::StagingRing` keeps three `MAP_WRITE | COPY_SRC` buffers
mapped between uses and hands rows to the GPU with `copy_buffer_to_texture`.
Chunking (32 MB per slot) bounds resident staging at 96 MB for any sensor
size; a 105 MP frame uploads in seven chunks, so the GPU starts consuming
the first rows while the CPU is still writing later ones.

Deviations:

- Buffer→texture copies require 256-byte row alignment; `write_texture` does
  not. Rows are padded on the way in. A Hasselblad X2D II row (11904 × 2 B)
  happens to be aligned, an X1D II row (8280 × 2 B) is not, so both paths are
  live and both are tested.
- §10.1 asks for the decode thread to write rows directly into the ring.
  `upload_texture_rows` exposes exactly that (the callback receives a
  `wgpu::WriteOnly` slice of mapped memory), but the decode worker does not
  use it yet: rawler decodes into its own `Vec`, so eliminating that copy
  means patching the vendored decoder, not the uploader. The API is in place
  for when that happens.

Measured: warm X2D II open+render 51–60 ms, against 63 ms before. The change
removes a 200 MB memcpy and an allocation; the wall-clock difference is
within run-to-run noise on this machine, and the memory-pressure difference
is the real win.

## §10.2 SHADER_F16 with an f32 fallback

Both device-creation sites request the feature when the adapter offers it,
through one shared helper (`GpuContext::preferred_features`) so they cannot
drift apart — the limits mistake §12 already records happened because two
sites were edited independently.

A shader opts in by placing `//#SP_PRECISION` before its first declaration;
`shader_with_precision` substitutes either

```wgsl
enable f16;
alias fp = f16;   // fp2/fp3/fp4 likewise
```

or the f32 equivalent. One source, both paths.

`shaders_compile.rs` validates BOTH variants through naga with no device, so
an f16-only typo cannot pass CI on a runner whose adapter lacks the feature —
and an f32-only typo cannot pass on a Mac.

Deviation: §10.2 says "tone/color/NR inner loops". Only NR uses it. Tone and
color are ΔE-critical — the golden tests hold ±2/255 and BUILD.md §7 requires
ΔE < 1.0 — and f16's ~1e-3 relative precision would spend a quarter of that
budget for a pass that is not bandwidth-bound. The mechanism is per-shader,
so flipping tone later is a one-line change plus a golden re-check.

## §10.3 Workgroup shared-memory tiling

`nr_main.wgsl` stages its 16×16 tile plus a 3 px apron in `var<workgroup>`
as pre-computed Oklab + cube-root luma. Each thread's 25 luma and 49 chroma
taps then read shared memory, and the Oklab transform (three `pow` calls per
tap) runs 484 times per workgroup instead of roughly 12 500.

All 256 threads fill the tile before the barrier, including threads whose own
pixel lies outside the image — the bounds check moved to *after*
`workgroupBarrier()` so the barrier stays in uniform control flow. Getting
this backwards is undefined behavior, not a visible bug, so it is worth
stating: **the early return must never precede the barrier.**

Shared memory: 3.9 KB with f16, 7.7 KB without, both inside the 16 KB
default limit.

Correctness after both changes: NR still cuts MSE 65% on the seeded-noise
fixture, unchanged from the pre-optimization number in `docs/phase-4.md`.

## Pre-demosaic NR (`nr_raw.wgsl`)

`plan_passes` has scheduled `PassId::RawDenoise` since Phase 0 and the shader
never existed. It does now: a bilateral over the 5×5 **same-color**
neighborhood of the prepared mosaic — a stride of two photosites in each
axis, which holds for every Bayer pattern and both green planes, so it needs
no pattern id.

Denoising before the demosaic is what the post-demosaic pass structurally
cannot do: by the time `nr_main` sees a pixel, Malvar has already spread each
noisy photosite across its 5×5 neighborhood as correlated error.

The range sigma tracks the shot-noise floor (`sqrt` of signal plus a
read-noise term) and is deliberately sized just above the noise it averages
away — below that, the range term rejects the very samples that would cancel
the noise, and the pass does nothing. Tile plus 4-site apron in shared
memory (2.3 KB). Values stay f32: this is pre-demosaic sensor data, where
f16 would quantize coarser than the 16-bit raw itself.

Scheduled only when luminance NR is engaged. Chroma-only NR is served
entirely by the post-demosaic Oklab bilateral, and the proxy path already
averages 2×2 quads.

Measured, luminance-only NR at 100 on the seeded-noise fixture: MSE against
the clean reference falls to 0.318 of the un-denoised render, against 0.535
with this pass bypassed. A step-edge fixture keeps its contrast at the same
strength (0.3098 → 0.3216), which is the assertion that stops anyone
"improving" the strength until it smears detail.

## Benches and budgets (BUILD.md §7)

`cargo bench -p sp-pipeline` reports open, proxy render, 1:1 tile (each with
and without NR) and the CPU tone-LUT build. Both the benches and
`tests/perf_budget.rs` wait on the GPU rather than on submit — without that,
every measurement reports command-encoding time and reads as ~0 ms.

`tests/perf_budget.rs` is the pass/fail half: a generous default tier that
any real GPU clears, and `SP_PERF_STRICT=1` at roughly 2× this machine. It
skips on software rasterizers, so CI cannot fail on lavapipe.

Release measurements on this machine, 6 MP synthetic:

| Operation | Time |
|---|---|
| `open_image` | 5.4 ms |
| `proxy_render` | 3.8 ms |
| `proxy_render_nr` | 8.0 ms |
| `tile_1to1` | 3.4 ms |
| `tile_1to1_nr` | 7.9 ms |
| `build_tone_lut` | 41 µs |
