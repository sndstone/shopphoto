//! Camera color profiles (BUILD.md §3.4).
//!
//! A [`CameraProfile`] describes how camera RGB becomes working-space color:
//! one or more illuminant-tagged matrices, optional hue/sat LUTs, and an
//! optional base tone curve. Tier 1 (Standard) profiles are built directly
//! from decoder metadata; Tier 2 (DCP) and Tier 3 (Reference Match) use the
//! same resolver and therefore react to every white-balance change.

use crate::color::Mat3;
use crate::edit::ToneCurve;
use serde::{Deserialize, Serialize};

/// Standard illuminants a profile matrix can be tagged with, ordered by
/// correlated color temperature. Matches the DNG calibration-illuminant set
/// we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Illuminant {
    /// Tungsten, ~2856 K (DNG: StdA).
    StdA,
    /// Horizon daylight ~5003 K (DNG: D50).
    D50,
    /// Daylight ~5500 K (DNG: D55).
    D55,
    /// Daylight ~6500 K (DNG: D65).
    D65,
}

impl Illuminant {
    /// Nominal correlated color temperature in kelvin.
    #[must_use]
    pub fn cct_kelvin(self) -> f32 {
        match self {
            Illuminant::StdA => 2_856.0,
            Illuminant::D50 => 5_003.0,
            Illuminant::D55 => 5_503.0,
            Illuminant::D65 => 6_504.0,
        }
    }
}

/// A color matrix valid under one illuminant: camera RGB → CIE XYZ.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IlluminantMatrix {
    pub illuminant: Illuminant,
    pub cam_to_xyz: Mat3,
}

/// A DNG HueSatDelta 3-D table (ProfileHueSatMapData / ProfileLookTable).
/// Entry order per the DNG spec: value-major, then hue, then sat
/// (`idx = (v * hue_divisions + h) * sat_divisions + s`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HueSatLut {
    pub hue_divisions: u32,
    pub sat_divisions: u32,
    pub val_divisions: u32,
    /// (hue_shift_deg, sat_scale, val_scale) triples.
    pub entries: Vec<(f32, f32, f32)>,
}

/// A regular RGB 3-D LUT. Entries are red-major, then green, then blue:
/// `idx = (r * size + g) * size + b`. Values are scene-linear Rec.2020.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lut3d {
    /// Edge length. Phase 3B Reference Match profiles use 33.
    pub size: u32,
    pub entries: Vec<[f32; 3]>,
}

impl Lut3d {
    /// Number of entries the edge length implies.
    #[must_use]
    pub fn expected_len(&self) -> usize {
        self.size.saturating_pow(3) as usize
    }

    /// Identity LUT of `size` cubed nodes.
    #[must_use]
    pub fn identity(size: u32) -> Self {
        let denom = size.saturating_sub(1).max(1) as f32;
        let mut entries = Vec::with_capacity(size.saturating_pow(3) as usize);
        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    entries.push([r as f32 / denom, g as f32 / denom, b as f32 / denom]);
                }
            }
        }
        Self { size, entries }
    }

    /// Trilinearly sample the LUT, clamping inputs to its unit cube.
    #[must_use]
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.size < 2 || self.entries.len() != self.expected_len() {
            return rgb;
        }
        let max = self.size - 1;
        let f = rgb.map(|v| v.clamp(0.0, 1.0) * max as f32);
        let lo = f.map(|v| v.floor() as u32);
        let hi = lo.map(|v| (v + 1).min(max));
        let t = [
            f[0] - lo[0] as f32,
            f[1] - lo[1] as f32,
            f[2] - lo[2] as f32,
        ];
        let at =
            |r: u32, g: u32, b: u32| self.entries[((r * self.size + g) * self.size + b) as usize];
        let mix =
            |a: [f32; 3], b: [f32; 3], w: f32| std::array::from_fn(|i| a[i] * (1.0 - w) + b[i] * w);
        let c00 = mix(at(lo[0], lo[1], lo[2]), at(lo[0], lo[1], hi[2]), t[2]);
        let c01 = mix(at(lo[0], hi[1], lo[2]), at(lo[0], hi[1], hi[2]), t[2]);
        let c10 = mix(at(hi[0], lo[1], lo[2]), at(hi[0], lo[1], hi[2]), t[2]);
        let c11 = mix(at(hi[0], hi[1], lo[2]), at(hi[0], hi[1], hi[2]), t[2]);
        mix(mix(c00, c01, t[1]), mix(c10, c11, t[1]), t[0])
    }

    /// Compose a profile RGB LUT followed by a creative RGB LUT.
    #[must_use]
    pub fn compose(base: Option<&Self>, look: Option<&Self>) -> Option<Self> {
        match (base, look) {
            (None, None) => None,
            (Some(lut), None) | (None, Some(lut)) => Some(lut.clone()),
            (Some(base), Some(look)) => {
                let size = base.size.max(look.size).clamp(2, 65);
                let denominator = (size - 1) as f32;
                let mut entries = Vec::with_capacity(size.saturating_pow(3) as usize);
                for red in 0..size {
                    for green in 0..size {
                        for blue in 0..size {
                            let rgb = [
                                red as f32 / denominator,
                                green as f32 / denominator,
                                blue as f32 / denominator,
                            ];
                            entries.push(look.sample(base.sample(rgb)));
                        }
                    }
                }
                Some(Self { size, entries })
            }
        }
    }
}

impl HueSatLut {
    /// Number of entries the dims imply.
    #[must_use]
    pub fn expected_len(&self) -> usize {
        self.hue_divisions
            .checked_mul(self.sat_divisions)
            .and_then(|n| n.checked_mul(self.val_divisions))
            .map_or(usize::MAX, |n| n as usize)
    }

