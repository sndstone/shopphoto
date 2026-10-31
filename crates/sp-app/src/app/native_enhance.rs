//! Native public-model inference and image processing. All tensors stay in
//! this process; the optional Python worker is only a compatibility fallback.
use image::{imageops::FilterType, ImageBuffer, Luma, RgbImage, RgbaImage};
use sp_ai::enhancement::{Manifest, Tensor};
use std::{
    cell::RefCell,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) fn bundle(id: &str) -> Option<PathBuf> {
    if !sp_ai::enhancement::available() {
        return None;
    }
    let mut roots = Vec::new();
    if let Some(root) = std::env::var_os("SHOPPHOTO_ENHANCEMENT_MODELS") {
        let path = PathBuf::from(root).join(id);
        return path.join("manifest.json").is_file().then_some(path);
    }
    if let Some(root) = dirs::data_local_dir() {
        roots.push(root.join("shopphoto/enhance-models/native"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(contents) = exe.parent().and_then(Path::parent) {
            roots.push(contents.join("Resources/EnhancementModels"));
        }
    }
    roots
        .into_iter()
        .map(|root| root.join(id))
        .find(|path| path.join("manifest.json").is_file())
}

fn sky_bundle() -> Option<PathBuf> {
    bundle("oneformer-ade").or_else(|| bundle("maskformer-ade"))
}

pub(super) fn mask_source(kind: &str) -> Option<String> {
    supports_mask(kind).then(|| {
        match kind {
            "semantic_sky" => {
                if bundle("oneformer-ade").is_some() {
                    "Native Core ML · OneFormer + ViTMatte"
                } else {
                    "Native Core ML · MaskFormer + ViTMatte"
                }
            }
            "object" => "Native Core ML · SlimSAM + ViTMatte",
            _ => "Native Core ML · SSDLite + SlimSAM + ViTMatte",
        }
        .into()
    })
}

pub(super) fn supports_mask(kind: &str) -> bool {
    if bundle("vitmatte-small").is_none() {
        return false;
    }
    match kind {
        "semantic_sky" => sky_bundle().is_some(),
        "semantic_people" | "semantic_cars" | "semantic_motorcycles" => {
            ["ssdlite-coco", "slimsam-encoder", "slimsam-box"]
                .iter()
                .all(|id| bundle(id).is_some())
        }
        "object" => ["slimsam-encoder", "slimsam-points"]
            .iter()
            .all(|id| bundle(id).is_some()),
        _ => false,
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Enhancement cancelled".into())
    } else {
        Ok(())
    }
}

pub(super) fn clear() {
    PRODUCTS.with(|s| *s.borrow_mut() = None);
    ENCODINGS.with(|s| *s.borrow_mut() = None);
    DETECTIONS.with(|s| *s.borrow_mut() = None);
    sp_ai::enhancement::clear_sessions();
}

fn infer(path: &Path, pixels: Vec<f32>, task: &str) -> Result<Vec<Tensor>, String> {
    infer_many(path, vec![pixels], task)
}

fn infer_many(path: &Path, values: Vec<Vec<f32>>, task: &str) -> Result<Vec<Tensor>, String> {
    let manifest = Manifest::read(path)?;
    if manifest.task != task || manifest.inputs.len() != values.len() {
        return Err("Native model task or input contract mismatch".into());
    }
    let inputs = manifest
        .inputs
        .iter()
        .cloned()
        .zip(values)
        .map(|(mut spec, values)| {
            if task == "sam_points" && matches!(spec.name.as_str(), "points" | "labels") {
                if spec.flexible_axes.get(&2) != Some(&[1, 32]) {
                    return Err("Native point model lacks variable prompt support".into());
                }
                let components = if spec.name == "points" { 2 } else { 1 };
                if values.len() % components != 0 {
                    return Err("Invalid native point dimensions".into());
                }
                spec.shape[2] = values.len() / components;
            }
            Tensor::new(spec, values)
        })
        .collect::<Result<Vec<_>, _>>()?;
    sp_ai::enhancement::predict(path, inputs, sp_ai::ComputeUnits::from_environment())
}

fn load_rgb(request: &serde_json::Value) -> Result<RgbImage, String> {
    let path = request["input"].as_str().ok_or("Missing native input")?;
    if request["input_format"] == "rgba8" {
        let w = request["width"].as_u64().ok_or("Missing native width")?;
        let h = request["height"].as_u64().ok_or("Missing native height")?;
        if w == 0 || h == 0 || w > 150_000_000 || h > 150_000_000 || w * h > 150_000_000 {
            return Err("Invalid native image dimensions".into());
        }
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() != w * h * 4 {
            return Err("Native RGBA8 input size does not match its dimensions".into());
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let image =
            RgbaImage::from_raw(w as u32, h as u32, bytes).ok_or("Invalid native RGBA8 input")?;
        Ok(image::DynamicImage::ImageRgba8(image).into_rgb8())
    } else {
        if !matches!(request["input_format"].as_str(), None | Some("image"))
            || (!request["input_format"].is_null() && !request["input_format"].is_string())
        {
            return Err("Unknown native mask input format".into());
        }
        let dimensions = image::image_dimensions(path).map_err(|e| e.to_string())?;
        if dimensions.0 == 0
            || dimensions.1 == 0
            || u64::from(dimensions.0) * u64::from(dimensions.1) > 150_000_000
        {
            return Err("Native input exceeds 150 MP".into());
        }
        let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(900_000_000);
        reader.limits(limits);
        Ok(reader.decode().map_err(|e| e.to_string())?.into_rgb8())
    }
}

#[derive(Clone)]
struct Proposals {
    classes: Vec<f32>, // normalized query probabilities, including no-object
    masks: Vec<f32>,   // sigmoid masks
    queries: usize,
    labels: usize,
    width: usize,
    height: usize,
    valid: (f32, f32),
}
thread_local! { static PRODUCTS: RefCell<Option<(u64,Proposals)>> = const { RefCell::new(None) }; }

fn proposals(rgb: &RgbImage, path: &Path, cancel: &AtomicBool) -> Result<Proposals, String> {
    let manifest = Manifest::read(path)?;
    if manifest.inputs[0].shape != [1, 3, 512, 512] {
        return Err("Unsupported semantic native input shape".into());
    }
    let mut key = std::collections::hash_map::DefaultHasher::new();
    rgb.as_raw().hash(&mut key);
    rgb.dimensions().hash(&mut key);
    path.hash(&mut key);
    std::fs::read(path.join("manifest.json"))
        .map_err(|e| e.to_string())?
        .hash(&mut key);
    for name in manifest.files.keys() {
        let metadata = std::fs::metadata(path.join(name)).map_err(|e| e.to_string())?;
        metadata.len().hash(&mut key);
        metadata.modified().ok().hash(&mut key);
    }
    let key = key.finish();
    if let Some(value) = PRODUCTS.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, p)| p.clone())
    }) {
        return Ok(value);
    }
    let scale = (512.0 / rgb.width() as f64).min(512.0 / rgb.height() as f64);
    let (w, h) = (
        (rgb.width() as f64 * scale).round().clamp(1., 512.) as u32,
        (rgb.height() as f64 * scale).round().clamp(1., 512.) as u32,
    );
    let resized = image::imageops::resize(rgb, w, h, FilterType::Triangle);
    let mut pixels = vec![0.; 3 * 512 * 512];
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..3 {
                pixels[c * 512 * 512 + y * 512 + x] =
                    (f32::from(resized.get_pixel(x as u32, y as u32)[c]) / 255.
                        - [0.485, 0.456, 0.406][c])
                        / [0.229, 0.224, 0.225][c];
            }
        }
    }
    check_cancel(cancel)?;
    let mut output = infer(path, pixels, "semantic")?;
    check_cancel(cancel)?;
    let classes = output
        .iter()
        .position(|v| v.spec.name == "class_logits")
        .ok_or("Native semantic class logits missing")?;
    let class_tensor = output.remove(classes);
    let mask_tensor = output
        .into_iter()
        .find(|v| v.spec.name == "mask_logits")
        .ok_or("Native semantic mask logits missing")?;
    if class_tensor.spec.shape.len() != 3
        || class_tensor.spec.shape[0] != 1
        || mask_tensor.spec.shape.len() != 4
        || mask_tensor.spec.shape[0] != 1
        || mask_tensor.spec.shape[1] != class_tensor.spec.shape[1]
    {
        return Err("Invalid semantic graph outputs".into());
    }
    let queries = class_tensor.spec.shape[1];
    let labels = class_tensor.spec.shape[2];
    let mut classes = class_tensor.values;
    for row in classes.chunks_exact_mut(labels) {
        let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let sum = row
            .iter_mut()
            .map(|v| {
                *v = (*v - max).exp();
                *v
            })
            .sum::<f32>();
        for v in row {
            *v /= sum;
        }
    }
    let value = Proposals {
        classes,
        masks: mask_tensor
            .values
            .into_iter()
            .map(|v| 1. / (1. + (-v).exp()))
            .collect(),
        queries,
        labels,
        height: mask_tensor.spec.shape[2],
        width: mask_tensor.spec.shape[3],
        valid: (w as f32 / 512., h as f32 / 512.),
    };
    PRODUCTS.with(|s| *s.borrow_mut() = Some((key, value.clone())));
    Ok(value)
}

