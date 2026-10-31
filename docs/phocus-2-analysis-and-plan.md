# Phocus-informed integration plan

This note replaces an earlier teardown-oriented draft. ShopPhoto may learn
from publicly documented product behavior and from reference files created by
the user, but public or redistributed builds must not ship or derive private
Hasselblad LUTs, factory calibration tables, lens maps,
endpoints, or extracted code.

The public-source research used for this decision is collected in
[`research-2026-08-31/phocus-hasselblad.html`](research-2026-08-31/phocus-hasselblad.html).

## What public Phocus behavior teaches us

- HDR editing benefits from separate highlight recovery, a user-visible peak
  limit, and an indication of current monitor headroom.
- Phocus Mobile can export Ultra HDR and uses a camera-aware color-rendering
  path.
- Hasselblad describes HNCS, its LUT, film curve, and factory lens/color
  calibration as Hasselblad technology. Those payloads are not reusable.

These observations define useful product requirements; they do not provide an
implementation or data license.

## ShopPhoto's independent equivalents

### Reference Match

Reference Match fits a profile from RAW files and neutral 16-bit TIFFs that
the user created and is entitled to use. A TIFF can come from Phocus or another
trusted renderer. The fitter uses ShopPhoto-authored matrix, chromaticity-warp,
curve, and residual-LUT code and reports holdout ΔE00.

Rules:

1. Keep every input path and renderer choice visible to the user.
2. Require matched RAW/TIFF stems and one camera model per fit.
3. Use varied, non-clipped samples and hold out data from optimization.
4. Never bundle a fitted profile made from somebody else's protected files.
5. Store provenance with future profile versions.

### AI RAW denoise

ShopPhoto RAW Denoise uses ShopPhoto's own `r4_plain` model on Bayer sensor
data before demosaic. See [model, installation and runtime details](raw-denoise.md).

### Lens shading

The `.splens` format stores user-measured per-CFA gain grids. Capture flat
fields wide open and at a commonly available stopped-down aperture such as
f/8, adding zoom endpoints where relevant. Fit a smooth field per CFA channel,
normalize it at the optical center, and validate it on a second flat frame.

The application does not include factory lens maps or factory-selected
calibration apertures. Sensor dimensions come from decoded RAW metadata; the
grid center is derived geometrically.

### Multi-illuminant color

For a clean-room profile, capture a target under measured lights near ordinary
reference points: CIE A (2856 K), a warm-neutral source around 4000 K,
daylight/flash around 5500 K, and D65 (6504 K). These are generic capture
targets, not Hasselblad factory nodes. Store the measured CCT and interpolate
in reciprocal temperature.

## Validation gates

- Full workspace tests and strict Clippy pass.
- CPU/GPU convention tests cover grid indexing, neutral fade, luma retention,
  and illuminant interpolation.
- Reference Match reports training and holdout error separately.
- Lens profiles are tested on captures not used for fitting.
- Ultra HDR files are structurally validated and must still be viewed on at
  least one compatible phone/browser before making a broad compatibility
  claim.
- All user-facing labels state what ShopPhoto actually does and avoid
  proprietary-equivalence wording.

## Primary public references

- [Phocus for Mac and PC](https://www.hasselblad.com/phocus/phocus-for-pc-mac)
- [Phocus 4.0.1 read-me](https://cdn.hasselblad.com/software/Phocus_for_Mac/4.0.1/Phocus-Mac-read-me.pdf)
- [Phocus Mobile](https://www.hasselblad.com/phocus/phocus-mobile-2/)
- [Hasselblad Natural Colour Solution](https://www.hasselblad.com/learn/hasselblad-natural-colour-solution/)
- [Android Ultra HDR](https://developer.android.com/media/platform/hdr-image-format)
- [Apple EDR](https://developer.apple.com/videos/play/wwdc2021/10161/)
