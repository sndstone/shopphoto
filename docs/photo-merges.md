# HDR, burst and focus merging

Select 2–16 original photographs in the Library and open **Photo merge**.
Choose HDR, Burst average or Focus stack, then select the reference frame.
The reference fixes composition, RAW white balance and the preferred position
of moving objects. The job runs in the background, shows progress, supports
cancellation and opens the finished photograph for normal non-destructive edits.

Sources must have equal upright dimensions, the same camera identity and the
same RAW/raster source type. The current bound is 150 MP per frame and 320 MP
across the stack. A new name is chosen beside the reference; existing files
are never overwritten. Source edits remain separate: merging uses neutral
originals, with RAW camera matrix/WB but no camera base curve, creative look,
highlight reconstruction, sharpening or neural denoise.

Registration uses a luminance pyramid and exposure-insensitive normalized
correlation, followed by subpixel translation, rotation and scale refinement.
It handles small handheld displacement and modest focus breathing. The supported
search is approximately ±4.5° rotation and ±6% scale. Low-confidence matches
fail visibly. A locked tripod can bypass alignment. The output is cropped to
a conservative rectangle inside every transformed frame, with no invented fill.

- **HDR:** supply exposure EV values in the displayed filename order, or use
  EXIF shutter, ISO and aperture. There must be at least 0.25 EV of separation.
  The merge normalizes radiance to the reference exposure and weights usable
  samples away from clipping and the noise floor. Motion rejection compares
  normalized color with the reference wherever its exposure is usable. RAW or
  linear TIFF brackets give the strongest radiometric basis; already rendered
  JPEG tone curves reduce accuracy.
- **Burst average:** exposures must match when capture EXIF is available.
  Registered scene-linear samples are averaged; samples differing from the
  reference beyond the chosen motion tolerance are excluded. This preserves
  the reference placement of moving objects while reducing static noise.
- **Focus stack:** fixed-exposure, static scenes use a multiscale Laplacian
  focus score, neighborhood stabilization and soft contrast weighting.
  Occlusion boundaries, transparent objects and very fine overlapping edges
  can require manual retouching afterward.

The master is an opaque RGB32F TIFF with a linear Rec.2020 ICC profile. Negative
working RGB and values above diffuse white survive save/reopen. TIFF writing is
bounded by strips and committed atomically. The full source list, reference,
exposures, transforms, common crop, rejected-sample count and algorithm version
are embedded in TIFF ImageDescription and written to a `.spmerge.json` recipe.
A normal edit sidecar preserves reference descriptive metadata and enables HDR
viewing for an HDR merge. The catalog records all parents and provides source
navigation; moving a source preserves the relationship through stable photo IDs.

Validation includes recovered subpixel translation and rotation/scale from
synthetic scenes, unclipped HDR highlight recovery, burst noise variance and
moving-object rejection, focus selection from separate sharp regions,
cancellation, real GPU neutral RAW/raster linear readback, and signed HDR
float-TIFF roundtrips. The GUI still needs broad camera/bracket acceptance
testing. Sensor pixel-shift is not inferred from an arbitrary photo selection.

## Panorama

Photo merge also offers planar, cylindrical and spherical projection. Enter
the source lens's horizontal field of view for curved projections. Automatic
overlap matching finds normalized local features, requires a consistent
displacement consensus, then verifies and refines the overlap at full source
resolution to subpixel precision. Ordered adjacent frames from a level camera
are supported; strong roll, parallax or multi-row projective reconstruction
can fail explicitly. Planar mode is useful for approximately parallel views.

Manual overlap is an explicit left-to-right alternative. Overlap exposure
matching uses robust corresponding luminance ratios. Linear RGB feather
blending joins the sources. Crop chooses the largest opaque rectangle; Keep
canvas fills empty boundaries with black. Source placement, projection, FOV,
inlier counts, gains and crop are written into the same provenance recipe.
The canvas is bounded at 150 MP. Automated tests cover a 50% overlap,
exposure normalization, cylindrical/spherical edges, crop and cancellation.

## Focus repair

Focus stacking offers a registered preview capped at 1024 pixels. Select a source
and paint up to 128 radial feathered repair regions; the final full-resolution
merge applies those choices and saves/reloads them in `.spmerge.json`. This preview
is SDR. A numerical floor gives equal weights in flat regions rather than letting
roundoff become an arbitrary winner after contrast weighting. Tests cover repair
centers, feather transitions and unchanged flat-region averaging.

## Expert four-shot pixel shift

**Pixel shift** accepts exactly four RAW Bayer frames with explicit integer sensor
offsets in filename order, bounded to ±2 pixels. The first frame is the phase and
composition anchor. The UI requires confirmation of a static scene, same camera,
exposure and lighting before enabling Create. Offsets are an expert input; there
are no guessed camera presets. The engine reconstructs color from the four sensor
phases before demosaicing, applies source calibration, checks green agreement,
rejects incompatible metadata and saves the descriptor/crop/rejection provenance
with the linear float master.

This is color reconstruction, not spatial super-resolution or arbitrary 16-shot
support. Independently calibrated second-green channels are explicitly refused
because the current RAW representation cannot retain that calibration separately.
Synthetic high-frequency color, unequal RGB calibration, orientation, motion,
invalid-offset and cancellation tests pass. No real camera pixel-shift sequence
has been validated; use only known four-phase frame definitions and inspect the
result before treating a camera workflow as supported.
