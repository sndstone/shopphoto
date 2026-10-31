//! Decoded RAW image data and capture metadata.
//!
//! Produced by `sp-raw` on a decode-pool thread, consumed by `sp-pipeline`
//! (uploaded to the GPU as an `R16Uint` texture). CPU-side CFA data for a
//! 100 MP file is ~200 MB; the LRU in `sp-pipeline` keeps at most 2 of these
//! alive (BUILD.md §3.5).

use crate::color::Mat3;
use serde::{Deserialize, Serialize};

/// Camera RAW extensions accepted across decoding, library scanning, file
/// dialogs, and format filters. Lowercase, without a leading dot.
pub const SUPPORTED_RAW_EXTENSIONS: &[&str] = &[
    "3fr", "ari", "arw", "cr2", "cr3", "crm", "crw", "dcr", "dcs", "dng", "erf", "fff", "iiq",
    "kdc", "mef", "mos", "mrw", "nef", "nrw", "orf", "ori", "pef", "qtk", "raf", "raw", "rw2",
    "rwl", "srw", "x3f",
];

/// Bayer color-filter-array layout, named by the 2×2 quad reading order
/// (row-major, top-left first) after crop to the active area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfaPattern {
    Rggb,
    Bggr,
    Grbg,
    Gbrg,
}

impl CfaPattern {
    /// Color channel (0=R, 1=G, 2=B) at absolute CFA coordinate `(x, y)`.
    #[must_use]
    pub fn color_at(self, x: u32, y: u32) -> u8 {
        // 2×2 quads as [top-left, top-right, bottom-left, bottom-right]
        let quad: [u8; 4] = match self {
            CfaPattern::Rggb => [0, 1, 1, 2],
            CfaPattern::Bggr => [2, 1, 1, 0],
            CfaPattern::Grbg => [1, 0, 2, 1],
            CfaPattern::Gbrg => [1, 2, 0, 1],
        };
        quad[((y & 1) * 2 + (x & 1)) as usize]
    }

    /// The pattern as seen from an origin shifted by `(dx, dy)` photosites —
    /// needed when demosaicing a tile whose corner is not quad-aligned with
    /// the frame this pattern is relative to.
    #[must_use]
    pub fn shifted(self, dx: u32, dy: u32) -> CfaPattern {
        let colors = (
            self.color_at(dx, dy),
            self.color_at(dx + 1, dy),
            self.color_at(dx, dy + 1),
        );
        match colors {
            (0, 1, 1) => CfaPattern::Rggb,
            (2, 1, 1) => CfaPattern::Bggr,
            (1, 0, 2) => CfaPattern::Grbg,
            (1, 2, 0) => CfaPattern::Gbrg,
            _ => unreachable!("2x2 Bayer shift always yields a Bayer pattern"),
        }
    }
}

/// EXIF-style capture metadata, displayed in the UI and copied on export.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageMeta {
    pub camera_make: String,
    pub camera_model: String,
    pub lens_model: Option<String>,
    pub iso: Option<u32>,
    /// Shutter time in seconds (e.g. 0.004 for 1/250).
    pub shutter_s: Option<f32>,
    pub aperture_f: Option<f32>,
    pub focal_length_mm: Option<f32>,
    /// Capture time as RFC 3339 string, if present in EXIF.
    pub captured_at: Option<String>,
    /// Capture coordinates as `[latitude, longitude]` in decimal degrees.
    #[serde(default)]
    pub gps: Option<[f64; 2]>,
}

/// Rectangular crop in sensor pixels: the photosite area actually holding
/// image data (excludes masked calibration borders).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveArea {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// EXIF orientation, reduced to the cases cameras actually produce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
}

/// A decoded, unprocessed RAW frame: sensor CFA data plus everything the GPU
/// pipeline needs to render it (black/white levels, WB, color matrix).
#[derive(Debug, Clone)]
pub struct RawImage {
    /// Sensor data, row-major, one `u16` per photosite, full sensor frame
    /// (crop to [`Self::active_area`] happens on the GPU).
    pub cfa_data: Vec<u16>,
    /// Full sensor width in photosites (stride of `cfa_data`).
    pub width: u32,
    /// Full sensor height in photosites.
    pub height: u32,
    /// CFA layout relative to the full sensor frame origin.
    pub cfa: CfaPattern,
    /// Per-channel black level [R, G, B, E/G2], in raw counts.
    pub black_levels: [f32; 4],
    /// Per-channel clip point [R, G, B, E/G2], in raw counts.
    pub white_levels: [f32; 4],
    /// As-shot white-balance multipliers [R, G, B, G2] (G usually 1.0).
    pub wb_coeffs: [f32; 4],
    /// Camera RGB → CIE XYZ (D65) matrix from decoder metadata.
    /// Basis of profile Tier 1 (BUILD.md §3.4).
    pub cam_to_xyz: Mat3,
    pub active_area: ActiveArea,
    pub orientation: Orientation,
}

impl RawImage {
    /// Effective megapixels of the active area, e.g. `101.6` for an X2D.
    #[must_use]
    pub fn active_megapixels(&self) -> f32 {
        (self.active_area.width as f32 * self.active_area.height as f32) / 1.0e6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfa_color_at_covers_quad() {
        // RGGB: (0,0)=R (1,0)=G (0,1)=G (1,1)=B, repeating.
        let p = CfaPattern::Rggb;
        assert_eq!(p.color_at(0, 0), 0);
        assert_eq!(p.color_at(1, 0), 1);
        assert_eq!(p.color_at(0, 1), 1);
        assert_eq!(p.color_at(1, 1), 2);
        // Periodicity
        assert_eq!(p.color_at(2, 2), 0);
        assert_eq!(p.color_at(3, 3), 2);
    }

    #[test]
    fn shifted_patterns() {
        assert_eq!(CfaPattern::Rggb.shifted(0, 0), CfaPattern::Rggb);
        assert_eq!(CfaPattern::Rggb.shifted(1, 0), CfaPattern::Grbg);
        assert_eq!(CfaPattern::Rggb.shifted(0, 1), CfaPattern::Gbrg);
        assert_eq!(CfaPattern::Rggb.shifted(1, 1), CfaPattern::Bggr);
        assert_eq!(CfaPattern::Rggb.shifted(2, 2), CfaPattern::Rggb);
        // Shift consistency for every pattern: color_at must agree.
        for p in [
            CfaPattern::Rggb,
            CfaPattern::Bggr,
            CfaPattern::Grbg,
            CfaPattern::Gbrg,
        ] {
            for dy in 0..2 {
                for dx in 0..2 {
                    let sh = p.shifted(dx, dy);
                    for y in 0..2 {
                        for x in 0..2 {
                            assert_eq!(sh.color_at(x, y), p.color_at(x + dx, y + dy));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_pattern_has_two_greens_per_quad() {
        for p in [
            CfaPattern::Rggb,
            CfaPattern::Bggr,
            CfaPattern::Grbg,
            CfaPattern::Gbrg,
        ] {
            let greens = (0..2)
                .flat_map(|y| (0..2).map(move |x| p.color_at(x, y)))
                .filter(|&c| c == 1)
                .count();
            assert_eq!(greens, 2, "{p:?}");
        }
    }
}
