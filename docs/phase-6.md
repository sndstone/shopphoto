# Phase 6 — HDR

- Scene headroom is preserved by ratio-based clipped-channel reconstruction.
- HDR histogram reads the pre-output float working texture; bins 224–255 cover four stops above SDR white.
- BT.2100 PQ OETF/EOTF have reference-vector tests.
- HDR edits have separate SDR fallback and Rec.2100 PQ export transforms.
- 16-bit PNG writes BT.2020/PQ cICP; AVIF consumes the high-precision PQ handoff, converts with BT.2020 non-constant-luminance coefficients, and writes a verified high-bit-depth payload tagged BT.2020/ST-2084 nclx.

Native EDR presentation is ENABLED on macOS (follow-up session). The
original conclusion that wgpu 30 was required proved wrong: wgpu 29's Metal
backend already sets `CAMetalLayer.wantsExtendedDynamicRangeContent` for
`Rgba16Float` surfaces — wgpu 30 only formalizes the colorspace API. The
enablement:

- `vendor/egui-wgpu` (patched, `[patch.crates-io]`): prefers an
  `Rgba16Float` surface when `SHOPPHOTO_HDR_SURFACE` is set (sp-app sets it
  by default on macOS; `SHOPPHOTO_SDR=1` opts out), and routes float
  framebuffers through egui's linear fragment path so the UI renders
  correctly (UI whites stay at 1.0 = SDR white).
- New `output_scrgb.wgsl` pass + `hdr_display` texture: scene-linear
  Rec.2020 → linear Rec.709 scRGB with a smooth shoulder limited to the
  edit's headroom stops; runs only when the surface is float AND the edit
  has HDR on.
- `sp-app/src/hdr_view.rs`: egui paint-callback blit of that texture
  straight into the float swapchain — highlight values above 1.0 reach
  Metal EDR unmodified. Badges show "HDR display · N stops" / "EDR ready".

Follow-up (session 11): a nil layer colorspace turned out to CLAMP at 1.0
on-device — EDR showed no extra brightness at all. `sp-app/src/macos_edr.rs`
(objc2, the documented §10.6 exception) now finds wgpu's CAMetalLayer
(raw-window-metal attaches it as a content-view sublayer) and sets
`kCGColorSpaceExtendedLinearSRGB` + wantsEDR; it also polls
`NSScreen.maximumExtendedDynamicRangeColorComponentValue` for the live
panel headroom shown in the top-bar badge ("panel ×N"; ×1.0 means the
display currently offers no headroom — common off-XDR or at max
brightness). Delete the shim when eframe adopts wgpu ≥ 30.

Deep-color viewport: the scRGB float pass now runs for EVERY render on the
EDR surface — SDR edits display through the float path with a hard 1.0
ceiling (bit-identical look to the 8-bit path, ~16-bit on-screen precision,
answering the "looks 8-bit" report), HDR edits additionally get a
luminance-weighted highlight expansion into the panel headroom plus true
>1.0 reconstructed-highlight pass-through and the soft shoulder.

Known deviations:
- The straighten-preview rotation path and zoomed 1:1 tiles fall back to
  the SDR texture; fit view is the HDR reference.
- eframe's screenshot readback assumes 4-byte pixels and would panic on the
  float surface — `SP_SCREENSHOT` QA now requires `SHOPPHOTO_SDR=1` (the
  hook exits with a warning otherwise). Visual EDR confirmation is by eye
  on an XDR panel: enable HDR on a bright-highlight image and the sky/
  speculars should exceed the white UI chrome.
