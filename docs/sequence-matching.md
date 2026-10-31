# Sequence exposure and white-balance continuity

Select 2–64 related photographs and open **Match sequence**. Pick a reference,
choose whether to use its resolved white balance, and analyze. The result is
a review list with actual develop previews, original/proposed exposure values,
WB values, sample counts, scene variation and alignment confidence. Check the
changes to retain, then apply them explicitly.

RAW reference As shot is resolved from camera WB coefficients into temperature
and tint. Rendered RGB uses the editor's relative WB convention. RAW sequences
must use the same camera family, and RAW and rendered sources cannot be mixed.
Matching transfers the reference's resolved WB; it does not assume every scene
contains a neutral gray subject. Camera calibration and native Bayer quads
form bounded linear analysis thumbnails for RAW. RGB analysis uses its
ICC-converted linear Rec.2020 source.

Exposure changes are the median of paired unclipped scene-luminance ratios
after the selected WB correction. Small camera movements can be registered
before matching; a locked tripod can bypass registration. Low overlap, too few
usable samples, inconsistent light/content or exposure outside the develop
slider range produce visible review errors rather than fabricated settings.

Only exposure and, when selected, WB are changed. Every other develop setting,
annotation, descriptive field and snapshot remains in the source sidecar.
Each proposal records a before-history entry. Applying uses the existing
serialized, atomic before/after persistence lane; a changed source edit is
skipped. A changed reference requires reanalysis. Current in-memory manual
edits also take precedence. Analysis is cancellable and does not write edits.

Validation covers paired 2 EV recovery, raster relative-WB behavior, RAW
as-shot WB recovery and calibrated linear thumbnails. The shared persistence
lane separately tests stale-sidecar rejection. Histogram matching across
unrelated compositions and learned relighting are outside this workflow.
