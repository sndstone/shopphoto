# ShopPhoto 0.4.0 — personal photography build

The [approved photography checklist](photography-roadmap.md) is implemented within
its stated capture/model/platform limits. ShopPhoto remains a non-destructive
photo editor; document layers, arbitrary compositing, text/vector editing and
generative scene construction remain excluded.

## Added photography workflows

- Verified/resumable imports, templates, XMP interoperability, safe catalog and
  file operations, verified offline originals and movable project packaging.
- Named versions, stacks and grouping review, nested collections, hierarchical
  keywords/search, external editor recipes and monitored derivatives.
- Compound/local masks, brush flow/pressure/erase, asynchronous guided edge
  refinement, multi-point object selection and optional semantic people/sky masks.
- Precise clone/heal and texture/dust assistance, film inversion, red-eye/moiré,
  selective color/channel mixing/skin consistency, conservative blur restoration,
  persistent color samples and photographic scopes.
- Aligned/deghosted HDR, burst averaging, focus stack/repair, panorama projections,
  expert four-phase Bayer pixel shift, tracked RAW astro calibration and sequence
  exposure/WB matching with separate float photographic masters and provenance.
- Custom ICC/ProPhoto output, printer proofing, physical PDF/contact sheets,
  watermarks, size-target export, persistent queues, proof galleries/client picks,
  slideshows, saved workflows and headless GPU automation.
- English/Dutch interface, tool hints and dynamic status/error templates,
  configurable shortcuts/workspaces/accessibility, capability reporting and
  guided optional AI runtime with bounded model reuse.

## Checks

The final workspace/all-target run with native codecs passed **517 tests**, zero
failures and three explicit manual-fixture ignores. All three ignored checks were
also invoked explicitly and passed: mask sampling benchmark, actual CMYK printer
profile and the real optional-runtime Rust/Python mask/sidecar bridge.
Strict workspace/all-target Clippy, formatting and whitespace checks passed.

All eleven Python acceptance tests passed with actual object/matting/depth,
semantic people/sky, face/eye-detail and SwinIR SDR/PQ HDR inference.

Nine dialogs/inspectors rendered through offscreen WGPU in English and Dutch.
Translation checks cover control catalogs, built-in presets/inspector titles,
exact template placeholders, argument reordering, preservation of filenames and
independent UI-context languages. Actual render/export, queue recovery, PDF
parsing and offline-gallery interactions are detailed in
[photography acceptance](photography-acceptance.md).

## Package

`dist/ShopPhoto-0.4.0-personal.dmg` contains a universal arm64/x86_64 app,
an Applications shortcut,
photography guidance and the existing research notebook. It uses ad-hoc signing
for this local build. The checksum is provided alongside the final DMG.

The Apple Silicon slice includes six relocated codec libraries and reopens HEIC
and AVIF without Homebrew install paths. These installed libraries require
macOS 26.0 or later. The bundle declares the codec slice's measured minimum using
Apple's [per-architecture system-version key](https://developer.apple.com/documentation/bundleresources/information-property-list/lsminimumsystemversionbyarchitecture).
The Intel slice retains the standard raster codecs and macOS 12 baseline;
HEIC/AVIF input is visibly unavailable there. Architecture presence and compilation
do not establish physical Intel-machine acceptance.

Semantic people/sky inference uses a separately opted-in noncommercial model;
its weights are not in the DMG. The optional Python runtime remains separately
managed and checks its actual packages/device/model cache.

Real camera pixel-shift/astro/lens/tether capture, physical printer output,
Linux/Windows HDR and full assistive-technology acceptance require appropriate
equipment and fixtures. These remain capability limits rather than enabled
placeholders or claims of universal device support.

## Final artifact verification

- Artifact: `dist/ShopPhoto-0.4.0-personal.dmg`, 702,767,288 bytes (670.2 MiB).
- SHA-256: `dca82a8efc7484f8dbcba7727589232f6bb5b48e7dc75522942a9d2790704521`. Matching `.dmg.sha256` file is beside it.
- `hdiutil verify` passed; read-only mounting and deep/strict signature verification of both generated and mounted apps passed.
- Universal arm64/x86_64 slices, version 0.4.0 and six codec libraries verified.
- The mounted app rendered the unchanged JPEG original to 900×1197 16-bit PNG, and reopened/rendered actual HEIC (1280×854) and AVIF (800×533) sources. Dynamic-loader tracing confirms all six codec libraries loaded from the mounted app, with no Homebrew runtime paths.
- Intel execution under Rosetta is unavailable on this host (`Bad CPU type in executable`); the Intel slice was compiled and inspected, not physically executed.
- A launch through macOS Launch Services reached the mounted app and delivered the selected photo. No full-window capture was produced on this host; the scoped smoke-test process was stopped after the bounded attempt. Offscreen UI acceptance remains the visual evidence.