fn bilinear(values: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0., (w - 1) as f32);
    let y = y.clamp(0., (h - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    (values[y0 * w + x0] * (1. - fx) + values[y0 * w + x1] * fx) * (1. - fy)
        + (values[y1 * w + x0] * (1. - fx) + values[y1 * w + x1] * fx) * fy
}

fn coverage(p: &Proposals, class: usize) -> Result<Vec<f32>, String> {
    if class + 1 >= p.labels {
        return Err("Native semantic class is unavailable".into());
    }
    let area = p.width * p.height;
    let mut result = vec![0.; area];
    let mut total = vec![0.; area];
    for q in 0..p.queries {
        let confidence = p.classes[q * p.labels + class];
        let foreground = 1. - p.classes[(q + 1) * p.labels - 1];
        for i in 0..area {
            result[i] += confidence * p.masks[q * area + i];
            total[i] += foreground * p.masks[q * area + i];
        }
    }
    for (a, t) in result.iter_mut().zip(total) {
        *a = (*a / t.max(1e-6)).clamp(0., 1.);
    }
    if result.iter().copied().fold(0., f32::max) < 0.5 {
        return Err("No confidently detected element; use an include point or painted mask".into());
    }
    Ok(result)
}

#[derive(Clone)]
struct Detection {
    id: usize,
    class: usize,
    confidence: f32,
    bounds: [f32; 4],
}
type Encoding = (u64, Vec<f32>, (u32, u32));
thread_local! {
    static ENCODINGS: RefCell<Option<Encoding>> = const {RefCell::new(None)};
    static DETECTIONS: RefCell<Option<(u64,Vec<Detection>)>> = const {RefCell::new(None)};
}

fn image_model_key(rgb: &RgbImage, path: &Path) -> Result<u64, String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    rgb.as_raw().hash(&mut hash);
    rgb.dimensions().hash(&mut hash);
    path.hash(&mut hash);
    let manifest = Manifest::read(path)?;
    std::fs::read(path.join("manifest.json"))
        .map_err(|e| e.to_string())?
        .hash(&mut hash);
    for file in manifest.files.keys() {
        let metadata = std::fs::metadata(path.join(file)).map_err(|e| e.to_string())?;
        metadata.len().hash(&mut hash);
        metadata.modified().ok().hash(&mut hash);
    }
    Ok(hash.finish())
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let x = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let y = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    let intersection = x.max(0.) * y.max(0.);
    intersection / (a[2] * a[3] + b[2] * b[3] - intersection).max(1e-9)
}

fn detections(rgb: &RgbImage, cancel: &AtomicBool) -> Result<Vec<Detection>, String> {
    let path = bundle("ssdlite-coco").ok_or("Native element detector missing")?;
    let key = image_model_key(rgb, &path)?;
    if let Some(result) = DETECTIONS.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
    }) {
        return Ok(result);
    }
    let manifest = Manifest::read(&path)?;
    if manifest.inputs[0].shape != [1, 3, 320, 320] {
        return Err("Unsupported native detector contract".into());
    }
    let resized = image::imageops::resize(rgb, 320, 320, FilterType::Triangle);
    let mut pixels = vec![0.; 3 * 320 * 320];
    for y in 0..320 {
        for x in 0..320 {
            for c in 0..3 {
                pixels[c * 320 * 320 + y * 320 + x] =
                    (f32::from(resized.get_pixel(x as u32, y as u32)[c]) / 255. - 0.5) / 0.5;
            }
        }
    }
    check_cancel(cancel)?;
    let output = infer(&path, pixels, "detector")?;
    let boxes = output
        .iter()
        .find(|v| v.spec.name == "boxes")
        .ok_or("Native detector boxes missing")?;
    let logits = output
        .iter()
        .find(|v| v.spec.name == "class_logits")
        .ok_or("Native detector classes missing")?;
    if boxes.spec.shape != [1, 3234, 4] || logits.spec.shape != [1, 3234, 91] {
        return Err("Native detector output contract mismatch".into());
    }
    let mut all = Vec::new();
    for class in [1, 3, 4] {
        let mut candidates = Vec::new();
        for (i, row) in logits.values.chunks_exact(91).enumerate() {
            let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let denominator = row.iter().map(|v| (*v - max).exp()).sum::<f32>();
            let confidence = (row[class] - max).exp() / denominator;
            if confidence < 0.45 {
                continue;
            }
            let v = &boxes.values[i * 4..][..4];
            let x0 = v[0].clamp(0., 320.) / 320.;
            let y0 = v[1].clamp(0., 320.) / 320.;
            let x1 = v[2].clamp(0., 320.) / 320.;
            let y1 = v[3].clamp(0., 320.) / 320.;
            if (x1 - x0) * (rgb.width() as f32) < 2. || (y1 - y0) * (rgb.height() as f32) < 2. {
                continue;
            }
            candidates.push(Detection {
                id: class * 10000 + i,
                class,
                confidence,
                bounds: [x0, y0, x1 - x0, y1 - y0],
            });
        }
        candidates.sort_by(|a, b| b.confidence.total_cmp(&a.confidence).then(a.id.cmp(&b.id)));
        candidates.truncate(400);
        let mut accepted: Vec<Detection> = Vec::new();
        for candidate in candidates {
            if accepted
                .iter()
                .all(|other| overlap(other.bounds, candidate.bounds) <= 0.55)
            {
                accepted.push(candidate);
            }
            if accepted.len() == 16 {
                break;
            }
        }
        all.extend(accepted);
    }
    check_cancel(cancel)?;
    DETECTIONS.with(|s| *s.borrow_mut() = Some((key, all.clone())));
    Ok(all)
}