    /// Bound imported table shapes before indexing or uploading them.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        (1..=360).contains(&self.hue_divisions)
            && (2..=256).contains(&self.sat_divisions)
            && (1..=256).contains(&self.val_divisions)
            && self.expected_len() <= 4_000_000
            && self.entries.len() == self.expected_len()
            && self
                .entries
                .iter()
                .all(|&(h, s, v)| h.is_finite() && s.is_finite() && v.is_finite())
    }

    /// Linear blend of two same-shaped tables (dual-illuminant resolve).
    /// Returns `a`'s clone when shapes differ (defensive).
    #[must_use]
    pub fn lerp(a: &HueSatLut, b: &HueSatLut, w: f32) -> HueSatLut {
        if a.hue_divisions != b.hue_divisions
            || a.sat_divisions != b.sat_divisions
            || a.val_divisions != b.val_divisions
            || a.entries.len() != b.entries.len()
        {
            return a.clone();
        }
        let w = w.clamp(0.0, 1.0);
        HueSatLut {
            hue_divisions: a.hue_divisions,
            sat_divisions: a.sat_divisions,
            val_divisions: a.val_divisions,
            entries: a
                .entries
                .iter()
                .zip(&b.entries)
                .map(|(ea, eb)| {
                    (
                        ea.0 * (1.0 - w) + eb.0 * w,
                        ea.1 * (1.0 - w) + eb.1 * w,
                        ea.2 * (1.0 - w) + eb.2 * w,
                    )
                })
                .collect(),
        }
    }

    /// Compose two DNG-style delta tables. The second table is resampled to
    /// the first table's grid; hue offsets add and saturation/value scales
    /// multiply. This keeps camera-profile and creative-look processing in
    /// one GPU pass.
    #[must_use]
    pub fn compose(base: Option<&HueSatLut>, look: Option<&HueSatLut>) -> Option<HueSatLut> {
        match (base, look) {
            (None, None) => None,
            (Some(value), None) | (None, Some(value)) => Some(value.clone()),
            (Some(base), Some(look)) => {
                if base.entries.len() != base.expected_len() {
                    return Some(look.clone());
                }
                let mut out = base.clone();
                for v in 0..base.val_divisions {
                    for h in 0..base.hue_divisions {
                        for s in 0..base.sat_divisions {
                            let idx =
                                ((v * base.hue_divisions + h) * base.sat_divisions + s) as usize;
                            let hue = h as f32 / base.hue_divisions.max(1) as f32;
                            let sat = s as f32 / base.sat_divisions.saturating_sub(1).max(1) as f32;
                            let val = v as f32 / base.val_divisions.saturating_sub(1).max(1) as f32;
                            let (lh, ls, lv) = look.sample_nearest(hue, sat, val);
                            out.entries[idx].0 += lh;
                            out.entries[idx].1 *= ls;
                            out.entries[idx].2 *= lv;
                        }
                    }
                }
                Some(out)
            }
        }
    }

    /// Nearest-neighbor sample on normalized HSV coordinates.
    #[must_use]
    pub fn sample_nearest(&self, hue: f32, sat: f32, val: f32) -> (f32, f32, f32) {
        if self.entries.len() != self.expected_len() || self.entries.is_empty() {
            return (0.0, 1.0, 1.0);
        }
        let h = ((hue.rem_euclid(1.0) * self.hue_divisions as f32).round() as u32)
            % self.hue_divisions.max(1);
        let s = (sat.clamp(0.0, 1.0) * self.sat_divisions.saturating_sub(1) as f32).round() as u32;
        let v = (val.clamp(0.0, 1.0) * self.val_divisions.saturating_sub(1) as f32).round() as u32;
        self.entries[((v * self.hue_divisions + h) * self.sat_divisions + s) as usize]
    }
}

/// A creative look applied after the camera profile. A look curve replaces
/// the profile/base curve when present; it is never applied in addition to
/// another base curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub name: String,
    pub huesat: Option<HueSatLut>,
    pub base_curve: Option<ToneCurve>,
    /// Multiplicative saturation scale (1.0 is neutral).
    pub base_saturation: f32,
}

