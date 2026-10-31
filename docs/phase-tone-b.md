# Tone upgrade B — Parametric and RGB curves

Status: complete (2026-07-22).

`CurveParams` contains four raised-cosine parametric regions plus Y/R/G/B point
curves. Custom deserialization accepts the old bare `points` object, so schema
version 1 remains valid. Y and parametric moves fold into the 1024-entry tone
LUT; RGB curves use a packed 3×256 encoded-domain buffer and are skipped by a
uniform flag when neutral. The curve editor has a colored channel selector.

Tests load a pre-upgrade sidecar, enforce LUT monotonicity, and verify on the
GPU that a red curve changes only the red working-space channel.

Performance: one small storage-buffer lookup only when channel curves are
active; no additional pass.
