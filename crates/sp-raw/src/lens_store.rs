//! Lens shading profile persistence: versioned JSON metadata plus a
//! companion binary blob for the gain grids.
//!
//! Grids are large — a 117×88 grid is ~41k floats per calibrated aperture —
//! so they live in a sibling `.splensdata` file rather than inflating the
//! JSON by an order of magnitude. Both writes are atomic (temp + rename),
//! matching `profile_store`.

use serde::{Deserialize, Serialize};
use sp_core::lens::{LensProfile, ShadingGrid, QUAD_SLOTS};
use sp_core::SpError;
use std::path::{Path, PathBuf};

const LENS_VERSION: u32 = 1;
const GRID_MAGIC: &[u8; 8] = b"SPLENS01";
/// Refuse absurd headers rather than trying to allocate from them.
const MAX_CELLS: u64 = 4_000_000;
const MAX_APERTURES: u32 = 64;

#[derive(Serialize, Deserialize)]
struct StoredLens {
    version: u32,
    /// The profile with `apertures` emptied — grids live in `grid_file`.
    profile: LensProfile,
    grid_file: String,
}

fn corrupt(path: &Path, reason: impl Into<String>) -> SpError {
    SpError::CorruptData {
        path: path.display().to_string(),
        reason: reason.into(),
    }
}

fn grid_path(path: &Path) -> PathBuf {
    path.with_extension("splensdata")
}

/// File extension for lens profiles.
pub const LENS_EXTENSION: &str = "splens";

/// Save a `.splens` profile and its binary grid companion, atomically.
pub fn save_lens(path: &Path, profile: &LensProfile) -> Result<(), SpError> {
    if !profile.is_valid() {
        return Err(corrupt(path, "lens profile grids do not match its shape"));
    }
    if profile.apertures.len() > MAX_APERTURES as usize {
        return Err(corrupt(path, "too many calibrated apertures"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SpError::io(parent.display().to_string(), e))?;
    }

    let companion = grid_path(path);
    let cells = profile.cell_count();
    let mut bytes = Vec::with_capacity(20 + profile.apertures.len() * (4 + cells * 16));
    bytes.extend_from_slice(GRID_MAGIC);
    bytes.extend_from_slice(&profile.grid_w.to_le_bytes());
    bytes.extend_from_slice(&profile.grid_h.to_le_bytes());
    bytes.extend_from_slice(&(profile.apertures.len() as u32).to_le_bytes());
    for aperture in &profile.apertures {
        bytes.extend_from_slice(&aperture.f_number.to_le_bytes());
        for cell in &aperture.gains {
            for gain in cell {
                bytes.extend_from_slice(&gain.to_le_bytes());
            }
        }
    }
    let tmp = companion.with_extension("splensdata.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| SpError::io(tmp.display().to_string(), e))?;
    std::fs::rename(&tmp, &companion)
        .map_err(|e| SpError::io(companion.display().to_string(), e))?;

    let mut metadata = profile.clone();
    metadata.apertures.clear();
    let stored = StoredLens {
        version: LENS_VERSION,
        profile: metadata,
        grid_file: companion
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| corrupt(path, "companion path has no file name"))?
            .to_owned(),
    };
    let bytes = serde_json::to_vec_pretty(&stored)
        .map_err(|e| corrupt(path, format!("cannot serialize lens profile: {e}")))?;
    let tmp = path.with_extension("splens.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| SpError::io(tmp.display().to_string(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| SpError::io(path.display().to_string(), e))
}

/// Load a `.splens` profile and its binary grid companion.
pub fn load_lens(path: &Path) -> Result<LensProfile, SpError> {
    let bytes = std::fs::read(path).map_err(|e| SpError::io(path.display().to_string(), e))?;
    let stored: StoredLens = serde_json::from_slice(&bytes)
        .map_err(|e| corrupt(path, format!("invalid lens JSON: {e}")))?;
    if stored.version != LENS_VERSION {
        return Err(corrupt(
            path,
            format!("unsupported lens profile version {}", stored.version),
        ));
    }
    let mut profile = stored.profile;

    let companion = crate::profile_store::sibling_file(path, &stored.grid_file)?;
    let bytes =
        std::fs::read(&companion).map_err(|e| SpError::io(companion.display().to_string(), e))?;
    if bytes.get(..8) != Some(GRID_MAGIC) || bytes.len() < 20 {
        return Err(corrupt(&companion, "invalid lens grid header"));
    }
    let read_u32 =
        |at: usize| -> u32 { u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4])) };
    let (grid_w, grid_h, count) = (read_u32(8), read_u32(12), read_u32(16));
    if grid_w != profile.grid_w || grid_h != profile.grid_h {
        return Err(corrupt(
            &companion,
            "lens grid dimensions disagree with the profile",
        ));
    }
    if count == 0 || count > MAX_APERTURES {
        return Err(corrupt(&companion, "implausible aperture count"));
    }
    let cells = u64::from(grid_w) * u64::from(grid_h);
    if cells == 0 || cells > MAX_CELLS {
        return Err(corrupt(&companion, "implausible grid size"));
    }
    let stride = 4 + (cells as usize) * QUAD_SLOTS * 4;
    let expected = 20 + (count as usize) * stride;
    if bytes.len() != expected {
        return Err(corrupt(&companion, "lens grid length mismatch"));
    }

    profile.apertures = (0..count as usize)
        .map(|i| {
            let base = 20 + i * stride;
            let f_number = f32::from_le_bytes(bytes[base..base + 4].try_into().unwrap_or([0; 4]));
            let gains = bytes[base + 4..base + stride]
                .chunks_exact(QUAD_SLOTS * 4)
                .map(|chunk| {
                    std::array::from_fn(|s| {
                        f32::from_le_bytes(chunk[s * 4..s * 4 + 4].try_into().unwrap_or([0; 4]))
                    })
                })
                .collect();
            ShadingGrid { f_number, gains }
        })
        .collect();
    profile
        .apertures
        .sort_by(|a, b| a.f_number.total_cmp(&b.f_number));

    if !profile.is_valid() {
        return Err(corrupt(path, "loaded lens profile is not self-consistent"));
    }
    Ok(profile)
}