impl Look {
    pub const NAMES: [&'static str; 6] = [
        "Landscape",
        "Portrait",
        "Product",
        "Vibrant",
        "Muted",
        "Flat",
    ];

    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "Landscape" => Some(Self::landscape()),
            "Portrait" => Some(Self::portrait()),
            "Product" => Some(Self::product()),
            "Vibrant" => Some(Self::vibrant()),
            "Muted" => Some(Self::muted()),
            "Flat" => Some(Self::flat()),
            _ => None,
        }
    }

    /// Explicit no-op look, equivalent to `EditState.look == None`.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            name: "Standard".to_owned(),
            huesat: None,
            base_curve: None,
            base_saturation: 1.0,
        }
    }

    fn generated(
        name: &str,
        saturation: f32,
        curve: Option<ToneCurve>,
        adjustment: impl Fn(f32, f32, f32) -> (f32, f32, f32),
    ) -> Self {
        let (hue_divisions, sat_divisions, val_divisions) = (36, 8, 4);
        let mut entries =
            Vec::with_capacity((hue_divisions * sat_divisions * val_divisions) as usize);
        for v in 0..val_divisions {
            for h in 0..hue_divisions {
                for s in 0..sat_divisions {
                    entries.push(adjustment(
                        h as f32 * 360.0 / hue_divisions as f32,
                        s as f32 / (sat_divisions - 1) as f32,
                        v as f32 / (val_divisions - 1) as f32,
                    ));
                }
            }
        }
        Self {
            name: name.to_owned(),
            huesat: Some(HueSatLut {
                hue_divisions,
                sat_divisions,
                val_divisions,
                entries,
            }),
            base_curve: curve,
            base_saturation: saturation,
        }
    }

    fn band(hue: f32, center: f32, half_width: f32) -> f32 {
        let distance = ((hue - center + 180.0).rem_euclid(360.0) - 180.0).abs();
        if distance >= half_width {
            0.0
        } else {
            0.5 + 0.5 * (std::f32::consts::PI * distance / half_width).cos()
        }
    }

    fn curve(points: &[(f32, f32)]) -> ToneCurve {
        ToneCurve {
            points: points
                .iter()
                .map(|&(x, y)| crate::edit::CurvePoint { x, y })
                .collect(),
        }
    }

    #[must_use]
    pub fn landscape() -> Self {
        Self::generated(
            "Landscape",
            1.0,
            Some(Self::curve(&[
                (0.0, 0.0),
                (0.05, 0.055),
                (0.15, 0.20),
                (0.35, 0.47),
                (0.60, 0.74),
                (0.85, 0.925),
                (1.0, 1.0),
            ])),
            |h, _s, v| {
                let green = Self::band(h, 125.0, 70.0);
                let blue = Self::band(h, 220.0, 75.0);
                let skin = Self::band(h, 42.0, 28.0);
                let cool = (green + blue).min(1.0) * (1.0 - skin);
                let sky_protect = blue * v * v;
                (
                    -4.0 * Self::band(h, 85.0, 35.0),
                    1.0 + 0.07 * cool,
                    1.0 - 0.02 * sky_protect,
                )
            },
        )
    }

    #[must_use]
    pub fn portrait() -> Self {
        Self::generated(
            "Portrait",
            0.99,
            Some(Self::curve(&[
                (0.0, 0.008),
                (0.05, 0.07),
                (0.15, 0.22),
                (0.35, 0.47),
                (0.60, 0.71),
                (0.85, 0.90),
                (1.0, 0.99),
            ])),
            |h, _s, _v| (0.0, 1.0 - 0.05 * Self::band(h, 42.0, 42.0), 1.0),
        )
    }

    /// Color-faithful product rendering. The LUT gently reins in already
    /// saturated colors while leaving hue and luminance untouched, avoiding
    /// the synthetic color boost that makes catalog work harder to match.
    #[must_use]
    pub fn product() -> Self {
        Self::generated(
            "Product",
            1.0,
            Some(Self::curve(&[
                (0.0, 0.0),
                (0.05, 0.05),
                (0.15, 0.195),
                (0.35, 0.45),
                (0.60, 0.72),
                (0.85, 0.92),
                (1.0, 1.0),
            ])),
            |_h, s, _v| (0.0, 1.0 - 0.05 * s, 1.0),
        )
    }

    #[must_use]
    pub fn vibrant() -> Self {
        Self::generated(
            "Vibrant",
            1.0,
            Some(Self::curve(&[
                (0.0, 0.0),
                (0.05, 0.052),
                (0.15, 0.19),
                (0.35, 0.47),
                (0.60, 0.74),
                (0.85, 0.93),
                (1.0, 1.0),
            ])),
            |_h, s, _v| (0.0, 1.0 + 0.07 * (1.0 - s * 0.8), 1.0),
        )
    }

    #[must_use]
    pub fn muted() -> Self {
        Self::generated(
            "Muted",
            0.95,
            Some(Self::curve(&[
                (0.0, 0.01),
                (0.05, 0.08),
                (0.15, 0.22),
                (0.35, 0.46),
                (0.60, 0.70),
                (0.85, 0.89),
                (1.0, 0.985),
            ])),
            |_h, _s, _v| (0.0, 0.98, 1.0),
        )
    }

    #[must_use]
    pub fn flat() -> Self {
        Self::generated(
            "Flat",
            0.96,
            Some(Self::curve(&[
                (0.0, 0.015),
                (0.05, 0.085),
                (0.15, 0.23),
                (0.35, 0.46),
                (0.60, 0.69),
                (0.85, 0.88),
                (1.0, 0.975),
            ])),
            |_h, _s, _v| (0.0, 0.985, 1.0),
        )
    }
}

/// A 2-D chromaticity warp: the luma-invariant half of a colour rendering.
///
/// Colour is corrected as a function of **chromaticity alone**. A pixel's
/// linear Rec.2020 RGB is split into luma `Y` and its position on the
/// normalized-RGB chromaticity plane
///
/// ```text
/// n = RGB / (R + G + B)      coordinates (n_r, n_b), with n_g = 1 - n_r - n_b
/// ```
///
/// which is invariant under exposure: scaling RGB scales the sum equally.
/// The warped chromaticity is looked up bilinearly and rescaled to the
/// pixel's original `Y`, so the stage never changes luminance — only hue and
/// saturation, identically in shadows and highlights.
///
/// That is the property a 3-D RGB cube cannot offer. A cube must spend most
/// of its nodes representing the luma axis and its correction drifts as
/// exposure moves, which is why [`Lut3d`] is a residual polish and this is a
/// rendering.
///
/// Normalized RGB rather than a `(B−Y)/Y`-style chroma pair specifically
/// because it is **bounded**: every physical colour lands inside the unit
/// square regardless of how dark or saturated it is, so the grid never has
/// to clamp a real colour to its edge, and reconstruction divides by a luma
/// that cannot approach zero. The formulation is ShopPhoto's own clean-room
/// design; profiles are fitted only from paired RAW/reference renders the
/// user supplies.
///
/// Near neutral, chromaticity is dominated by sensor noise and a correction
/// there would produce coloured mottle, so the effect fades out within
/// [`Self::gray_threshold`] of the neutral point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromaWarp {
    /// Grid dimensions along the red and blue chromaticity axes. Both ≥ 2.
    pub size_r: u32,
    pub size_b: u32,
    /// The chromaticity range the grid spans.
    pub r_min: f32,
    pub r_max: f32,
    pub b_min: f32,
    pub b_max: f32,
    /// `size_r * size_b` warped `(n_r, n_b)` pairs, **red varying fastest**
    /// (index `b * size_r + r`).
    pub entries: Vec<[f32; 2]>,
    /// Distance from the neutral point below which correction fades out.
    pub gray_threshold: f32,
}

