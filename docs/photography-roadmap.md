# Photography workflow expansion

Approved scope: 2026-09-29. ShopPhoto remains a non-destructive photo editor.
The user approved the photography recommendations in the feature audit and
excluded Photoshop-style document editing. This supersedes the original v1
non-goals in BUILD.md for photography workflows, without changing its color,
memory or sidecar compatibility requirements.

No pixel-layer compositor, text/vector editor, Smart Objects, arbitrary
multi-image design canvas, or generative scene construction is planned.
Local adjustment masks and photographic merging remain in scope.

Checked items have working application integration and relevant local checks
within the documented limits; they do not establish every camera/platform or
physical-device workflow. Unchecked items remain unfinished. Hardware, model,
license and fixture dependencies are recorded explicitly.

## Reliability and interoperability

- [x] Preserve foreign XMP fields, read descriptive metadata, detect write conflicts.
- [x] Carry virtual-copy sidecars and catalog relationships through rename/move.
- [x] Verify imported originals and backup copies; resumable import.
- [x] Export captions, keywords, creator, copyright, rights and chosen GPS metadata.
- [x] Catalog backup, restore, integrity checking and missing-file relinking.
- [x] Editable JPEG/TIFF/PNG with ICC/bit-depth handling; additional raster codecs.
- [x] Reopen/edit external-editor derivatives and retain their source relationship.
- [x] Pinned offline copies/previews and reconnect behavior.

## Editing and masks

- [x] User-selected heal/clone source; individual correction selection/edit/delete.
- [x] Texture-aware healing/patch removal; dust inspection and correction workflow.
- [x] Local WB, curves/HSL/grading, contrast/detail/NR and overall strength.
- [x] Add/subtract/intersect masks; invert, duplicate, reorder and mask presets.
- [x] Stronger selection prompts/refinement, edge-aware brushing and semantic masks.
- [x] Preserve masks/retouch coordinates across lens/geometry edits.
- [x] Brush opacity/flow/spacing/smoothing/pressure and presets where supported.
- [x] Red-eye and moiré/false-color correction.
- [x] Precision selective color, skin-tone consistency and channel mixing.
- [x] Persistent color samples, numerical readouts and photographic scopes.
- [x] Conservative detail restoration and deblur with inspectable results.
- [x] Film-negative conversion.

## AI and image quality

- [x] Guided enhancement installation and model/device status.
- [x] Reuse enhancement models across batches with bounded lifetime/memory.
- [x] Lens-profile import/management and user calibration workflow; additional proprietary or measured lens datasets remain data-dependent.
- [x] Document and validate actual hardware/camera coverage and SR limits.

## Multi-photo photography

- [x] Exposure-bracket HDR merge with alignment and deghosting.
- [x] Panorama stitching with alignment/projection/boundary controls.
- [x] Focus stacking with alignment and repair workflow.
- [x] Burst averaging/noise reduction and motion rejection.
- [x] Expert four-shot Bayer pixel-shift color reconstruction with explicit offsets; synthetic validation only, no camera presets or 16-shot support.
- [x] Consistent exposure/WB matching across a sequence.
- [x] Tethered capture integration with explicit supported-camera/backend detection.
- [x] Astro calibration/stacking where supplied inputs support a valid workflow.

## Color, output and delivery

- [x] Printer/paper proof profiles, intent, black-point and paper/ink simulation.
- [x] ProPhoto RGB and custom output ICC profiles.
- [x] Physical print sizes, page layouts, contact sheets and printing.
- [x] Multiple export recipes, persistent queue, retry/pause and size targets.
- [x] Text watermarks and lossy WebP.
- [x] Proof galleries/client picks, slideshows and portable project packaging.

## Library and automation

- [x] Named/manageable virtual copies with Library visibility and version export.
- [x] Burst/bracket/focus stacks, RAW+JPEG pairs and derivative grouping.
- [x] Advisory detail, face/eye-region detail and perceptual similarity; float HDR comparison. No blink/closed-eye classification.
- [x] Hierarchical keywords, nested collections and richer image search.
- [x] Import preview/subsets, date folders and metadata templates.
- [x] External-editor recipes and ongoing derivative monitoring.
- [x] Reusable workflows/actions and command-line rendering; useful plugin hooks.
- [x] Configurable shortcuts, saved workspaces and accessibility improvements.
- [x] English/Dutch application controls, dialogs, inspector search, accessibility names, tool hints, progress, status and error context. Indexed templates preserve filenames/user data and support reordered arguments; original third-party technical diagnostics remain verbatim. Feature documentation and personal-package guidance are updated.
- [x] Linux HDR where supported, platform checks and large-image/mask budgets.

## Verification

For each completed group, record the implementation and acceptance in a phase
note. Required checks: formatting, strict workspace Clippy, workspace tests,
targeted CPU/GPU integration and application smoke checks. Tests requiring real
camera originals, model weights, printers, tethered cameras or other operating
systems must be recorded as such; skipped tests do not establish support.

The local 0.4.0 implementation and acceptance checkpoint is recorded in
[photography-acceptance.md](photography-acceptance.md). Read the limits there
before treating a checked item as universal camera or platform support.

Phase notes:

- [Catalog reliability](phase-catalog-reliability.md), [Library workflows](phase-library-workflows.md), [external recipes and culling](phase-external-culling.md).
- [Raster inputs](raster-input.md), [local masks](phase-local-masks.md), [corrective editing](phase-corrective-editing.md), [AI runtime](ai-runtime.md), [lens calibration](lens-calibration.md).
- [HDR/burst/focus/panorama/pixel shift](photo-merges.md), [astro stacking](astro-stacking.md), [sequence matching](sequence-matching.md), [tether capture](tether-capture.md).
- [Output and printing](phase-photo-output.md), [automation](phase-photo-automation.md), [platform capabilities](platform-photo-capabilities.md).

Important bounds: automatic people/sky masks use separately opted-in
noncommercial model weights; edge refinement and face analysis are preview-based;
panorama is ordered adjacent-frame stitching; astro assumes tracked scenes;
pixel shift requires a known four-phase descriptor. The local machine has no
gphoto2 camera backend. Linux/Windows HDR, physical printer output, camera-specific
pixel shift remain external acceptance gates.
Pinned offline editing uses verified originals, not lossy smart proxies.
