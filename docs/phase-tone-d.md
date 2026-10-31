# Tone upgrade D — Vignette and HDR brightness

Status: complete (2026-07-22).

Added post-crop vignette controls with radial EV gain, midpoint, roundness, and
feather. The output shaders use global tile coordinates, so vignette and dither
are seam-stable during tiled export. Highlight-priority weighting protects
values above reference white. The CPU mirror verifies neutral center and the
±1.5 EV corner range.

HDR now has Scene white (-2…+2 EV), independent of Exposure. The SDR, PQ, and
scRGB output variants apply the same value, and the HDR panel shows the current
display capability in stops.

Performance: output-transform math only; no additional pass.
