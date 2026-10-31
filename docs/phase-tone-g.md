# Tone upgrade G — Built-in looks

Status: complete (2026-07-22).

Added programmatic Landscape, Portrait, Vibrant, Muted, and Flat looks. Each
contains a generated DNG-style HueSat table, optional base curve, and saturation
scale. Camera-profile and creative tables compose on the CPU into the existing
single post-profile GPU pass. A look curve replaces the camera/default base
curve, preventing double application. Unknown names load safely, render as
Standard, and show a warning in the picker.

The Basic panel includes a hover-preview picker; committing a look is a normal
undoable sidecar edit. Tests cover Standard equivalence, cool-color targeting
with skin protection, and one-pass table composition.

Performance: LUT/profile-stage reuse; no new pass.
