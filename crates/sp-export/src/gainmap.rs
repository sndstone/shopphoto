//! Ultra HDR gain maps (BUILD.md §9).
//!
//! An Ultra HDR JPEG is an ordinary SDR JPEG that carries a second,
//! smaller JPEG — the gain map — plus metadata describing how to combine
//! them. Readers that know nothing about it (every existing JPEG decoder)
//! show the SDR image; readers that do (Android, iOS 18+, Chrome, Safari)
//! reconstruct the HDR rendition:
//!
//! ```text
//! HDR = (SDR + offset_sdr) * 2^(gain * (max - min) + min) - offset_hdr
//! ```
//!
//! We produce a single-channel map from the luminance ratio between the
//! SDR render and the PQ HDR render of the same edit. A per-channel map is
//! permitted by the format but buys little here: our HDR and SDR renditions
//! differ in highlight rolloff, not hue.
//!
//! The absolute-luminance reference is BT.2408's 203 cd/m² diffuse white —
//! the value that makes "1.0 in the SDR rendition" and "PQ-coded 203 nits"
//! the same brightness, so a scene with no highlights above SDR white
//! yields a flat, zero gain map.

use crate::{ExportColorSpace, ExportImage};
use sp_core::color::pq_eotf;
#[cfg(test)]
use sp_core::color::srgb_eotf;

/// BT.2408 HDR reference white in cd/m². PQ is absolute, the SDR rendition
/// is relative, and this is the constant that ties them together.
const REFERENCE_WHITE_NITS: f32 = 203.0;

/// Ultra HDR's default offsets, applied to both renditions so the ratio
/// stays finite in the deep shadows (1/64 in Google's reference encoder).
const OFFSET: f32 = 1.0 / 64.0;

/// Largest boost we will encode, in stops. Beyond ~4 stops the map costs
/// precision without any display being able to show the difference.
const MAX_BOOST_STOPS: f32 = 4.0;

/// The `hdrgm:` metadata that describes a [`GainMap`] to a decoder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainMapParams {
    /// log2 gain encoded by byte value 0.
    pub min_log2: f32,
    /// log2 gain encoded by byte value 255.
    pub max_log2: f32,
    pub gamma: f32,
    pub offset_sdr: f32,
    pub offset_hdr: f32,
    pub hdr_capacity_min: f32,
    pub hdr_capacity_max: f32,
}

/// A computed gain map: single-channel, half resolution.
#[derive(Debug, Clone)]
pub struct GainMap {
    pub width: u32,
    pub height: u32,
    /// One byte per pixel.
    pub data: Vec<u8>,
    pub params: GainMapParams,
}

/// Relative luminance in the base rendition's RGB space, in SDR-white units.
fn sdr_luminance(rgb: &[u8], space: ExportColorSpace) -> f32 {
    let weights = match space {
        ExportColorSpace::Srgb => [0.2126, 0.7152, 0.0722],
        ExportColorSpace::DisplayP3 => [0.228975, 0.691739, 0.079287],
        ExportColorSpace::Rec2020 => sp_core::color::REC2020_LUMA,
        ExportColorSpace::AdobeRgb => sp_core::color::ADOBERGB_TO_XYZ[1],
    };
    rgb.iter()
        .take(3)
        .zip(weights)
        .map(|(value, weight)| space.transfer().eotf(f32::from(*value) / 255.0) * weight)
        .sum()
}

/// Luminance of a Rec.2020 PQ pixel, in SDR-white units.
fn hdr_luminance(rgb: [f32; 3]) -> f32 {
    // pq_eotf returns luminance normalized to 10,000 cd/m².
    let nits = |v: f32| pq_eotf(v) * 10_000.0;
    let y = 0.2627 * nits(rgb[0]) + 0.6780 * nits(rgb[1]) + 0.0593 * nits(rgb[2]);
    y / REFERENCE_WHITE_NITS
}

