# Phase 4 — Noise Control Suite: completion note

Completed 2026-07-20 (session 8). Maintained inventory in PROGRESS.md
("Phase 4" section); this file records acceptance status and deviations.

## Acceptance status (BUILD.md Phase 4)

- [x] Hot-pixel removal in raw prepare, toggleable, neighbor-ratio clamp
      (fixed mild always-on variant in the once-at-open superpixel path).
- [x] Chroma NR: edge-aware bilateral on Oklab a/b — no luma detail loss by
      construction (L axis untouched by the chroma branch).
- [x] Luminance NR: NLM-lite with detail blend-back.
- [x] Runs after demosaic + color transform, before tone, in scene-linear.
- [x] ISO-seeded defaults (documented table, only for sidecar-less files).
- [x] 1:1 noise loupe in the Detail panel (hidden when already at 1:1).
- [x] Synthetic-noise test: >45% MSE reduction required, 65% measured;
      NR-off bit-identical; hot-pixel toggle verified both ways.
- [x] Perf: full-chain GPU render of the 105 MP file is 51 ms (dev build)
      with NR compiled in; NR itself adds ~10 ms at proxy resolution on
      Apple Silicon — far inside the 150 ms budget.

## Deviations

1. No separate pre-demosaic NR pass (`nr_raw.wgsl` reserved): the proxy
   path is already 2×2-averaged and the Malvar tile path benefits more
   from the post-demosaic Oklab treatment. Revisit only if Phase 5
   full-res exports show demosaic-amplified chroma speckle.
2. NLM search window is 5×5 with 3×3 patches (not 9×9/5×5): measured
   quality is good and the cost fits interactive tile renders; Phase 8 can
   widen it behind a "quality" toggle with workgroup-shared tiles.
3. The loupe shares tile GPU resources instead of owning a second set —
   cheaper, with explicit cross-invalidation of the two cache keys.
