//! Optical shading calibration from evenly illuminated, defocused RAW flats.
use sp_core::lens::{GridGeometry, LensProfile, ShadingGrid};
use sp_core::RawImage;

/// Measure each physical CFA slot independently; exposure and white balance
/// cancel against the corresponding central patch. Masked sensor borders are
/// excluded. This cannot establish whether a photographed target was uniform.
pub fn measure_flat(raw: &RawImage, f_number: f32) -> Result<(GridGeometry, ShadingGrid), String> {
    let area = raw.active_area;
    if !f_number.is_finite()
        || f_number <= 0.0
        || area.width < 32
        || area.height < 32
        || area.x.checked_add(area.width).is_none_or(|v| v > raw.width)
        || area
            .y
            .checked_add(area.height)
            .is_none_or(|v| v > raw.height)
        || raw.cfa_data.len() != raw.width as usize * raw.height as usize
    {
        return Err("The flat-field RAW has invalid sensor geometry or aperture".into());
    }
    let spacing = (raw.width.max(raw.height) as f32 / 80.0).max(16.0);
    let geometry = GridGeometry::for_sensor(raw.width, raw.height, spacing);
    let radius = ((spacing * 0.4).clamp(4.0, 32.0) as u32).min(area.width.min(area.height) / 4);
    let patch = |cx: f32, cy: f32| -> Result<[f32; 4], String> {
        let cx = cx.clamp(
            (area.x + radius) as f32,
            (area.x + area.width - radius - 1) as f32,
        ) as u32;
        let cy = cy.clamp(
            (area.y + radius) as f32,
            (area.y + area.height - radius - 1) as f32,
        ) as u32;
        let mut sum = [0.0f64; 4];
        let mut count = [0u32; 4];
        let mut clipped = 0u32;
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                let slot = ((y & 1) * 2 + (x & 1)) as usize;
                // Match the decoder and RAW prepare pass: levels use RGBE order.
                let channel = raw.cfa.color_at(x, y) as usize;
                let black = raw.black_levels[channel];
                let range = raw.white_levels[channel] - black;
                if !range.is_finite() || range <= 0.0 {
                    return Err("The flat-field RAW has invalid black or white levels".into());
                }
                let value = (f32::from(raw.cfa_data[(y * raw.width + x) as usize]) - black) / range;
                clipped += u32::from(value >= 0.98);
                sum[slot] += f64::from(value);
                count[slot] += 1;
            }
        }
        if clipped > count.iter().sum::<u32>() / 100 {
            return Err("The flat field is clipped; use a lower exposure".into());
        }
        let mean = std::array::from_fn(|slot| (sum[slot] / f64::from(count[slot].max(1))) as f32);
        if mean.iter().any(|value| !value.is_finite() || *value < 0.02) {
            return Err("The flat field is too dark; use a brighter exposure".into());
        }
        Ok(mean)
    };
    let center = patch(geometry.center_x, geometry.center_y)?;
    let mut gains = Vec::with_capacity((geometry.grid_w * geometry.grid_h) as usize);
    for gy in 0..geometry.grid_h {
        for gx in 0..geometry.grid_w {
            let sample = patch(
                geometry.center_x + (gx as f32 - (geometry.grid_w - 1) as f32 / 2.0) * spacing,
                geometry.center_y + (gy as f32 - (geometry.grid_h - 1) as f32 / 2.0) * spacing,
            )?;
            let gain: [f32; 4] = std::array::from_fn(|slot| center[slot] / sample[slot]);
            if gain.iter().any(|g| *g < 0.25 || *g > 8.0) {
                return Err(
                    "The flat field has excessive variation; check the target and exposure".into(),
                );
            }
            gains.push(gain);
        }
    }
    Ok((geometry, ShadingGrid { f_number, gains }))
}

/// Combine multiple apertures, averaging repeated captures at one aperture.
pub fn calibrate(paths: &[std::path::PathBuf], name: &str) -> Result<LensProfile, String> {
    if paths.is_empty() || paths.len() > 64 || name.trim().is_empty() {
        return Err("Choose 1–64 RAW flat fields and a profile name".into());
    }
    let mut profile: Option<LensProfile> = None;
    let mut identity = None;
    let mut repeated = std::collections::HashMap::<u32, u32>::new();
    for path in paths {
        let output = crate::decode(path).map_err(|e| e.to_string())?;
        let lens = output
            .meta
            .lens_model
            .as_ref()
            .filter(|v| !v.trim().is_empty())
            .ok_or("The flat-field RAW needs a lens name in its metadata")?;
        let aperture = output
            .meta
            .aperture_f
            .ok_or("The flat-field RAW needs an aperture in its metadata")?;
        let focal = output.meta.focal_length_mm;
        let current = (
            output.meta.camera_model.clone(),
            sp_core::lens::lens_key(lens),
            focal.map(f32::to_bits),
            output.raw.width,
            output.raw.height,
        );
        if identity.as_ref().is_some_and(|id| *id != current) {
            return Err("All flat fields must use the same camera, lens, focal length and sensor dimensions".into());
        }
        identity = Some(current);
        let (geometry, grid) = measure_flat(&output.raw, aperture)?;
        let p = profile.get_or_insert_with(|| {
            let mut p = LensProfile::identity(name.trim(), sp_core::lens::lens_key(lens), geometry);
            p.apertures.clear();
            p.focal_mm = focal;
            p
        });
        let count = repeated.entry(aperture.to_bits()).or_default();
        if let Some(existing) = p.apertures.iter_mut().find(|g| g.f_number == aperture) {
            for (old, new) in existing.gains.iter_mut().zip(grid.gains) {
                for i in 0..4 {
                    old[i] = (old[i] * *count as f32 + new[i]) / (*count + 1) as f32;
                }
            }
        } else {
            p.apertures.push(grid);
        }
        *count += 1;
    }
    let mut profile = profile.ok_or("No flat field was decoded")?;
    profile
        .apertures
        .sort_by(|a, b| a.f_number.total_cmp(&b.f_number));
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flat() -> RawImage {
        use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
        let mut data = vec![0; 128 * 128];
        for y in 0..128 {
            for x in 0..128 {
                let r = ((x as f32 - 64.0).powi(2) + (y as f32 - 64.0).powi(2)) / 8192.0;
                data[y * 128 + x] = (1000.0 + 24000.0 * (1.0 - 0.4 * r)) as u16;
            }
        }
        RawImage {
            cfa_data: data,
            width: 128,
            height: 128,
            cfa: CfaPattern::Rggb,
            black_levels: [1000.0; 4],
            white_levels: [65535.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            active_area: ActiveArea {
                x: 0,
                y: 0,
                width: 128,
                height: 128,
            },
            orientation: Orientation::Normal,
        }
    }
    #[test]
    fn measures_radial_falloff_and_rejects_bad_exposure() {
        let raw = flat();
        let (g, grid) = measure_flat(&raw, 8.0).unwrap();
        let center = ((g.grid_h / 2) * g.grid_w + g.grid_w / 2) as usize;
        assert!(grid.gains[0][0] > 1.3);
        assert!((grid.gains[center][0] - 1.0).abs() < 1e-6);
        let mut clipped = raw.clone();
        clipped.cfa_data.fill(65535);
        assert!(measure_flat(&clipped, 8.0).is_err());
        clipped.cfa_data.fill(1000);
        assert!(measure_flat(&clipped, 8.0).is_err());
    }
}
