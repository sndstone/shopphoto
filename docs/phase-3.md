# Phase 3 — Develop Controls & Non-Destructive Editing: completion note

Core completed 2026-07-20 (session 4). See PROGRESS.md "Phase 3" section for
the full inventory (it is the maintained copy; this file records acceptance
and deviations).

## Acceptance status (BUILD.md Phase 3)

- [x] Basic panel fully live: WB temp/tint (CCT model), exposure, contrast,
      highlights, shadows, whites, blacks — all through the tone LUT.
- [x] Tone curve: interactive editor, applied in encoded domain via the LUT.
- [x] HSL 8-band + vibrance + saturation (Oklab LCh, GPU).
- [x] Live RGB histogram (GPU reduction, per-render readback).
- [x] Edits survive restart via sidecars; corrupt sidecars quarantined.
- [x] Undo/redo with drag coalescing (tested); 200-step cap.
- [x] Before/after (`\`) and Reset-all.
- [x] ≥30 fps proxy interaction: full chain is ~60 ms for 105 MP open;
      per-edit renders are proxy-res only (2048px, ~4 ms) — headroom is
      large; LUT rebuild is CPU-side ~1 ms.
- [ ] Crop/rotate viewport tool — NOT built (panel sliders exist but are
      not wired to the pipeline). Folded into the remainder list.
- [ ] 1-D LUT "one texture lookup" is actually a storage-buffer lookup —
      equivalent cost, simpler binding.

## Deviations

1. Tone-region masks and whites/blacks strengths are pragmatic
   approximations (documented in tone.rs) — tuned for smoothness and
   monotonicity, not Lightroom equivalence. Revisit only with real-image
   A/B feedback.
2. HSL runs in Oklab over BT.709; extreme wide-gamut colors take a small
   gamut clip through the detour. Acceptable at proxy/SDR stage; Phase 3B
   (profiles) is the right place to refine.
3. Histogram readback is blocking (~1 ms, only on edit change). Make async
   in Phase 8 only if profiling demands.

## Remainder completion (session 5)

All Phase 3 remainder items landed: 1:1 Malvar tile rendering with pan/zoom
(golden-tested incl. odd-origin CFA parity), IGN output dither, session
filmstrip with thumbnails, crop overlay tool + flip/straighten display
wiring. 52 tests green; 105 MP real-file: decode 75 ms + 56 ms GPU.
Geometry is display-only until Phase 5's export resampler.