/// The neutral point on the normalized-RGB plane: equal parts R, G and B.
pub const CHROMA_NEUTRAL: f32 = 1.0 / 3.0;
/// Default grid resolution for a fitted warp, per axis.
pub const CHROMA_WARP_SIZE: u32 = 33;
/// Default fade-in radius around the neutral point.
pub const CHROMA_WARP_GRAY: f32 = 0.012;

impl ChromaWarp {
    /// A warp that maps every chromaticity to itself, spanning the whole
    /// unit square.
    #[must_use]
    pub fn identity(size: u32) -> Self {
        let size = size.max(2);
        let node = |i: u32| i as f32 / (size - 1) as f32;
        let mut entries = Vec::with_capacity((size * size) as usize);
        for b in 0..size {
            for r in 0..size {
                entries.push([node(r), node(b)]);
            }
        }
        Self {
            size_r: size,
            size_b: size,
            r_min: 0.0,
            r_max: 1.0,
            b_min: 0.0,
            b_max: 1.0,
            entries,
            gray_threshold: CHROMA_WARP_GRAY,
        }
    }

    /// Number of entries a well-formed grid holds.
    #[must_use]
    pub fn expected_len(&self) -> usize {
        (self.size_r as usize) * (self.size_b as usize)
    }

    /// Whether the grid is structurally usable.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.size_r >= 2
            && self.size_b >= 2
            && self.r_max > self.r_min
            && self.b_max > self.b_min
            && self.entries.len() == self.expected_len()
    }

    /// Bilinearly sample the warp at chromaticity `(n_r, n_b)`, clamping to
    /// the grid edge.
    ///
    /// CPU mirror of the WGSL in `shaders/wb_color.wgsl`. `sp-pipeline`'s
    /// `cpu_chroma_warp_mirror_agrees_with_the_shader_conventions` checks
    /// that both read the grid with the same axis order and neutral point.
    #[must_use]
    pub fn sample(&self, n_r: f32, n_b: f32) -> [f32; 2] {
        debug_assert!(self.is_valid());
        let axis = |v: f32, min: f32, max: f32, n: u32| {
            let t = (((v - min) / (max - min)) * (n - 1) as f32).clamp(0.0, (n - 1) as f32);
            let i0 = t.floor() as u32;
            (i0, (i0 + 1).min(n - 1), t - i0 as f32)
        };
        let (r0, r1, fr) = axis(n_r, self.r_min, self.r_max, self.size_r);
        let (b0, b1, fb) = axis(n_b, self.b_min, self.b_max, self.size_b);
        let at = |r: u32, b: u32| self.entries[(b * self.size_r + r) as usize];
        let (c00, c10, c01, c11) = (at(r0, b0), at(r1, b0), at(r0, b1), at(r1, b1));
        let mut out = [0.0; 2];
        for k in 0..2 {
            let top = c00[k] + (c10[k] - c00[k]) * fr;
            let bottom = c01[k] + (c11[k] - c01[k]) * fr;
            out[k] = top + (bottom - top) * fb;
        }
        out
    }

    /// Blend two warps of identical shape. Used to interpolate between
    /// per-illuminant renderings by white-balance temperature.
    #[must_use]
    pub fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        if a.size_r != b.size_r || a.size_b != b.size_b || !a.is_valid() || !b.is_valid() {
            return a.clone();
        }
        let t = t.clamp(0.0, 1.0);
        let mix = |x: f32, y: f32| x + (y - x) * t;
        Self {
            size_r: a.size_r,
            size_b: a.size_b,
            r_min: mix(a.r_min, b.r_min),
            r_max: mix(a.r_max, b.r_max),
            b_min: mix(a.b_min, b.b_min),
            b_max: mix(a.b_max, b.b_max),
            entries: a
                .entries
                .iter()
                .zip(&b.entries)
                .map(|(p, q)| [mix(p[0], q[0]), mix(p[1], q[1])])
                .collect(),
            gray_threshold: mix(a.gray_threshold, b.gray_threshold),
        }
    }
}

/// A complete camera color profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraProfile {
    /// Display name ("ShopPhoto Standard", "Adobe Standard (X2D)", …).
    pub name: String,
    /// Camera model string this profile applies to (EXIF `Model`).
    pub camera_model: String,
    /// At least one entry. Sorted by CCT ascending.
    pub matrices: Vec<IlluminantMatrix>,
    /// Optional DNG ForwardMatrix1/2 values, camera RGB → XYZ D50. When
    /// present these are preferred to inverse ColorMatrix values.
    #[serde(default)]
    pub forward_matrices: Vec<IlluminantMatrix>,
    /// HueSat map for the FIRST (lowest-CCT) illuminant.
    pub huesat1: Option<HueSatLut>,
    /// HueSat map for the LAST illuminant (dual-illuminant profiles).
    pub huesat2: Option<HueSatLut>,
    /// DNG ProfileLookTable (applied after the hue/sat map, not
    /// WB-interpolated).
    pub look: Option<HueSatLut>,
    /// General scene-linear RGB LUT used by fitted Reference Match profiles.
    #[serde(default)]
    pub rgb_lut: Option<Lut3d>,
    /// Chroma-plane warp for the FIRST (lowest-CCT) illuminant, applied with
    /// the camera matrix before tone.
    #[serde(default)]
    pub chroma_warp1: Option<ChromaWarp>,
    /// Chroma-plane warp for the LAST illuminant. Interpolated against
    /// `chroma_warp1` by WB temperature, like the hue/sat maps.
    #[serde(default)]
    pub chroma_warp2: Option<ChromaWarp>,
    /// Base look curve applied before user tone controls. `None` = linear.
    pub base_curve: Option<ToneCurve>,
}

