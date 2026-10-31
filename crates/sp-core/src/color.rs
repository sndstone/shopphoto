//! Color spaces and conversion matrices.
//!
//! The working space for the entire develop pipeline is **scene-referred
//! Linear Rec.2020** (BUILD.md §3.2). Values may exceed 1.0; display/export
//! encoding happens only in the output transform.
//!
//! Matrices here are published reference values (BT.709 / BT.2020, D65) and
//! are covered by round-trip unit tests at the bottom of this file.

use serde::{Deserialize, Serialize};

/// RGB primaries used for SDR proofing and export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputColorSpace {
    #[default]
    Srgb,
    DisplayP3,
    Rec2020,
    /// Adobe RGB (1998): D65 white, gamma 563/256. Print labs and
    /// fine-art workflows still ask for it; only export encodes it.
    AdobeRgb,
}

impl OutputColorSpace {
    /// Which transfer function encodes this space. Every space but Adobe
    /// RGB uses the sRGB curve (Display P3 by definition; Rec.2020 SDR
    /// exports carry an sRGB-curve ICC).
    #[must_use]
    pub const fn transfer(self) -> Transfer {
        match self {
            Self::AdobeRgb => Transfer::AdobeRgb,
            Self::Srgb | Self::DisplayP3 | Self::Rec2020 => Transfer::Srgb,
        }
    }
}

/// Display-referred transfer function of an SDR output space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Srgb,
    /// Pure power curve, exponent 563/256 (2.19921875).
    AdobeRgb,
}

impl Transfer {
    #[must_use]
    pub fn oetf(self, x: f32) -> f32 {
        match self {
            Self::Srgb => srgb_oetf(x),
            Self::AdobeRgb => adobe_rgb_oetf(x),
        }
    }

    #[must_use]
    pub fn eotf(self, x: f32) -> f32 {
        match self {
            Self::Srgb => srgb_eotf(x),
            Self::AdobeRgb => adobe_rgb_eotf(x),
        }
    }

    /// The value the output shaders read from `proof_flags.z`.
    #[must_use]
    pub const fn shader_id(self) -> u32 {
        match self {
            Self::Srgb => 0,
            Self::AdobeRgb => 1,
        }
    }
}

/// A 3×3 row-major color matrix. `m[row][col]`, applied as `out = M * rgb`.
pub type Mat3 = [[f32; 3]; 3];

/// An RGB triple in scene-referred linear Rec.2020. The pipeline's working
/// space. Unbounded above; 1.0 = SDR diffuse white after the exposure stage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinearRec2020(pub [f32; 3]);

/// An RGB triple in CIE XYZ (D65).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Xyz(pub [f32; 3]);

/// XYZ (D65) → linear Rec.2020, from BT.2020 (derived, standard values).
pub const XYZ_TO_REC2020: Mat3 = [
    [1.716_651, -0.355_671, -0.253_366],
    [-0.666_684, 1.616_481, 0.015_768],
    [0.017_640, -0.042_771, 0.942_103],
];

/// Linear Rec.2020 → XYZ (D65), from BT.2020.
pub const REC2020_TO_XYZ: Mat3 = [
    [0.636_958, 0.144_617, 0.168_881],
    [0.262_700, 0.677_998, 0.059_302],
    [0.000_000, 0.028_073, 1.060_985],
];

/// Luminance weights for linear Rec.2020 — the Y row of [`REC2020_TO_XYZ`].
///
/// The chroma-warp stage divides by this luma to make its lookup depend on
/// chromaticity alone, so the same constants must appear in
/// `shaders/wb_color.wgsl`; `sp-gpu`'s shader tests assert they agree.
pub const REC2020_LUMA: [f32; 3] = [0.262_700, 0.677_998, 0.059_302];

/// Linear Rec.2020 → linear Rec.709/sRGB primaries (for SDR output transform).
pub const REC2020_TO_REC709: Mat3 = [
    [1.660_491, -0.587_641, -0.072_85],
    [-0.124_55, 1.132_9, -0.008_349],
    [-0.018_151, -0.100_579, 1.118_73],
];

/// Linear Rec.2020 → linear Display P3-D65. Must match the output shaders.
pub const REC2020_TO_P3D65: Mat3 = [
    [1.343_578, -0.282_179, -0.061_399],
    [-0.065_298, 1.075_788, -0.010_490],
    [0.002_822, -0.019_598, 1.016_777],
];

/// Linear Display P3-D65 → linear Rec.2020.
pub const P3D65_TO_REC2020: Mat3 = [
    [0.753_833, 0.198_597, 0.047_570],
    [0.045_744, 0.941_777, 0.012_479],
    [-0.001_210, 0.017_602, 0.983_608],
];

/// Linear Adobe RGB (1998) → XYZ (D65). Published Adobe values.
pub const ADOBERGB_TO_XYZ: Mat3 = [
    [0.576_731, 0.185_554, 0.188_185],
    [0.297_377, 0.627_349, 0.075_274],
    [0.027_034, 0.070_687, 0.991_109],
];