fn sam_embedding(rgb: &RgbImage, cancel: &AtomicBool) -> Result<(Vec<f32>, (u32, u32)), String> {
    let path = bundle("slimsam-encoder").ok_or("Native object encoder missing")?;
    let key = image_model_key(rgb, &path)?;
    if let Some(result) = ENCODINGS.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|(k, _, _)| *k == key)
            .map(|(_, v, size)| (v.clone(), *size))
    }) {
        return Ok(result);
    }
    let scale = (1024. / rgb.width() as f64).min(1024. / rgb.height() as f64);
    let (w, h) = (
        (rgb.width() as f64 * scale + 0.5).floor().clamp(1., 1024.) as u32,
        (rgb.height() as f64 * scale + 0.5).floor().clamp(1., 1024.) as u32,
    );
    let resized = image::imageops::resize(rgb, w, h, FilterType::Triangle);
    let mut pixels = vec![0.; 3 * 1024 * 1024];
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..3 {
                pixels[c * 1024 * 1024 + y * 1024 + x] =
                    (f32::from(resized.get_pixel(x as u32, y as u32)[c]) / 255.
                        - [0.485, 0.456, 0.406][c])
                        / [0.229, 0.224, 0.225][c];
            }
        }
    }
    check_cancel(cancel)?;
    let output = infer(&path, pixels, "sam_encoder")?;
    let encoding = output
        .into_iter()
        .find(|v| v.spec.name == "embeddings" && v.spec.shape == [1, 256, 64, 64])
        .ok_or("Native object embedding mismatch")?
        .values;
    check_cancel(cancel)?;
    ENCODINGS.with(|s| *s.borrow_mut() = Some((key, encoding.clone(), (w, h))));
    Ok((encoding, (w, h)))
}

fn sam_mask(
    rgb: &RgbImage,
    bounds: Option<[f32; 4]>,
    request: &serde_json::Value,
    cancel: &AtomicBool,
) -> Result<(Proposals, Vec<f32>), String> {
    let (embedding, (w, h)) = sam_embedding(rgb, cancel)?;
    let (path, inputs, task) = if let Some([x, y, bw, bh]) = bounds {
        (
            bundle("slimsam-box").ok_or("Native box segmenter missing")?,
            vec![
                embedding,
                vec![
                    x * w as f32,
                    y * h as f32,
                    (x + bw) * w as f32,
                    (y + bh) * h as f32,
                ],
            ],
            "sam_box",
        )
    } else {
        let points = request["points"].as_array().ok_or("Add an include point")?;
        let labels = request["labels"].as_array().ok_or("Missing point labels")?;
        if points.is_empty()
            || points.len() > 32
            || points.len() != labels.len()
            || !labels.iter().any(|v| v.as_u64() == Some(1))
        {
            return Err("Use 1–32 include/exclude points with a foreground point".into());
        }
        let mut coordinates = vec![0.; points.len() * 2];
        let mut packed_labels = vec![0.; points.len()];
        for (i, (point, label)) in points.iter().zip(labels).enumerate() {
            let p = point
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("Invalid point")?;
            let x = p[0].as_f64().ok_or("Invalid point")?;
            let y = p[1].as_f64().ok_or("Invalid point")?;
            if !x.is_finite()
                || !y.is_finite()
                || !(0.0..=1.0).contains(&x)
                || !(0.0..=1.0).contains(&y)
                || !matches!(label.as_u64(), Some(0 | 1))
            {
                return Err("Invalid include/exclude point".into());
            }
            coordinates[i * 2] = x as f32 * w as f32;
            coordinates[i * 2 + 1] = y as f32 * h as f32;
            packed_labels[i] = label.as_u64().unwrap() as f32;
        }
        (
            bundle("slimsam-points").ok_or("Native point segmenter missing")?,
            vec![embedding, coordinates, packed_labels],
            "sam_points",
        )
    };
    check_cancel(cancel)?;
    let output = infer_many(&path, inputs, task)?;
    let masks = output
        .iter()
        .find(|v| v.spec.name == "mask_logits")
        .ok_or("Native object mask missing")?;
    let scores = output
        .iter()
        .find(|v| v.spec.name == "iou_scores")
        .ok_or("Native object confidence missing")?;
    if masks.spec.shape != [1, 1, 3, 256, 256] || scores.values.len() != 3 {
        return Err("Native object decoder output mismatch".into());
    }
    let best = scores
        .values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    let logits = &masks.values[best * 256 * 256..(best + 1) * 256 * 256];
    // SAM first upsamples logits to1024, removes padded image, and only then
    // computes coverage. Keep the resize in logit space for soft boundaries.
    let mut alpha = vec![0.; 1024 * 1024];
    for y in 0..1024 {
        for x in 0..1024 {
            let logit = bilinear(
                logits,
                256,
                256,
                (x as f32 + 0.5) / 4. - 0.5,
                (y as f32 + 0.5) / 4. - 0.5,
            );
            alpha[y * 1024 + x] = 1. / (1. + (-logit).exp());
        }
    }
    if !(0..h as usize).any(|y| {
        alpha[y * 1024..y * 1024 + w as usize]
            .iter()
            .any(|value| *value >= 0.5)
    }) {
        return Err(
            "No confidently detected object mask; use additional include points or painted repair"
                .into(),
        );
    }
    check_cancel(cancel)?;
    Ok((
        Proposals {
            classes: vec![],
            masks: vec![],
            queries: 0,
            labels: 0,
            width: 1024,
            height: 1024,
            valid: (w as f32 / 1024., h as f32 / 1024.),
        },
        alpha,
    ))
}

fn instance_backend() -> Result<String, String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for id in [
        "ssdlite-coco",
        "slimsam-encoder",
        "slimsam-box",
        "vitmatte-small",
    ] {
        let path = bundle(id).ok_or("Native instance models missing")?;
        std::fs::read(path.join("manifest.json"))
            .map_err(|e| e.to_string())?
            .hash(&mut hash);
    }
    Ok(format!("native-ssdlite-slimsam-v1-{:016x}", hash.finish()))
}

fn coarse_at(p: &Proposals, alpha: &[f32], x: usize, y: usize, w: usize, h: usize) -> f32 {
    bilinear(
        alpha,
        p.width,
        p.height,
        (x as f32 + 0.5) / w as f32 * p.valid.0 * p.width as f32 - 0.5,
        (y as f32 + 0.5) / h as f32 * p.valid.1 * p.height as f32 - 0.5,
    )
}

