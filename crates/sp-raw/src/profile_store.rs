//! ShopPhoto profile persistence: small versioned JSON metadata plus a
//! companion binary RGB-LUT blob for fitted Reference Match profiles.

use serde::{Deserialize, Serialize};
use sp_core::profile::{CameraProfile, Lut3d};
use sp_core::SpError;
use std::path::{Path, PathBuf};

const PROFILE_VERSION: u32 = 1;
const LUT_MAGIC: &[u8; 8] = b"SPLUT001";

#[derive(Serialize, Deserialize)]
struct StoredProfile {
    version: u32,
    profile: CameraProfile,
    lut_file: Option<String>,
}

fn corrupt(path: &Path, reason: impl Into<String>) -> SpError {
    SpError::CorruptData {
        path: path.display().to_string(),
        reason: reason.into(),
    }
}

/// Resolve only real sibling files, including after following symlinks.
pub(crate) fn sibling_file(path: &Path, file: &str) -> Result<PathBuf, SpError> {
    if file.is_empty()
        || file.contains(['/', '\\'])
        || file.contains("..")
        || !matches!(
            Path::new(file).components().next(),
            Some(std::path::Component::Normal(_))
        )
        || Path::new(file).components().count() != 1
    {
        return Err(corrupt(path, "companion must be a plain sibling file name"));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent =
        std::fs::canonicalize(parent).map_err(|e| SpError::io(parent.display().to_string(), e))?;
    let companion = parent.join(file);
    let resolved = std::fs::canonicalize(&companion)
        .map_err(|e| SpError::io(companion.display().to_string(), e))?;
    if resolved.parent() != Some(parent.as_path()) || !resolved.is_file() {
        return Err(corrupt(
            path,
            "companion resolves outside its profile directory or is not a file",
        ));
    }
    Ok(resolved)
}

fn lut_path(path: &Path) -> PathBuf {
    path.with_extension("splut")
}

/// Save a `.spprofile` atomically. A 33³ RGB LUT is stored in the sibling
/// `.splut` file so JSON remains inspectable and fast to parse.
pub fn save_profile(path: &Path, profile: &CameraProfile) -> Result<(), SpError> {
    if !profile.is_valid() {
        return Err(corrupt(
            path,
            "invalid camera profile matrices, values or LUT shape",
        ));
    }
    let mut metadata = profile.clone();
    let lut = metadata.rgb_lut.take();
    let companion = lut.as_ref().map(|_| lut_path(path));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SpError::io(parent.display().to_string(), e))?;
    }
    if let (Some(lut), Some(companion)) = (&lut, &companion) {
        if lut.entries.len() != lut.expected_len() {
            return Err(corrupt(path, "RGB LUT dimensions do not match its data"));
        }
        let mut bytes = Vec::with_capacity(12 + lut.entries.len() * 12);
        bytes.extend_from_slice(LUT_MAGIC);
        bytes.extend_from_slice(&lut.size.to_le_bytes());
        for rgb in &lut.entries {
            for value in rgb {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let tmp = companion.with_extension("splut.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| SpError::io(tmp.display().to_string(), e))?;
        std::fs::rename(&tmp, companion)
            .map_err(|e| SpError::io(companion.display().to_string(), e))?;
    }
    let stored = StoredProfile {
        version: PROFILE_VERSION,
        profile: metadata,
        lut_file: companion
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(ToOwned::to_owned),
    };
    let bytes = serde_json::to_vec_pretty(&stored)
        .map_err(|e| corrupt(path, format!("cannot serialize profile: {e}")))?;
    let tmp = path.with_extension("spprofile.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| SpError::io(tmp.display().to_string(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| SpError::io(path.display().to_string(), e))
}

/// Load a versioned `.spprofile` and its optional binary LUT companion.
pub fn load_profile(path: &Path) -> Result<CameraProfile, SpError> {
    let bytes = std::fs::read(path).map_err(|e| SpError::io(path.display().to_string(), e))?;
    let stored: StoredProfile = serde_json::from_slice(&bytes)
        .map_err(|e| corrupt(path, format!("invalid profile JSON: {e}")))?;
    if stored.version != PROFILE_VERSION {
        return Err(corrupt(
            path,
            format!("unsupported profile version {}", stored.version),
        ));
    }
    let mut profile = stored.profile;
    if let Some(file) = stored.lut_file {
        let companion = sibling_file(path, &file)?;
        let bytes = std::fs::read(&companion)
            .map_err(|e| SpError::io(companion.display().to_string(), e))?;
        if bytes.get(..8) != Some(LUT_MAGIC) || bytes.len() < 12 {
            return Err(corrupt(&companion, "invalid RGB LUT header"));
        }
        let size = u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| corrupt(&companion, "truncated RGB LUT size"))?,
        );
        if !(2..=65).contains(&size) {
            return Err(corrupt(&companion, "RGB LUT size must be 2..=65"));
        }
        let count = size
            .checked_pow(3)
            .and_then(|n| n.checked_mul(12))
            .ok_or_else(|| corrupt(&companion, "RGB LUT dimensions overflow"))?
            as usize;
        if bytes.len() != 12 + count {
            return Err(corrupt(&companion, "RGB LUT length mismatch"));
        }
        let entries = bytes[12..]
            .chunks_exact(12)
            .map(|chunk| {
                std::array::from_fn(|c| {
                    f32::from_le_bytes(chunk[c * 4..c * 4 + 4].try_into().unwrap_or([0; 4]))
                })
            })
            .collect();
        profile.rgb_lut = Some(Lut3d { size, entries });
    }
    if !profile.is_valid() {
        return Err(corrupt(
            path,
            "invalid camera profile matrices, values or LUT shape",
        ));
    }
    // Matrix interpolation expects CCT order, including imported JSON metadata.
    profile.matrices.sort_by(|a, b| {
        a.illuminant
            .cct_kelvin()
            .total_cmp(&b.illuminant.cct_kelvin())
    });
    profile.forward_matrices.sort_by(|a, b| {
        a.illuminant
            .cct_kelvin()
            .total_cmp(&b.illuminant.cct_kelvin())
    });
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_matrices_and_invalid_lut_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.spprofile");
        let mut profile = CameraProfile::standard("camera", [[1.0; 3]; 3]);
        profile.matrices.clear();
        let stored = StoredProfile {
            version: PROFILE_VERSION,
            profile,
            lut_file: None,
        };
        std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(load_profile(&path).is_err());
        let mut profile = CameraProfile::standard("camera", [[1.0; 3]; 3]);
        profile.rgb_lut = Some(Lut3d {
            size: 0,
            entries: vec![],
        });
        let stored = StoredProfile {
            version: PROFILE_VERSION,
            profile,
            lut_file: None,
        };
        std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(load_profile(&path).is_err());
    }

    #[test]
    fn companion_paths_must_stay_in_the_profile_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.spprofile");
        for name in [
            "../outside.splut",
            "/outside.splut",
            "a/b.splut",
            "a\\b.splut",
            "..",
            ".",
            "",
        ] {
            assert!(sibling_file(&path, name).is_err(), "{name}");
        }
        let companion = dir.path().join("valid.splut");
        std::fs::write(&companion, b"data").unwrap();
        assert_eq!(
            sibling_file(&path, "valid.splut").unwrap(),
            companion.canonicalize().unwrap()
        );
        #[cfg(unix)]
        {
            let outside = tempfile::NamedTempFile::new().unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("escape.splut")).unwrap();
            assert!(sibling_file(&path, "escape.splut").is_err());
        }
    }

    #[test]
    fn binary_lut_rejects_nonfinite_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.spprofile");
        let mut profile = CameraProfile::standard("camera", [[1.0; 3]; 3]);
        profile.rgb_lut = Some(Lut3d::identity(2));
        save_profile(&path, &profile).unwrap();
        let companion = lut_path(&path);
        let mut bytes = std::fs::read(&companion).unwrap();
        bytes[12..16].copy_from_slice(&f32::NAN.to_le_bytes());
        std::fs::write(companion, bytes).unwrap();
        assert!(load_profile(&path).is_err());
    }

    #[test]
    fn profile_and_binary_lut_round_trip() {
        let dir = std::env::temp_dir().join(format!("sp-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.spprofile");
        let mut profile = CameraProfile::standard(
            "camera",
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        );
        profile.name = "Reference Match test".into();
        profile.rgb_lut = Some(Lut3d::identity(3));
        save_profile(&path, &profile).expect("save");
        let loaded = load_profile(&path).expect("load");
        assert_eq!(loaded, profile);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(dir.join("test.splut"));
        let _ = std::fs::remove_dir(dir);
    }
}
