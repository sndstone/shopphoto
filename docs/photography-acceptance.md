# Photography implementation and acceptance — 2026-09-30

ShopPhoto remains a non-destructive photo editor. The approved additions are
integrated into the native application and its normal develop/export pipeline;
no document layer compositor, text/vector editor or arbitrary design canvas was
added. The [roadmap](photography-roadmap.md) records individual implementation
status, and its phase links explain behavior and practical limits.

## Available workflows

- Library: reviewed/resumable verified imports, metadata templates, XMP
  interoperability, hierarchical search/collections, stacks, named versions,
  safe file operations, catalog backup/restore/relink, verified offline originals
  and portable projects.
- Develop: compound/local adjustment masks, brush flow/pressure/erase and
  background edge refinement, foreground/exclusion selection prompts, optional
  semantic people/sky masks, precise heal/clone and texture/dust assistance,
  selective color/channel mixing/skin consistency, film inversion, red-eye/moiré,
  conservative blur restoration, persistent color samples and scopes.
- Photo merge: aligned/deghosted HDR, burst averaging, focus stack/repair,
  projected panoramas, expert four-shot pixel shift, tracked RAW astro calibration
  and sequence exposure/WB matching. All create separate photographic derivatives
  with provenance rather than altering their sources.
- Delivery: custom RGB/ProPhoto ICC output, CMYK printer proofing, text attribution
  watermarks, file-size targets/lossy WebP, immutable persistent recipe queues,
  physical-size PDF/contact sheets, offline proof galleries/client picks and
  slideshows, external-editor recipes and monitored derivatives.
- Automation/interface: saved workflows, headless GPU rendering, queue CLI,
  configurable shortcuts/workspaces/accessibility controls, optional AI installer
  and readiness, bounded model reuse, capability reporting and Dutch UI controls.

## Checks performed

| Check | Result |
| --- | --- |
| `cargo test --workspace --all-targets --features sp-app/native-codecs` | 517 passed, 0 failed, 3 explicitly ignored |
| Strict workspace/all-target Clippy, including `sp-app/native-codecs` | Passed |
| Rust formatting and whitespace checks | Passed |
| Native application build with `native-codecs` | Passed; local binary `target/debug/shopphoto` |
| Real CPU/GPU editing and export regressions | Passed, including mask coverage, canonical geometry, bounded donor patches, film/color/restoration parity and retained HDR |
| Optional native codecs | Four targeted tests passed: AVIF/HEIF decode, PQ HDR, independent metadata/container decode |
| Actual optional Python model/runtime tests | Eleven passed: persistent worker/cache, prompts/matting/depth, semantic people/sky, face/eye detail, SwinIR SDR/PQ HDR |
| Explicit real Rust-to-Python acceptance | Passed: persistent inference, imported 16-bit alpha and sidecar roundtrip, face analysis |
| Bundled codec portability | Mounted DMG app reopened/rendered HEIC and AVIF; loader tracing confirmed all six libraries came from the app |
| Actual CMYK printer profile | Explicit fixture test passed with macOS Generic CMYK Profile |
| Offscreen native UI | Nine dialogs/inspectors rendered through WGPU in English and Dutch; representative layouts inspected |
| Headless application rendering | Actual edited 900×1197 16-bit PNG; source bytes unchanged; overwrite refused |
| Workflow/queue CLI | Target-size lossy WebP completed; failed source persisted and returned nonzero; finished tasks retained |
| Native application delivery | Edited-photo PDF and offline proof gallery produced; independent PDF viewer and browser interactions exercised |

The three default ignores are the manual mask performance benchmark, external
CMYK profile test and fixture/model-dependent runtime acceptance. All three
were run explicitly and passed. Tests whose camera fixtures are absent do not
establish support for those cameras. Synthetic RAW/multi-frame fixtures establish
algorithm and refusal behavior, not real-camera acceptance.

English/Dutch offscreen artifacts and local logs are under
`/tmp/shopphoto-photography-check`; these are development evidence, not packaged
assets. Full native-window capture could not be validated because this host's
compositor reports an occluded surface. Browser snapshots were unavailable, while
gallery image loading, picks/ratings persistence, slideshow and downloaded picks
JSON were validated through browser interactions.

## Remaining work and explicit limits

- English/Dutch application controls, dialogs, search, accessibility names, tool
  hints, progress, status and error context are connected. Translated indexed
  templates preserve exact filenames and user data; tests validate placeholder
  agreement and argument reordering. Third-party technical diagnostics, model
  names, metadata values and user names retain their original text.
- Automatic people/sky masks use an optional noncommercial MaskFormer checkpoint,
  with a separate installer opt-in and license links. The checkpoint is downloaded separately.
  Facial-part semantic masks and blink/closed-eye classification are not supplied;
  face/eye-detail scores are advisory detections.
- Edge refinement is capped at a 1536-pixel preview; face detail uses a
  1024-pixel preview. Dust proposals and culling scores require inspection.
  Blur restoration only treats small Gaussian-like softness.
- Pixel shift requires four known integer Bayer phase offsets and an explicit
  static-scene confirmation. It has synthetic acceptance, no real-camera presets,
  spatial super-resolution or arbitrary sixteen-shot decoding.
- Panorama supports ordered adjacent frames with small displacement and explicit
  projection/FOV, not general multi-row projective reconstruction or parallax
  correction. Focus repair previews are SDR. Astro assumes tracked RAW exposures
  and supplied compatible calibration; real astro/lens capture sets remain untested.
- Direct tethering requires an installed gphoto2 backend and a detected capture
  capability. This host has neither an installed backend nor an attached camera.
  Process/atomic-download tests pass; physical capture remains untested. Watched
  folders support other camera-control applications.
- Linux Wayland/Vulkan HDR is opt-in and experimental; surface capability never
  invents display headroom. Other OS/HDR hardware and full screen-reader behavior
  require their own acceptance. The personal package includes native-codec libraries for Apple Silicon;
  the Intel slice retains standard raster formats and reports AVIF/HEIF input as
  unavailable. Other host builds can package codecs for their host architecture.
- PDFs use ICC-tagged 8-bit JPEG photos, actual-size layouts and WinAnsi Latin
  captions. No physical printer was measured. Custom SDR output is RGB; CMYK is
  proofing. Font shaping/full Unicode captions are not implemented.
- Offline editing pins verified full originals and sidecars rather than lossy
  smart proxies. A queued job still needs its originals, profiles and models.
  Gallery receipts use source paths; moved photos need a reissued gallery.

The 0.4.0 DMG is an ad-hoc-signed universal macOS package with native codec
libraries. The final artifact, checksum
and package checks are recorded in [release-0.4.0.md](release-0.4.0.md).
It is not a notarized public distribution or a guarantee of every camera and
physical-device workflow.
