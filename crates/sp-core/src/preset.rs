//! Curated, camera-independent develop presets.
//!
//! Presets deliberately replace only creative controls. Camera profile,
//! white balance, denoise, crop, output resolution and HDR intent remain
//! image-specific and are preserved.

use crate::edit::{ColorGradeWheel, EditState};
use serde::{Deserialize, Serialize};

/// Selects the edit groups copied, synchronized, or stored in a user preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditGroups {
    pub profile: bool,
    pub white_balance: bool,
    pub tone: bool,
    pub color: bool,
    pub detail: bool,
    pub lens: bool,
    pub effects: bool,
    pub geometry: bool,
    pub output: bool,
    pub spots: bool,
}

impl Default for EditGroups {
    fn default() -> Self {
        Self {
            profile: false,
            white_balance: false,
            tone: true,
            color: true,
            detail: true,
            lens: true,
            effects: true,
            geometry: false,
            output: false,
            spots: false,
        }
    }
}

impl EditGroups {
    /// Copy the selected groups from `source`, preserving image-specific
    /// state when its group is disabled.
    pub fn apply(self, source: &EditState, target: &mut EditState) {
        if self.profile {
            target.profile = source.profile.clone();
            target.look = source.look.clone();
        }
        if self.white_balance {
            target.wb = source.wb;
            target.negative = source.negative;
        }
        if self.tone {
            target.tone = source.tone;
            target.curve = source.curve.clone();
        }
        if self.color {
            target.color = source.color;
        }
        if self.detail {
            target.sharpen = source.sharpen;
            target.nr = source.nr;
        }
        if self.lens {
            target.lens = source.lens.clone();
        }
        if self.effects {
            target.local = source.local;
            target.vignette = source.vignette;
            target.grain = source.grain;
        }
        if self.geometry {
            target.geometry = source.geometry;
        }
        if self.output {
            target.resolution = source.resolution;
            target.hdr = source.hdr;
        }
        if self.spots {
            target.spots = source.spots.clone();
            target.sample_points = source.sample_points.clone();
            target.local_adjustments = source.local_adjustments.clone();
        }
        *target = target.clone().clamped();
    }
}

/// Portable user-created develop preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPreset {
    pub version: u32,
    pub name: String,
    pub groups: EditGroups,
    pub edit: EditState,
}

impl UserPreset {
    pub const VERSION: u32 = 1;

    #[must_use]
    pub fn from_edit(name: impl Into<String>, groups: EditGroups, edit: &EditState) -> Self {
        Self {
            version: Self::VERSION,
            name: name.into(),
            groups,
            edit: edit.clone().clamped(),
        }
    }

    pub fn apply(&self, target: &mut EditState) {
        self.groups.apply(&self.edit, target);
    }
}

/// Stable identifiers for the built-in develop presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevelopPreset {
    StandardFinish,
    Nature,
    Portrait,
    Product,
    VividColor,
    EditorialMuted,
    FilmWarm,
    GoldenHourRadiance,
    NeonNight,
    FilmCool,
    SoftNeutral,
    BlackAndWhite,
    HighlightRecovery,
}