/// XYZ (D65) → linear Adobe RGB (1998).
pub const XYZ_TO_ADOBERGB: Mat3 = [
    [2.041_369, -0.564_947, -0.344_694],
    [-0.969_266, 1.876_011, 0.041_556],
    [0.013_447, -0.118_39, 1.015_41],
];

/// Linear Rec.2020 → linear Adobe RGB (1998). Must match the output
/// shaders' gamut matrix for Adobe RGB exports.
pub const REC2020_TO_ADOBERGB: Mat3 = [
    [1.151_855, -0.097_493, -0.054_47],
    [-0.124_554, 1.132_926, -0.008_349],
    [-0.022_536, -0.049_818, 1.072_585],
];

/// Linear Adobe RGB (1998) → linear Rec.2020.
pub const ADOBERGB_TO_REC2020: Mat3 = [
    [0.877_428, 0.077_492, 0.045_162],
    [0.096_633, 0.891_507, 0.011_847],
    [0.022_924, 0.043_035, 0.933_826],
];

/// Adobe RGB (1998) gamma, 563/256.
pub const ADOBERGB_GAMMA: f32 = 563.0 / 256.0;

#[must_use]
pub fn adobe_rgb_oetf(x: f32) -> f32 {
    x.max(0.0).powf(1.0 / ADOBERGB_GAMMA)
}

#[must_use]
pub fn adobe_rgb_eotf(x: f32) -> f32 {
    x.max(0.0).powf(ADOBERGB_GAMMA)
}

/// Multiply two 3×3 matrices: `a * b`.
#[must_use]
pub fn mat3_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0f32; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    out
}

