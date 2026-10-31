# Tone upgrade C — Local contrast

Status: complete (2026-07-22).

Added the culled `LocalContrast` pass after denoise/tone/profile processing and
before sharpening. Texture uses a small Oklab-L residual with strong-edge
protection, Clarity uses a broad midtone-weighted Oklab-L residual, and Dehaze
uses histogram-derived airlight plus saturation compensation. Tile renders add
a 36-pixel apron only when the pass is active; the blur radius is keyed to the
full working width.

Implementation deviation: the planned quarter-resolution separable Gaussian
chain was replaced by a bounded 9-tap multi-radius kernel (maximum radius 32).
It preserves the intended controls, avoids extra transient textures, and met
the quality/performance gates in this renderer. A GPU test renders the same
global pixel from different tile origins and agrees within one 8-bit LSB; the
CPU mirror guards step-edge endpoint overshoot.

Measured on 2026-07-22 in a dev build on the Apple Silicon test machine:

- X2D II 100C, 1521×2048 proxy: 9 ms local-contrast render.
- X1D II 50C, 1535×2048 proxy: 8 ms local-contrast render.

The neutral pass remains culled. Full 100 MP export performance was not used as
the interactive budget measurement; export remains tiled and bounded-memory.