/// Binary 33-pixel erosion/dilation by separable sliding sums. Replicated
/// borders avoid a frame around subjects which touch the photograph edge.
fn trimap(p: &Proposals, alpha: &[f32], w: usize, h: usize) -> Vec<u8> {
    const R: isize = 16;
    const K: usize = 33;
    let mut horizontal = vec![0u8; w * h];
    for y in 0..h {
        let foreground = |x: isize| {
            usize::from(coarse_at(p, alpha, x.clamp(0, w as isize - 1) as usize, y, w, h) >= 0.5)
        };
        let mut sum: usize = (-R..=R).map(foreground).sum();
        for x in 0..w {
            horizontal[y * w + x] = sum as u8;
            sum = sum + foreground(x as isize + R + 1) - foreground(x as isize - R);
        }
    }
    let mut result = vec![128u8; w * h];
    for x in 0..w {
        let value = |y: isize| horizontal[y.clamp(0, h as isize - 1) as usize * w + x] as usize;
        let mut sum: usize = (-R..=R).map(value).sum();
        for y in 0..h {
            let c = coarse_at(p, alpha, x, y, w, h);
            result[y * w + x] = if c > 0.01 && c < 0.99 {
                128
            } else if sum == K * K {
                255
            } else if sum == 0 {
                0
            } else {
                128
            };
            sum = sum + value(y as isize + R + 1) - value(y as isize - R);
        }
    }
    result
}

fn reflect(x: isize, limit: usize) -> usize {
    if limit <= 1 {
        return 0;
    }
    let period = (2 * limit - 2) as isize;
    let x = x.rem_euclid(period) as usize;
    if x < limit {
        x
    } else {
        2 * limit - 2 - x
    }
}

