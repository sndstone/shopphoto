# Tone upgrade A — Brightness and Auto tone

Status: complete (2026-07-22).

Implemented encoded-domain midtone brightness in the CPU tone LUT. The gamma
move pins black and SDR white and preserves the linear offset above 1.0. Auto
tone derives exposure, blacks, whites, and bounded highlight recovery from the
existing asynchronous RGB histogram. Auto tone and auto exposure are explicit
one-step undo actions; Shift-double-clicking Exposure invokes auto exposure.

Tests cover endpoint pinning, monotonicity at combined extremes, low/high/flat
synthetic histograms, and schema-v1 defaults. On the real X2D II sample, the
histogram path produces a finite bounded automatic starting point.

Performance: LUT-only; no GPU pass or neutral-render cost was added.