/// Per-pixel log2 boost of `hdr` over `sdr`, before quantization.
fn gain_at(sdr: &[u8], hdr: [f32; 3], space: ExportColorSpace) -> f32 {
    let y_sdr = sdr_luminance(sdr, space);
    let y_hdr = hdr_luminance(hdr);
    ((y_hdr + OFFSET) / (y_sdr + OFFSET)).max(1e-6).log2()
}

/// Read pixel `i` of a PQ-encoded [`ExportImage`] as 0..=1 floats,
/// preferring the 16-bit buffer when the renderer supplied one.
fn pq_pixel(image: &ExportImage, i: usize) -> [f32; 3] {
    match &image.rgba16 {
        Some(rgba16) => {
            let px = &rgba16[i * 4..i * 4 + 3];
            [
                f32::from(px[0]) / 65_535.0,
                f32::from(px[1]) / 65_535.0,
                f32::from(px[2]) / 65_535.0,
            ]
        }
        None => {
            let px = &image.rgba8[i * 4..i * 4 + 3];
            [
                f32::from(px[0]) / 255.0,
                f32::from(px[1]) / 255.0,
                f32::from(px[2]) / 255.0,
            ]
        }
    }
}

/// Compute the gain map relating an SDR rendition to a PQ HDR rendition of
/// the same image. Both must have identical dimensions.
///
/// The SDR rendition uses sRGB. Returns `None` for invalid pixel buffers or
/// different dimensions.
#[must_use]
pub fn compute(sdr: &ExportImage, hdr: &ExportImage) -> Option<GainMap> {
    compute_with_color_space(sdr, hdr, ExportColorSpace::Srgb)
}

/// Compute a gain map using the actual SDR base primaries and transfer curve.
/// The HDR rendition always uses Rec.2020 PQ.
#[must_use]
pub fn compute_with_color_space(
    sdr: &ExportImage,
    hdr: &ExportImage,
    space: ExportColorSpace,
) -> Option<GainMap> {
    let pixels = (sdr.width as usize).checked_mul(sdr.height as usize)?;
    let samples = pixels.checked_mul(4)?;
    if sdr.width == 0
        || sdr.height == 0
        || sdr.width != hdr.width
        || sdr.height != hdr.height
        || sdr.rgba8.len() != samples
        || hdr.rgba8.len() != samples
        || hdr
            .rgba16
            .as_ref()
            .is_some_and(|buffer| buffer.len() != samples)
    {
        return None;
    }
    let (w, h) = (sdr.width, sdr.height);

    let mut gains = Vec::with_capacity(pixels);
    let mut max_gain = 0.0f32;
    for i in 0..pixels {
        let g = gain_at(&sdr.rgba8[i * 4..i * 4 + 3], pq_pixel(hdr, i), space).max(0.0);
        max_gain = max_gain.max(g);
        gains.push(g);
    }

    // The map only ever brightens: min stays at 0 so an SDR-only decoder's
    // rendition is exactly the base image, and a boost of 0 is representable
    // without quantization error.
    let min_log2 = 0.0;
    let max_log2 = max_gain.clamp(0.0, MAX_BOOST_STOPS);
    // A flat map (no highlights above SDR white) still needs a non-zero
    // range, or the decoder divides by zero.
    let range = if max_log2 > 1e-3 { max_log2 } else { 1.0 };

    // Half resolution: the gain map is a low-frequency signal, decoders
    // upsample it, and this keeps a 100 MP export from carrying a 100 MP
    // second image.
    let map_w = w.div_ceil(2).max(1);
    let map_h = h.div_ceil(2).max(1);
    let mut data = Vec::with_capacity((map_w as usize) * (map_h as usize));
    for my in 0..map_h {
        for mx in 0..map_w {
            // Box-average the (up to) 2×2 source block.
            let mut sum = 0.0f32;
            let mut count = 0.0f32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (x, y) = (mx * 2 + dx, my * 2 + dy);
                    if x < w && y < h {
                        sum += gains[(y as usize) * (w as usize) + x as usize];
                        count += 1.0;
                    }
                }
            }
            let g = sum / f32::max(count, 1.0);
            let normalized = ((g - min_log2) / range).clamp(0.0, 1.0);
            data.push((normalized * 255.0).round() as u8);
        }
    }

    Some(GainMap {
        width: map_w,
        height: map_h,
        data,
        params: GainMapParams {
            min_log2,
            max_log2: range,
            gamma: 1.0,
            offset_sdr: OFFSET,
            offset_hdr: OFFSET,
            hdr_capacity_min: 0.0,
            hdr_capacity_max: range,
        },
    })
}

