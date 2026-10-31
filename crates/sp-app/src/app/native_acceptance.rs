//! Explicit real-model acceptance, including multiple instances in one photo.
use super::native_enhance;
use std::{path::PathBuf, sync::atomic::AtomicBool};

#[test]
#[ignore = "requires native models and explicit real-photo fixtures"]
fn actual_native_individual_people_cars_motorcycles_and_points() {
    let root = std::env::var_os("SHOPPHOTO_TEST_NATIVE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("shopphoto-native-subject-qa"));
    std::fs::create_dir_all(&root).unwrap();
    let cancel = AtomicBool::new(false);
    for (name, kind, variable, count) in [
        (
            "people",
            "semantic_people",
            "SHOPPHOTO_TEST_PEOPLE_PHOTO",
            2,
        ),
        ("cars", "semantic_cars", "SHOPPHOTO_TEST_CAR_PHOTO", 1),
        (
            "motorcycles",
            "semantic_motorcycles",
            "SHOPPHOTO_TEST_MOTORCYCLE_PHOTO",
            1,
        ),
    ] {
        let photo = PathBuf::from(std::env::var_os(variable).expect(variable));
        let original = std::fs::read(&photo).unwrap();
        let case = root.join(name);
        std::fs::create_dir_all(&case).unwrap();
        let output = case.join("instances.json");
        let mut request = serde_json::json!({"operation":"mask_instances","kind":kind,"input":photo,"output":output});
        let started = std::time::Instant::now();
        native_enhance::try_mask(&request, &cancel)
            .expect("native model availability")
            .unwrap();
        let detected: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let candidates = detected["candidates"].as_array().unwrap();
        assert!(
            candidates.len() >= count,
            "{name} found{}",
            candidates.len()
        );
        request["selected_ids"] = candidates
            .iter()
            .take(count)
            .map(|c| c["id"].clone())
            .collect();
        request["selected_backend"] = detected["backend"].clone();
        native_enhance::try_mask(&request, &cancel)
            .unwrap()
            .unwrap();
        let selected: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let mut sidecar = sp_catalog::Sidecar::default();
        let mut mattes = Vec::new();
        for candidate in selected["candidates"].as_array().unwrap() {
            let path = case.join(candidate["mask"].as_str().unwrap());
            let mask = image::open(&path).unwrap().into_luma16();
            assert_eq!(mask.dimensions(), image::image_dimensions(&photo).unwrap());
            let selected_pixels = mask.as_raw().iter().filter(|v| **v > 32767).count();
            assert!(selected_pixels > 10 && selected_pixels < mask.as_raw().len());
            let rgb = image::open(&photo).unwrap().into_rgb8();
            let overlay = image::ImageBuffer::from_fn(rgb.width(), rgb.height(), |x, y| {
                let alpha = f32::from(mask.get_pixel(x, y)[0]) / 65535. * 0.5;
                let p = rgb.get_pixel(x, y);
                image::Rgb([
                    (f32::from(p[0]) * (1. - alpha) + 255. * alpha) as u8,
                    (f32::from(p[1]) * (1. - alpha)) as u8,
                    (f32::from(p[2]) * (1. - alpha)) as u8,
                ])
            });
            overlay.save(path.with_extension("overlay.png")).unwrap();
            let alpha = sp_core::mask::AlphaMask {
                width: mask.width(),
                height: mask.height(),
                alpha: mask.clone().into_raw().into(),
                source: detected["backend"].as_str().unwrap().into(),
                inverted: false,
                repairs: vec![],
            };
            sidecar
                .edit
                .local_adjustments
                .push(sp_core::edit::LocalAdjustment {
                    name: format!("{name} {}", candidate["id"]),
                    mask: sp_core::edit::LocalMask::Alpha(alpha),
                    ..Default::default()
                });
            mattes.push(mask);
        }
        assert_eq!(sidecar.edit.local_adjustments.len(), count);
        let bytes = serde_json::to_vec(&sidecar).unwrap();
        assert_eq!(
            serde_json::from_slice::<sp_catalog::Sidecar>(&bytes)
                .unwrap()
                .edit,
            sidecar.edit
        );
        if count == 2 {
            let intersection = mattes[0]
                .as_raw()
                .iter()
                .zip(mattes[1].as_raw())
                .filter(|(a, b)| **a > 32767 && **b > 32767)
                .count();
            assert!(
                intersection < 50,
                "Separate people must not share the same mask"
            );
        }
        request["selected_backend"] = serde_json::json!("different-model");
        assert!(native_enhance::try_mask(&request, &cancel)
            .unwrap()
            .unwrap_err()
            .contains("backend changed"));
        assert_eq!(std::fs::read(&photo).unwrap(), original);
        eprintln!("Native{name} detect+selected+saved {:?}", started.elapsed());
    }
    if native_enhance::supports_mask("object") {
        let photo = PathBuf::from(std::env::var_os("SHOPPHOTO_TEST_MOTORCYCLE_PHOTO").unwrap());
        let output = root.join("point-mask.png");
        native_enhance::try_mask(&serde_json::json!({"operation":"mask","kind":"object","input":photo,"output":output,"points":[[0.5,0.5],[0.05,0.05]],"labels":[1,0]}),&cancel).unwrap().unwrap();
        assert!(image::open(output)
            .unwrap()
            .into_luma16()
            .as_raw()
            .iter()
            .any(|v| *v > 32767));
    }
    native_enhance::clear();
}

#[test]
#[ignore = "requires native variable-prompt SAM models and focused two-person fixture"]
fn actual_native_variable_point_counts_and_exclusion_refinement() {
    let photo = PathBuf::from(
        std::env::var_os("SHOPPHOTO_TEST_PEOPLE_PHOTO").expect("focused two-person fixture"),
    );
    let root = std::env::var_os("SHOPPHOTO_TEST_NATIVE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("shopphoto-native-subject-qa"));
    std::fs::create_dir_all(&root).unwrap();
    let cancel = AtomicBool::new(false);
    let original = std::fs::read(&photo).unwrap();
    for (name, points, labels) in [
        (
            "one",
            serde_json::json!([[0.75, 0.45]]),
            serde_json::json!([1]),
        ),
        (
            "four",
            serde_json::json!([[0.75, 0.25], [0.75, 0.45], [0.75, 0.72], [0.25, 0.45]]),
            serde_json::json!([1, 1, 1, 0]),
        ),
    ] {
        let output = root.join(format!("point-{name}.png"));
        native_enhance::try_mask(&serde_json::json!({"operation":"mask","kind":"object","input":photo,"output":output,"points":points,"labels":labels}),&cancel).expect("native models available").unwrap();
        let alpha = image::open(output).unwrap().into_luma16();
        assert_eq!(alpha.dimensions(), (800, 450));
        assert!(alpha.as_raw().iter().any(|v| *v > 32767));
        if name == "four" {
            let mean = |x0: u32, x1: u32| {
                (150..270)
                    .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                    .map(|(x, y)| f64::from(alpha.get_pixel(x, y)[0]) / 65535.)
                    .sum::<f64>()
                    / f64::from(120 * (x1 - x0))
            };
            let kept = mean(575, 625);
            let removed = mean(175, 225);
            assert!(
                kept > 0.8 && removed < 0.01,
                "include/exclude ROI means: kept{kept} removed{removed}"
            );
            eprintln!("Native4point refinement keeps{kept:0.5} excludes{removed:0.5}");
        }
    }
    assert_eq!(std::fs::read(photo).unwrap(), original);
    native_enhance::clear();
}
