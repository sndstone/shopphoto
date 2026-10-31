//! Scene-linear sample readouts and bounded photographic scopes.
use crate::{
    color::{mat3_apply, LinearRec2020, REC2020_LUMA, REC2020_TO_XYZ},
    source::RasterImage,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorSamplePoint {
    pub name: String,
    pub position: [f32; 2],
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabD65(pub [f32; 3]);
#[derive(Debug, Clone, Copy)]
pub struct ColorReadout {
    pub rgb: LinearRec2020,
    pub lab: LabD65,
    pub ev_from_middle_gray: Option<f32>,
}
impl ColorReadout {
    pub fn from_linear(rgb: LinearRec2020) -> Self {
        let xyz = mat3_apply(&REC2020_TO_XYZ, rgb.0);
        let f = |t: f32| {
            if t > 216.0 / 24389.0 {
                t.cbrt()
            } else {
                t * (841.0 / 108.0) + 4.0 / 29.0
            }
        };
        let x = f(xyz[0] / 0.95047);
        let y = f(xyz[1]);
        let z = f(xyz[2] / 1.08883);
        let luminance = rgb
            .0
            .iter()
            .zip(REC2020_LUMA)
            .map(|(v, w)| v * w)
            .sum::<f32>();
        Self {
            rgb,
            lab: LabD65([116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]),
            ev_from_middle_gray: (luminance > 0.0).then(|| (luminance / 0.18).log2()),
        }
    }
}

pub fn sample(frame: &RasterImage, position: [f32; 2]) -> Option<ColorReadout> {
    if frame.width == 0 || frame.height == 0 || !position.iter().all(|v| v.is_finite()) {
        return None;
    }
    let x = position[0].clamp(0.0, 1.0) * (frame.width - 1) as f32;
    let y = position[1].clamp(0.0, 1.0) * (frame.height - 1) as f32;
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(frame.width - 1);
    let y1 = (y0 + 1).min(frame.height - 1);
    let fetch = |x, y| {
        frame
            .linear_rec2020
            .get((y * frame.width + x) as usize)
            .copied()
    };
    let (a, b, c, d) = (
        fetch(x0, y0)?,
        fetch(x1, y0)?,
        fetch(x0, y1)?,
        fetch(x1, y1)?,
    );
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    Some(ColorReadout::from_linear(LinearRec2020(
        std::array::from_fn(|channel| {
            let top = a[channel] + (b[channel] - a[channel]) * fx;
            let bottom = c[channel] + (d[channel] - c[channel]) * fx;
            top + (bottom - top) * fy
        }),
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeMode {
    Waveform,
    Parade,
    Vectorscope,
}
#[derive(Debug, Clone)]
pub struct ScopeData {
    pub width: usize,
    pub height: usize,
    pub counts: Vec<[u32; 3]>,
}
impl ScopeData {
    /// Whole-image working-space scopes. `ceiling_ev` is relative to linear
    /// diffuse white, allowing HDR values up to 16× white to remain visible.
    pub fn build(frame: &RasterImage, mode: ScopeMode, ceiling_ev: f32) -> Self {
        let (width, height) = match mode {
            ScopeMode::Waveform => (256, 128),
            ScopeMode::Parade => (384, 128),
            ScopeMode::Vectorscope => (256, 256),
        };
        let mut result = Self {
            width,
            height,
            counts: vec![[0; 3]; width * height],
        };
        let ceiling = 2.0f32.powf(ceiling_ev.clamp(0.0, 4.0));
        for (i, pixel) in frame.linear_rec2020.iter().enumerate() {
            if frame.width == 0 || !pixel[..3].iter().all(|v| v.is_finite()) {
                continue;
            }
            match mode {
                ScopeMode::Waveform => {
                    let x = (i % frame.width as usize) * width / frame.width as usize;
                    let value = pixel
                        .iter()
                        .zip(REC2020_LUMA)
                        .map(|(v, w)| v * w)
                        .sum::<f32>();
                    let y = height
                        - 1
                        - (value / ceiling * (height - 1) as f32)
                            .round()
                            .clamp(0.0, (height - 1) as f32) as usize;
                    result.counts[y * width + x][0] += 1;
                }
                ScopeMode::Parade => {
                    for (channel, value) in pixel.iter().take(3).enumerate() {
                        let x =
                            channel * 128 + (i % frame.width as usize) * 128 / frame.width as usize;
                        let y = height
                            - 1
                            - (*value / ceiling * (height - 1) as f32)
                                .round()
                                .clamp(0.0, (height - 1) as f32)
                                as usize;
                        result.counts[y * width + x][channel] += 1;
                    }
                }
                ScopeMode::Vectorscope => {
                    let lab =
                        ColorReadout::from_linear(LinearRec2020([pixel[0], pixel[1], pixel[2]]))
                            .lab
                            .0;
                    let x = ((lab[1] / 256.0 + 0.5) * (width - 1) as f32)
                        .round()
                        .clamp(0.0, (width - 1) as f32) as usize;
                    let y = ((0.5 - lab[2] / 256.0) * (height - 1) as f32)
                        .round()
                        .clamp(0.0, (height - 1) as f32) as usize;
                    result.counts[y * width + x][0] += 1;
                }
            }
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample_annotations_roundtrip_without_changing_default_processing() {
        let mut edit = crate::EditState::default();
        edit.sample_points.push(ColorSamplePoint {
            name: "Skin reference".into(),
            position: [0.32, 0.48],
        });
        let loaded: crate::EditState =
            serde_json::from_str(&serde_json::to_string(&edit).unwrap()).unwrap();
        assert_eq!(loaded.sample_points, edit.sample_points);
        let mut old = serde_json::to_value(edit).unwrap();
        old.as_object_mut().unwrap().remove("sample_points");
        let loaded: crate::EditState = serde_json::from_value(old).unwrap();
        assert!(loaded.sample_points.is_empty());
    }
    #[test]
    fn neutral_reference_readouts_keep_hdr_and_d65_white() {
        let white = ColorReadout::from_linear(LinearRec2020([1.0; 3]));
        assert!((white.lab.0[0] - 100.0).abs() < 0.001);
        assert!(white.lab.0[1].abs() < 0.02 && white.lab.0[2].abs() < 0.02);
        let gray = ColorReadout::from_linear(LinearRec2020([0.18; 3]));
        assert!(gray.ev_from_middle_gray.unwrap().abs() < 1e-5);
        let hdr = ColorReadout::from_linear(LinearRec2020([4.0; 3]));
        assert!(hdr.rgb.0[0] > 1.0);
        assert!(hdr.lab.0[0] > 100.0);
    }
    #[test]
    fn scopes_account_for_every_sample_and_center_neutrals() {
        let image = RasterImage {
            width: 8,
            height: 4,
            linear_rec2020: vec![[0.5, 0.5, 0.5, 1.0]; 32],
            bits_per_channel: 16,
            embedded_icc: None,
        };
        for mode in [
            ScopeMode::Waveform,
            ScopeMode::Parade,
            ScopeMode::Vectorscope,
        ] {
            let scope = ScopeData::build(&image, mode, 0.0);
            let count: u32 = scope.counts.iter().flat_map(|v| v.iter()).sum();
            assert_eq!(count, if mode == ScopeMode::Parade { 96 } else { 32 });
            if mode == ScopeMode::Vectorscope {
                let index = scope.counts.iter().position(|v| v[0] > 0).unwrap();
                assert!(
                    (index % scope.width).abs_diff(128) <= 1
                        && (index / scope.width).abs_diff(128) <= 1
                );
            }
        }
        assert!((sample(&image, [0.25, 0.75]).unwrap().rgb.0[0] - 0.5).abs() < 1e-6);
    }
}