fn matte(
    rgb: &RgbImage,
    p: &Proposals,
    alpha: &[f32],
    path: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<u16>, String> {
    let manifest = Manifest::read(path)?;
    if manifest.inputs[0].shape != [1, 4, 512, 512] || manifest.outputs[0].shape != [1, 1, 512, 512]
    {
        return Err("Unsupported native matte contract".into());
    }
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let trimap = trimap(p, alpha, w, h);
    let mut result: Vec<u16> = trimap
        .iter()
        .map(|v| if *v == 255 { 65535 } else { 0 })
        .collect();
    const TILE: usize = 512;
    const HALO: usize = 64;
    const CORE: usize = TILE - 2 * HALO;
    for y in (0..h).step_by(CORE) {
        for x in (0..w).step_by(CORE) {
            check_cancel(cancel)?;
            let hh = CORE.min(h - y);
            let ww = CORE.min(w - x);
            if !(y..y + hh).any(|yy| trimap[yy * w + x..yy * w + x + ww].contains(&128)) {
                continue;
            }
            let mut pixels = vec![0.; 4 * TILE * TILE];
            for yy in 0..TILE {
                for xx in 0..TILE {
                    let sx = reflect(x as isize + xx as isize - HALO as isize, w);
                    let sy = reflect(y as isize + yy as isize - HALO as isize, h);
                    for c in 0..3 {
                        pixels[c * TILE * TILE + yy * TILE + xx] =
                            (f32::from(rgb.get_pixel(sx as u32, sy as u32)[c]) / 255.
                                - [0.485, 0.456, 0.406][c])
                                / [0.229, 0.224, 0.225][c];
                    }
                    pixels[3 * TILE * TILE + yy * TILE + xx] =
                        f32::from(trimap[sy * w + sx]) / 255.;
                }
            }
            let output = infer(path, pixels, "matte")?;
            let output = &output[0].values;
            for yy in 0..hh {
                for xx in 0..ww {
                    if trimap[(y + yy) * w + x + xx] == 128 {
                        result[(y + yy) * w + x + xx] =
                            (output[(yy + HALO) * TILE + xx + HALO].clamp(0., 1.) * 65535.).round()
                                as u16;
                    }
                }
            }
        }
    }
    check_cancel(cancel)?;
    if !result.iter().any(|v| *v > 0) {
        return Err("AI returned an empty mask; no empty layer was created".into());
    }
    Ok(result)
}

fn save_mask(path: &Path, w: u32, h: u32, pixels: Vec<u16>) -> Result<(), String> {
    ImageBuffer::<Luma<u16>, _>::from_raw(w, h, pixels)
        .ok_or("Invalid native mask")?
        .save(path)
        .map_err(|e| e.to_string())
}

fn instance_preview(rgb: &RgbImage, p: &Proposals, alpha: &[f32], bounds: [f32; 4]) -> RgbaImage {
    let [x, y, w, h] = bounds;
    let margin = w.max(h) * 0.08;
    let x0 = ((x - margin).max(0.) * rgb.width() as f32).floor() as u32;
    let y0 = ((y - margin).max(0.) * rgb.height() as f32).floor() as u32;
    let x1 =
        (((x + w + margin).min(1.) * rgb.width() as f32).ceil() as u32).clamp(x0 + 1, rgb.width());
    let y1 = (((y + h + margin).min(1.) * rgb.height() as f32).ceil() as u32)
        .clamp(y0 + 1, rgb.height());
    let crop = image::imageops::crop_imm(rgb, x0, y0, x1 - x0, y1 - y0).to_image();
    let mut preview =
        image::DynamicImage::ImageRgb8(image::imageops::thumbnail(&crop, 160, 160)).into_rgba8();
    let (pw, ph) = preview.dimensions();
    for (x, y, pixel) in preview.enumerate_pixels_mut() {
        let source_x = x0 as f32 + (x as f32 + 0.5) / pw as f32 * (x1 - x0) as f32;
        let source_y = y0 as f32 + (y as f32 + 0.5) / ph as f32 * (y1 - y0) as f32;
        let coverage = bilinear(
            alpha,
            p.width,
            p.height,
            source_x / rgb.width() as f32 * p.valid.0 * p.width as f32 - 0.5,
            source_y / rgb.height() as f32 * p.valid.1 * p.height as f32 - 0.5,
        );
        pixel[3] = (coverage.clamp(0., 1.) * 255.).round() as u8;
    }
    preview
}

pub(super) fn try_mask(
    request: &serde_json::Value,
    cancel: &AtomicBool,
) -> Option<Result<(), String>> {
    let kind = request["kind"].as_str()?;
    if !matches!(
        request["operation"].as_str(),
        Some("mask" | "mask_instances")
    ) || !supports_mask(kind)
    {
        return None;
    }
    if request["operation"] == "mask"
        && matches!(
            kind,
            "semantic_people" | "semantic_cars" | "semantic_motorcycles"
        )
    {
        return None;
    }
    Some((|| {
        check_cancel(cancel)?;
        if !request["selected_ids"].is_null() && !request["selected_ids"].is_array() {
            return Err("Selected element IDs must be an array".into());
        }
        let matte_path = bundle("vitmatte-small").ok_or("Native matte model missing")?;
        let rgb = load_rgb(request)?;
        let (w, h) = rgb.dimensions();
        let destination = Path::new(
            request["output"]
                .as_str()
                .ok_or("Missing native destination")?,
        );
        if request["operation"] == "mask_instances" {
            let class = match kind {
                "semantic_people" => 1,
                "semantic_cars" => 3,
                "semantic_motorcycles" => 4,
                _ => return Err("Unsupported instance class".into()),
            };
            let backend = instance_backend()?;
            let mut candidates: Vec<_> = detections(&rgb, cancel)?
                .into_iter()
                .filter(|c| c.class == class)
                .collect();
            if candidates.is_empty() {
                return Err("No confidently detected individual elements; use a point selection or painted mask".into());
            }
            let selected = request["selected_ids"].as_array();
            if let Some(selected) = selected {
                if request["selected_backend"].as_str() != Some(backend.as_str()) {
                    return Err("Element backend changed; detect elements again".into());
                }
                let ids: Vec<_> = selected
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .collect();
                if ids.is_empty()
                    || ids.len() != selected.len()
                    || ids.iter().enumerate().any(|(i, id)| {
                        ids[..i].contains(id) || !candidates.iter().any(|c| c.id as u64 == *id)
                    })
                {
                    return Err("Selected elements changed; detect elements again".into());
                }
                if u64::from(w) * u64::from(h) * 2 * ids.len() as u64 > 600_000_000 {
                    return Err(
                        "Selected masks exceed the 600 MB budget; create fewer layers".into(),
                    );
                }
                candidates.retain(|c| ids.contains(&(c.id as u64)));
            }
            let root = destination.parent().ok_or("Missing mask directory")?;
            let mut entries = Vec::new();
            for c in candidates {
                check_cancel(cancel)?;
                let (p, alpha) = sam_mask(&rgb, Some(c.bounds), request, cancel)?;
                let (preview, mask) = if selected.is_some() {
                    let name = format!("mask-{}.png", c.id);
                    save_mask(
                        &root.join(&name),
                        w,
                        h,
                        matte(&rgb, &p, &alpha, &matte_path, cancel)?,
                    )?;
                    (None, Some(name))
                } else {
                    let preview = instance_preview(&rgb, &p, &alpha, c.bounds);
                    let name = format!("candidate-{}.png", c.id);
                    preview.save(root.join(&name)).map_err(|e| e.to_string())?;
                    (Some(name), None)
                };
                entries.push(serde_json::json!({"id":c.id,"confidence":c.confidence,"bounds":c.bounds,"preview":preview,"mask":mask}));
            }
            check_cancel(cancel)?;
            std::fs::write(destination,serde_json::to_vec(&serde_json::json!({"version":1,"backend":backend,"width":w,"height":h,"candidates":entries})).map_err(|e|e.to_string())?).map_err(|e|e.to_string())
        } else if kind == "object" {
            let (p, alpha) = sam_mask(&rgb, None, request, cancel)?;
            save_mask(
                destination,
                w,
                h,
                matte(&rgb, &p, &alpha, &matte_path, cancel)?,
            )
        } else {
            let semantic = sky_bundle().ok_or("Native semantic model missing")?;
            let manifest = Manifest::read(&semantic)?;
            let class = *manifest
                .classes
                .get(kind.trim_start_matches("semantic_"))
                .ok_or("Native model does not support this class")?;
            let p = proposals(&rgb, &semantic, cancel)?;
            let alpha = coverage(&p, class)?;
            save_mask(
                destination,
                w,
                h,
                matte(&rgb, &p, &alpha, &matte_path, cancel)?,
            )
        }
    })())
}

#[cfg(test)]
mod mask_boundary_tests {
    use super::*;

    /// Edge QA: `SHOPPHOTO_TEST_EDGE_PHOTO=photo.png cargo test -p sp-app
    /// native_person_edges -- --ignored` writes the coarse coverage, trimap
    /// and final matte of every detected person for visual inspection.
    #[test]
    #[ignore = "requires native models and SHOPPHOTO_TEST_EDGE_PHOTO"]
    fn native_person_edges() {
        let photo = std::env::var("SHOPPHOTO_TEST_EDGE_PHOTO").expect("SHOPPHOTO_TEST_EDGE_PHOTO");
        let root = std::env::temp_dir().join("shopphoto-edge-qa");
        std::fs::create_dir_all(&root).unwrap();
        let cancel = AtomicBool::new(false);
        let request = serde_json::json!({"input": photo});
        let rgb = load_rgb(&request).unwrap();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);
        let matte_path = bundle("vitmatte-small").expect("matte model");
        for detection in detections(&rgb, &cancel)
            .unwrap()
            .into_iter()
            .filter(|d| d.class == 1)
        {
            let (p, alpha) = sam_mask(&rgb, Some(detection.bounds), &request, &cancel).unwrap();
            let coarse: Vec<u8> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (y, x)))
                .map(|(y, x)| (coarse_at(&p, &alpha, x, y, w, h) * 255.).round() as u8)
                .collect();
            let gray = |name: &str, pixels: Vec<u8>| {
                image::GrayImage::from_raw(w as u32, h as u32, pixels)
                    .unwrap()
                    .save(root.join(format!("{}-{name}.png", detection.id)))
                    .unwrap();
            };
            gray("coarse", coarse);
            gray("trimap", trimap(&p, &alpha, w, h));
            let matte = matte(&rgb, &p, &alpha, &matte_path, &cancel).unwrap();
            gray("matte", matte.iter().map(|v| (v >> 8) as u8).collect());
            eprintln!(
                "person {} bounds {:?} -> {}",
                detection.id,
                detection.bounds,
                root.display()
            );
        }
    }
    #[test]
    fn native_transport_checks_format_and_exact_buffer_size_before_decoding() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), [0u8; 3]).unwrap();
        for format in [serde_json::json!("unknown"), serde_json::json!(42)] {
            let request = serde_json::json!({"input":file.path(),"input_format":format});
            assert!(load_rgb(&request).unwrap_err().contains("input format"));
        }
        let request =
            serde_json::json!({"input":file.path(),"input_format":"rgba8","width":2,"height":2});
        assert!(load_rgb(&request).unwrap_err().contains("input size"));
    }
    #[test]
    fn instance_previews_crop_to_the_selected_object_and_preserve_alpha() {
        let rgb = RgbImage::from_pixel(1000, 500, image::Rgb([12, 34, 56]));
        let p = Proposals {
            classes: vec![],
            masks: vec![],
            queries: 0,
            labels: 0,
            width: 10,
            height: 5,
            valid: (1., 1.),
        };
        let mut alpha = vec![0.; 50];
        alpha[2 * 10 + 5] = 1.;
        let preview = instance_preview(&rgb, &p, &alpha, [0.5, 0.4, 0.1, 0.2]);
        assert!(preview.width() <= 160 && preview.height() <= 160);
        assert!(
            preview.height() > 100,
            "a small object should fill its proposal preview"
        );
        assert!(preview.as_raw().chunks_exact(4).any(|pixel| pixel[3] > 127));
        assert!(preview.as_raw().chunks_exact(4).all(|pixel| pixel[..3]
            .iter()
            .zip([12u8, 34, 56])
            .all(|(a, b)| a.abs_diff(b) <= 1)));
    }
}
fn selected_upscaler(model: &Path) -> Option<PathBuf> {
    if model.is_dir() {
        return Some(model.into());
    }
    let path = match model.file_name()?.to_str()? {
        "SwinIR-M-x2.pth" | "001_classicalSR_DF2K_s64w8_SwinIR-M_x2.pth" => bundle("swinir-2x"),
        "SwinIR-M-x4.pth" | "001_classicalSR_DF2K_s64w8_SwinIR-M_x4.pth" => bundle("swinir-4x"),
        _ => None,
    }?;
    // A familiar filename does not establish model identity: preserve the
    // user's selected weights unless the native graph uses that checkpoint.
    let manifest = Manifest::read(&path).ok()?;
    let selected_hash = sp_ai::enhancement::sha256_file(model).ok()?;
    (manifest.task == "upscale" && manifest.revision == selected_hash).then_some(path)
}

pub(super) fn try_upscale(
    image: &sp_export::ExportImage,
    model: &Path,
    hdr: bool,
    cancel: &AtomicBool,
) -> Option<Result<sp_export::ExportImage, String>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let path = selected_upscaler(model)?;
    Some(upscale(image, &path, hdr, cancel))
}

