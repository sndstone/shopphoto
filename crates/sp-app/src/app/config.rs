//! Paths, preference files and the on-disk resources the app loads at start.

use super::*;

/// Directory holding imported camera profiles.
pub(super) fn profiles_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("shopphoto").join("profiles"))
}

/// Directory holding lens shading profiles (`.splens`).
pub(super) fn lenses_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("shopphoto").join("lenses"))
}

/// Load all stored lens profiles at startup. A broken profile is logged and
/// skipped — it must never stop images from opening.
pub(super) fn load_lens_profiles() -> Vec<sp_core::lens::LensProfile> {
    let Some(dir) = lenses_dir() else {
        return vec![];
    };
    let (profiles, errors) = sp_raw::lens_store::load_lens_dir(&dir);
    for error in errors {
        tracing::warn!("skipping bad lens profile: {error}");
    }
    profiles
}

/// Public lens-identification names represented by Phocus' installed XCD
/// catalog. The private GMap payloads are intentionally not copied or parsed:
/// this list fixes missing identification without claiming unavailable
/// calibration data is being applied.
pub(super) fn hasselblad_xcd_lens_catalog() -> Vec<String> {
    [
        "XCD 20-35E",
        "XCD 21",
        "XCD 25",
        "XCD 25V",
        "XCD 28P",
        "XCD 30",
        "XCD 35-75",
        "XCD 35-100E",
        "XCD 38",
        "XCD 38V",
        "XCD 45",
        "XCD 45 II",
        "XCD 45P",
        "XCD 55",
        "XCD 55V",
        "XCD 65",
        "XCD 75P",
        "XCD 80",
        "XCD 90",
        "XCD 90V",
        "XCD 120 Macro",
        "XCD 135 + 1.7x",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub(super) fn rgb8_hsv(rgb: [u8; 3]) -> Option<(f32, f32, f32)> {
    let [r, g, b] = rgb.map(|value| f32::from(value) / 255.0);
    let hi = r.max(g).max(b);
    let lo = r.min(g).min(b);
    let delta = hi - lo;
    if hi < 0.02 || delta < 0.01 {
        return None;
    }
    let sector = if hi == r {
        (g - b) / delta
    } else if hi == g {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    Some(((sector * 60.0).rem_euclid(360.0), delta / hi, hi))
}

/// Convert a displayed sRGB sample to the Oklab a/b coordinates used by a
/// color-range mask. The GPU evaluates the same public-domain transform.
pub(super) fn rgb8_oklab_ab(rgb: [u8; 3]) -> [f32; 2] {
    let [r, g, b] = rgb.map(|value| sp_core::color::srgb_eotf(f32::from(value) / 255.0));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub(super) fn curve_input_from_rgb8(rgb: [u8; 3], channel: crate::state::CurveChannel) -> f32 {
    let encoded = rgb.map(|value| f32::from(value) / 255.0);
    match channel {
        crate::state::CurveChannel::Red => encoded[0],
        crate::state::CurveChannel::Green => encoded[1],
        crate::state::CurveChannel::Blue => encoded[2],
        crate::state::CurveChannel::Luma => {
            let linear = encoded.map(sp_core::color::srgb_eotf);
            sp_core::color::srgb_oetf(0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2])
        }
    }
}

/// The lens profile to apply to a shot, honouring an explicit choice and
/// otherwise auto-matching on the recorded lens.
pub(super) fn resolve_lens<'a>(
    lenses: &'a [sp_core::lens::LensProfile],
    lens: &sp_core::edit::LensParams,
    meta: &sp_core::ImageMeta,
) -> Option<&'a sp_core::lens::LensProfile> {
    if !lens.enable {
        return None;
    }
    // A named profile that has since been deleted falls back to
    // auto-matching rather than silently disabling correction.
    lens.profile
        .as_deref()
        .and_then(|name| lenses.iter().find(|p| p.name == name))
        .or_else(|| {
            sp_raw::lens_store::match_lens(
                lenses,
                lens.lens_model_override
                    .as_deref()
                    .or(meta.lens_model.as_deref()),
                meta.focal_length_mm,
            )
        })
}

pub(super) fn catalog_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("shopphoto").join("catalog.sqlite3"))
}

