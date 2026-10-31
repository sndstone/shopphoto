# Phase 5 — Supersampling and export

Implemented in `sp-pipeline`, `sp-export`, and the app export modal.

- Exact Bayer-quad Super2x2 render path; tiled 1024² Malvar full-resolution path.
- Linear-light 3×3/4×4 box downsampling, crop/straighten/flip, Lanczos-3 sizing.
- Luminance unsharp-mask shader after tone/profile look.
- Atomic JPEG (4:4:4 or 4:2:2), Deflate TIFF 8/16, PNG 8/16, 10-bit AVIF, and 16-bit JPEG XL output. JPEG XL supports lossy/lossless SDR with ICC and HDR with PQ signaling in the default build. High-bit-depth outputs use a no-dither `rgba16uint` GPU handoff retained through geometry and sizing; round-trip tests enforce fidelity.
- Generated sRGB, Display P3, and Rec.2020 ICC profiles; named presets persist to versioned JSON and mirror into the catalog database.
- Dedicated export renderer on the shared wgpu device, per-file/tile progress, cancellation, command-click filmstrip/grid multi-selection, and sequential batch export without blocking editing.
- Camera, lens, capture-time, ISO, exposure, aperture, focal-length and `Software=ShopPhoto` metadata are embedded in JPEG/PNG/AVIF and TIFF where enabled. ShopPhoto never synthesizes GPS tags, so Strip GPS is enforced by construction.
