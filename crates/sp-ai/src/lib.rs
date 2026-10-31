//! ShopPhoto's neural RAW denoise: its own `rawdenoise-lab` NAFNet
//! (`r4_plain`) on Bayer data before demosaic. macOS runs the compiled Core ML
//! model; Windows and Linux run the ONNX export through tract.

mod compute;
mod defects;
pub mod distribution;
pub mod enhancement;
mod model_session;
pub use compute::ComputeUnits;

use sp_core::RawImage;
use std::path::{Path, PathBuf};

const TILE: usize = 512;
const PLANES: usize = 4;
/// Tile overlap and mirrored apron in packed pixels, the `rawdenoise` CLI
/// default.
const OVERLAP: usize = 60;

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("Core ML denoise is only available on macOS")]
    Platform,
    #[error("model load or inference failed: {0}")]
    CoreMl(String),
}

/// Core ML build of `rawdenoise-lab`'s `r4_plain` release, compiled from
/// `r4_plain_fp16.mlpackage` (see `scripts/install-rawdenoise-model.sh`).
pub const CORE_ML_MODEL: &str = "rawdenoise_r4_plain.mlmodelc";
/// The same release's `r4_plain_fp32.onnx`.
pub const ONNX_MODEL: &str = "rawdenoise_r4_plain.onnx";

/// This platform's model file name: Core ML on macOS, ONNX elsewhere.
#[must_use]
pub fn model_file_name() -> &'static str {
    #[cfg(target_os = "macos")]
    return CORE_ML_MODEL;
    #[cfg(not(target_os = "macos"))]
    return ONNX_MODEL;
}

/// Whether the denoise model is installed (bundled or for this user).
#[must_use]
pub fn model_installed() -> bool {
    model_root().is_some_and(|root| model_path(&root).exists())
}

/// Compile a Core ML `.mlpackage` with the system framework (no Xcode
/// needed). Returns the compiled `.mlmodelc` in a temporary directory.
#[cfg(target_os = "macos")]
fn compile_core_ml(package: &Path) -> Result<PathBuf, String> {
    use std::ffi::{c_char, CStr, CString};
    unsafe extern "C" {
        fn sp_ai_model_compile(
            path: *const c_char,
            output: *mut c_char,
            output_len: usize,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
    }
    let path = CString::new(package.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let mut output = [0 as c_char; 4096];
    let mut error = [0 as c_char; 512];
    let ok = unsafe {
        sp_ai_model_compile(
            path.as_ptr(),
            output.as_mut_ptr(),
            output.len(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned());
    }
    Ok(PathBuf::from(
        unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    ))
}

pub fn model_root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SHOPPHOTO_DENOISE_MODELS") {
        return Some(path.into());
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|executable| bundled_model_root(&executable))
        .filter(|path| path.join(model_file_name()).exists())
    {
        return Some(path);
    }
    model_install_root()
}

/// Writable destination for models installed for the current user. This is
/// deliberately separate from [`model_root`]: the signed app bundle carries
/// the model, but an on-demand install must never modify it.
pub fn model_install_root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SHOPPHOTO_DENOISE_MODELS") {
        return Some(path.into());
    }
    dirs::data_local_dir().map(|directory| directory.join("shopphoto").join("denoise-models"))
}

fn bundled_model_root(executable: &Path) -> Option<PathBuf> {
    let contents = executable.parent()?.parent()?;
    Some(contents.join("Resources").join("DenoiseModels"))
}

/// The model file under `root`, falling back to the per-user install
/// directory when `root` lacks it.
#[must_use]
pub fn model_path(root: &Path) -> PathBuf {
    let name = model_file_name();
    let path = root.join(name);
    if !path.exists() {
        if let Some(installed) = model_install_root()
            .map(|installed| installed.join(name))
            .filter(|installed| installed.exists())
        {
            return installed;
        }
    }
    path
}

/// Run the model at full strength and blend the result into `raw`.
pub fn denoise(raw: &RawImage, root: &Path, strength: f32) -> Result<RawImage, AiError> {
    let inferred = infer(raw, root)?;
    Ok(blend(raw, &inferred, strength))
}

