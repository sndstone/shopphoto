# ShopPhoto 0.4.5 — edit safety and image correctness

This release packages the [2026-10-15 reliability review](phase-review-2026-10-15.md)
and retains the [0.4.4 live preview and RAW hardware settings](release-0.4.4.md).

## Changes

- Switching photos no longer risks saving the outgoing edit under the incoming
  photo. Failed local saves keep the current document open for retry, including
  normal window-close, version-switch and file-move operations.
- Auto Finish sends new local edits back to the network original in the correct
  direction. Concurrent sidecar saves use independent atomic staging files;
  settings-sync batches preflight their destinations and undo correctly.
- Truncated working-cache originals are repaired, cache-manifest errors are
  reported, and non-UTF8 source filenames retain distinct cache identities.
- Rec.2020 exports embed matching transfer curves. Ultra HDR gain maps use the
  actual SDR base color space, including Display P3 and Adobe RGB.
- RAW metadata/preview panics fall back without discarding valid sensor data.
  Sensor/crop geometry, matrix math, raster/sequence input buffers and GPU
  transfers are checked before use. AI blending rejects incompatible layouts.
- The persistent optional enhancement worker recovers from malformed protocol
  requests and invalid response paths; face-image dimensions are checked before
  pixel conversion. Closed/failed photos cannot display stale GPU textures.

## Validation

The final all-feature workspace passed 582 tests, zero failures and six explicit
ignores. Strict workspace/all-target/all-feature Clippy, Rust formatting,
whitespace checks, packaging shell syntax and plist validation passed. The
preceding review also passed 21 actual optional Python tests (two portrait
fixture skips) and all three explicitly invoked Rust/Python worker tests.

Final mounted package checks are recorded below.

## Package

The DMG contains the universal arm64/x86_64 application.

The package uses ad-hoc signing and is not notarized unless signing credentials
are explicitly supplied. Native HEIC/AVIF input is included in the Apple Silicon
slice; its minimum OS is declared by the packaged codec libraries. The Intel
slice retains standard codecs and macOS 12.0 compatibility. Compilation does
not establish physical Intel or cross-platform GUI acceptance.

Install by dragging ShopPhoto.app to Applications. Building the DMG does not
replace the installed application or previous DMGs. The adjacent checksum and
PROGRESS.md record final package verification. Physical HDR, printer, camera,
network-disconnect and portrait-fixture acceptance remain separate checks.

## Final artifact verification

- Artifact: `dist/ShopPhoto-0.4.5-personal.dmg`, 703,864,515 bytes (671.3 MiB).
- SHA-256: `118b4857d0ef596973d1d43e6a296918312fb082fc40cba2374b6d5ca89005a0`.
  The adjacent basename-based checksum file was independently checked.
- DMG integrity, mounted version, both universal slices and deep strict app
  signature passed. All six relocated codec libraries loaded from the bundle.
- Mounted PNG/JPEG/HEIC/AVIF rendering, resized 16-bit PNG, explicit sidecar
  save, original-overwrite refusal, JSONL edits/export/undo and MCP PNG preview
  passed. Disposable original image hashes were preserved.
- An actual Rec.2020 export's embedded profile matched independent LittleCMS
  neutral-swatch conversion. Display P3 Ultra HDR export carried gain metadata
  and retained its readable SDR base.
- Mounted GUI startup, edited controls at 100% zoom and full-window SDR capture
  passed; screenshot reviewed. QA edits did not create a source sidecar.
- The mounted HDR diagnostic passed with Rgba16Float and extended linear sRGB;
  the panel reported ×16 potential headroom. This is diagnostic evidence,
  distinct from by-eye HDR acceptance.
- The QA mount was detached. The installed app remains 0.4.4, earlier DMGs
  remain in place, and this source/packaging work is uncommitted.

The DMG's enclosed release notes predate final mounted-artifact checks; this
repository record and the adjacent checksum provide final verification.
