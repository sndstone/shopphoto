//! Disposable inference results, not model storage. Existing sp-ai inference
//! and blend functions are unchanged. Cache identity includes sensor pixels
//! and every model file's revision; changing a blend strength reuses inference.
use sp_core::RawImage;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

fn key(raw: &RawImage, root: &Path, compute: sp_ai::ComputeUnits) -> Option<PathBuf> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"ShopPhoto-inference-cache-compute-v2");
    hash.update(&(compute as i32).to_le_bytes());
    hash.update(bytemuck::cast_slice(&raw.cfa_data));
    hash.update(
        format!(
            "{}:{}:{:?}:{:?}:{:?}:{:?}:{:?}",
            raw.width,
            raw.height,
            raw.cfa,
            raw.active_area,
            raw.black_levels,
            raw.white_levels,
            sp_ai::model_file_name()
        )
        .as_bytes(),
    );
    let mut pending = vec![sp_ai::model_path(root)];
    let mut visited = 0;
    while let Some(path) = pending.pop() {
        visited += 1;
        if visited > 10000 {
            return None;
        }
        let metadata = std::fs::metadata(&path).ok()?;
        hash.update(path.as_os_str().to_string_lossy().as_bytes());
        hash.update(&metadata.len().to_le_bytes());
        hash.update(
            &metadata
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos()
                .to_le_bytes(),
        );
        if metadata.is_dir() {
            let mut children = std::fs::read_dir(path)
                .ok()?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            children.sort();
            pending.extend(children);
        }
    }
    Some(
        dirs::cache_dir()?
            .join("shopphoto/inference-v1")
            .join(format!("{}.cfa", hash.finalize().to_hex())),
    )
}

fn load(path: &Path, raw: &RawImage) -> Option<RawImage> {
    let expected = raw.cfa_data.len().checked_mul(2)?.checked_add(32)?;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() != expected as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(expected as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    // Recheck after reading: a cache entry may change while it is open.
    if bytes.len() != expected {
        return None;
    }
    let (checksum, data) = bytes.split_at(32);
    if checksum != blake3::hash(data).as_bytes() {
        return None;
    }
    let result = RawImage {
        cfa_data: data
            .chunks_exact(2)
            .map(|v| u16::from_ne_bytes([v[0], v[1]]))
            .collect(),
        width: raw.width,
        height: raw.height,
        cfa: raw.cfa,
        black_levels: raw.black_levels,
        white_levels: raw.white_levels,
        wb_coeffs: raw.wb_coeffs,
        cam_to_xyz: raw.cam_to_xyz,
        active_area: raw.active_area,
        orientation: raw.orientation,
    };
    if let Ok(file) = std::fs::File::open(path) {
        let _ =
            file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
    }
    Some(result)
}

pub fn infer(raw: &RawImage, root: &Path) -> Result<Arc<RawImage>, sp_ai::AiError> {
    infer_with_compute(raw, root, sp_ai::ComputeUnits::from_environment())
}

pub fn infer_with_compute(
    raw: &RawImage,
    root: &Path,
    compute: sp_ai::ComputeUnits,
) -> Result<Arc<RawImage>, sp_ai::AiError> {
    if !compute.supported() {
        return Err(sp_ai::AiError::CoreMl(format!(
            "{} is unavailable on this platform",
            compute.label()
        )));
    }
    let path = key(raw, root, compute);
    if let Some(result) = path.as_ref().and_then(|p| load(p, raw)) {
        return Ok(Arc::new(result));
    }
    let result = sp_ai::infer_with_compute(raw, root, compute)?;
    if let Some(path) = path {
        let store = || -> std::io::Result<()> {
            let directory = path.parent().expect("cache parent");
            std::fs::create_dir_all(directory)?;
            let mut file = tempfile::NamedTempFile::new_in(directory)?;
            let data: &[u8] = bytemuck::cast_slice(&result.cfa_data);
            file.write_all(blake3::hash(data).as_bytes())?;
            file.write_all(data)?;
            file.persist(path.clone()).map_err(|e| e.error)?;
            let _ = sp_catalog::thumbnails::enforce_budget(directory, 8 * 1024 * 1024 * 1024);
            Ok(())
        };
        if let Err(error) = store() {
            tracing::warn!("Inference cache unavailable: {error}");
        }
    }
    Ok(Arc::new(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_policies_and_model_revisions_never_share_disk_inference_results() {
        let root = tempfile::tempdir().unwrap();
        // Write inside the temporary root; `model_path` would fall back to a
        // real per-user install when the root lacks the model.
        let model = root.path().join(sp_ai::model_file_name());
        std::fs::write(&model, b"model revision").unwrap();
        let raw = RawImage {
            cfa_data: vec![1; 4],
            width: 2,
            height: 2,
            cfa: sp_core::CfaPattern::Rggb,
            black_levels: [0.; 4],
            white_levels: [65535.; 4],
            wb_coeffs: [1.; 4],
            cam_to_xyz: [[1.; 3]; 3],
            active_area: sp_core::raw::ActiveArea {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            orientation: sp_core::raw::Orientation::Normal,
        };
        let units = [
            sp_ai::ComputeUnits::Automatic,
            sp_ai::ComputeUnits::CpuGpu,
            sp_ai::ComputeUnits::CpuNeuralEngine,
            sp_ai::ComputeUnits::CpuOnly,
        ];
        let paths: std::collections::HashSet<_> = units
            .into_iter()
            .map(|u| key(&raw, root.path(), u).unwrap())
            .collect();
        assert_eq!(paths.len(), 4);
        assert_eq!(
            key(&raw, root.path(), units[0]),
            key(&raw, root.path(), units[0])
        );
        // A replaced model never reuses the previous model's result.
        std::fs::write(&model, b"updated model revision").unwrap();
        let updated = key(&raw, root.path(), units[0]).unwrap();
        assert!(!paths.contains(&updated));
    }

    #[test]
    fn cache_checks_size_checksum_and_preserves_capture_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.cfa");
        let raw = RawImage {
            cfa_data: vec![10; 4],
            width: 2,
            height: 2,
            cfa: sp_core::raw::CfaPattern::Rggb,
            black_levels: [1.0; 4],
            white_levels: [65535.0; 4],
            wb_coeffs: [2.0; 4],
            cam_to_xyz: [[1.0; 3]; 3],
            active_area: sp_core::raw::ActiveArea {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            orientation: sp_core::raw::Orientation::Rotate90,
        };
        let pixels = [100u16, 200, 300, 400];
        let data: &[u8] = bytemuck::cast_slice(&pixels);
        let mut bytes = blake3::hash(data).as_bytes().to_vec();
        bytes.extend_from_slice(data);
        std::fs::write(&path, &bytes).unwrap();
        let loaded = load(&path, &raw).unwrap();
        assert_eq!(loaded.cfa_data, pixels);
        assert_eq!(loaded.wb_coeffs, raw.wb_coeffs);
        assert_eq!(loaded.orientation, raw.orientation);
        bytes[32] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(load(&path, &raw).is_none());
        std::fs::write(&path, &bytes[..10]).unwrap();
        assert!(load(&path, &raw).is_none());
    }
}
