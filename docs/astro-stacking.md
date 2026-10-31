# Calibrated astro stacking

In Library, select 3–16 tracked Bayer RAW light frames, open **Photo merge**, and choose **Astro stack**. Select the reference and optional bias, dark, flat, and flat-dark RAW sets. Confirm that these were taken with the same physical camera and appropriate sensor temperature; flats must also match the lens, aperture, focus and optical setup. ShopPhoto verifies the decoded camera model, ISO, active sensor geometry, CFA, raw range and exposure, but the decoder does not supply a sensor serial number or temperature to verify those automatically.

Calibration runs on signed floating-point sensor samples before demosaicing:

- Factory black levels are removed and samples are normalized by sensor range. Each calibration group forms a running arithmetic mean, retaining one sensor plane regardless of frame count (up to 64 frames per group).
- A dark must match the light exposure within 1%. A matched dark contains bias, so bias is not subtracted a second time. Without darks, an optional bias master is subtracted instead. Bias captures must be 1/20 second or shorter. There is no dark-exposure scaling.
- Flats require a bias or a flat-dark master. Flat-darks must match flat exposure within 1%; flats longer than one second require flat-darks. Corrected flats are normalized independently for each Bayer color. Clipped, zero/negative, or extreme flat responses are rejected, with a specific error, rather than silently filled.
- Corrected sensor values remain signed float through bilinear demosaicing, reference as-shot WB, upright orientation and conversion to scene-linear Rec.2020. This path does not quantize a calibrated mosaic to unsigned RAW counts and does not apply lens profiles, tone curves or AI enhancement.
- Optional registration supports small shifts, rotations and scale changes. Corresponding RGB samples are averaged after median/MAD sigma rejection; rejection does not favor a potentially contaminated reference frame. Common opaque coverage is cropped.

These calibration choices follow the matched-dark/bias distinction and corrected-flat requirements described in the [Astropy CCD reduction guide on dark calibration](https://www.astropy.org/ccd-reduction-and-photometry-guide/v/3.0.0a1/notebooks/03-05-Calibrate-dark-images.html) and [flat calibration](https://www.astropy.org/ccd-reduction-and-photometry-guide/v/2.0.1/notebooks/05-03-Calibrating-the-flats.html).

The result is an RGB32F linear TIFF with an embedded ICC profile and source/calibration recipe, plus a `.spmerge.json` recipe. All light and calibration parents are linked in the catalog. Output creation never overwrites an existing TIFF, and input files and edits remain unchanged. Empty calibration groups explicitly produce an uncalibrated stack.

This is a bounded workflow for tracked camera RAW exposures, not a full observatory reduction system. It does not support FITS, mono sensor filter-wheel channels, master reuse on disk, dark scaling, bad-pixel interpolation, drizzle, plate solving, untracked wide-field projection changes, or photometric catalog calibration. Mean calibration masters should be prepared from clean frames. Existing stack memory bounds remain 150 MP per light and 320 MP across all light frames, in addition to calibration planes. Cancellation is checked during sensor preparation, demosaic, registration, combination and atomic output writing.

Validation uses synthetic Bayer calibration with known flat response and signed signal, exposure/ISO mismatch and clipping failures, upright rotation, bounded running means, a sparse Gaussian star field with subpixel dither, and a contaminated reference frame whose transient is removed by sigma rejection. Real telescope/camera capture sets have not been validated in this environment.