/// Load every `.splens` profile in `dir`, skipping unreadable ones.
///
/// A corrupt profile must never stop the app from opening images, so this
/// reports failures to the caller for display and returns what it could
/// read.
#[must_use]
pub fn load_lens_dir(dir: &Path) -> (Vec<LensProfile>, Vec<String>) {
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (profiles, errors);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(LENS_EXTENSION) {
            continue;
        }
        match load_lens(&path) {
            Ok(profile) => profiles.push(profile),
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    (profiles, errors)
}

/// The best profile in `profiles` for a shot on `lens_model` at `focal_mm`.
///
/// Prefers a focal-length-specific (zoom) profile over a generic one, since
/// a zoom map calibrated at the shot's focal length is strictly better than
/// one that ignores focal length.
#[must_use]
pub fn match_lens<'a>(
    profiles: &'a [LensProfile],
    lens_model: Option<&str>,
    focal_mm: Option<f32>,
) -> Option<&'a LensProfile> {
    let model = lens_model?;
    profiles
        .iter()
        .filter(|p| p.matches(model, focal_mm))
        .min_by_key(|p| u8::from(p.focal_mm.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_core::lens::GridGeometry;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sp-lens-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sample(name: &str, key: &str) -> LensProfile {
        let geom = GridGeometry::for_sensor(400, 300, 100.0);
        let cells = (geom.grid_w * geom.grid_h) as usize;
        LensProfile {
            apertures: vec![
                ShadingGrid {
                    f_number: 4.0,
                    gains: (0..cells)
                        .map(|i| [1.0 + i as f32 * 0.01; QUAD_SLOTS])
                        .collect(),
                },
                ShadingGrid {
                    f_number: 11.0,
                    gains: vec![[1.05; QUAD_SLOTS]; cells],
                },
            ],
            ..LensProfile::identity(name, key, geom)
        }
    }

    #[test]
    fn lens_profile_and_grids_round_trip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("xcd90.splens");
        let profile = sample("XCD 90mm", "xcd90");
        save_lens(&path, &profile).expect("save");
        assert_eq!(load_lens(&path).expect("load"), profile);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(grid_path(&path));
        let _ = std::fs::remove_dir(dir);
    }

    #[cfg(unix)]
    #[test]
    fn companion_symlink_cannot_escape_the_profile_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.splens");
        save_lens(&path, &sample("XCD 55V", "xcd55v")).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::copy(grid_path(&path), outside.path()).unwrap();
        std::fs::remove_file(grid_path(&path)).unwrap();
        std::os::unix::fs::symlink(outside.path(), grid_path(&path)).unwrap();
        assert!(load_lens(&path).is_err());
    }

    #[test]
    fn a_truncated_grid_is_rejected_not_guessed() {
        let dir = temp_dir("truncated");
        let path = dir.join("xcd55.splens");
        save_lens(&path, &sample("XCD 55V", "xcd55v")).expect("save");
        let companion = grid_path(&path);
        let mut bytes = std::fs::read(&companion).expect("read");
        bytes.truncate(bytes.len() - 16);
        std::fs::write(&companion, bytes).expect("write");
        assert!(load_lens(&path).is_err());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&companion);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn a_bad_header_is_rejected() {
        let dir = temp_dir("header");
        let path = dir.join("xcd21.splens");
        save_lens(&path, &sample("XCD 21mm", "xcd21")).expect("save");
        let companion = grid_path(&path);
        std::fs::write(&companion, b"not a lens grid at all!!").expect("write");
        assert!(load_lens(&path).is_err());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&companion);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn directory_scan_skips_broken_profiles_and_reports_them() {
        let dir = temp_dir("scan");
        save_lens(&dir.join("good.splens"), &sample("XCD 80mm", "xcd80")).expect("save");
        std::fs::write(dir.join("bad.splens"), b"{ not json").expect("write");
        std::fs::write(dir.join("ignored.txt"), b"hello").expect("write");
        let (profiles, errors) = load_lens_dir(&dir);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "XCD 80mm");
        assert_eq!(errors.len(), 1, "the broken profile must be reported");
        for name in [
            "good.splens",
            "good.splensdata",
            "bad.splens",
            "ignored.txt",
        ] {
            let _ = std::fs::remove_file(dir.join(name));
        }
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn matching_prefers_a_focal_specific_profile() {
        let geom = GridGeometry::for_sensor(400, 300, 100.0);
        let generic = LensProfile::identity("XCD 20-35E", "xcd2035", geom);
        let mut specific = LensProfile::identity("XCD 20-35E @ 24", "xcd2035", geom);
        specific.focal_mm = Some(24.0);
        let profiles = vec![generic, specific];

        let hit = match_lens(&profiles, Some("XCD2035"), Some(24.0)).expect("match");
        assert_eq!(hit.focal_mm, Some(24.0));
        // At a focal length the zoom map does not cover, the generic one
        // still applies rather than nothing.
        let hit = match_lens(&profiles, Some("XCD2035"), Some(35.0)).expect("match");
        assert!(hit.focal_mm.is_none());
        assert!(match_lens(&profiles, Some("XCD 90mm"), Some(90.0)).is_none());
        assert!(match_lens(&profiles, None, None).is_none());
    }
}