pub(super) fn thumbnail_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("shopphoto").join("thumbnails"))
}

pub(super) fn source_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("shopphoto").join("network-sources"))
}

pub(super) fn export_presets_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("shopphoto").join("export-presets.json"))
}

pub(super) fn user_presets_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("shopphoto").join("develop-presets.json"))
}

pub(super) fn looks_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("shopphoto").join("looks"))
}

pub(super) fn proof_profiles_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("shopphoto").join("proof-profiles"))
}

pub(super) fn load_custom_proofs() -> Vec<sp_pipeline::proof::CustomProof> {
    let Some(directory) = proof_profiles_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut proofs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_icc = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("icc") || ext.eq_ignore_ascii_case("icm"));
        if !is_icc {
            continue;
        }
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                sp_pipeline::proof::build_custom_proof(name, &bytes, 33)
                    .map_err(|error| error.to_string())
            }) {
            Ok(proof) => proofs.push(proof),
            Err(error) => tracing::warn!(path = %path.display(), "skipping bad proof ICC: {error}"),
        }
    }
    proofs.sort_by(|left, right| left.name.cmp(&right.name));
    proofs
}

pub(super) fn load_custom_looks() -> Vec<(String, sp_core::cube::CubeLut)> {
    let Some(directory) = looks_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut looks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("cube") {
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|source| sp_core::cube::parse(&source).map_err(|error| error.to_string()));
        match parsed {
            Ok(cube) => {
                let name = cube
                    .title
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    });
                looks.retain(|(candidate, _)| candidate != &name);
                looks.push((name, cube));
            }
            Err(error) => tracing::warn!(path = %path.display(), "skipping bad .cube: {error}"),
        }
    }
    looks.sort_by(|left, right| left.0.cmp(&right.0));
    looks
}

pub(super) fn app_preferences_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("shopphoto").join("preferences.json"))
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct AppPreferences {
    #[serde(default)]
    pub(super) proof_space: sp_core::edit::ProofSpace,
    #[serde(default)]
    pub(super) proof_options: sp_pipeline::proof::ProofOptions,
    #[serde(default)]
    pub(super) gamut_warning: bool,
    /// Mirror ratings/flags into `<name>.xmp` for Lightroom/Bridge/Phocus
    /// (BUILD.md §9). Off by default: writing extra files into someone's
    /// photo folders is opt-in. Reading existing XMP is always on, since
    /// that only fills in annotations we would otherwise show as unrated.
    #[serde(default)]
    pub(super) write_xmp: bool,
    #[serde(default)]
    pub(super) raw_denoise_compute: sp_ai::ComputeUnits,
    #[serde(default)]
    pub(super) language: crate::i18n::Language,
    #[serde(default)]
    pub(super) left_rail_tab: crate::state::LeftRailTab,
    #[serde(default = "default_ui_scale")]
    pub(super) ui_scale: f32,
    #[serde(default)]
    pub(super) external_editor: Option<PathBuf>,
    #[serde(default)]
    pub(super) shortcuts: super::customize::Bindings,
    #[serde(default)]
    pub(super) workspaces: Vec<super::customize::Workspace>,
    #[serde(default)]
    pub(super) high_contrast: bool,
    #[serde(default)]
    pub(super) reduced_motion: bool,
}

pub(super) fn load_app_preferences() -> AppPreferences {
    app_preferences_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(super) fn save_app_preferences(preferences: AppPreferences) {
    let Some(path) = app_preferences_path() else {
        return;
    };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&preferences) else {
        return;
    };
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, bytes).is_ok() {
        let _ = std::fs::rename(temp, path);
    }
}

