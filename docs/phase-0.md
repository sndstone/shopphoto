# Phase 0 — Scaffolding & Standards: completion note

Completed 2026-07-20. Acceptance criteria from BUILD.md Phase 0: met.

- `cargo run -p sp-app` opens a themed 1440×900 window with the full
  three-pane layout (top bar, left rail 240px, right develop panel 320px,
  filmstrip 96px, viewport center) on macOS. Windows/Linux compile is covered
  by CI config (not yet executed — repo not pushed).
- `cargo fmt --check`, `clippy --all-targets -D warnings`, and
  `cargo test --workspace` (27 tests) all pass locally.
- CONTRIBUTING.md carries the BUILD.md §4 standards; justfile provides
  `run/test/lint/fmt/bench`.

## Deviations / additions vs BUILD.md

1. **Went beyond "empty modules"**: `sp-core` (EditState/color/raw/profile),
   sidecar IO in `sp-catalog`, pass planning in `sp-pipeline`, and export
   sizing in `sp-export` are fully implemented and tested. Rationale: the
   user asked for a rock-solid core for other models to build on; these are
   the contracts (sidecar schema, pass order, parameter ranges) that all
   later phases consume, and they are pure-CPU testable now.
2. **Develop panel sliders are live** (bound to `EditState`) instead of
   placeholder panels — exercises the design tokens and the slider spec
   (double-click reset) from day one. No rendering yet, as specced.
3. egui 0.35's API differs substantially from pre-0.31 material; see the
   API note in PROGRESS.md. `eframe` is built with the wgpu backend only.
4. Edition 2021 kept (BUILD.md said 2021+); revisit 2024 when convenient.

## Not done (deliberately)

- No git commit yet (pending user approval).
- CI has never run (repo has no remote). First push should be watched.
- No app icon / bundle metadata (Phase 9).