impl CameraProfile {
    /// Validate imported profiles before resolving matrices or sampling tables.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.matrices.is_empty()
            && self
                .matrices
                .iter()
                .chain(&self.forward_matrices)
                .all(|m| m.cam_to_xyz.iter().flatten().all(|v| v.is_finite()))
            && [&self.huesat1, &self.huesat2, &self.look]
                .into_iter()
                .flatten()
                .all(HueSatLut::is_valid)
            && self.rgb_lut.as_ref().is_none_or(|lut| {
                (2..=65).contains(&lut.size)
                    && lut.entries.len() == lut.expected_len()
                    && lut.entries.iter().flatten().all(|v| v.is_finite())
            })
            && [&self.chroma_warp1, &self.chroma_warp2]
                .into_iter()
                .flatten()
                .all(|warp| {
                    (2..=256).contains(&warp.size_r)
                        && (2..=256).contains(&warp.size_b)
                        && warp.is_valid()
                        && [
                            warp.r_min,
                            warp.r_max,
                            warp.b_min,
                            warp.b_max,
                            warp.gray_threshold,
                        ]
                        .iter()
                        .all(|v| v.is_finite())
                        && warp.gray_threshold >= 0.0
                        && warp.entries.iter().flatten().all(|v| v.is_finite())
                })
            && self.base_curve.as_ref().is_none_or(|curve| {
                curve.points.len() >= 2
                    && curve.points.len() <= 4096
                    && curve
                        .points
                        .iter()
                        .all(|p| p.x.is_finite() && p.y.is_finite())
                    && curve.points.windows(2).all(|p| p[0].x < p[1].x)
            })
    }

    /// Tier-1 profile from decoder metadata: single matrix, no LUTs.
    #[must_use]
    pub fn standard(camera_model: impl Into<String>, cam_to_xyz: Mat3) -> Self {
        Self {
            name: "ShopPhoto Standard".to_owned(),
            camera_model: camera_model.into(),
            matrices: vec![IlluminantMatrix {
                illuminant: Illuminant::D65,
                cam_to_xyz,
            }],
            forward_matrices: Vec::new(),
            huesat1: None,
            huesat2: None,
            look: None,
            rgb_lut: None,
            chroma_warp1: None,
            chroma_warp2: None,
            base_curve: None,
        }
    }

    /// Resolve the matrix for a white-balance temperature by interpolating
    /// between the two nearest illuminant matrices (linear in 1/CCT, the
    /// DNG-spec method). With a single matrix this returns it unchanged.
    #[must_use]
    pub fn resolve_matrix(&self, wb_temp_kelvin: f32) -> Mat3 {
        let matrices = if self.forward_matrices.is_empty() {
            &self.matrices
        } else {
            &self.forward_matrices
        };
        match matrices.as_slice() {
            [] => unreachable!("CameraProfile invariant: at least one matrix"),
            [only] => only.cam_to_xyz,
            list => {
                let t = 1.0 / wb_temp_kelvin.clamp(1_000.0, 100_000.0);
                // Find bracketing pair in 1/CCT space (list sorted by CCT asc
                // ⇒ descending in 1/CCT).
                let lo = list.first().expect("non-empty");
                let hi = list.last().expect("non-empty");
                let (a, b) = list
                    .windows(2)
                    .map(|w| (&w[0], &w[1]))
                    .find(|(a, b)| {
                        let (ia, ib) = (
                            1.0 / a.illuminant.cct_kelvin(),
                            1.0 / b.illuminant.cct_kelvin(),
                        );
                        t <= ia && t >= ib
                    })
                    .unwrap_or_else(|| {
                        if t > 1.0 / lo.illuminant.cct_kelvin() {
                            (lo, lo)
                        } else {
                            (hi, hi)
                        }
                    });
                let (ia, ib) = (
                    1.0 / a.illuminant.cct_kelvin(),
                    1.0 / b.illuminant.cct_kelvin(),
                );
                let w = if (ia - ib).abs() < f32::EPSILON {
                    0.0
                } else {
                    ((ia - t) / (ia - ib)).clamp(0.0, 1.0)
                };
                let mut out = [[0.0f32; 3]; 3];
                for (r, row) in out.iter_mut().enumerate() {
                    for (c, cell) in row.iter_mut().enumerate() {
                        *cell = a.cam_to_xyz[r][c] * (1.0 - w) + b.cam_to_xyz[r][c] * w;
                    }
                }
                out
            }
        }
    }
}

/// Fully resolved profile parameters for one white-balance temperature.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedColorParams {
    pub cam_to_xyz: Mat3,
    /// True when `cam_to_xyz` came from DNG ForwardMatrix data: it then
    /// expects WHITE-BALANCED camera values and maps neutral to D50 white
    /// (no further white normalization needed — only D50→D65 adaptation).
    pub matrix_is_forward: bool,
    pub huesat: Option<HueSatLut>,
    pub look: Option<HueSatLut>,
    pub rgb_lut: Option<Lut3d>,
    pub chroma_warp: Option<ChromaWarp>,
    pub base_curve: Option<ToneCurve>,
}

impl CameraProfile {
    /// Interpolation weight toward the LAST illuminant for a WB temp
    /// (linear in 1/CCT between the first and last matrices).
    ///
    /// Reads the same list `resolve_matrix` interpolates — forward matrices
    /// when present — so the LUTs and the matrix are always weighted against
    /// one set of illuminants. They can differ in count and CCT, and
    /// weighting the LUTs by the other list silently desynchronises them.
    #[must_use]
    pub fn illuminant_weight(&self, wb_temp_kelvin: f32) -> f32 {
        let matrices = if self.forward_matrices.is_empty() {
            &self.matrices
        } else {
            &self.forward_matrices
        };
        let (Some(first), Some(last)) = (matrices.first(), matrices.last()) else {
            return 0.0;
        };
        let (ia, ib) = (
            1.0 / first.illuminant.cct_kelvin(),
            1.0 / last.illuminant.cct_kelvin(),
        );
        if (ia - ib).abs() < f32::EPSILON {
            return 0.0;
        }
        let t = 1.0 / wb_temp_kelvin.clamp(1_000.0, 100_000.0);
        ((ia - t) / (ia - ib)).clamp(0.0, 1.0)
    }

