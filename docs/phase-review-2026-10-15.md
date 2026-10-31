# Reliability and image correctness review — 2026-10-15

The user requested an extensive pass with `gpt-6.1-sol` subagents. Three
subagents reviewed catalog persistence, image processing/export, and application
workflows. The parent reviewed GPU transfers, AI blending and the Python worker;
a further independent review checked fixes across those areas. All work used
the existing working tree, including its untracked modules. Existing edits were
preserved; no commits, pushes, releases or packages were published.

## Changes

### Application and persistence

- Image switching retires outgoing edit/undo/metadata/version state before the
  new decode. Editing, saves, export preparation, delivery and Auto Finish are
  guarded while the incoming source is loading. Pending renderer uploads from
  the old source are invalidated.
- Forced saves verify local persistence success rather than only waiting for
  queue processing. Image/version switches, file moves/trash and normal close
  requests retain the current document when the latest local write failed.
  Successful retries clear the failure; optional XMP or remote-mirror failures
  do not block a transition once the local sidecar is safe.
- Auto Finish now mirrors the newly written local sidecar to the network
  original. Reversed arguments previously copied the old remote edit back over
  the new local result.
- Sidecar, preset, metadata-template, network mirror and cache-manifest writes
  use uniquely created sibling staging files, flush before atomic publication,
  and clean staging files after failures. Concurrent writers no longer share
  and truncate the same temporary path.
- Settings sync preloads every destination and deduplicates normalized paths.
  A corrupt later sidecar cannot leave earlier edits applied without an undo
  batch; repeated targets no longer undo to an intermediate state.
- Source-cache reuse checks cached file length and manifest version. Truncated
  online entries are repaired while local edits are retained. Exact path bytes
  distinguish non-UTF8 filenames, and manifest publication failures propagate.

### Image correctness and malformed inputs

- SDR Rec.2020 export ICC curves match the renderer's sRGB transfer function.
  Previously the profile declared the stock BT.2020/Rec.709 transfer function,
  causing incorrect interpretation in color-managed applications.
- Ultra HDR gain maps use the SDR base's actual primaries and transfer function
  for sRGB, Display P3, Rec.2020 and Adobe RGB. Malformed/empty buffers are
  rejected before pixel access.
- Registration, panorama projection and sequence matching reject incomplete,
  nonfinite and nonopaque raster sources, including planar projection and
  sequence matching without registration. Sequence thumbnails also validate
  RAW buffer bounds and finite exposure/white-balance parameters.
- Optional RAW metadata and embedded-preview decoding each have panic
  protection; a bad preview can fall back to another embedded image without
  discarding successfully decoded sensor data. RAW conversion validates sensor
  dimensions, buffer length and active-area bounds before rotation. Matrix
  inversion rejects nonfinite inputs or arithmetic results.
- GPU staging uploads validate format, dimensions, texture usage, texel stride,
  checked row alignment and device buffer limits before filling rows or
  submitting copies. Readback validates texture usage/dimensionality as well.
  Invalid layouts return errors instead of triggering wgpu validation panics.
- AI denoise blending preserves the source for NaN strength or an incompatible
  sensor layout, preventing black output or mixing differently arranged CFA
  samples.
- The persistent Python enhancement worker survives malformed request envelopes
  and invalid/unwritable response paths. Face analysis checks dimensions before
  image conversion and shares the validated image loader.

## Validation

The initial workspace baseline passed 550 tests with six ignored. The first
post-fix pass passed 568 tests with six ignored. The final all-feature run,
including native codecs, passed **582 tests with zero failures and six ignored**.
Validation commands:

```sh
cargo fmt --all --check
cargo clippy --all-targets --workspace --all-features -- -D warnings
cargo test --workspace --all-features
git diff --check
```

Using the existing enhancement virtual environment and cached weights with
`HF_HUB_OFFLINE=1`, Python passed 21 tests, with two portrait-fixture skips.
This included actual SlimSAM, depth, CLIPSeg, semantic and SwinIR cases.
The three explicitly ignored Rust `real_interactive_` enhancement-worker tests
also passed with `SHOPPHOTO_ENHANCE_PYTHON` set. These verify actual process
reuse, cancellation, idle release, input transport parity and invalidation.

Regression coverage includes real persistence workers, concurrent writers,
malformed buffers, GPU transfers, independent LittleCMS luminance reference
values, and HDR reconstruction. RAW panic recovery is exercised through an
injected decoder using the production decoding path; new proprietary malformed
camera binaries were not added. Cargo reports an existing future-compatibility
notice for `proc-macro-error2`; strict Clippy passes.

## Scope and remaining acceptance

No architecture or sidecar-schema changes were required. Plain catalog free-text
search retains its documented token-prefix behavior after checking the existing
tests and review notes.

Online cache reuse checks length and source stamp rather than rehashing a large
RAW on every open; equal-length corruption is still detected by offline hash
verification. This is not a completed fuzzing audit. Physical HDR display,
printer, tether, network-disconnect and cross-platform GUI acceptance remain
separate from unit/integration coverage. The review initially updated source only. The follow-up
[0.4.5 personal DMG](release-0.4.5.md) packages the fixes and records mounted
acceptance. Earlier DMGs and the installed application are preserved.
