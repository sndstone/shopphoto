# Local masks and adjustment controls

Implemented photography-editor additions:

- Non-destructive soft-mask add, subtract and intersect expressions. Each operand retains its own original alpha or procedural mask, including coverage modifiers. Expressions are bounded to 32 levels and 1024 nodes; invalid expressions are disabled when loading.
- Invert coverage and Amount for every local adjustment, plus duplicate, move up/down and combine-with-mask commands. Combining copies coverage; subsequent changes to the source adjustment do not mutate the copy. Nested components retain editable range and geometry parameters.
- Relative local Temperature/Tint, whites/blacks, luma/RGB point and parametric curves, HSL, three-way grading, black-and-white mix, clarity/texture/dehaze, sharpening and edge-aware noise reduction.
- Extended controls render in the GPU retouch pass in scene-linear Rec.2020. Curves share the global curve LUT builders. Existing local tone/saturation formulas remain intact for old sidecars. Local temperature/tint are relative developed-image controls, not a replacement for sensor white balance. Local noise reduction is a bilateral filter, not neural inference.
- Old local-adjustment JSON receives neutral defaults for every added field. Existing alpha masks retain lossless storage and repair strokes. All mask overlays include final inversion and Amount.

Validation: core sidecar compatibility/depth tests; GPU soft-mask union/difference/intersection, inversion/amount, nested soft alpha and modifier tests; all eleven new control families tested against unselected pixels; existing shader precision and golden-render tests.

The combined-mask inspector edits copied operands; it does not presently select an individual operand for viewport brush repair.

## Bounded retouch source sampling

Full-resolution heal/clone now renders an independent pre-retouch scene-linear source patch for each correction intersecting the requested tile. Source distance never becomes a padding radius. The target scene stays immutable for sample selection; corrections accumulate in separate ping-pong textures, followed by local adjustments, sharpening and output conversion. Healing-ring samples outside the two patches are fetched through small independent regions. Work is submitted and completed per correction so queued resources cannot grow with the correction count. This adds GPU synchronization per active correction in exchange for bounded memory; a persistent source-patch cache can reduce repeated work later.

Procedural brush and heal/clone circles use pixel-space distance normalized by the image short edge. Cursor outlines follow the same convention and include viewport zoom. Spatial tile padding sums consecutive filter reaches, including masked detail, to prevent filter seams.

GPU regressions compare distant clone and heal corrections, including large healing rings, to an independent full-scene render. A 160×96 target remains at most 168×104 pixels with a source thousands of pixels away. Disabled and zero-opacity corrections are exact bypasses. Proxy/superpixel rendering retains its existing complete-source path.


## Mask authoring and coordinates

The coverage toggle (M) now renders every mask kind through the actual GPU mask evaluator. Luminance and color selections see the same scene-linear pixels and preceding local edits as the final effect. The dedicated coverage output reuses the existing compiled mask data, and the viewport uses the GPU texture directly without a synchronous CPU readback. Previewing coverage never changes the developed image.

New painted masks retain separate additive/eraser strokes and per-dab pressure. Flow accumulates continuously; spacing is measured against brush diameter in source-image pixels, with optional smoothing and pressure-scaled radius. Mouse input uses full pressure, and tablet/touch pressure is used when the egui platform backend supplies it. Undo stroke removes the latest stroke. Existing brush sidecars retain their original representation. Built-in local adjustment recipes and versioned `.splocal` save/load preserve the selected image region and its identity.

Masks, heal/clone targets and sources are authored in canonical pre-lens coordinates. Fitted preview and export now share the same crop, quarter-turn, straighten, flip, keystone and scale mapping. Fitted HDR previews apply this mapping in the HDR blit without falling back to SDR. The mask overlay, outlines and pointer authoring invert that map and the effective lens correction; image color pickers use the appropriate displayed-image coordinates. Zoomed inspection continues to show the original image coordinate frame, as the existing 1:1 workflow specifies, and its mask mapping includes zoom and lens correction.

New AI mask inference renders bypass only the final lens geometric warp, retaining the development/shading controls. This makes newly generated alpha masks stable when lens correction or geometry is changed later. Historical AI alpha sidecars do not record the lens transform used during generation, so they cannot be losslessly migrated automatically; they retain their saved interpretation and can be regenerated once if alignment is wrong.

Validation includes brush event-rate/spacing and sidecar-pressure tests; local preset identity/schema tests; GPU flow/erase/circular-pressure coverage and exact composite/color/luminance overlay tests; canonical AI input equivalence; core and viewport inverse-coordinate regressions across lens changes, crop, flip, rotation, scale, keystone and zoom; and export geometry/HDR shader regressions. SDR preview meshes approximate the continuous inverse warp with a 48×48 grid; final exports use per-pixel mapping.

## Edge refinement and semantic selection

**Refine visible edges** applies a joint bilateral filter guided by the developed
image. **Protect edges after each stroke** runs the same operation after paint or
alpha-repair gestures. Refinement bakes the current coverage to a lossless alpha
mask at preview resolution (maximum long edge 1536), so it is not a full-resolution
edge reconstruction. Inversion and Amount are folded into that coverage once.
The CPU filter runs off the UI thread, with only one job in flight. Results are
accepted only for the unchanged image, virtual copy and edit state; painting while
a job is running schedules the latest state again. Every accepted result is
undoable, and additive/subtractive painting can continue afterward.

Object selection now collects foreground and exclusion points, up to 32, with
numbered viewport markers and undo/clear controls. Automatic people/sky selection
uses the optional noncommercial MaskFormer model, not luminance or skin-color
thresholds. See [AI runtime](ai-runtime.md) for its license, installation and real
model acceptance. Facial-part selection and blink classification are not provided.
