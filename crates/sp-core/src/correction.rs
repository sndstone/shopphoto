//! Photographic source and localized defect corrections.
use serde::{Deserialize, Serialize};

/// Linear source channels: camera RGB for RAW, linear Rec.2020 for raster input.
/// A film-base sample is meaningful only for the source from which it was taken.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinearSourceRgb(pub [f32; 3]);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FilmNegative {
    pub enabled: bool,
    pub base: LinearSourceRgb,
    /// Optical density range represented by diffuse white, log10 units.
    pub density_range: f32,
    pub black_density: f32,
    pub gamma: f32,
}
impl Default for FilmNegative {
    fn default() -> Self {
        Self {
            enabled: false,
            base: LinearSourceRgb([1.0; 3]),
            density_range: 2.0,
            black_density: 0.0,
            gamma: 1.0,
        }
    }
}
impl FilmNegative {
    pub fn clamped(mut self) -> Self {
        for v in &mut self.base.0 {
            *v = finite(*v, 1.0).clamp(0.0001, 16.0);
        }
        self.density_range = finite(self.density_range, 2.0).clamp(0.1, 4.0);
        self.black_density = finite(self.black_density, 0.0).clamp(-1.0, 1.0);
        self.gamma = finite(self.gamma, 1.0).clamp(0.2, 5.0);
        self
    }
    /// Divide out the orange/clear base before logarithmic density inversion.
    /// Result stays unbounded above so downstream exposure/HDR can recover it.
    pub fn apply(self, input: LinearSourceRgb) -> LinearSourceRgb {
        if !self.enabled {
            return input;
        }
        let settings = self.clamped();
        LinearSourceRgb(std::array::from_fn(|c| {
            let transmission = (input.0[c] / settings.base.0[c]).max(1e-6);
            ((-transmission.log10() - settings.black_density) / settings.density_range)
                .max(0.0)
                .powf(1.0 / settings.gamma)
        }))
    }
}
fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LocalCorrection {
    pub red_eye: f32,
    pub moire: f32,
    pub moire_radius_px: f32,
}
impl Default for LocalCorrection {
    fn default() -> Self {
        Self {
            red_eye: 0.0,
            moire: 0.0,
            moire_radius_px: 4.0,
        }
    }
}
impl LocalCorrection {
    pub fn clamped(mut self) -> Self {
        self.red_eye = finite(self.red_eye, 0.0).clamp(0.0, 100.0);
        self.moire = finite(self.moire, 0.0).clamp(0.0, 100.0);
        self.moire_radius_px = finite(self.moire_radius_px, 4.0).clamp(1.0, 16.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn film_base_is_black_and_density_maps_monotonically_to_positive() {
        let negative = FilmNegative {
            enabled: true,
            base: LinearSourceRgb([0.8, 0.5, 0.25]),
            ..Default::default()
        };
        assert_eq!(negative.apply(negative.base), LinearSourceRgb([0.0; 3]));
        let middle = negative.apply(LinearSourceRgb(negative.base.0.map(|v| v * 0.1)));
        let white = negative.apply(LinearSourceRgb(negative.base.0.map(|v| v * 0.01)));
        for c in 0..3 {
            assert!((middle.0[c] - 0.5).abs() < 1e-6);
            assert!((white.0[c] - 1.0).abs() < 1e-6);
        }
        let original = LinearSourceRgb([-0.1, 0.2, 5.0]);
        assert_eq!(FilmNegative::default().apply(original), original);
    }
    #[test]
    fn malformed_film_controls_are_finite_and_old_sidecars_are_neutral() {
        let film = FilmNegative {
            base: LinearSourceRgb([0.0, f32::NAN, 100.0]),
            gamma: f32::INFINITY,
            ..Default::default()
        }
        .clamped();
        assert_eq!(film.base.0, [0.0001, 1.0, 16.0]);
        assert_eq!(film.gamma, 1.0);
        let mut json = serde_json::to_value(crate::EditState::default()).unwrap();
        json.as_object_mut().unwrap().remove("negative");
        let old: crate::EditState = serde_json::from_value(json).unwrap();
        assert_eq!(old.negative, FilmNegative::default());
    }
}

/// Working-RGB channel mixer, nine selective CMYK ranges, and a targeted warm
/// hue uniformity control. Neutral settings bypass all color conversions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhotographicColor {
    /// Output rows × input R/G/B coefficients; 1.0 means 100%.
    pub mixer: [[f32; 3]; 3],
    /// Reds, yellows, greens, cyans, blues, magentas, whites, neutrals, blacks.
    /// Each row is cyan/magenta/yellow/black adjustment, -100..=100.
    pub selective: [[f32; 4]; 9],
    pub relative: bool,
    pub skin: SkinColor,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SkinColor {
    pub hue_deg: f32,
    pub range_deg: f32,
    pub hue_uniformity: f32,
    pub chroma_uniformity: f32,
    pub target_chroma: f32,
}
impl Default for SkinColor {
    fn default() -> Self {
        Self {
            hue_deg: 55.0,
            range_deg: 30.0,
            hue_uniformity: 0.0,
            chroma_uniformity: 0.0,
            target_chroma: 0.12,
        }
    }
}
impl Default for PhotographicColor {
    fn default() -> Self {
        Self {
            mixer: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            selective: [[0.0; 4]; 9],
            relative: true,
            skin: Default::default(),
        }
    }
}
impl PhotographicColor {
    pub fn is_neutral(self) -> bool {
        self.mixer == Self::default().mixer
            && self.selective == [[0.0; 4]; 9]
            && self.skin.hue_uniformity == 0.0
            && self.skin.chroma_uniformity == 0.0
    }
    pub fn clamped(mut self) -> Self {
        for (row, values) in self.mixer.iter_mut().enumerate() {
            for (column, value) in values.iter_mut().enumerate() {
                *value = finite(*value, if row == column { 1.0 } else { 0.0 }).clamp(-2.0, 2.0);
            }
        }
        for values in &mut self.selective {
            for value in values {
                *value = finite(*value, 0.0).clamp(-100.0, 100.0);
            }
        }
        self.skin.hue_deg = finite(self.skin.hue_deg, 55.0).rem_euclid(360.0);
        self.skin.range_deg = finite(self.skin.range_deg, 30.0).clamp(2.0, 90.0);
        self.skin.hue_uniformity = finite(self.skin.hue_uniformity, 0.0).clamp(0.0, 100.0);
        self.skin.chroma_uniformity = finite(self.skin.chroma_uniformity, 0.0).clamp(0.0, 100.0);
        self.skin.target_chroma = finite(self.skin.target_chroma, 0.12).clamp(0.0, 0.4);
        self
    }
    pub fn packed(self) -> [[f32; 4]; 14] {
        let mut output = [[0.0; 4]; 14];
        for (row, values) in output.iter_mut().take(3).enumerate() {
            values[..3].copy_from_slice(&self.mixer[row]);
        }
        output[3..12].copy_from_slice(&self.selective);
        output[12] = [
            self.skin.hue_deg,
            self.skin.range_deg,
            self.skin.hue_uniformity,
            self.skin.chroma_uniformity,
        ];
        output[13] = [
            self.skin.target_chroma,
            f32::from(self.relative),
            f32::from(self.selective != [[0.0; 4]; 9]),
            f32::from(!self.is_neutral()),
        ];
        output
    }
    pub fn apply(self, input: crate::color::LinearRec2020) -> crate::color::LinearRec2020 {
        use crate::color::{mat3_apply, REC2020_TO_REC709};
        if self.is_neutral() {
            return input;
        }
        let settings = self.clamped();
        let mut rgb = mat3_apply(&settings.mixer, input.0).map(|v| v.max(0.0));
        if settings.selective != [[0.0; 4]; 9] {
            let color = mat3_apply(&REC2020_TO_REC709, rgb).map(|v| v.max(0.0));
            let high = color.into_iter().fold(0.0, f32::max);
            let low = color.into_iter().fold(f32::INFINITY, f32::min);
            let delta = high - low;
            let saturation = delta / high.max(1e-6);
            let hue = if delta < 1e-6 {
                0.0
            } else if high == color[0] {
                ((color[1] - color[2]) / delta).rem_euclid(6.0) * 60.0
            } else if high == color[1] {
                ((color[2] - color[0]) / delta + 2.0) * 60.0
            } else {
                ((color[0] - color[1]) / delta + 4.0) * 60.0
            };
            let y = color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722;
            let chromatic = smooth(0.05, 0.25, saturation);
            let white = smooth(0.35, 0.85, y) * (1.0 - chromatic);
            let black = (1.0 - smooth(0.02, 0.25, y)) * (1.0 - chromatic);
            let mut correction = [0.0; 4];
            for (band, values) in settings.selective.iter().enumerate() {
                let weight = match band {
                    6 => white,
                    7 => (1.0 - white - black) * (1.0 - chromatic),
                    8 => black,
                    _ => {
                        let difference =
                            (hue - band as f32 * 60.0 + 180.0).rem_euclid(360.0) - 180.0;
                        (1.0 - difference.abs() / 60.0).max(0.0) * chromatic
                    }
                };
                for c in 0..4 {
                    correction[c] += values[c] * weight * 0.01;
                }
            }
            let result = std::array::from_fn(|c| {
                (color[c]
                    - correction[c] * if settings.relative { color[c] } else { 1.0 }
                    - correction[3] * color[c])
                    .max(0.0)
            });
            rgb = mat3_apply(&LINEAR709_TO_2020, result);
        }
        if settings.skin.hue_uniformity > 0.0 || settings.skin.chroma_uniformity > 0.0 {
            let lab = to_oklab(rgb);
            let c = lab[1].hypot(lab[2]);
            let h = lab[2].atan2(lab[1]).to_degrees();
            let difference = (h - settings.skin.hue_deg + 180.0).rem_euclid(360.0) - 180.0;
            let weight =
                (1.0 - smooth(
                    settings.skin.range_deg * 0.5,
                    settings.skin.range_deg,
                    difference.abs(),
                )) * smooth(0.01, 0.04, c);
            let angle =
                (h - difference * weight * settings.skin.hue_uniformity * 0.01).to_radians();
            let chroma = c
                + (settings.skin.target_chroma - c)
                    * weight
                    * settings.skin.chroma_uniformity
                    * 0.01;
            rgb = from_oklab([lab[0], chroma * angle.cos(), chroma * angle.sin()]);
        }
        crate::color::LinearRec2020(rgb.map(|v| v.max(0.0)))
    }
}
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
const LINEAR709_TO_2020: [[f32; 3]; 3] = [
    [0.627404, 0.329283, 0.043313],
    [0.069097, 0.919540, 0.011362],
    [0.016391, 0.088013, 0.895595],
];
// Published Oklab reference coefficients, kept verbatim to match WGSL.
#[allow(clippy::excessive_precision)]
fn to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    use crate::color::mat3_apply;
    let rgb = mat3_apply(&crate::color::REC2020_TO_REC709, rgb).map(|v| v.max(0.0));
    let lms = mat3_apply(
        &[
            [0.4122214708, 0.5363325363, 0.0514459929],
            [0.2119034982, 0.6806995451, 0.1073969566],
            [0.0883024619, 0.2817188376, 0.6299787005],
        ],
        rgb,
    )
    .map(f32::cbrt);
    mat3_apply(
        &[
            [0.2104542553, 0.7936177850, -0.0040720468],
            [1.9779984951, -2.4285922050, 0.4505937099],
            [0.0259040371, 0.7827717662, -0.8086757660],
        ],
        lms,
    )
}
#[allow(clippy::excessive_precision)]
fn from_oklab(lab: [f32; 3]) -> [f32; 3] {
    use crate::color::mat3_apply;
    let lms = mat3_apply(
        &[
            [1.0, 0.3963377774, 0.2158037573],
            [1.0, -0.1055613458, -0.0638541728],
            [1.0, -0.0894841775, -1.2914855480],
        ],
        lab,
    )
    .map(|v| v * v * v);
    let rgb = mat3_apply(
        &[
            [4.0767416621, -3.3077115913, 0.2309699292],
            [-1.2684380046, 2.6097574011, -0.3413193965],
            [-0.0041960863, -0.7034186147, 1.7076147010],
        ],
        lms,
    );
    mat3_apply(&LINEAR709_TO_2020, rgb)
}

#[cfg(test)]
mod photographic_tests {
    use super::*;
    use crate::color::LinearRec2020;
    #[test]
    fn neutral_is_exact_and_channel_mixer_preserves_its_linear_coefficients() {
        let input = LinearRec2020([0.1, 0.2, 2.0]);
        assert_eq!(PhotographicColor::default().apply(input), input);
        let params = PhotographicColor {
            mixer: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            ..Default::default()
        };
        assert_eq!(params.apply(input), LinearRec2020([2.0, 0.2, 0.1]));
    }
    #[test]
    fn selective_red_leaves_neutral_and_blue_alone() {
        let mut params = PhotographicColor::default();
        params.selective[0][0] = 50.0;
        let red = LinearRec2020(crate::color::mat3_apply(
            &LINEAR709_TO_2020,
            [0.6, 0.02, 0.02],
        ));
        assert!(params.apply(red).0[0] < red.0[0] * 0.8);
        for input in [
            LinearRec2020([0.4; 3]),
            LinearRec2020(crate::color::mat3_apply(
                &LINEAR709_TO_2020,
                [0.02, 0.02, 0.6],
            )),
        ] {
            let actual = params.apply(input);
            for c in 0..3 {
                assert!((actual.0[c] - input.0[c]).abs() < 1e-5);
            }
        }
    }
    #[test]
    fn skin_uniformity_moves_hue_and_chroma_without_lightness_or_other_hues() {
        let params = PhotographicColor {
            skin: SkinColor {
                hue_uniformity: 80.0,
                chroma_uniformity: 60.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let input = from_oklab([
            0.6,
            0.1 * 45f32.to_radians().cos(),
            0.1 * 45f32.to_radians().sin(),
        ]);
        let output = to_oklab(params.apply(LinearRec2020(input)).0);
        assert!((output[0] - 0.6).abs() < 1e-5);
        assert!(output[2].atan2(output[1]).to_degrees() > 51.0);
        let blue = from_oklab([
            0.6,
            0.1 * 240f32.to_radians().cos(),
            0.1 * 240f32.to_radians().sin(),
        ]);
        let output = params.apply(LinearRec2020(blue));
        for (actual, expected) in output.0.into_iter().zip(blue) {
            assert!((actual - expected).abs() < 1e-5);
        }
    }
}