fn pq_decode(v: f32) -> f32 {
    let p = v.max(0.).powf(1. / 78.84375);
    10000. * ((p - 0.8359375).max(0.) / (18.851563 - 18.6875 * p).max(1e-6)).powf(1. / 0.15930176)
}
fn pq_encode(v: f32) -> f32 {
    let p = (v.max(0.) / 10000.).powf(0.15930176);
    ((0.8359375 + 18.851563 * p) / (1. + 18.6875 * p)).powf(78.84375)
}
fn guide(linear: [f32; 3]) -> [f32; 3] {
    let y = linear[0] * 0.2627 + linear[1] * 0.6780 + linear[2] * 0.0593;
    let v = linear.map(|v| v / (y + 203.));
    [
        [1.660491, -0.587641, -0.072850],
        [-0.124550, 1.1329, -0.008349],
        [-0.018151, -0.100579, 1.11873],
    ]
    .map(|row| {
        let v = (row[0] * v[0] + row[1] * v[1] + row[2] * v[2]).clamp(0., 1.);
        if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1. / 2.4) - 0.055
        }
    })
}

fn upscale(
    image: &sp_export::ExportImage,
    path: &Path,
    hdr: bool,
    cancel: &AtomicBool,
) -> Result<sp_export::ExportImage, String> {
    check_cancel(cancel)?;
    let manifest = Manifest::read(path)?;
    let (scale, tile, halo) = (manifest.scale, manifest.tile, manifest.halo);
    if manifest.task != "upscale"
        || ![2, 4].contains(&scale)
        || !(64..=512).contains(&tile)
        || halo == 0
        || halo >= tile.div_ceil(2)
        || manifest.inputs.len() != 1
        || manifest.outputs.len() != 1
        || manifest.inputs[0].shape != [1, 3, tile, tile]
        || manifest.outputs[0].shape != [1, 3, tile * scale, tile * scale]
    {
        return Err("Unsupported native upscaler contract".into());
    }
    let (w, h) = (image.width as usize, image.height as usize);
    if w == 0
        || h == 0
        || w.checked_mul(h)
            .and_then(|n| n.checked_mul(scale * scale))
            .is_none_or(|n| n > 150_000_000)
    {
        return Err("Upscale exceeds 150 MP; crop more tightly or select 2×".into());
    }
    let valid = image
        .rgba16
        .as_ref()
        .map_or(image.rgba8.len() == w * h * 4, |v| v.len() == w * h * 4);
    if !valid {
        return Err("Invalid native upscale pixels".into());
    }
    let original = |x: usize, y: usize| -> [f32; 3] {
        let i = (y * w + x) * 4;
        if let Some(values) = &image.rgba16 {
            std::array::from_fn(|c| f32::from(values[i + c]) / 65535.)
        } else {
            std::array::from_fn(|c| f32::from(image.rgba8[i + c]) / 255.)
        }
    };
    let (ow, oh) = (w * scale, h * scale);
    let core = tile - halo * 2;
    let ot = tile * scale;
    let mut result = vec![65535u16; ow * oh * 4];
    for y in (0..h).step_by(core) {
        for x in (0..w).step_by(core) {
            check_cancel(cancel)?;
            let mut pixels = vec![0.; 3 * tile * tile];
            // Keep only this tile's HDR master and compressed guide. Reusing
            // these samples avoids 12 PQ decodes and four RGB guide conversions per
            // output pixel without allocating full-resolution float masters.
            let mut linear_tile = if hdr {
                vec![[0.; 3]; tile * tile]
            } else {
                Vec::new()
            };
            let mut guide_tile = if hdr {
                vec![[0.; 3]; tile * tile]
            } else {
                Vec::new()
            };
            for yy in 0..tile {
                for xx in 0..tile {
                    let rgb = original(
                        reflect(x as isize + xx as isize - halo as isize, w),
                        reflect(y as isize + yy as isize - halo as isize, h),
                    );
                    let v = if hdr {
                        let linear = rgb.map(pq_decode);
                        let compressed = guide(linear);
                        linear_tile[yy * tile + xx] = linear;
                        guide_tile[yy * tile + xx] = compressed;
                        compressed
                    } else {
                        rgb
                    };
                    for c in 0..3 {
                        pixels[c * tile * tile + yy * tile + xx] = v[c];
                    }
                }
            }
            let predicted = sp_ai::enhancement::predict(
                path,
                vec![Tensor::new(manifest.inputs[0].clone(), pixels)?],
                sp_ai::ComputeUnits::from_environment(),
            )?;
            let predicted = &predicted[0].values;
            for yy in 0..(core.min(h - y) * scale) {
                for xx in 0..(core.min(w - x) * scale) {
                    let (ox, oy) = (x * scale + xx, y * scale + yy);
                    let ai: [f32; 3] = std::array::from_fn(|c| {
                        predicted[c * ot * ot + (yy + halo * scale) * ot + xx + halo * scale]
                    });
                    let value = if hdr {
                        let sx = (ox as f32 + 0.5) / scale as f32 - 0.5;
                        let sy = (oy as f32 + 0.5) / scale as f32 - 0.5;
                        let ix = sx.floor() as isize;
                        let iy = sy.floor() as isize;
                        let fx = sx - ix as f32;
                        let fy = sy - iy as f32;
                        let sample = |values: &[[f32; 3]], dx: isize, dy: isize| {
                            let sx = ((ix + dx).clamp(0, w as isize - 1) - x as isize
                                + halo as isize) as usize;
                            let sy = ((iy + dy).clamp(0, h as isize - 1) - y as isize
                                + halo as isize) as usize;
                            values[sy * tile + sx]
                        };
                        let a = sample(&linear_tile, 0, 0);
                        let b = sample(&linear_tile, 1, 0);
                        let c = sample(&linear_tile, 0, 1);
                        let d = sample(&linear_tile, 1, 1);
                        let base: [f32; 3] = std::array::from_fn(|i| {
                            (a[i] * (1. - fx) + b[i] * fx) * (1. - fy)
                                + (c[i] * (1. - fx) + d[i] * fx) * fy
                        });
                        // Interpolate the compressed guide separately, as in the
                        // reference HDR path; preserve master luminance and chroma.
                        let a = sample(&guide_tile, 0, 0);
                        let b = sample(&guide_tile, 1, 0);
                        let c = sample(&guide_tile, 0, 1);
                        let d = sample(&guide_tile, 1, 1);
                        let ordinary: [f32; 3] = std::array::from_fn(|i| {
                            (a[i] * (1. - fx) + b[i] * fx) * (1. - fy)
                                + (c[i] * (1. - fx) + d[i] * fx) * fy
                        });
                        let residual = (ai[0] - ordinary[0]) * 0.2126
                            + (ai[1] - ordinary[1]) * 0.7152
                            + (ai[2] - ordinary[2]) * 0.0722;
                        let y = base[0] * 0.2627 + base[1] * 0.6780 + base[2] * 0.0593;
                        let confidence =
                            (y / 4.).clamp(0., 1.) * ((4000. - y) / 1000.).clamp(0., 1.);
                        base.map(|v| {
                            pq_encode(v * 2f32.powf((residual * 2.).clamp(-0.2, 0.2) * confidence))
                        })
                    } else {
                        ai
                    };
                    for c in 0..3 {
                        result[(oy * ow + ox) * 4 + c] =
                            (value[c].clamp(0., 1.) * 65535.).round() as u16;
                    }
                }
            }
        }
    }
    check_cancel(cancel)?;
    sp_export::ExportImage::new_16(ow as u32, oh as u32, result)
        .ok_or("Invalid native upscale result".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the native SwinIR 2x bundle and original checkpoint"]
    fn renamed_checkpoint_cannot_silently_select_different_native_weights() {
        let native = bundle("swinir-2x").expect("native SwinIR 2x bundle");
        let original = dirs::data_local_dir()
            .unwrap()
            .join("shopphoto/enhance-models/SwinIR-M-x2.pth");
        assert_eq!(selected_upscaler(&original), Some(native));
        let directory = tempfile::tempdir().unwrap();
        let renamed = directory.path().join("SwinIR-M-x2.pth");
        std::fs::write(&renamed, b"different user-selected weights").unwrap();
        assert!(selected_upscaler(&renamed).is_none());
    }

    #[test]
    fn malformed_sr_contracts_and_oversized_images_fail_before_prediction() {
        let directory = tempfile::tempdir().unwrap();
        let mut manifest = serde_json::json!({
            "version":1,"id":"unit-upscale","task":"upscale","model":"model.mlmodelc",
            "inputs":[{"name":"pixels","shape":[1,3,192,192]}],
            "outputs":[{"name":"rgb","shape":[1,3,384,384]}],
            "scale":2,"tile":192,"halo":32,"license":"test","source":"test","revision":"test",
            "files":{"model.mlmodelc/file":"a".repeat(64)}
        });
        let small = sp_export::ExportImage::new_16(1, 1, vec![0, 0, 0, 65535]).unwrap();
        let cancel = AtomicBool::new(false);
        for halo in [0, 96, usize::MAX] {
            manifest["halo"] = halo.into();
            std::fs::write(
                directory.path().join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            assert!(upscale(&small, directory.path(), false, &cancel)
                .unwrap_err()
                .contains("contract"));
        }
        manifest["halo"] = 32.into();
        let input = manifest["inputs"][0].clone();
        manifest["inputs"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"name":"extra","shape":input["shape"]}));
        std::fs::write(
            directory.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(upscale(&small, directory.path(), false, &cancel)
            .unwrap_err()
            .contains("contract"));
        manifest["inputs"] = serde_json::json!([input]);
        std::fs::write(
            directory.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let oversized = sp_export::ExportImage {
            width: 10_000,
            height: 10_000,
            rgba8: vec![],
            rgba16: None,
        };
        assert!(upscale(&oversized, directory.path(), false, &cancel)
            .unwrap_err()
            .contains("150 MP"));
        let malformed = sp_export::ExportImage {
            width: 2,
            height: 2,
            rgba8: vec![],
            rgba16: None,
        };
        assert!(upscale(&malformed, directory.path(), false, &cancel)
            .unwrap_err()
            .contains("pixels"));
        assert!(upscale(
            &small,
            Path::new("must-not-read"),
            false,
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .contains("cancelled"));
    }

    #[test]
    #[ignore = "requires native SwinIR 2x/4x and the optional Python checkpoints; writes parity images to SHOPPHOTO_TEST_SR_QA"]
    fn actual_native_sr_photo_crop_parity_seams_and_warm_timing() {
        use std::time::Instant;
        let temporary = tempfile::tempdir().unwrap();
        let root = std::env::var_os("SHOPPHOTO_TEST_SR_QA")
            .map_or_else(|| temporary.path().to_owned(), PathBuf::from);
        std::fs::create_dir_all(&root).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/advanced-ai-features/ai-mask-scene.png");
        let source = image::open(fixture).unwrap().into_rgb16();
        // Both dimensions cross the 128-pixel core boundary. The narrow
        // final row/column exercises halo extraction and reflected edges.
        let crop = image::imageops::crop_imm(&source, 650, 430, 137, 131).to_image();
        let python = super::super::enhance_worker::python();
        let checkpoint_root = dirs::data_local_dir()
            .unwrap()
            .join("shopphoto/enhance-models");
        let reference = r#"
import sys,json,time
from pathlib import Path
sys.path.insert(0,sys.argv[1])
import enhance, numpy as np, torch
root,checkpoint,hdr=Path(sys.argv[2]),sys.argv[3],sys.argv[4]=='true'
request={'input':str(root/'input.png'),'model':checkpoint,'hdr':hdr}
times=[]
for iteration in ('cold','warm'):
    request['output']=str(root/f'python-{iteration}.png')
    start=time.perf_counter();enhance.upscale(request);times.append(time.perf_counter()-start)
# Use the identical fixed-size reflected input contract to isolate graph
# conversion and native HDR math from the portable tiler's boundary policy.
def fixed(rgb,model,scale,tile=128,halo=32,batch_size=1):
    h,w,_=rgb.shape;result=np.empty((h*scale,w*scale,3),np.float32)
    def indices(start,n):
        if n==1:return np.zeros(192,dtype=np.int64)
        z=np.arange(start-32,start-32+192)%(2*n-2)
        return np.where(z<n,z,2*n-2-z)
    for y in range(0,h,128):
        for x in range(0,w,128):
            values=rgb[np.ix_(indices(y,h),indices(x,w))].copy()
            tensor=torch.from_numpy(values).permute(2,0,1).unsqueeze(0).to(device=model.device,dtype=model.dtype)
            with torch.inference_mode():out=model(tensor).float().cpu()[0].permute(1,2,0).numpy()
            hh,ww=min(128,h-y)*scale,min(128,w-x)*scale
            result[y*scale:y*scale+hh,x*scale:x*scale+ww]=out[32*scale:32*scale+hh,32*scale:32*scale+ww]
    return result
enhance.tiled_sr=fixed
request['output']=str(root/'python-fixed.png')
start=time.perf_counter();enhance.upscale(request);fixed_time=time.perf_counter()-start
(root/'timing.json').write_text(json.dumps({'python_cold_seconds':times[0],'python_warm_seconds':times[1],'python_fixed_seconds':fixed_time,'device':enhance.device(torch)}))
"#;
        let cancel = AtomicBool::new(false);
        for scale in [2u32, 4] {
            for hdr in [false, true] {
                let case = root.join(format!("{scale}x-hdr-{hdr}"));
                std::fs::create_dir_all(&case).unwrap();
                let rgba: Vec<u16> = crop
                    .pixels()
                    .flat_map(|p| {
                        let channels = if hdr {
                            p.0.map(|v| {
                                (pq_encode((f32::from(v) / 65535.).powi(2) * 1200.) * 65535.)
                                    .round() as u16
                            })
                        } else {
                            p.0
                        };
                        [channels[0], channels[1], channels[2], 65535]
                    })
                    .collect();
                let input = sp_export::ExportImage::new_16(137, 131, rgba.clone()).unwrap();
                ImageBuffer::<image::Rgba<u16>, _>::from_raw(137, 131, rgba)
                    .unwrap()
                    .save(case.join("input.png"))
                    .unwrap();
                clear();
                let model = bundle(&format!("swinir-{scale}x")).expect("native SwinIR bundle");
                let started = Instant::now();
                let cold = upscale(&input, &model, hdr, &cancel).unwrap();
                let cold_seconds = started.elapsed().as_secs_f64();
                let started = Instant::now();
                let warm = upscale(&input, &model, hdr, &cancel).unwrap();
                let warm_seconds = started.elapsed().as_secs_f64();
                assert_eq!((warm.width, warm.height), (137 * scale, 131 * scale));
                let values = warm.rgba16.unwrap();
                let distinct_red: std::collections::BTreeSet<_> =
                    values.chunks_exact(4).map(|pixel| pixel[0]).collect();
                assert!(
                    distinct_red.len() > 512,
                    "SR delivery lost 16-bit precision"
                );
                let repeated_max = values
                    .iter()
                    .zip(cold.rgba16.unwrap())
                    .map(|(a, b)| a.abs_diff(b))
                    .max()
                    .unwrap();
                assert!(
                    repeated_max <= 8,
                    "warm/cold native mismatch: {repeated_max}"
                );
                ImageBuffer::<image::Rgba<u16>, _>::from_raw(
                    warm.width,
                    warm.height,
                    values.clone(),
                )
                .unwrap()
                .save(case.join("native.png"))
                .unwrap();
                let output = std::process::Command::new(&python)
                    .arg("-c")
                    .arg(reference)
                    .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts"))
                    .arg(&case)
                    .arg(checkpoint_root.join(format!("SwinIR-M-x{scale}.pth")))
                    .arg(hdr.to_string())
                    .env("HF_HUB_OFFLINE", "1")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "Python reference: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let metrics = |file: &str, seam_only: bool| {
                    let reference = image::open(case.join(file)).unwrap().into_rgb16();
                    assert_eq!(reference.dimensions(), (warm.width, warm.height));
                    let mut max = 0u16;
                    let mut total = 0u64;
                    let mut count = 0u64;
                    for (x, y, pixel) in reference.enumerate_pixels() {
                        if seam_only
                            && x.abs_diff(128 * scale) > 2 * scale
                            && y.abs_diff(128 * scale) > 2 * scale
                        {
                            continue;
                        }
                        for c in 0..3 {
                            let error =
                                values[((y * warm.width + x) * 4) as usize + c].abs_diff(pixel[c]);
                            max = max.max(error);
                            total += u64::from(error);
                            count += 1;
                        }
                    }
                    (max, total as f64 / count as f64)
                };
                let fixed = metrics("python-fixed.png", false);
                let seam = metrics("python-fixed.png", true);
                let portable = metrics("python-warm.png", false);
                assert!(
                    fixed.0 <= 64 && fixed.1 <= 3.,
                    "native {scale}x HDR{hdr} fixed-contract error {fixed:?}"
                );
                assert!(seam.1 <= 3., "native stitching error {seam:?}");
                let mut timing: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(case.join("timing.json")).unwrap())
                        .unwrap();
                timing["native_cold_seconds"] = cold_seconds.into();
                timing["native_warm_seconds"] = warm_seconds.into();
                timing["fixed_max_u16"] = fixed.0.into();
                timing["fixed_mean_u16"] = fixed.1.into();
                timing["seam_mean_u16"] = seam.1.into();
                timing["portable_max_u16"] = portable.0.into();
                timing["portable_mean_u16"] = portable.1.into();
                std::fs::write(
                    case.join("comparison.json"),
                    serde_json::to_vec_pretty(&timing).unwrap(),
                )
                .unwrap();
                eprintln!("{scale}x HDR{hdr}: {timing}");
            }
        }
        clear();
    }
    #[test]
    fn interpolation_reflection_and_hdr_are_bounded() {
        assert_eq!((reflect(-1, 4), reflect(4, 4), reflect(999, 1)), (1, 2, 0));
        assert_eq!(bilinear(&[0., 1., 1., 0.], 2, 2, 0.5, 0.5), 0.5);
        for nits in [0., 0.1, 203., 1000., 4000., 10000.] {
            assert!((pq_decode(pq_encode(nits)) - nits).abs() < nits * 0.0002 + 0.005);
        }
        assert!(guide([0., 203., 4000.])
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
    }

    #[test]
    #[ignore = "requires native semantic/matte/SwinIR 2x+4x bundles"]
    fn actual_native_sky_upscale_and_saved_mask() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/advanced-ai-features/ai-mask-scene.png");
        let output = std::env::var_os("SHOPPHOTO_TEST_NATIVE_SKY_OUTPUT")
            .map_or_else(|| dir.path().join("sky.png"), PathBuf::from);
        let started = std::time::Instant::now();
        try_mask(&serde_json::json!({"operation":"mask","kind":"semantic_sky","input":fixture,"output":output}),&cancel).expect("native model availability").unwrap();
        let alpha = image::open(&output).unwrap().into_luma16();
        assert_eq!(
            alpha.dimensions(),
            image::image_dimensions(fixture).unwrap()
        );
        let selected = alpha.as_raw().iter().filter(|v| **v > 32767).count();
        let coverage = selected as f64 / alpha.as_raw().len() as f64;
        assert!((0.05..0.6).contains(&coverage), "sky coverage {coverage}");
        eprintln!(
            "Native sky+matte {:?}, coverage {coverage:0.4}",
            started.elapsed()
        );
        let mask = sp_core::mask::AlphaMask {
            width: alpha.width(),
            height: alpha.height(),
            alpha: alpha.into_raw().into(),
            source: "Native MaskFormer + ViTMatte".into(),
            inverted: false,
            repairs: vec![],
        };
        let mut sidecar = sp_catalog::Sidecar::default();
        sidecar
            .edit
            .local_adjustments
            .push(sp_core::edit::LocalAdjustment {
                mask: sp_core::edit::LocalMask::Alpha(mask),
                ..Default::default()
            });
        let encoded = serde_json::to_vec(&sidecar).unwrap();
        assert_eq!(
            serde_json::from_slice::<sp_catalog::Sidecar>(&encoded)
                .unwrap()
                .edit,
            sidecar.edit
        );
        for scale in [2, 4] {
            for hdr in [false, true] {
                let rgb = if hdr {
                    [pq_encode(50.), pq_encode(203.), pq_encode(500.)]
                } else {
                    [0.25, 0.5, 0.75]
                };
                let pixels = (0..83 * 67)
                    .flat_map(|_| [rgb[0], rgb[1], rgb[2], 1.].map(|v| (v * 65535.).round() as u16))
                    .collect();
                let input = sp_export::ExportImage::new_16(83, 67, pixels).unwrap();
                let started = std::time::Instant::now();
                let enhanced = upscale(
                    &input,
                    &bundle(&format!("swinir-{scale}x")).unwrap(),
                    hdr,
                    &cancel,
                )
                .unwrap();
                assert_eq!((enhanced.width, enhanced.height), (83 * scale, 67 * scale));
                let values = enhanced.rgba16.unwrap();
                let center =
                    &values[((33 * scale) * (83 * scale) + 41 * scale) as usize * 4..][..3];
                for c in 0..3 {
                    assert!(
                        (f32::from(center[c]) / 65535. - rgb[c]).abs() < 0.02,
                        "native{scale}x hdr{hdr} channel{c}"
                    );
                }
                eprintln!("Native{scale}x hdr{hdr}: {:?}", started.elapsed());
            }
        }
        let request = serde_json::json!({"operation":"mask","kind":"semantic_sky","input":"must-not-read","output":output});
        assert!(try_mask(&request, &AtomicBool::new(true))
            .unwrap()
            .unwrap_err()
            .contains("cancelled"));
        clear();
    }
}