    /// The hue/sat map for a WB temperature (dual maps lerped; single map
    /// passed through).
    #[must_use]
    pub fn resolve_huesat(&self, wb_temp_kelvin: f32) -> Option<HueSatLut> {
        match (&self.huesat1, &self.huesat2) {
            (Some(a), Some(b)) => Some(HueSatLut::lerp(
                a,
                b,
                self.illuminant_weight(wb_temp_kelvin),
            )),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        }
    }

    /// The chroma warp for a WB temperature, blended between illuminants the
    /// same way [`Self::resolve_huesat`] blends the hue/sat maps.
    #[must_use]
    pub fn resolve_chroma_warp(&self, wb_temp_kelvin: f32) -> Option<ChromaWarp> {
        match (&self.chroma_warp1, &self.chroma_warp2) {
            (Some(a), Some(b)) => Some(ChromaWarp::lerp(
                a,
                b,
                self.illuminant_weight(wb_temp_kelvin),
            )),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        }
        .filter(ChromaWarp::is_valid)
    }

    /// Resolve every WB-dependent part of this profile for one render.
    #[must_use]
    pub fn resolve(&self, wb_temp_kelvin: f32) -> ResolvedColorParams {
        ResolvedColorParams {
            cam_to_xyz: self.resolve_matrix(wb_temp_kelvin),
            matrix_is_forward: !self.forward_matrices.is_empty(),
            huesat: self.resolve_huesat(wb_temp_kelvin),
            look: self.look.clone(),
            rgb_lut: self.rgb_lut.clone(),
            chroma_warp: self.resolve_chroma_warp(wb_temp_kelvin),
            base_curve: self.base_curve.clone(),
        }
    }
}

/// Bradford chromatic adaptation, D50 → D65 (standard published values).
pub const BRADFORD_D50_TO_D65: Mat3 = [
    [0.955_576_6, -0.023_039_3, 0.063_163_6],
    [-0.028_289_5, 1.009_941_6, 0.021_007_7],
    [0.012_298_2, -0.020_483, 1.329_909_8],
];

/// CIE D65 white point (Y = 1).
pub const D65_XYZ: [f32; 3] = [0.950_47, 1.0, 1.088_83];