/// The model's own output for `raw`, unblended. Inference dominates the
/// cost (about 1.7 s for 50 MP and 3.3 s for 100 MP on Apple Silicon) and
/// does not depend on the blend mode, so callers keep this and re-[`blend`]
/// when the mode changes.
pub fn infer(raw: &RawImage, root: &Path) -> Result<RawImage, AiError> {
    infer_with_compute(raw, root, ComputeUnits::from_environment())
}

/// Run with an explicit hardware policy, without changing process-wide environment.
pub fn infer_with_compute(
    raw: &RawImage,
    root: &Path,
    compute: ComputeUnits,
) -> Result<RawImage, AiError> {
    if !compute.supported() {
        return Err(AiError::CoreMl(format!(
            "{} is unavailable on this platform",
            compute.label()
        )));
    }
    let area = raw.active_area;
    let end_x = area
        .x
        .checked_add(area.width)
        .filter(|&end| end <= raw.width);
    let end_y = area
        .y
        .checked_add(area.height)
        .filter(|&end| end <= raw.height);
    if area.width < 2
        || area.height < 2
        || end_x.is_none()
        || end_y.is_none()
        || (raw.width as usize).checked_mul(raw.height as usize) != Some(raw.cfa_data.len())
    {
        return Err(AiError::CoreMl(
            "Invalid RAW sensor dimensions or active area".into(),
        ));
    }
    let (ox, oy) = red_quad_origin(raw);
    let qw = (end_x.unwrap_or(0) as usize - ox) / 2;
    let qh = (end_y.unwrap_or(0) as usize - oy) / 2;
    if qw == 0 || qh == 0 {
        return Err(AiError::CoreMl(
            "RAW active area is too small for the denoise model".into(),
        ));
    }
    let mut result = raw.clone();
    // The reference CLI corrects isolated stuck photosites before inference;
    // blending still mixes against the untouched source.
    defects::correct(&mut result, (ox, oy), (qw, qh));
    denoise_impl(&mut result, model_path(root), compute)?;
    Ok(result)
}

/// Per-photosite linear mix in raw codes: `source + (inferred − source) ×
/// strength`. Strength 0 returns the source, 1 the inference.
#[must_use]
pub fn blend(source: &RawImage, inferred: &RawImage, strength: f32) -> RawImage {
    let strength = strength.clamp(0.0, 1.0);
    let mut out = source.clone();
    // A NaN strength would cast every mixed sample to zero. Inference from a
    // different sensor layout cannot be mixed photosite-for-photosite either.
    if strength.is_nan()
        || inferred.width != source.width
        || inferred.height != source.height
        || inferred.cfa != source.cfa
        || inferred.active_area != source.active_area
        || inferred.cfa_data.len() != source.cfa_data.len()
    {
        return out;
    }
    for ((dst, &src), &ai) in out
        .cfa_data
        .iter_mut()
        .zip(&source.cfa_data)
        .zip(&inferred.cfa_data)
    {
        let (src, ai) = (f32::from(src), f32::from(ai));
        *dst = (src + (ai - src) * strength)
            .round()
            .clamp(0.0, f32::from(u16::MAX)) as u16;
    }
    out
}

/// Model channel → photosite offset inside a Bayer quad whose top-left
/// photosite is red: the channels are ordered R, G1, B, G2, i.e. top-left,
/// top-right, bottom-right, bottom-left. This is *not* row-major quad order.
///
/// `rawdenoise-lab` trains with this packing; row-major order gives a
/// magenta cast of about +2 % red/blue.
const PLANE_OFFSETS: [(usize, usize); PLANES] = [(0, 0), (1, 0), (1, 1), (0, 1)];

/// Sensor coordinate of the first red-topped quad inside the active area,
/// so that model channel 0 always sees red regardless of the file's CFA
/// phase. Bayer patterns repeat every two photosites, so the search only
/// needs the four phases.
fn red_quad_origin(raw: &RawImage) -> (usize, usize) {
    let a = raw.active_area;
    for dy in 0..2 {
        for dx in 0..2 {
            if raw.cfa.color_at(a.x + dx, a.y + dy) == 0 {
                return ((a.x + dx) as usize, (a.y + dy) as usize);
            }
        }
    }
    (a.x as usize, a.y as usize)
}

