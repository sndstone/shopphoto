//! Display and printer ICC soft proofing. LUTs contain linear Rec.2020 RGB.
//! Little CMS is statically linked for consistent RGB/CMYK proofing and BPC.
use lcms2::{Flags, Intent, PixelFormat, Profile, ThreadContext, Transform};
use moxcms::{ColorProfile, ToneReprCurve};
use serde::{Deserialize, Serialize};
use sp_core::profile::Lut3d;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofIntent {
    Perceptual,
    #[default]
    Relative,
    Saturation,
    Absolute,
}
impl ProofIntent {
    fn lcms(self) -> Intent {
        match self {
            Self::Perceptual => Intent::Perceptual,
            Self::Relative => Intent::RelativeColorimetric,
            Self::Saturation => Intent::Saturation,
            Self::Absolute => Intent::AbsoluteColorimetric,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofOptions {
    pub intent: ProofIntent,
    pub black_point_compensation: bool,
    /// Absolute proof rendering exposes the media's paper white and black.
    pub simulate_paper: bool,
}
impl Default for ProofOptions {
    fn default() -> Self {
        Self {
            intent: ProofIntent::Relative,
            black_point_compensation: true,
            simulate_paper: false,
        }
    }
}
#[derive(Debug, Clone)]
pub struct CustomProof {
    pub name: String,
    pub simulation: Lut3d,
    pub gamut_warning: Lut3d,
    pub options: ProofOptions,
    /// Retained for rebuilding the LUT when the user changes proof options.
    pub icc: Arc<Vec<u8>>,
}
#[derive(Debug, Error)]
pub enum ProofError {
    #[error("ICC file is truncated or too large")]
    Truncated,
    #[error("Use a display or printer/output ICC profile; device links are not proof profiles")]
    UnsupportedClass,
    #[error("Proof profile must describe RGB or CMYK colors")]
    UnsupportedColorSpace,
    #[error("invalid ICC profile: {0}")]
    Invalid(String),
    #[error("could not build ICC proof transform: {0}")]
    Transform(String),
}
pub fn build_custom_proof(
    name: impl Into<String>,
    bytes: &[u8],
    size: u32,
) -> Result<CustomProof, ProofError> {
    build_custom_proof_with_options(name, bytes, size, ProofOptions::default())
}
/// Bake an actual printer soft-proof transform rather than interpreting CMYK
/// values as RGB. A private CMS context isolates per-job gamut alarm settings.
pub fn build_custom_proof_with_options(
    name: impl Into<String>,
    bytes: &[u8],
    size: u32,
    options: ProofOptions,
) -> Result<CustomProof, ProofError> {
    if !(128..=32 * 1024 * 1024).contains(&bytes.len()) {
        return Err(ProofError::Truncated);
    }
    if !matches!(&bytes[12..16], b"mntr" | b"prtr") {
        return Err(ProofError::UnsupportedClass);
    }
    if !matches!(&bytes[16..20], b"RGB " | b"CMYK") {
        return Err(ProofError::UnsupportedColorSpace);
    }
    let mut context = ThreadContext::new();
    let mut alarms = [0u16; 16];
    alarms[..3].copy_from_slice(&[65535, 1, 65535]);
    context.set_alarm_codes(alarms);
    let target = Profile::new_icc_context(&context, bytes)
        .map_err(|e| ProofError::Invalid(e.to_string()))?;
    let mut working = ColorProfile::new_bt2020();
    working.red_trc = Some(ToneReprCurve::Lut(Vec::new()));
    working.green_trc = Some(ToneReprCurve::Lut(Vec::new()));
    working.blue_trc = Some(ToneReprCurve::Lut(Vec::new()));
    let encoded = working
        .encode()
        .map_err(|e| ProofError::Invalid(e.to_string()))?;
    let working = Profile::new_icc_context(&context, &encoded)
        .map_err(|e| ProofError::Invalid(e.to_string()))?;
    let mut flags = Flags::SOFT_PROOFING | Flags::NO_OPTIMIZE;
    if options.black_point_compensation {
        flags = flags | Flags::BLACKPOINT_COMPENSATION;
    }
    let proof_intent = if options.simulate_paper {
        Intent::AbsoluteColorimetric
    } else {
        Intent::RelativeColorimetric
    };
    let transform: Transform<[f32; 3], [f32; 3], ThreadContext> = Transform::new_proofing_context(
        &context,
        &working,
        PixelFormat::RGB_FLT,
        &working,
        PixelFormat::RGB_FLT,
        &target,
        options.intent.lcms(),
        proof_intent,
        flags,
    )
    .map_err(|e| ProofError::Transform(e.to_string()))?;
    let gamut: Transform<[f32; 3], [u16; 3], ThreadContext> = Transform::new_proofing_context(
        &context,
        &working,
        PixelFormat::RGB_FLT,
        &working,
        PixelFormat::RGB_16,
        &target,
        options.intent.lcms(),
        proof_intent,
        flags | Flags::GAMUT_CHECK,
    )
    .map_err(|e| ProofError::Transform(e.to_string()))?;
    let size = size.clamp(2, 65);
    let denominator = (size - 1) as f32;
    let mut input = Vec::with_capacity(size.pow(3) as usize);
    for r in 0..size {
        for g in 0..size {
            for b in 0..size {
                input.push([r, g, b].map(|v| v as f32 / denominator));
            }
        }
    }
    let mut simulation = vec![[0.0; 3]; input.len()];
    let mut warning = vec![[0u16; 3]; input.len()];
    transform.transform_pixels(&input, &mut simulation);
    gamut.transform_pixels(&input, &mut warning);
    if simulation.iter().flatten().any(|v| !v.is_finite()) {
        return Err(ProofError::Transform(
            "profile produced non-finite colors".into(),
        ));
    }
    let warnings = simulation
        .iter()
        .zip(warning)
        .map(|(color, alarm)| {
            if alarm == [65535, 1, 65535] {
                [-1.0; 3]
            } else {
                *color
            }
        })
        .collect();
    Ok(CustomProof {
        name: name.into(),
        simulation: Lut3d {
            size,
            entries: simulation,
        },
        gamut_warning: Lut3d {
            size,
            entries: warnings,
        },
        options,
        icc: Arc::new(bytes.to_vec()),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_display_and_output_rgb_profiles_and_builds_real_gamut_warning() {
        let mut icc = ColorProfile::new_display_p3().encode().unwrap();
        for class in [b"mntr", b"prtr"] {
            icc[12..16].copy_from_slice(class);
            let proof = build_custom_proof("P3", &icc, 9).unwrap();
            assert_eq!(proof.simulation.expected_len(), 9usize.pow(3));
            assert!(proof
                .simulation
                .entries
                .iter()
                .flatten()
                .all(|v| v.is_finite()));
            assert!(proof.gamut_warning.entries.iter().any(|rgb| rgb[0] < 0.0));
        }
    }
    #[test]
    fn proof_options_are_applied_without_altering_the_source_profile() {
        let icc = ColorProfile::new_srgb().encode().unwrap();
        for intent in [
            ProofIntent::Relative,
            ProofIntent::Perceptual,
            ProofIntent::Absolute,
            ProofIntent::Saturation,
        ] {
            let options = ProofOptions {
                intent,
                black_point_compensation: false,
                simulate_paper: true,
            };
            let proof = build_custom_proof_with_options("Print", &icc, 9, options).unwrap();
            assert_eq!(proof.options, options);
            assert_eq!(*proof.icc, icc);
        }
    }
    #[test]
    fn rejects_device_links_and_invalid_color_spaces() {
        let mut icc = ColorProfile::new_srgb().encode().unwrap();
        icc[12..16].copy_from_slice(b"link");
        assert!(matches!(
            build_custom_proof("bad", &icc, 9),
            Err(ProofError::UnsupportedClass)
        ));
        icc[12..16].copy_from_slice(b"prtr");
        icc[16..20].copy_from_slice(b"GRAY");
        assert!(matches!(
            build_custom_proof("bad", &icc, 9),
            Err(ProofError::UnsupportedColorSpace)
        ));
        assert!(build_custom_proof("bad", &[0; 32], 9).is_err());
    }
    #[test]
    #[ignore = "requires an externally supplied CMYK printer ICC via SP_TEST_CMYK_PROFILE"]
    fn real_cmyk_printer_profile_proofs_and_simulates_media() {
        let path = std::env::var_os("SP_TEST_CMYK_PROFILE").expect("supply a printer profile");
        let icc = std::fs::read(path).unwrap();
        assert_eq!(&icc[16..20], b"CMYK");
        let normal = build_custom_proof("CMYK", &icc, 17).unwrap();
        let paper = build_custom_proof_with_options(
            "CMYK",
            &icc,
            17,
            ProofOptions {
                simulate_paper: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(normal.gamut_warning.entries.iter().any(|rgb| rgb[0] < 0.0));
        assert!(normal
            .simulation
            .entries
            .iter()
            .flatten()
            .zip(paper.simulation.entries.iter().flatten())
            .any(|(a, b)| (a - b).abs() > 0.001));
        let last = normal.simulation.entries.last().unwrap();
        assert!(last.iter().all(|v| *v > 0.5 && *v < 1.2), "{last:?}");
    }
}
