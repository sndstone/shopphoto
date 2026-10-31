# Tone upgrade F — Proof spaces and export parity

Status: complete (2026-07-22).

Added persisted view-only proof selection for Display native, sRGB, Display P3,
and Rec.2020, plus a gamut-warning marker. Proof state never enters EditState or
sidecars. Rec.2020→P3-D65 matrices live in `sp_core::color`, have a 1e-5
round-trip test, and are mirrored through output uniforms rather than shader
constants.

Export uses the shared core output-space enum. The GPU writes directly in the
selected primaries before clipping; CPU geometry/resizing then preserves those
primaries and the encoder attaches the matching ICC/cICP metadata. A GPU golden
test verifies P3 proof/export parity, and another verifies that a saturated
green is marked in sRGB proof but not Rec.2020.

Platform note: the existing macOS float surface remains extended-linear sRGB
(scRGB), because the whole egui layer shares one colorspace. Display native is
therefore the correctly tagged scRGB/Rec.709 presentation path on EDR; explicit
P3 proof is a simulation in the image output rather than retagging the entire
UI layer.