/// The matrix actually used to render WHITE-BALANCED camera RGB to XYZ(D65).
///
/// A raw inverse ColorMatrix maps UNBALANCED camera values, so feeding it
/// WB'd values double-corrects the illuminant (magenta casts). The standard
/// dcraw/DNG remedy: scale its columns by the camera-space D65 white so the
/// balanced neutral axis maps exactly to D65 white. ForwardMatrix data is
/// already white-preserving to D50 and only needs Bradford adaptation.
#[must_use]
pub fn display_cam_to_xyz(cam_to_xyz: &Mat3, matrix_is_forward: bool) -> Mat3 {
    use crate::color::{mat3_apply, mat3_inverse, mat3_mul};
    if matrix_is_forward {
        return mat3_mul(&BRADFORD_D50_TO_D65, cam_to_xyz);
    }
    let Some(xyz_to_cam) = mat3_inverse(cam_to_xyz) else {
        return *cam_to_xyz; // singular metadata; degrade gracefully
    };
    let white_cam = mat3_apply(&xyz_to_cam, D65_XYZ);
    if white_cam.iter().any(|v| *v <= 1e-6) {
        return *cam_to_xyz;
    }
    let mut m = *cam_to_xyz;
    for row in &mut m {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell *= white_cam[c];
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    fn scaled(s: f32) -> Mat3 {
        [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]]
    }

    #[test]
    fn imported_profile_validation_covers_all_tables_and_nonfinite_values() {
        let profile = CameraProfile::standard("test", ID);
        assert!(profile.is_valid());
        let mut bad = profile.clone();
        bad.matrices[0].cam_to_xyz[1][1] = f32::NAN;
        assert!(!bad.is_valid());
        bad = profile.clone();
        bad.forward_matrices = vec![IlluminantMatrix {
            illuminant: Illuminant::D50,
            cam_to_xyz: [[f32::INFINITY; 3]; 3],
        }];
        assert!(!bad.is_valid());
        for dimensions in [[u32::MAX, u32::MAX, u32::MAX], [2, 2, 0], [2, 1, 1]] {
            bad = profile.clone();
            bad.huesat1 = Some(HueSatLut {
                hue_divisions: dimensions[0],
                sat_divisions: dimensions[1],
                val_divisions: dimensions[2],
                entries: vec![],
            });
            assert!(!bad.is_valid());
        }
        bad = profile.clone();
        bad.look = Some(HueSatLut {
            hue_divisions: 1,
            sat_divisions: 2,
            val_divisions: 1,
            entries: vec![(0.0, 1.0, f32::NAN); 2],
        });
        assert!(!bad.is_valid());
        bad = profile.clone();
        let mut warp = ChromaWarp::identity(2);
        warp.entries[0][0] = f32::INFINITY;
        bad.chroma_warp2 = Some(warp);
        assert!(!bad.is_valid());
        bad = profile;
        bad.base_curve = Some(ToneCurve {
            points: vec![
                crate::edit::CurvePoint {
                    x: f32::NAN,
                    y: 0.0
                };
                2
            ],
        });
        assert!(!bad.is_valid());
    }

    #[test]
    fn single_matrix_resolves_to_itself() {
        let p = CameraProfile::standard("X2D 100C", ID);
        assert_eq!(p.resolve_matrix(3_000.0), ID);
        assert_eq!(p.resolve_matrix(20_000.0), ID);
    }

    #[test]
    fn dual_illuminant_interpolates_and_clamps() {
        let p = CameraProfile {
            name: "test".into(),
            camera_model: "test".into(),
            matrices: vec![
                IlluminantMatrix {
                    illuminant: Illuminant::StdA,
                    cam_to_xyz: scaled(1.0),
                },
                IlluminantMatrix {
                    illuminant: Illuminant::D65,
                    cam_to_xyz: scaled(2.0),
                },
            ],
            forward_matrices: Vec::new(),
            huesat1: None,
            huesat2: None,
            look: None,
            rgb_lut: None,
            chroma_warp1: None,
            chroma_warp2: None,
            base_curve: None,
        };
        // At the endpoints, exact.
        assert!((p.resolve_matrix(2_856.0)[0][0] - 1.0).abs() < 1e-4);
        assert!((p.resolve_matrix(6_504.0)[0][0] - 2.0).abs() < 1e-4);
        // Beyond the endpoints, clamped.
        assert!((p.resolve_matrix(2_000.0)[0][0] - 1.0).abs() < 1e-4);
        assert!((p.resolve_matrix(50_000.0)[0][0] - 2.0).abs() < 1e-4);
        // In between, strictly between.
        let mid = p.resolve_matrix(4_500.0)[0][0];
        assert!(mid > 1.0 && mid < 2.0, "mid = {mid}");
    }

    #[test]
    fn identity_rgb_lut_trilinear_sample() {
        let lut = Lut3d::identity(5);
        let input = [0.13, 0.57, 0.91];
        let output = lut.sample(input);
        for channel in 0..3 {
            assert!((input[channel] - output[channel]).abs() < 1e-6);
        }
    }

    #[test]
    fn display_matrix_maps_balanced_white_to_d65() {
        use crate::color::mat3_apply;
        // Hasselblad X2D XYZ→cam reference values.
        let xyz_to_cam: Mat3 = [
            [0.6468, -0.1899, -0.0545],
            [-0.4526, 1.2267, 0.2542],
            [-0.0388, 0.1276, 0.6096],
        ];
        let cam_to_xyz = crate::color::mat3_inverse(&xyz_to_cam).unwrap();
        let m = display_cam_to_xyz(&cam_to_xyz, false);
        let white = mat3_apply(&m, [1.0, 1.0, 1.0]);
        for (got, want) in white.iter().zip(D65_XYZ) {
            assert!((got - want).abs() < 2e-3, "white {white:?}");
        }
    }

    #[test]
    fn display_matrix_forward_gets_bradford_only() {
        use crate::color::mat3_apply;
        // A white-preserving-to-D50 forward matrix (diagonal: row sums are
        // exactly the D50 components, so FM · (1,1,1) = D50 white).
        let d50 = [0.9642f32, 1.0, 0.8249];
        let fm: Mat3 = [[d50[0], 0.0, 0.0], [0.0, d50[1], 0.0], [0.0, 0.0, d50[2]]];
        let m = display_cam_to_xyz(&fm, true);
        let white = mat3_apply(&m, [1.0, 1.0, 1.0]);
        for (got, want) in white.iter().zip(D65_XYZ) {
            assert!((got - want).abs() < 5e-3, "white {white:?}");
        }
    }

    #[test]
    fn forward_matrices_are_preferred() {
        let mut profile = CameraProfile::standard("camera", scaled(0.5));
        profile.forward_matrices = vec![IlluminantMatrix {
            illuminant: Illuminant::D65,
            cam_to_xyz: scaled(1.25),
        }];
        assert_eq!(profile.resolve_matrix(6504.0), scaled(1.25));
    }

    #[test]
    fn standard_look_is_an_explicit_no_op() {
        let look = Look::standard();
        assert!(look.huesat.is_none());
        assert!(look.base_curve.is_none());
        assert_eq!(look.base_saturation, 1.0);
    }

    #[test]
    fn landscape_targets_cool_colors_and_protects_skin() {
        let look = Look::landscape();
        let lut = look.huesat.as_ref().expect("landscape table");
        let (_, green_sat, _) = lut.sample_nearest(125.0 / 360.0, 0.5, 0.5);
        let (_, skin_sat, _) = lut.sample_nearest(42.0 / 360.0, 0.5, 0.5);
        assert!((1.05..=1.10).contains(&green_sat));
        assert!((skin_sat - 1.0).abs() < 0.03);
    }

    #[test]
    fn product_lut_preserves_hue_and_luminance() {
        let look = Look::product();
        let lut = look.huesat.as_ref().expect("product table");
        let (hue_delta, saturation, value) = lut.sample_nearest(0.1, 1.0, 0.5);
        assert_eq!(hue_delta, 0.0);
        assert!((0.94..=0.96).contains(&saturation));
        assert_eq!(value, 1.0);
    }

    #[test]
    fn profile_and_creative_look_compose_without_second_pass() {
        let profile = Look::vibrant().huesat.unwrap();
        let creative = Look::landscape().huesat.unwrap();
        let composed = HueSatLut::compose(Some(&profile), Some(&creative)).unwrap();
        assert_eq!(composed.expected_len(), profile.expected_len());
        let (_, sat, _) = composed.sample_nearest(125.0 / 360.0, 0.25, 0.5);
        assert!((1.10..1.20).contains(&sat));
    }

    #[test]
    fn rgb_profile_and_creative_look_compose_in_order() {
        let mut base = Lut3d::identity(2);
        for entry in &mut base.entries {
            entry[0] *= 0.5;
        }
        let mut look = Lut3d::identity(2);
        for entry in &mut look.entries {
            entry[1] = entry[0];
        }
        let composed = Lut3d::compose(Some(&base), Some(&look)).unwrap();
        assert_eq!(composed.size, 2);
        let output = composed.sample([1.0, 0.25, 0.75]);
        assert!((output[0] - 0.5).abs() < 1e-6);
        assert!((output[1] - 0.5).abs() < 1e-6);
        assert!((output[2] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn identity_chroma_warp_returns_its_input() {
        let warp = ChromaWarp::identity(CHROMA_WARP_SIZE);
        assert!(warp.is_valid());
        for (r, b) in [
            (CHROMA_NEUTRAL, CHROMA_NEUTRAL),
            (0.6, 0.1),
            (0.0, 1.0),
            (0.9375, 0.0),
        ] {
            let [or, ob] = warp.sample(r, b);
            assert!(
                (or - r).abs() < 1e-4 && (ob - b).abs() < 1e-4,
                "identity warp moved ({r}, {b}) to ({or}, {ob})"
            );
        }
    }

    #[test]
    fn every_physical_colour_lands_inside_the_grid() {
        // The reason for normalized RGB over a (B-Y)/Y chroma pair: a dark
        // saturated blue has chroma ~11 in the latter and would be clamped
        // to the grid edge, losing its correction entirely.
        for rgb in [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.02, 0.02, 0.92],
            [1.0, 1.0, 1.0],
            [0.001, 0.0, 0.0],
        ] {
            let sum = rgb[0] + rgb[1] + rgb[2];
            let (n_r, n_b) = (rgb[0] / sum, rgb[2] / sum);
            assert!(
                (0.0..=1.0).contains(&n_r) && (0.0..=1.0).contains(&n_b),
                "{rgb:?} maps outside the unit square: ({n_r}, {n_b})"
            );
        }
    }

    #[test]
    fn chroma_warp_sampling_clamps_outside_the_grid() {
        let warp = ChromaWarp::identity(9);
        // Outside the grid the edge value is held, never extrapolated.
        assert_eq!(warp.sample(5.0, 0.0)[0], 1.0);
        assert_eq!(warp.sample(-5.0, 0.0)[0], 0.0);
        assert_eq!(warp.sample(0.0, 9.0)[1], 1.0);
    }

    #[test]
    fn chroma_warp_entries_are_red_major() {
        let warp_size = 3;
        let mut warp = ChromaWarp::identity(warp_size);
        // Index (b * size_r + r): entry 1 is the second red node at b = 0.
        warp.entries[1] = [0.75, 0.0];
        let [r, b] = warp.sample(0.5, 0.0);
        assert!(
            (r - 0.75).abs() < 1e-5 && b.abs() < 1e-5,
            "expected the edited node at (r=0.5, b=0), got ({r}, {b})"
        );
    }

    #[test]
    fn chroma_warp_lerp_blends_and_rejects_mismatched_shapes() {
        let a = ChromaWarp::identity(5);
        let mut b = ChromaWarp::identity(5);
        for e in &mut b.entries {
            e[0] += 1.0;
        }
        let mid = ChromaWarp::lerp(&a, &b, 0.5);
        assert!((mid.sample(0.0, 0.0)[0] - 0.5).abs() < 1e-5);
        assert_eq!(ChromaWarp::lerp(&a, &b, 0.0), a);
        // Mismatched shapes fall back to the first rather than panicking.
        assert_eq!(ChromaWarp::lerp(&a, &ChromaWarp::identity(7), 0.5), a);
    }

    #[test]
    fn invalid_chroma_warps_are_rejected_and_never_resolve() {
        let mut warp = ChromaWarp::identity(5);
        warp.entries.pop();
        assert!(!warp.is_valid());
        let mut profile = CameraProfile::standard("X2D II 100C", [[1.0, 0.0, 0.0]; 3]);
        profile.chroma_warp1 = Some(warp);
        assert!(
            profile.resolve(5500.0).chroma_warp.is_none(),
            "a malformed warp must be dropped, not uploaded"
        );
    }

    #[test]
    fn chroma_warp_resolves_across_illuminants_with_the_matrix_list() {
        let mut profile = CameraProfile::standard("X2D II 100C", [[1.0, 0.0, 0.0]; 3]);
        profile.matrices = vec![
            IlluminantMatrix {
                illuminant: Illuminant::StdA,
                cam_to_xyz: [[1.0, 0.0, 0.0]; 3],
            },
            IlluminantMatrix {
                illuminant: Illuminant::D65,
                cam_to_xyz: [[1.0, 0.0, 0.0]; 3],
            },
        ];
        let tungsten = ChromaWarp::identity(5);
        let mut daylight = ChromaWarp::identity(5);
        for e in &mut daylight.entries {
            e[1] += 2.0;
        }
        profile.chroma_warp1 = Some(tungsten);
        profile.chroma_warp2 = Some(daylight);

        let warm = profile.resolve(2856.0).chroma_warp.expect("warp");
        let cool = profile.resolve(6504.0).chroma_warp.expect("warp");
        assert!(warm.sample(0.0, 0.0)[1].abs() < 1e-4, "StdA end is warp1");
        assert!(
            (cool.sample(0.0, 0.0)[1] - 2.0).abs() < 1e-4,
            "D65 end is warp2"
        );
    }

    #[test]
    fn illuminant_weight_follows_the_forward_matrices_when_present() {
        // resolve_matrix prefers forward matrices, so the LUT weight must
        // read the same list or the two desynchronise.
        let mut profile = CameraProfile::standard("X2D II 100C", [[1.0, 0.0, 0.0]; 3]);
        profile.matrices = vec![IlluminantMatrix {
            illuminant: Illuminant::D65,
            cam_to_xyz: [[1.0, 0.0, 0.0]; 3],
        }];
        profile.forward_matrices = vec![
            IlluminantMatrix {
                illuminant: Illuminant::StdA,
                cam_to_xyz: [[1.0, 0.0, 0.0]; 3],
            },
            IlluminantMatrix {
                illuminant: Illuminant::D65,
                cam_to_xyz: [[1.0, 0.0, 0.0]; 3],
            },
        ];
        // With a single `matrices` entry the old reading returned 0.0 flat.
        assert!(profile.illuminant_weight(6504.0) > 0.99);
        assert!(profile.illuminant_weight(2856.0) < 0.01);
    }
}
