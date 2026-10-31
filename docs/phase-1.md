# Phase 1 — RAW Decoding Core: completion note

Code completed 2026-07-20, same session as Phase 0. **Verification against
real Hasselblad sample files is still pending** (no samples available in the
build environment); the harness exists and must be run before Phase 2 starts:

```sh
SP_TEST_RAWS=/path/to/hasselblad/samples \
  cargo test -p sp-raw -- --nocapture
```

## What was built

- `sp-raw::decode()` implemented over rawler 0.7.2 (`convert.rs` owns the
  entire rawler↔sp-core mapping; rawler types do not escape the crate).
- `sp-core::color::mat3_inverse` added (tested) — rawler supplies XYZ→cam,
  we store cam→XYZ.
- Decode worker thread + ⌘O/Open button/drag-drop + preview display +
  metadata panel + error toasts in `sp-app` (see PROGRESS.md for detail).

## Deviations from BUILD.md Phase 1

1. Preview is returned as decoded RGBA8, not JPEG bytes — rawler already
   decodes the embedded image to `DynamicImage`, so re-encoding would be
   wasted work; `sp-app` uploads it directly as an egui texture.
2. Decode "pool" is a single worker thread for now; a real pool is deferred
   to Phase 7 (library thumbnails) where parallelism first matters.
3. Filmstrip does not yet list opened files (folded into Phase 2 viewport
   work).
4. rawler 0.7 pulls a JPEG XL decoder (jxl-oxide) — kept, since recent
   Hasselblad X2D firmware writes JXL-compressed raw data.

## Acceptance status

- [x] Decode errors surface as toasts, never panics (missing-file and
      bad-extension paths unit-tested).
- [x] Async decode; UI never blocks; stale-result guard.
- [x] Harness green on real files: X2D II 100C `.3fr` (105.3 MP) and
      X1D II 50C Phocus `.fff` (51.4 MP), both with previews and metadata.
      Required vendoring rawler with three small patches — see PROGRESS.md
      "VERIFIED on real files" for the list and rationale.
- [x] Both sample decodes complete well inside budget (harness wall time
      0.4 s for both files in a dev build).
- [ ] A Hasselblad-exported DNG sample has not been tested yet (no sample
      on hand); the DNG path is rawler's standard decoder, low risk.