impl GainMapParams {
    /// The `hdrgm:` XMP packet that rides on the gain-map image.
    #[must_use]
    pub fn xmp_packet(&self) -> String {
        format!(
            r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/"
    hdrgm:Version="1.0"
    hdrgm:GainMapMin="{min}"
    hdrgm:GainMapMax="{max}"
    hdrgm:Gamma="{gamma}"
    hdrgm:OffsetSDR="{offset_sdr}"
    hdrgm:OffsetHDR="{offset_hdr}"
    hdrgm:HDRCapacityMin="{capacity_min}"
    hdrgm:HDRCapacityMax="{capacity_max}"
    hdrgm:BaseRenditionIsHDR="False"/>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
            min = self.min_log2,
            max = self.max_log2,
            gamma = self.gamma,
            offset_sdr = self.offset_sdr,
            offset_hdr = self.offset_hdr,
            capacity_min = self.hdr_capacity_min,
            capacity_max = self.hdr_capacity_max,
        )
    }

    /// Reconstruct an HDR luminance from an SDR one and an encoded gain
    /// byte, exactly as a decoder would. Used by tests, and the reference
    /// for anyone changing [`compute`].
    #[must_use]
    pub fn apply(&self, sdr: f32, encoded: u8) -> f32 {
        let normalized = f32::from(encoded) / 255.0;
        let gain = normalized.powf(self.gamma) * (self.max_log2 - self.min_log2) + self.min_log2;
        (sdr + self.offset_sdr) * gain.exp2() - self.offset_hdr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solid image: `sdr_value` sRGB-encoded, `hdr_nits` PQ-encoded.
    fn pair(w: u32, h: u32, sdr_value: u8, hdr_nits: f32) -> (ExportImage, ExportImage) {
        let sdr = ExportImage::new(
            w,
            h,
            std::iter::repeat_n([sdr_value, sdr_value, sdr_value, 255], (w * h) as usize)
                .flatten()
                .collect(),
        )
        .expect("sdr");
        let encoded = (sp_core::color::pq_oetf(hdr_nits / 10_000.0) * 65_535.0).round() as u16;
        let hdr = ExportImage::new_16(
            w,
            h,
            std::iter::repeat_n([encoded, encoded, encoded, 65_535], (w * h) as usize)
                .flatten()
                .collect(),
        )
        .expect("hdr");
        (sdr, hdr)
    }

    #[test]
    fn map_is_half_resolution_and_rounds_up() {
        let (sdr, hdr) = pair(101, 51, 128, 203.0);
        let map = compute(&sdr, &hdr).expect("map");
        assert_eq!((map.width, map.height), (51, 26));
        assert_eq!(map.data.len(), 51 * 26);
    }

    #[test]
    fn matching_brightness_yields_a_flat_zero_map() {
        // Mid-gray SDR and the same luminance in PQ: no boost anywhere.
        let sdr_value = 128u8;
        let sdr_luma = srgb_eotf(f32::from(sdr_value) / 255.0);
        let (sdr, hdr) = pair(8, 8, sdr_value, sdr_luma * REFERENCE_WHITE_NITS);
        let map = compute(&sdr, &hdr).expect("map");
        assert!(
            map.data.iter().all(|&v| v <= 1),
            "expected a flat map, got {:?}",
            &map.data[..4]
        );
    }

    #[test]
    fn two_stop_highlight_is_encoded_and_reconstructs() {
        let sdr_value = 255u8;
        let sdr_luma = srgb_eotf(1.0);
        // Four times the SDR luminance = two stops.
        let (sdr, hdr) = pair(8, 8, sdr_value, sdr_luma * REFERENCE_WHITE_NITS * 4.0);
        let map = compute(&sdr, &hdr).expect("map");
        assert!(
            (map.params.max_log2 - 2.0).abs() < 0.1,
            "expected ~2 stops, got {}",
            map.params.max_log2
        );
        // Full-scale byte at the brightest pixel.
        assert_eq!(map.data[0], 255);

        // A decoder applying the published formula must land back on the
        // HDR luminance it came from.
        let reconstructed = map.params.apply(sdr_luma, map.data[0]);
        let expected = sdr_luma * 4.0;
        assert!(
            (reconstructed - expected).abs() < expected * 0.05,
            "reconstruction {reconstructed} vs expected {expected}"
        );
    }

    #[test]
    fn boost_is_clamped_to_the_encodable_range() {
        let sdr_luma = srgb_eotf(1.0);
        // 8 stops of headroom, far past MAX_BOOST_STOPS.
        let (sdr, hdr) = pair(4, 4, 255, sdr_luma * REFERENCE_WHITE_NITS * 256.0);
        let map = compute(&sdr, &hdr).expect("map");
        assert!(map.params.max_log2 <= MAX_BOOST_STOPS);
        assert!(map.params.hdr_capacity_max <= MAX_BOOST_STOPS);
    }

    #[test]
    fn wide_gamut_bases_reconstruct_their_actual_luminance() {
        // Saturated mid-tone red exposes both incorrect primaries and an
        // incorrect Adobe RGB transfer curve; neutral gray would hide them.
        for (space, red_weight) in [
            (ExportColorSpace::DisplayP3, 0.228975),
            (ExportColorSpace::Rec2020, 0.2627),
            (ExportColorSpace::AdobeRgb, 0.297377),
        ] {
            let red_linear = if space == ExportColorSpace::AdobeRgb {
                (128.0f32 / 255.0).powf(563.0 / 256.0)
            } else {
                srgb_eotf(128.0 / 255.0)
            };
            let base_luminance = red_weight * red_linear;
            let (_, hdr) = pair(2, 2, 0, base_luminance * REFERENCE_WHITE_NITS * 4.0);
            let sdr = ExportImage::new(2, 2, [128, 0, 0, 255].repeat(4)).unwrap();
            let map = compute_with_color_space(&sdr, &hdr, space).unwrap();
            let reconstructed = map.params.apply(base_luminance, map.data[0]);
            assert!(
                (reconstructed - base_luminance * 4.0).abs() < 0.001,
                "{space:?}: {reconstructed}"
            );
        }
    }

    #[test]
    fn malformed_buffers_and_empty_images_are_rejected() {
        let (mut sdr, mut hdr) = pair(2, 2, 128, 203.0);
        sdr.width = 0;
        assert!(compute(&sdr, &hdr).is_none());
        sdr.width = 2;
        hdr.rgba16.as_mut().unwrap().pop();
        assert!(compute(&sdr, &hdr).is_none());
        hdr.rgba16 = None;
        sdr.rgba8.pop();
        assert!(compute(&sdr, &hdr).is_none());
    }

    #[test]
    fn mismatched_dimensions_are_rejected() {
        let (sdr, _) = pair(8, 8, 128, 203.0);
        let (_, hdr) = pair(4, 4, 128, 203.0);
        assert!(compute(&sdr, &hdr).is_none());
    }

    #[test]
    fn xmp_packet_carries_every_required_field() {
        let (sdr, hdr) = pair(4, 4, 200, 800.0);
        let packet = compute(&sdr, &hdr).expect("map").params.xmp_packet();
        for field in [
            "hdrgm:Version",
            "hdrgm:GainMapMin",
            "hdrgm:GainMapMax",
            "hdrgm:Gamma",
            "hdrgm:OffsetSDR",
            "hdrgm:OffsetHDR",
            "hdrgm:HDRCapacityMin",
            "hdrgm:HDRCapacityMax",
            "hdrgm:BaseRenditionIsHDR",
        ] {
            assert!(packet.contains(field), "missing {field}");
        }
    }
}
