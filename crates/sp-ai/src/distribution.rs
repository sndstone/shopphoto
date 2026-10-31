//! On-demand, checksum-pinned installation of the ShopPhoto denoise model.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

#[cfg(test)]
const MAX_MODEL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

// The reader has no implicit ureq read_to_vec limit. Keep a bounded buffer and
// verify the digest only after the complete response has reached the staging file.
fn stream_asset(
    mut input: impl Read,
    path: &Path,
    checksum: &str,
    limit: u64,
    received_bytes: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut received = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        received = received
            .checked_add(count as u64)
            .ok_or("model size overflow")?;
        if received > limit {
            return Err(format!("model asset exceeds {limit} byte limit"));
        }
        hash.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
        received_bytes(received);
    }
    if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(checksum) {
        return Err(format!("checksum mismatch for {}", path.display()));
    }
    output.sync_all().map_err(|error| error.to_string())
}

/// A pinned release asset of ShopPhoto's own `rawdenoise-lab` model.
struct ReleaseAsset {
    url: &'static str,
    sha256: &'static str,
    bytes: u64,
}

const RAWDENOISE_RELEASE: &str =
    "https://github.com/sndstone/rawdenoise-lab/releases/download/model-r4_plain-2026-10-30";

#[cfg(target_os = "macos")]
const SHOPPHOTO_ASSET: ReleaseAsset = ReleaseAsset {
    url: "r4_plain_fp16.mlpackage.zip",
    sha256: "664b4a3da56dcb7e447f30d29fc42e6675c50b238707ee38c243ee189d84caa7",
    bytes: 43_804_185,
};
#[cfg(not(target_os = "macos"))]
const SHOPPHOTO_ASSET: ReleaseAsset = ReleaseAsset {
    url: "r4_plain_fp32.onnx",
    sha256: "2f65a38a88144042821e7801c4f1adb620fcf8f50decaf2edfbe7c213edb2fae",
    bytes: 95_553_737,
};

/// Download the ShopPhoto model (`rawdenoise-lab` r4_plain) beside any
/// other installed models, verify its pinned SHA-256 and, on macOS, compile
/// the Core ML package on this Mac. `progress` receives 0..=1.
pub fn install_shopphoto_model(
    destination: &Path,
    mut progress: impl FnMut(f32),
) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let staging = tempfile::Builder::new()
        .prefix(".shopphoto-rawdenoise-")
        .tempdir_in(destination)
        .map_err(|error| error.to_string())?;
    let result = stage_shopphoto_model(staging.path(), destination, &mut progress);
    // The release archive unpacks read-only; TempDir could not remove it.
    make_writable(staging.path());
    result
}

fn make_writable(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    let mut permissions = metadata.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    let _ = std::fs::set_permissions(path, permissions);
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            make_writable(&entry.path());
        }
    }
}

fn stage_shopphoto_model(
    staging: &Path,
    destination: &Path,
    progress: &mut impl FnMut(f32),
) -> Result<std::path::PathBuf, String> {
    let asset = SHOPPHOTO_ASSET;
    let download = staging.join(asset.url);
    let mut response = ureq::get(&format!("{RAWDENOISE_RELEASE}/{}", asset.url))
        .header("User-Agent", "ShopPhoto model installer")
        .call()
        .map_err(|error| error.to_string())?;
    stream_asset(
        response.body_mut().as_reader(),
        &download,
        asset.sha256,
        asset.bytes,
        &mut |received| progress(0.9 * received as f32 / asset.bytes as f32),
    )?;
    let name = crate::model_file_name();
    let target = destination.join(name);
    #[cfg(target_os = "macos")]
    let staged = {
        let unpacked = staging.join("package");
        let status = std::process::Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&download)
            .arg(&unpacked)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err("could not unpack the Core ML package".into());
        }
        let package = std::fs::read_dir(&unpacked)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "mlpackage"))
            .ok_or("the release archive has no Core ML package")?;
        let compiled = crate::compile_core_ml(&package)?;
        let staged = staging.join(name);
        // Core ML compiles into a system temporary directory, which may be
        // another volume; copy before the atomic rename.
        let status = std::process::Command::new("/usr/bin/ditto")
            .arg(&compiled)
            .arg(&staged)
            .status()
            .map_err(|error| error.to_string())?;
        let _ = std::fs::remove_dir_all(&compiled);
        if !status.success() {
            return Err("could not stage the compiled model".into());
        }
        staged
    };
    #[cfg(not(target_os = "macos"))]
    let staged = download;
    // Swap the new model in; any previous copy is removed with the staging
    // directory.
    if target.exists() {
        std::fs::rename(&target, staging.join("previous")).map_err(|error| error.to_string())?;
    }
    std::fs::rename(&staged, &target).map_err(|error| error.to_string())?;
    progress(1.0);
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_assets_larger_than_ten_mib_and_enforces_size_and_hash() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = vec![42u8; 11 * 1024 * 1024];
        let checksum = format!("{:x}", Sha256::digest(&bytes));
        let path = directory.path().join("large.onnx");
        let mut reported = 0;
        stream_asset(
            bytes.as_slice(),
            &path,
            &checksum,
            MAX_MODEL_BYTES,
            &mut |bytes| reported = bytes,
        )
        .unwrap();
        assert_eq!(reported, bytes.len() as u64);
        assert_eq!(std::fs::metadata(path).unwrap().len(), bytes.len() as u64);
        assert!(stream_asset(
            bytes.as_slice(),
            &directory.path().join("limited"),
            &checksum,
            1024,
            &mut |_| {}
        )
        .unwrap_err()
        .contains("limit"));
        assert!(stream_asset(
            bytes.as_slice(),
            &directory.path().join("bad-hash"),
            &"00".repeat(32),
            MAX_MODEL_BYTES,
            &mut |_| {}
        )
        .unwrap_err()
        .contains("checksum"));
    }
}
