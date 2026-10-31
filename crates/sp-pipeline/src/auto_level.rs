//! Conservative horizon estimation for one-click auto level.

/// Estimate the rotation needed to make dominant near-horizontal edges level.
/// Returns `None` for flat or ambiguous frames rather than inventing an angle.
#[must_use]
pub fn estimate_auto_level(rgba: &[u8], width: u32, height: u32) -> Option<f32> {
    if width < 8 || height < 8 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let stride = (width.max(height) / 700).max(1) as usize;
    let luma = |x: usize, y: usize| {
        let offset = (y * width as usize + x) * 4;
        0.2126 * f32::from(rgba[offset])
            + 0.7152 * f32::from(rgba[offset + 1])
            + 0.0722 * f32::from(rgba[offset + 2])
    };
    let mut sine = 0.0f64;
    let mut cosine = 0.0f64;
    let mut total = 0.0f64;
    let mut samples = 0usize;
    for y in (1..height as usize - 1).step_by(stride) {
        for x in (1..width as usize - 1).step_by(stride) {
            let gx = -luma(x - 1, y - 1) + luma(x + 1, y - 1) - 2.0 * luma(x - 1, y)
                + 2.0 * luma(x + 1, y)
                - luma(x - 1, y + 1)
                + luma(x + 1, y + 1);
            let gy = -luma(x - 1, y - 1) - 2.0 * luma(x, y - 1) - luma(x + 1, y - 1)
                + luma(x - 1, y + 1)
                + 2.0 * luma(x, y + 1)
                + luma(x + 1, y + 1);
            let magnitude = gx.hypot(gy);
            if magnitude < 35.0 {
                continue;
            }
            let mut line_angle = gy.atan2(gx) + std::f32::consts::FRAC_PI_2;
            while line_angle > std::f32::consts::FRAC_PI_2 {
                line_angle -= std::f32::consts::PI;
            }
            while line_angle < -std::f32::consts::FRAC_PI_2 {
                line_angle += std::f32::consts::PI;
            }
            if line_angle.abs() > 25.0f32.to_radians() {
                continue;
            }
            let weight = f64::from(magnitude.min(1020.0)).powi(2);
            sine += f64::from((line_angle * 2.0).sin()) * weight;
            cosine += f64::from((line_angle * 2.0).cos()) * weight;
            total += weight;
            samples += 1;
        }
    }
    if samples < 16 || total <= 0.0 {
        return None;
    }
    let confidence = sine.hypot(cosine) / total;
    if confidence < 0.55 {
        return None;
    }
    let angle = 0.5 * sine.atan2(cosine).to_degrees() as f32;
    if angle.abs() < 0.08 {
        Some(0.0)
    } else {
        Some((-angle).clamp(-15.0, 15.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_tilted_horizon_and_refuses_flat_frames() {
        let (width, height) = (160u32, 100u32);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let slope = 0.10f32;
        for y in 0..height {
            for x in 0..width {
                let horizon = 35.0 + slope * x as f32;
                let value = (128.0 + (y as f32 - horizon) * 80.0).clamp(30.0, 220.0) as u8;
                let offset = ((y * width + x) * 4) as usize;
                pixels[offset..offset + 3].fill(value);
                pixels[offset + 3] = 255;
            }
        }
        let correction = estimate_auto_level(&pixels, width, height).unwrap();
        assert!(
            (correction + slope.atan().to_degrees()).abs() < 1.0,
            "{correction}"
        );
        assert!(estimate_auto_level(&vec![128; pixels.len()], width, height).is_none());
    }
}