pub(super) fn load_export_presets(
    db_path: Option<&std::path::Path>,
) -> Vec<sp_export::ExportPreset> {
    let json_path = export_presets_path();
    let mut presets = json_path
        .as_deref()
        .and_then(|path| sp_export::load_presets(path).ok())
        .unwrap_or_default();
    if presets.is_empty() {
        if let Some(db_path) = db_path {
            if let Ok(db) = sp_catalog::CatalogDb::open(db_path) {
                presets = db
                    .export_presets()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(name, settings)| {
                        serde_json::from_str(&settings)
                            .ok()
                            .map(|settings| sp_export::ExportPreset { name, settings })
                    })
                    .collect();
            }
        }
    }
    let mut combined = sp_export::ExportPreset::built_ins();
    for preset in presets {
        combined.retain(|candidate| candidate.name != preset.name);
        combined.push(preset);
    }
    combined.sort_by_key(|preset| preset.name.to_lowercase());
    combined
}

/// Load all stored profiles (.dcp and .spprofile JSON) at startup.
pub(super) fn load_profiles() -> Vec<sp_core::profile::CameraProfile> {
    let Some(dir) = profiles_dir() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let loaded = match ext.as_deref() {
            Some("dcp") => sp_raw::dcp::load_dcp(&path).map_err(|e| e.to_string()),
            Some("spprofile") => {
                sp_raw::profile_store::load_profile(&path).map_err(|e| e.to_string())
            }
            _ => continue,
        };
        match loaded {
            Ok(p) => out.push(p),
            Err(e) => tracing::warn!(path = %path.display(), "skipping bad profile: {e}"),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

pub(super) fn camera_key(model: &str) -> String {
    let key: String = model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    key.strip_prefix("hasselblad").unwrap_or(&key).to_owned()
}

pub(super) fn camera_matches(profile_model: &str, image_model: &str) -> bool {
    !profile_model.is_empty() && camera_key(profile_model) == camera_key(image_model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CurveChannel;

    #[test]
    fn raw_compute_preferences_default_for_older_files_and_roundtrip_each_policy() {
        let mut prefs: AppPreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs.raw_denoise_compute, sp_ai::ComputeUnits::Automatic);
        for units in [
            sp_ai::ComputeUnits::Automatic,
            sp_ai::ComputeUnits::CpuGpu,
            sp_ai::ComputeUnits::CpuNeuralEngine,
            sp_ai::ComputeUnits::CpuOnly,
        ] {
            prefs.raw_denoise_compute = units;
            let bytes = serde_json::to_vec(&prefs).unwrap();
            let restored: AppPreferences = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(restored, prefs);
        }
    }

    #[test]
    fn sidebar_preferences_keep_older_workspaces_and_roundtrip_selection() {
        let old = serde_json::json!({
            "ui_scale":1.25,
            "workspaces":[{"name":"Old workspace","panels":{},"rails_hidden":false,"library":false,"ui_scale":1.0}]
        });
        let mut preferences: AppPreferences = serde_json::from_value(old).unwrap();
        assert_eq!(
            preferences.left_rail_tab,
            crate::state::LeftRailTab::Presets
        );
        preferences.left_rail_tab = crate::state::LeftRailTab::Ai;
        let serialized = serde_json::to_value(&preferences).unwrap();
        assert_eq!(serialized["workspaces"][0]["left_rail_tab"], "presets");
        assert_eq!(serialized["left_rail_tab"], "ai");
        let restored: AppPreferences = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored, preferences);
    }

    #[test]
    fn color_dropper_uses_known_oklab_primaries() {
        let red = rgb8_oklab_ab([255, 0, 0]);
        assert!((red[0] - 0.22486).abs() < 0.0002, "{red:?}");
        assert!((red[1] - 0.12585).abs() < 0.0002, "{red:?}");
        assert_eq!(rgb8_oklab_ab([0, 0, 0]), [0.0, 0.0]);
    }

    #[test]
    fn curve_dropper_samples_the_active_channel() {
        let rgb = [64, 128, 192];
        assert_eq!(curve_input_from_rgb8(rgb, CurveChannel::Red), 64.0 / 255.0);
        assert_eq!(
            curve_input_from_rgb8(rgb, CurveChannel::Green),
            128.0 / 255.0
        );
        assert_eq!(
            curve_input_from_rgb8(rgb, CurveChannel::Blue),
            192.0 / 255.0
        );
        let luma = curve_input_from_rgb8(rgb, CurveChannel::Luma);
        assert!(luma > 0.45 && luma < 0.55, "{luma}");
    }
}