/// Quad coordinate for the tile apron outside the image: mirror about the
/// edge without repeating the edge row (`-1 → 1`). The NAFNet blocks pool
/// channel statistics over the whole 512×512 tile, so the apron must look
/// natural: a replicated, noise-free edge band shifts the color of the whole
/// tile row by up to 2 %. Mirrored rows keep natural noise and gradients.
fn mirror(i: isize, n: usize) -> usize {
    let n = n as isize;
    if n <= 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut i = i.rem_euclid(period);
    if i >= n {
        i = period - i;
    }
    i as usize
}

/// RAW calibration arrays are RGBE ordered; translate the absolute sensor
/// coordinate to its calibration channel (both greens share slot 1).
fn calibration_channel(raw: &RawImage, x: usize, y: usize) -> usize {
    usize::from(raw.cfa.color_at(x as u32, y as u32)).min(2)
}

/// Tile origins along one packed axis of length `n`, as in
/// `denoise_lab.tiling.tiled_inference`: an overlap-wide apron, then whole
/// strides until the mirrored extent covers the image.
fn tile_starts(n: usize) -> Vec<isize> {
    let stride = TILE - OVERLAP;
    let steps = (n + 2 * OVERLAP).saturating_sub(TILE).div_ceil(stride);
    (0..=steps)
        .map(|index| (index * stride) as isize - OVERLAP as isize)
        .collect()
}

/// The reference merge weight: a linear `(i + 1) / (overlap + 1)` ramp on
/// each tile edge, the 2-D product raised to 2.2. Indexed `y * TILE + x`.
fn feather() -> Vec<f32> {
    let axis: Vec<f32> = (0..TILE)
        .map(|index| {
            let edge = index.min(TILE - 1 - index);
            if edge < OVERLAP {
                (edge + 1) as f32 / (OVERLAP + 1) as f32
            } else {
                1.0
            }
        })
        .collect();
    let mut weights = Vec::with_capacity(TILE * TILE);
    for y in &axis {
        for x in &axis {
            weights.push((y * x).powf(2.2));
        }
    }
    weights
}

