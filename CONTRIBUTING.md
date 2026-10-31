# Contributing / Code Standards

These rules are binding for every phase (BUILD.md §4 is the source; this is the
working summary). CI enforces the mechanical ones.

## Mechanical (CI-enforced)
- `cargo fmt --all --check` clean.
- `cargo clippy --all-targets --workspace -- -D warnings` clean.
- `cargo test --workspace` green on macOS, Windows, Linux.

## Error handling
- Library crates (`sp-*` except `sp-app`): all fallible APIs return
  `Result<T, sp_core::SpError>`. No `unwrap()`/`expect()` except provably
  infallible cases, each with a comment. Malformed input files must produce
  errors, never panics.
- `sp-app`: converts to `anyhow` at the binary boundary; `expect` allowed
  during startup only.

## Types & naming
- Units in names: `_ev`, `_kelvin`, `_px`, `_norm`, `_s`, `_mm`.
- Color values crossing crate boundaries carry their space in the type
  (`sp_core::color`). Never bare `[f32; 3]` across a public API.
- Serialized structs carry a `version: u32`; `Default` = neutral edit.
  Never break sidecar compatibility — version and migrate.

## Architecture
- Crate dependencies point strictly downward
  (`sp-app → sp-pipeline/sp-catalog/sp-export → sp-gpu/sp-raw → sp-core`).
- `egui` types never leave `sp-app`. `wgpu` interaction lives in `sp-gpu`
  (surface/present wiring in `sp-app` is the one exception).
- UI colors/spacing/fonts come from `sp-app/src/theme.rs` tokens only.
- Platform-specific native code (`objc`, DXGI) is banned unless wgpu provably
  lacks the capability; then `#[cfg]`-gated in `sp-gpu` with a tracking issue.

## Shaders (Phase 2+)
- WGSL, one entry point per file, snake_case, constants at the top.
- Header comment: input/output formats and color space.
- 16×16 workgroups (`sp_gpu::WORKGROUP_DIM`) unless measured otherwise.
- Every shader compiles in CI on a headless adapter.

## Tests
- Color math: reference-vector and round-trip unit tests.
- Pipeline: golden-image tests, ΔE tolerance < 1.0.
- RAW samples: env var `SP_TEST_RAWS` points at a local folder of Hasselblad
  test files (never committed). Tests that need it skip when unset.
- Sony samples: `SP_TEST_SONY_RAWS` points at a local folder of ARW files;
  the Sony verification harness likewise skips when unset.

## Process
- Each phase ends with `docs/phase-N.md` (what was built, deviations from
  BUILD.md) and an update to PROGRESS.md.
- Commit messages: imperative mood, body explains why when non-obvious.