impl DevelopPreset {
    pub const ALL: [Self; 13] = [
        Self::StandardFinish,
        Self::Nature,
        Self::Portrait,
        Self::Product,
        Self::VividColor,
        Self::EditorialMuted,
        Self::FilmWarm,
        Self::GoldenHourRadiance,
        Self::NeonNight,
        Self::FilmCool,
        Self::SoftNeutral,
        Self::BlackAndWhite,
        Self::HighlightRecovery,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::StandardFinish => "Standard Finish",
            Self::Nature => "Nature",
            Self::Portrait => "Portrait",
            Self::Product => "Product",
            Self::VividColor => "Vivid Color",
            Self::EditorialMuted => "Editorial Muted",
            Self::FilmWarm => "Film Warm",
            Self::GoldenHourRadiance => "Golden Hour Radiance",
            Self::NeonNight => "Neon Night",
            Self::FilmCool => "Film Cool",
            Self::SoftNeutral => "Soft Neutral",
            Self::BlackAndWhite => "B&W Fine Art",
            Self::HighlightRecovery => "Highlight Recovery",
        }
    }

    #[must_use]
    pub const fn category(self) -> &'static str {
        match self {
            Self::StandardFinish | Self::Nature | Self::Portrait | Self::Product => "Essentials",
            Self::VividColor
            | Self::EditorialMuted
            | Self::FilmWarm
            | Self::GoldenHourRadiance
            | Self::NeonNight
            | Self::FilmCool
            | Self::SoftNeutral => "Creative Looks",
            Self::BlackAndWhite => "Monochrome",
            Self::HighlightRecovery => "Utility",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::StandardFinish => {
                "A restrained finished starting point with no creative color shift."
            }
            Self::Nature => "Finished landscape color with protected skies and natural detail.",
            Self::Portrait => "Protected skin color, soft texture and gentle sharpening.",
            Self::Product => "Color-faithful rendering with clean whites and precise detail.",
            Self::VividColor => "Adaptive color lift without crushed highlights.",
            Self::EditorialMuted => "Calm contrast and restrained editorial color.",
            Self::FilmWarm => "Muted LUT, warm highlights and cool shadows.",
            Self::GoldenHourRadiance => {
                "Luminous warm color with golden highlights and open shadows."
            }
            Self::NeonNight => "Selective neon color with controlled highlights and deep shadows.",
            Self::FilmCool => "Muted LUT with a cool editorial split tone.",
            Self::SoftNeutral => "Soft, neutral finish with open shadows and gentle color.",
            Self::BlackAndWhite => "Fine-art monochrome with a useful color-channel mix.",
            Self::HighlightRecovery => {
                "Conservative highlight and white recovery without changing color."
            }
        }
    }

    /// Name of the real DNG-style Hue/Sat look table used by this preset.
    /// Utility presets intentionally return `None`.
    #[must_use]
    pub const fn look_name(self) -> Option<&'static str> {
        match self {
            Self::Nature => Some("Landscape"),
            Self::Portrait => Some("Portrait"),
            Self::Product => Some("Product"),
            Self::VividColor => Some("Vibrant"),
            Self::EditorialMuted | Self::FilmWarm | Self::FilmCool => Some("Muted"),
            Self::SoftNeutral => Some("Flat"),
            Self::StandardFinish
            | Self::GoldenHourRadiance
            | Self::NeonNight
            | Self::BlackAndWhite
            | Self::HighlightRecovery => None,
        }
    }

    /// Apply the recipe while preserving image-specific settings and HDR intent.
    /// HDR-aware recipes adjust the shoulder only when HDR is already enabled.
    pub fn apply(self, edit: &mut EditState) {
        let profile = edit.profile.clone();
        let wb = edit.wb;
        // Exposure, the bounded midtone lift, and scene endpoints are
        // image-specific: applying a creative finish after Auto Balance must
        // not throw away the established brightness of a backlit photograph.
        // Recipes that intentionally define Whites or Blacks override these
        // saved values in the match below.
        let exposure_ev = edit.tone.exposure_ev;
        let brightness = edit.tone.brightness;
        let whites = edit.tone.whites;
        let blacks = edit.tone.blacks;
        let nr = edit.nr;
        let lens = edit.lens.clone();
        let resolution = edit.resolution;
        let hdr = edit.hdr;
        let geometry = edit.geometry;
        *edit = EditState {
            profile,
            wb,
            nr,
            lens,
            resolution,
            hdr,
            geometry,
            ..EditState::default()
        };
        edit.tone.exposure_ev = exposure_ev;
        edit.tone.brightness = brightness;
        edit.tone.whites = whites;
        edit.tone.blacks = blacks;

        match self {
            Self::StandardFinish => {
                edit.tone.contrast = 4.0;
                edit.tone.highlights = -8.0;
                edit.tone.shadows = 4.0;
                edit.color.vibrance = 5.0;
                edit.sharpen.amount = 28.0;
            }
            Self::Nature => {
                edit.look = Some("Landscape".into());
                edit.tone.highlights = -16.0;
                edit.tone.shadows = 6.0;
                edit.local.texture = 3.0;
                edit.local.clarity = 2.0;
                edit.sharpen.amount = 38.0;
                edit.sharpen.detail = 34.0;
            }
            Self::Portrait => {
                edit.look = Some("Portrait".into());
                edit.tone.highlights = -16.0;
                edit.tone.shadows = 6.0;
                edit.local.texture = -6.0;
                edit.local.clarity = -2.0;
                edit.sharpen.amount = 20.0;
                edit.sharpen.detail = 55.0;
            }
            Self::Product => {
                edit.look = Some("Product".into());
                edit.tone.contrast = 3.0;
                edit.tone.highlights = -10.0;
                edit.tone.whites = 2.0;
                edit.tone.blacks = -2.0;
                edit.local.texture = 4.0;
                edit.local.clarity = 1.0;
                edit.sharpen.amount = 48.0;
                edit.sharpen.detail = 40.0;
            }
            Self::VividColor => {
                edit.look = Some("Vibrant".into());
                edit.tone.highlights = -14.0;
                edit.color.vibrance = 3.0;
                edit.local.clarity = 2.0;
                edit.sharpen.amount = 28.0;
            }
            Self::EditorialMuted => {
                edit.look = Some("Muted".into());
                edit.tone.highlights = -12.0;
                edit.tone.shadows = 5.0;
                edit.color.saturation = -2.0;
                edit.sharpen.amount = 22.0;
            }
            Self::FilmWarm => {
                edit.look = Some("Muted".into());
                edit.tone.highlights = -13.0;
                edit.color.grading.shadows = ColorGradeWheel {
                    hue_degrees: 215.0,
                    saturation: 6.0,
                };
                edit.color.grading.highlights = ColorGradeWheel {
                    hue_degrees: 42.0,
                    saturation: 12.0,
                };
                edit.color.grading.balance = 14.0;
                edit.vignette.amount = -5.0;
                edit.sharpen.amount = 20.0;
            }
            Self::GoldenHourRadiance => {
                // Color grading adds warmth without replacing camera white balance.
                // Avoid the Muted LUT used by Film Warm: retain luminous color.
                edit.tone.contrast = 3.0;
                edit.tone.highlights = -12.0;
                edit.tone.shadows = 8.0;
                edit.color.vibrance = 9.0;
                edit.color.hsl[1].luminance = 4.0;
                edit.color.hsl[2].saturation = 4.0;
                edit.color.grading.midtones = ColorGradeWheel {
                    hue_degrees: 38.0,
                    saturation: 4.0,
                };
                edit.color.grading.highlights = ColorGradeWheel {
                    hue_degrees: 45.0,
                    saturation: 10.0,
                };
                edit.color.grading.balance = 12.0;
                edit.sharpen.amount = 24.0;
                if edit.hdr.enabled {
                    edit.hdr.highlight_rolloff = 55.0;
                }
            }
            Self::NeonNight => {
                edit.tone.contrast = 8.0;
                edit.tone.highlights = -24.0;
                edit.tone.shadows = -6.0;
                edit.color.saturation = -4.0;
                // Favor existing aqua/blue/magenta lights over a global color boost.
                edit.color.hsl[4].saturation = 12.0;
                edit.color.hsl[5].saturation = 8.0;
                edit.color.hsl[7].saturation = 12.0;
                edit.color.hsl[2].saturation = -8.0;
                edit.color.hsl[3].saturation = -8.0;
                edit.local.clarity = 3.0;
                edit.sharpen.amount = 24.0;
                if edit.hdr.enabled {
                    edit.hdr.highlight_rolloff = 65.0;
                }
            }
            Self::FilmCool => {
                edit.look = Some("Muted".into());
                edit.tone.highlights = -12.0;
                edit.color.saturation = -2.0;
                edit.color.grading.shadows = ColorGradeWheel {
                    hue_degrees: 220.0,
                    saturation: 12.0,
                };
                edit.color.grading.highlights = ColorGradeWheel {
                    hue_degrees: 32.0,
                    saturation: 4.0,
                };
                edit.color.grading.balance = -10.0;
                edit.sharpen.amount = 20.0;
            }
            Self::SoftNeutral => {
                edit.look = Some("Flat".into());
                edit.tone.highlights = -10.0;
                edit.tone.shadows = 5.0;
                edit.sharpen.amount = 20.0;
            }
            Self::BlackAndWhite => {
                edit.color.monochrome = true;
                edit.color.monochrome_mix = [20.0, 14.0, 5.0, -8.0, -5.0, -14.0, 5.0, 12.0];
                edit.tone.contrast = 10.0;
                edit.tone.highlights = -15.0;
                edit.tone.shadows = 8.0;
                edit.tone.blacks = -6.0;
                edit.local.clarity = 4.0;
                edit.sharpen.amount = 30.0;
            }
            Self::HighlightRecovery => {
                edit.tone.highlights = -34.0;
                edit.tone.whites = -10.0;
                edit.tone.shadows = 4.0;
                edit.sharpen.amount = 20.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_copy_preserves_image_specific_fields() {
        let mut source = EditState::default();
        source.tone.exposure_ev = 1.5;
        source.color.saturation = 20.0;
        source.geometry.crop = [0.1, 0.1, 0.9, 0.9];
        let mut target = EditState::default();
        EditGroups::default().apply(&source, &mut target);
        assert_eq!(target.tone.exposure_ev, 1.5);
        assert_eq!(target.color.saturation, 20.0);
        assert_eq!(target.geometry, Default::default());
    }

    #[test]
    fn user_preset_round_trips_and_applies_selected_groups() {
        let mut source = EditState::default();
        source.grain.amount = 30.0;
        let preset = UserPreset::from_edit("Grain", EditGroups::default(), &source);
        let json = serde_json::to_string(&preset).expect("serialize");
        let decoded: UserPreset = serde_json::from_str(&json).expect("deserialize");
        let mut target = EditState::default();
        decoded.apply(&mut target);
        assert_eq!(target.grain.amount, 30.0);
    }
    use crate::edit::{
        GeometryParams, HdrParams, HdrTarget, LensParams, NrParams, ProfileChoice, ResolutionMode,
        ResolutionParams, WhiteBalance,
    };

    #[test]
    fn every_preset_is_clamped_and_non_neutral() {
        for preset in DevelopPreset::ALL {
            let mut edit = EditState::default();
            preset.apply(&mut edit);
            assert_ne!(edit, EditState::default(), "{}", preset.name());
            assert_eq!(edit.clone().clamped(), edit, "{}", preset.name());
        }
    }

    #[test]
    fn applying_presets_preserves_image_specific_settings() {
        let mut edit = EditState {
            profile: ProfileChoice::Named("My camera".into()),
            wb: WhiteBalance {
                temp_kelvin: Some(4_800.0),
                tint: 7.0,
            },
            nr: NrParams {
                chroma: 33.0,
                ..NrParams::default()
            },
            lens: LensParams {
                enable: true,
                vignetting: 42.0,
                profile: Some("My lens".into()),
                ..LensParams::default()
            },
            geometry: GeometryParams {
                crop: [0.1, 0.2, 0.9, 0.8],
                ..GeometryParams::default()
            },
            resolution: ResolutionParams {
                mode: ResolutionMode::Super2x2,
            },
            hdr: HdrParams {
                enabled: true,
                target: HdrTarget::Hdr,
                ..HdrParams::default()
            },
            ..EditState::default()
        };
        edit.tone.exposure_ev = 0.7;
        edit.tone.brightness = 11.0;
        edit.tone.whites = -9.0;
        edit.tone.blacks = 13.0;

        DevelopPreset::FilmWarm.apply(&mut edit);

        assert_eq!(edit.profile, ProfileChoice::Named("My camera".into()));
        assert_eq!(edit.wb.temp_kelvin, Some(4_800.0));
        assert_eq!(edit.tone.exposure_ev, 0.7);
        assert_eq!(edit.tone.brightness, 11.0);
        assert_eq!(edit.tone.whites, -9.0);
        assert_eq!(edit.tone.blacks, 13.0);
        assert_eq!(edit.nr.chroma, 33.0);
        assert_eq!(edit.lens.vignetting, 42.0);
        assert_eq!(edit.lens.profile.as_deref(), Some("My lens"));
        assert_eq!(edit.geometry.crop, [0.1, 0.2, 0.9, 0.8]);
        assert_eq!(edit.resolution.mode, ResolutionMode::Super2x2);
        assert!(edit.hdr.enabled);
    }

    #[test]
    fn curated_list_adds_distinct_golden_hour_and_neon_looks() {
        assert_eq!(DevelopPreset::ALL.len(), 13);
        for preset in [
            DevelopPreset::Nature,
            DevelopPreset::Product,
            DevelopPreset::VividColor,
            DevelopPreset::GoldenHourRadiance,
            DevelopPreset::NeonNight,
        ] {
            assert_eq!(
                DevelopPreset::ALL.iter().filter(|&&p| p == preset).count(),
                1
            );
        }
        let mut golden = EditState::default();
        DevelopPreset::GoldenHourRadiance.apply(&mut golden);
        let mut film = EditState::default();
        DevelopPreset::FilmWarm.apply(&mut film);
        assert!(golden.look.is_none());
        assert!(golden.color.vibrance > film.color.vibrance);
        assert!(golden.color.grading.highlights.saturation > 0.0);
        let mut neon = EditState::default();
        DevelopPreset::NeonNight.apply(&mut neon);
        assert!(neon.color.hsl[4].saturation > neon.color.hsl[3].saturation);
        assert!(neon.color.hsl[7].saturation > 0.0);
        assert!(neon.tone.highlights < 0.0 && neon.tone.shadows < 0.0);
    }

    #[test]
    fn presets_preserve_disabled_hdr_exactly_and_never_change_intent() {
        for enabled in [false, true] {
            for target in [HdrTarget::Sdr, HdrTarget::Hdr] {
                for preset in DevelopPreset::ALL {
                    let mut edit = EditState {
                        hdr: HdrParams {
                            enabled,
                            target,
                            headroom_stops: 3.2,
                            highlight_rolloff: 23.0,
                            scene_white_ev: 0.7,
                        },
                        ..EditState::default()
                    };
                    let before = edit.hdr;
                    preset.apply(&mut edit);
                    let expected_rolloff = match (enabled, preset) {
                        (true, DevelopPreset::GoldenHourRadiance) => 55.0,
                        (true, DevelopPreset::NeonNight) => 65.0,
                        _ => before.highlight_rolloff,
                    };
                    assert_eq!(
                        edit.hdr,
                        HdrParams {
                            highlight_rolloff: expected_rolloff,
                            ..before
                        },
                        "{} enabled={enabled} target={target:?}",
                        preset.name()
                    );
                }
            }
        }
    }

    #[test]
    fn new_recipes_preserve_image_specific_values() {
        for preset in [DevelopPreset::GoldenHourRadiance, DevelopPreset::NeonNight] {
            let mut edit = EditState {
                profile: ProfileChoice::Named("Camera".into()),
                ..EditState::default()
            };
            edit.wb.tint = 17.0;
            edit.tone.exposure_ev = 1.2;
            edit.tone.brightness = 9.0;
            edit.tone.whites = -11.0;
            edit.tone.blacks = 7.0;
            edit.nr.chroma = 31.0;
            edit.lens.vignetting = 27.0;
            edit.geometry.crop = [0.1, 0.2, 0.8, 0.9];
            edit.resolution.mode = ResolutionMode::Super2x2;
            let before = edit.clone();
            preset.apply(&mut edit);
            assert_eq!(edit.profile, before.profile);
            assert_eq!(edit.wb, before.wb);
            assert_eq!(edit.tone.exposure_ev, before.tone.exposure_ev);
            assert_eq!(edit.tone.brightness, before.tone.brightness);
            assert_eq!(edit.tone.whites, before.tone.whites);
            assert_eq!(edit.tone.blacks, before.tone.blacks);
            assert_eq!(edit.nr, before.nr);
            assert_eq!(edit.lens, before.lens);
            assert_eq!(edit.geometry, before.geometry);
            assert_eq!(edit.resolution, before.resolution);
        }
    }

    #[test]
    fn monochrome_preset_enables_mixer() {
        let mut edit = EditState::default();
        DevelopPreset::BlackAndWhite.apply(&mut edit);
        assert!(edit.color.monochrome);
        assert!(edit.color.monochrome_mix.iter().any(|value| *value != 0.0));
    }

    #[test]
    fn utility_and_product_recipes_override_only_declared_endpoints() {
        let starting = |preset: DevelopPreset| {
            let mut edit = EditState::default();
            edit.tone.whites = -9.0;
            edit.tone.blacks = 13.0;
            preset.apply(&mut edit);
            edit
        };

        let product = starting(DevelopPreset::Product);
        assert_eq!(product.tone.whites, 2.0);
        assert_eq!(product.tone.blacks, -2.0);

        let recovery = starting(DevelopPreset::HighlightRecovery);
        assert_eq!(recovery.tone.whites, -10.0);
        assert_eq!(recovery.tone.blacks, 13.0);

        let nature = starting(DevelopPreset::Nature);
        assert_eq!(nature.tone.whites, -9.0);
        assert_eq!(nature.tone.blacks, 13.0);
    }

    #[test]
    fn every_named_lut_is_real_and_applied() {
        for preset in DevelopPreset::ALL {
            let Some(name) = preset.look_name() else {
                continue;
            };
            let mut edit = EditState::default();
            preset.apply(&mut edit);
            assert_eq!(edit.look.as_deref(), Some(name));
            assert!(crate::profile::Look::named(name)
                .and_then(|look| look.huesat)
                .is_some());
        }
    }
}