/// Pack every tile as normalized R/G1/B/G2 planes, run `predict` and write
/// the feathered merge back into `raw`.
fn run_tiles(
    raw: &mut RawImage,
    mut predict: impl FnMut(&[f32], &mut [f32]) -> Result<(), AiError>,
) -> Result<(), AiError> {
    let a = raw.active_area;
    let (ox, oy) = red_quad_origin(raw);
    let qw = ((a.x + a.width) as usize - ox) / 2;
    let qh = ((a.y + a.height) as usize - oy) / 2;
    let weights = feather();
    let mut sum = vec![0f32; qw * qh * PLANES];
    let mut count = vec![0f32; qw * qh];
    let mut input = vec![0f32; TILE * TILE * PLANES];
    let mut output = input.clone();
    for ty in tile_starts(qh) {
        for tx in tile_starts(qw) {
            for c in 0..PLANES {
                for y in 0..TILE {
                    for x in 0..TILE {
                        let qx = mirror(tx + x as isize, qw);
                        let qy = mirror(ty + y as isize, qh);
                        let (dx, dy) = PLANE_OFFSETS[c];
                        let sx = ox + qx * 2 + dx;
                        let sy = oy + qy * 2 + dy;
                        let calibration = calibration_channel(raw, sx, sy);
                        let black = raw.black_levels[calibration];
                        let white = raw.white_levels[calibration];
                        input[(c * TILE + y) * TILE + x] =
                            (raw.cfa_data[sy * raw.width as usize + sx] as f32 - black)
                                / (white - black).max(1.0);
                    }
                }
            }
            predict(&input, &mut output)?;
            for y in 0..TILE {
                for x in 0..TILE {
                    let qx = tx + x as isize;
                    let qy = ty + y as isize;
                    if qx < 0 || qy < 0 || qx >= qw as isize || qy >= qh as isize {
                        continue;
                    }
                    let q = qy as usize * qw + qx as usize;
                    let weight = weights[y * TILE + x];
                    count[q] += weight;
                    for c in 0..PLANES {
                        sum[q * PLANES + c] += output[(c * TILE + y) * TILE + x] * weight;
                    }
                }
            }
        }
    }
    for qy in 0..qh {
        for qx in 0..qw {
            let q = qy * qw + qx;
            if count[q] < 1e-6 {
                continue;
            }
            for c in 0..PLANES {
                let (dx, dy) = PLANE_OFFSETS[c];
                let sx = ox + qx * 2 + dx;
                let sy = oy + qy * 2 + dy;
                let index = sy * raw.width as usize + sx;
                let calibration = calibration_channel(raw, sx, sy);
                let value = sum[q * PLANES + c] / count[q]
                    * (raw.white_levels[calibration] - raw.black_levels[calibration])
                    + raw.black_levels[calibration];
                raw.cfa_data[index] = value.round().clamp(0.0, f32::from(u16::MAX)) as u16;
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn denoise_impl(raw: &mut RawImage, path: PathBuf, compute: ComputeUnits) -> Result<(), AiError> {
    use std::ffi::{c_char, c_void};
    unsafe extern "C" {
        fn sp_ai_model_predict(
            model: *mut c_void,
            input: *const f32,
            output: *mut f32,
            error: *mut c_char,
            len: usize,
        ) -> i32;
    }
    let session = model_session::get(&path, compute)?;
    let model = session.handle;
    let mut error = [0i8; 512];
    run_tiles(raw, |input, output| {
        error.fill(0);
        if unsafe {
            sp_ai_model_predict(
                model,
                input.as_ptr(),
                output.as_mut_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        } == 0
        {
            return Err(AiError::CoreMl(
                unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
        Ok(())
    })
}

#[cfg(not(target_os = "macos"))]
fn denoise_impl(raw: &mut RawImage, path: PathBuf, compute: ComputeUnits) -> Result<(), AiError> {
    use tract_onnx::prelude::*;

    let session = model_session::get(&path, compute)?;
    let model = &session.model;
    run_tiles(raw, |input, output| {
        let tensor = Tensor::from_shape(&[1, PLANES, TILE, TILE], input)
            .map_err(|error| AiError::CoreMl(error.to_string()))?;
        let result = model
            .run(tvec!(tensor.into()))
            .map_err(|error| AiError::CoreMl(error.to_string()))?;
        let result = result[0]
            .to_plain_array_view::<f32>()
            .map_err(|error| AiError::CoreMl(error.to_string()))?;
        let result = result
            .as_slice()
            .ok_or_else(|| AiError::CoreMl("ONNX output is not contiguous NCHW".into()))?;
        if result.len() != output.len() {
            return Err(AiError::CoreMl(format!(
                "ONNX output has {} values, expected {}",
                result.len(),
                output.len()
            )));
        }
        output.copy_from_slice(result);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_models_beside_a_macos_bundle_executable() {
        let executable = Path::new("/Applications/ShopPhoto.app/Contents/MacOS/ShopPhoto");
        assert_eq!(
            bundled_model_root(executable),
            Some(PathBuf::from(
                "/Applications/ShopPhoto.app/Contents/Resources/DenoiseModels"
            ))
        );
    }

    #[test]
    fn tiles_match_the_reference_grid() {
        let (apron, stride) = (60isize, 452isize);
        for n in [1usize, 100, 392, 393, 452, 845, 846, 4000, 5826] {
            let starts = tile_starts(n);
            // denoise_lab.tiling.tiled_inference: padded extent and grid.
            let extent = 512 + (n + 120).saturating_sub(512).div_ceil(452) * 452;
            assert_eq!(starts.len(), (extent - 512) / 452 + 1, "{n}");
            for (index, start) in starts.iter().enumerate() {
                assert_eq!(*start, index as isize * stride - apron, "{n}");
            }
            assert!(*starts.last().unwrap() + 512 >= n as isize + apron, "{n}");
        }
    }

    #[test]
    fn feather_matches_the_reference_ramp() {
        let feather = feather();
        let at = |y: usize, x: usize| feather[y * TILE + x];
        assert_eq!(at(256, 256), 1.0);
        assert_eq!(at(60, 451), 1.0);
        let corner = (1.0f32 / 61.0 * (1.0 / 61.0)).powf(2.2);
        assert!((at(0, 0) - corner).abs() < 1e-12);
        assert_eq!(at(0, 0), at(511, 511));
        let edge = (30.0f32 / 61.0).powf(2.2);
        assert!((at(29, 256) - edge).abs() < 1e-6);
        assert_eq!(at(29, 256), at(482, 256));
    }

    #[test]
    fn bayer_planes_use_rgb_calibration_channels() {
        use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
        let raw = RawImage {
            cfa_data: vec![0; 4],
            width: 2,
            height: 2,
            cfa: CfaPattern::Rggb,
            black_levels: [10.0, 20.0, 30.0, 999.0],
            white_levels: [100.0, 200.0, 300.0, 999.0],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            active_area: ActiveArea {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            orientation: Orientation::Normal,
        };
        assert_eq!(calibration_channel(&raw, 0, 0), 0);
        assert_eq!(calibration_channel(&raw, 1, 0), 1);
        assert_eq!(calibration_channel(&raw, 0, 1), 1);
        assert_eq!(calibration_channel(&raw, 1, 1), 2);
    }

    fn tiny(cfa: sp_core::raw::CfaPattern, x: u32, y: u32) -> RawImage {
        use sp_core::raw::{ActiveArea, Orientation};
        RawImage {
            cfa_data: vec![0; 16],
            width: 4,
            height: 4,
            cfa,
            black_levels: [0.0; 4],
            white_levels: [1.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            active_area: ActiveArea {
                x,
                y,
                width: 4 - x,
                height: 4 - y,
            },
            orientation: Orientation::Normal,
        }
    }

    #[test]
    fn blend_is_a_linear_mix_in_raw_codes() {
        let source = tiny(sp_core::raw::CfaPattern::Rggb, 0, 0);
        let mut inferred = source.clone();
        for (i, v) in inferred.cfa_data.iter_mut().enumerate() {
            *v = (i as u16 + 1) * 1000;
        }
        assert_eq!(blend(&source, &inferred, 0.0).cfa_data, source.cfa_data);
        assert_eq!(blend(&source, &inferred, 1.0).cfa_data, inferred.cfa_data);
        let half = blend(&source, &inferred, 0.5);
        assert_eq!(half.cfa_data[3], 2000);
        assert_eq!(half.cfa_data[15], 8000);
        // Out-of-range strengths clamp instead of extrapolating.
        assert_eq!(blend(&source, &inferred, 7.0).cfa_data, inferred.cfa_data);
        // Mismatched buffers leave the source untouched.
        let mut other = inferred.clone();
        other.cfa_data.truncate(3);
        assert_eq!(blend(&source, &other, 1.0).cfa_data, source.cfa_data);
    }

    #[test]
    fn invalid_blend_strength_or_sensor_layout_preserves_source() {
        let mut source = tiny(sp_core::raw::CfaPattern::Rggb, 0, 0);
        source.cfa_data.fill(500);
        let mut inferred = source.clone();
        inferred.cfa_data.fill(1500);
        assert_eq!(
            blend(&source, &inferred, f32::NAN).cfa_data,
            source.cfa_data
        );
        for incompatible in [
            RawImage {
                width: 2,
                height: 8,
                ..inferred.clone()
            },
            RawImage {
                cfa: sp_core::raw::CfaPattern::Bggr,
                ..inferred.clone()
            },
            RawImage {
                active_area: sp_core::raw::ActiveArea {
                    x: 1,
                    y: 0,
                    width: 3,
                    height: 4,
                },
                ..inferred
            },
        ] {
            assert_eq!(blend(&source, &incompatible, 1.0).cfa_data, source.cfa_data);
        }
    }

    #[test]
    fn apron_mirrors_without_repeating_the_edge() {
        assert_eq!(mirror(0, 10), 0);
        assert_eq!(mirror(-1, 10), 1);
        assert_eq!(mirror(-9, 10), 9);
        assert_eq!(mirror(-10, 10), 8);
        assert_eq!(mirror(9, 10), 9);
        assert_eq!(mirror(10, 10), 8);
        assert_eq!(mirror(18, 10), 0);
        assert_eq!(mirror(5, 1), 0);
    }

    #[test]
    fn model_channels_are_r_g1_b_g2_from_a_red_topped_quad() {
        use sp_core::raw::CfaPattern;
        for (cfa, x, y) in [
            (CfaPattern::Rggb, 0, 0),
            (CfaPattern::Rggb, 1, 1),
            (CfaPattern::Bggr, 0, 0),
            (CfaPattern::Grbg, 0, 0),
            (CfaPattern::Gbrg, 1, 0),
        ] {
            let raw = tiny(cfa, x, y);
            let (ox, oy) = red_quad_origin(&raw);
            assert!(ox >= x as usize && oy >= y as usize, "{cfa:?}");
            let colors: Vec<usize> = PLANE_OFFSETS
                .iter()
                .map(|&(dx, dy)| calibration_channel(&raw, ox + dx, oy + dy))
                .collect();
            assert_eq!(colors, vec![0, 1, 2, 1], "{cfa:?} at ({x},{y})");
        }
    }

    #[test]
    fn small_crops_reach_the_model_and_malformed_raw_is_rejected() {
        let mut raw = tiny(sp_core::CfaPattern::Rggb, 0, 0);
        // The mirrored layout accepts small crops: it either runs a model
        // installed for this user or fails to find one, never on size.
        match infer_with_compute(&raw, Path::new("no-model-folder"), ComputeUnits::Automatic) {
            Ok(out) => assert_eq!(out.cfa_data.len(), raw.cfa_data.len()),
            Err(error) => assert!(!error.to_string().contains("too small"), "{error}"),
        }
        raw.cfa_data.clear();
        let error = infer_with_compute(&raw, Path::new("no-model-folder"), ComputeUnits::Automatic)
            .unwrap_err();
        assert!(error.to_string().contains("Invalid RAW"));
        raw.active_area.x = u32::MAX;
        assert!(
            infer_with_compute(&raw, Path::new("no-model-folder"), ComputeUnits::Automatic)
                .is_err()
        );
    }

    fn flat_raw() -> RawImage {
        RawImage {
            cfa_data: vec![8192; 1024 * 1024],
            width: 1024,
            height: 1024,
            cfa: sp_core::CfaPattern::Rggb,
            black_levels: [4096.; 4],
            white_levels: [65535.; 4],
            wb_coeffs: [1.; 4],
            cam_to_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            active_area: sp_core::raw::ActiveArea {
                x: 0,
                y: 0,
                width: 1024,
                height: 1024,
            },
            orientation: sp_core::raw::Orientation::Normal,
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn installed_model_obeys_each_explicit_hardware_policy() {
        let Ok(root) = std::env::var("SHOPPHOTO_DENOISE_MODELS") else {
            return;
        };
        let raw = flat_raw();
        unsafe extern "C" {
            fn sp_ai_model_configured_units(handle: *mut std::ffi::c_void) -> i32;
        }
        let path = model_path(Path::new(&root));
        for (compute, expected) in [
            (ComputeUnits::Automatic, 2),
            (ComputeUnits::CpuGpu, 1),
            (ComputeUnits::CpuNeuralEngine, 3),
            (ComputeUnits::CpuOnly, 0),
        ] {
            let out = infer_with_compute(&raw, Path::new(&root), compute)
                .expect("explicit-policy inference");
            let session = model_session::get(&path, compute).unwrap();
            assert_eq!(
                unsafe { sp_ai_model_configured_units(session.handle) },
                expected,
                "{}",
                compute.label()
            );
            assert_eq!(out.cfa_data.len(), raw.cfa_data.len());
        }
        assert!(raw.cfa_data.iter().all(|&v| v == 8192));
    }

    #[test]
    fn installed_model_runs_and_returns_finite_codes() {
        let Ok(root) = std::env::var("SHOPPHOTO_DENOISE_MODELS") else {
            return;
        };
        let raw = flat_raw();
        let out = denoise(&raw, Path::new(&root), 0.75).expect("model inference");
        assert_eq!(out.cfa_data.len(), raw.cfa_data.len());
        // A flat field comes back (nearly) unchanged from an identity-trained
        // model; any NaN would round to zero codes.
        assert!(out.cfa_data.iter().all(|&v| v.abs_diff(8192) < 512));
        assert_eq!(raw.cfa_data[0], 8192, "Source samples stay immutable");
    }
}