/// Apply a 3×3 matrix to an RGB triple.
#[must_use]
pub fn mat3_apply(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Invert a 3×3 matrix. Returns `None` when singular (|det| < 1e-12),
/// non-finite, or when the calculation exceeds the finite `f32` range.
/// Callers dealing with camera matrices must treat that as corrupt metadata.
#[must_use]
pub fn mat3_inverse(m: &Mat3) -> Option<Mat3> {
    if m.iter().flatten().any(|value| !value.is_finite()) {
        return None;
    }
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    let inverse = [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ];
    inverse
        .iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(inverse)
}

/// sRGB OETF (linear → encoded), scalar. Input clamped to 0..=1.
#[must_use]
pub fn srgb_oetf(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB EOTF (encoded → linear), scalar. Input clamped to 0..=1.
#[must_use]
pub fn srgb_eotf(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// SMPTE ST 2084 / BT.2100 PQ OETF. Input is absolute luminance normalized
/// so `1.0 = 10,000 cd/m²`; output is 0..=1.
#[must_use]
pub fn pq_oetf(luminance_norm: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 32.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 128.0;
    const C3: f32 = 2392.0 / 128.0;
    let p = luminance_norm.max(0.0).powf(M1);
    ((C1 + C2 * p) / (1.0 + C3 * p)).powf(M2)
}

/// SMPTE ST 2084 / BT.2100 PQ EOTF. Returns absolute luminance normalized
/// so `1.0 = 10,000 cd/m²`.
#[must_use]
pub fn pq_eotf(encoded: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 32.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 128.0;
    const C3: f32 = 2392.0 / 128.0;
    let p = encoded.clamp(0.0, 1.0).powf(1.0 / M2);
    ((p - C1).max(0.0) / (C2 - C3 * p).max(1e-8)).powf(1.0 / M1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: f32, b: f32, tol: f32) {
        assert!((a - b).abs() < tol, "{a} !~ {b} (tol {tol})");
    }

    #[test]
    fn rec2020_xyz_round_trip_is_identity() {
        let id = mat3_mul(&XYZ_TO_REC2020, &REC2020_TO_XYZ);
        for (r, row) in id.iter().enumerate() {
            for (c, &v) in row.iter().enumerate() {
                let expect = if r == c { 1.0 } else { 0.0 };
                assert_close(v, expect, 2e-4);
            }
        }
    }

    #[test]
    fn rec2020_white_maps_to_d65() {
        // Equal-energy RGB white in Rec.2020 must land on D65 (x≈0.3127, y≈0.3290).
        let Xyz([x, y, z]) = Xyz(mat3_apply(&REC2020_TO_XYZ, [1.0, 1.0, 1.0]));
        let sum = x + y + z;
        assert_close(x / sum, 0.3127, 1e-3);
        assert_close(y / sum, 0.3290, 1e-3);
    }

    #[test]
    fn adobe_rgb_matrices_are_consistent() {
        // Derived matrices agree with the published Adobe primaries.
        let derived = mat3_mul(&XYZ_TO_ADOBERGB, &REC2020_TO_XYZ);
        for r in 0..3 {
            for c in 0..3 {
                assert!((derived[r][c] - REC2020_TO_ADOBERGB[r][c]).abs() < 2e-4);
            }
        }
        let round = mat3_mul(&ADOBERGB_TO_REC2020, &REC2020_TO_ADOBERGB);
        for r in 0..3 {
            for c in 0..3 {
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!((round[r][c] - expected).abs() < 2e-4, "{round:?}");
            }
        }
        // D65 white stays white, as both spaces share it.
        let white = mat3_apply(&REC2020_TO_ADOBERGB, [1.0, 1.0, 1.0]);
        for channel in white {
            assert!((channel - 1.0).abs() < 1e-3, "{white:?}");
        }
        // Adobe RGB green lies outside Rec.709 but inside Rec.2020.
        let green = mat3_apply(&ADOBERGB_TO_REC2020, [0.0, 1.0, 0.0]);
        assert!(
            green.iter().all(|c| *c >= -1e-3 && *c <= 1.0 + 1e-3),
            "{green:?}"
        );
    }

    #[test]
    fn adobe_rgb_transfer_round_trip() {
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            let back = adobe_rgb_eotf(adobe_rgb_oetf(x));
            assert!((back - x).abs() < 1e-5, "{x}");
        }
        assert_eq!(OutputColorSpace::AdobeRgb.transfer(), Transfer::AdobeRgb);
        assert_eq!(OutputColorSpace::DisplayP3.transfer(), Transfer::Srgb);
        assert_eq!(Transfer::AdobeRgb.shader_id(), 1);
        // 18% grey encodes to ~0.46 in Adobe RGB versus ~0.46 in sRGB; the
        // curves differ most in the toe.
        assert!((Transfer::AdobeRgb.oetf(0.01) - 0.123_2).abs() < 2e-3);
    }

    #[test]
    fn rec2020_p3_round_trip_is_identity() {
        let id = mat3_mul(&P3D65_TO_REC2020, &REC2020_TO_P3D65);
        for (r, row) in id.iter().enumerate() {
            for (c, &value) in row.iter().enumerate() {
                assert_close(value, if r == c { 1.0 } else { 0.0 }, 1e-5);
            }
        }
    }

    #[test]
    fn mat3_inverse_round_trips_rec2020() {
        let inv = mat3_inverse(&REC2020_TO_XYZ).expect("invertible");
        let id = mat3_mul(&inv, &REC2020_TO_XYZ);
        for (r, row) in id.iter().enumerate() {
            for (c, &v) in row.iter().enumerate() {
                let expect = if r == c { 1.0 } else { 0.0 };
                assert_close(v, expect, 1e-4);
            }
        }
        // Inverse of the standard matrix matches the published inverse.
        assert_close(inv[0][0], XYZ_TO_REC2020[0][0], 1e-3);
    }

    #[test]
    fn mat3_inverse_rejects_singular() {
        let singular: Mat3 = [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 0.0]];
        assert!(mat3_inverse(&singular).is_none());
    }

    #[test]
    fn mat3_inverse_rejects_nonfinite_input_and_arithmetic_overflow() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for row in 0..3 {
                for column in 0..3 {
                    let mut matrix = identity;
                    matrix[row][column] = value;
                    assert!(mat3_inverse(&matrix).is_none());
                }
            }
        }
        let overflowing_determinant = [
            [f32::MAX, 0.0, 0.0],
            [0.0, f32::MAX, 0.0],
            [0.0, 0.0, f32::MAX],
        ];
        assert!(mat3_inverse(&overflowing_determinant).is_none());
        // The determinant stays finite (~1e-10), but inverse[0][1]
        // overflows because the matrix is extremely ill-conditioned.
        let overflowing_inverse = [[1e-10, 1e30, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(mat3_inverse(&overflowing_inverse).is_none());
    }

    #[test]
    fn srgb_transfer_round_trip() {
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            assert_close(srgb_eotf(srgb_oetf(x)), x, 1e-5);
        }
    }

    #[test]
    fn srgb_reference_values() {
        // Published anchor points of the sRGB curve.
        assert_close(srgb_oetf(0.0), 0.0, 1e-6);
        assert_close(srgb_oetf(1.0), 1.0, 1e-6);
        assert_close(srgb_oetf(0.18), 0.4613, 2e-3); // 18% grey ≈ 46.1%
    }

    #[test]
    fn pq_reference_vectors_and_round_trip() {
        // BT.2100/ST 2084: 100 cd/m² -> code value ~0.5081.
        assert_close(pq_oetf(100.0 / 10_000.0), 0.508_08, 2e-4);
        assert_close(pq_eotf(0.508_08) * 10_000.0, 100.0, 0.2);
        for nits in [0.0, 0.1, 1.0, 100.0, 1_000.0, 10_000.0] {
            let norm = nits / 10_000.0;
            assert_close(pq_eotf(pq_oetf(norm)), norm, 2e-5);
        }
    }
}
