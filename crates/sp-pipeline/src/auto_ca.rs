//! Automatic lateral chromatic-aberration estimation from a rendered proxy.
//!
//! The estimator aligns red and blue edge gradients against green in the
//! outer image area. It is deliberately conservative: weak/flat images and
//! fits that do not improve the zero-correction score return no change.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaRecommendation {
    pub red_scale: f32,
    pub blue_scale: f32,
    pub improvement: f32,
}

fn channel_at(rgba: &[u8], width: u32, height: u32, x: f32, y: f32, channel: usize) -> f32 {
    let x = x.round().clamp(0.0, width.saturating_sub(1) as f32) as u32;
    let y = y.round().clamp(0.0, height.saturating_sub(1) as f32) as u32;
    f32::from(rgba[((y * width + x) * 4) as usize + channel]) / 255.0
}

fn score(rgba: &[u8], width: u32, height: u32, channel: usize, delta: f32) -> Option<f64> {
    if width < 16 || height < 16 || rgba.len() != (width * height * 4) as usize {
        return None;
    }
    let stride = (width.max(height) / 256).max(2) as usize;
    let center = [(width - 1) as f32 * 0.5, (height - 1) as f32 * 0.5];
    let scale = [center[0].max(1.0), center[1].max(1.0)];
    let mut error = 0.0f64;
    let mut weight_sum = 0.0f64;
    let mut samples = 0usize;
    for y in (2..height - 2).step_by(stride) {
        for x in (2..width - 2).step_by(stride) {
            let nx = (x as f32 - center[0]) / scale[0];
            let ny = (y as f32 - center[1]) / scale[1];
            let radius2 = nx * nx + ny * ny;
            if radius2 < 0.16 {
                continue;
            }
            let gx = channel_at(rgba, width, height, x as f32 + 1.0, y as f32, 1)
                - channel_at(rgba, width, height, x as f32 - 1.0, y as f32, 1);
            let gy = channel_at(rgba, width, height, x as f32, y as f32 + 1.0, 1)
                - channel_at(rgba, width, height, x as f32, y as f32 - 1.0, 1);
            let edge = gx.abs() + gy.abs();
            if edge < 0.035 {
                continue;
            }
            let warp = 1.0 + delta * radius2;
            let sx = center[0] + (x as f32 - center[0]) * warp;
            let sy = center[1] + (y as f32 - center[1]) * warp;
            let cgx = channel_at(rgba, width, height, sx + 1.0, sy, channel)
                - channel_at(rgba, width, height, sx - 1.0, sy, channel);
            let cgy = channel_at(rgba, width, height, sx, sy + 1.0, channel)
                - channel_at(rgba, width, height, sx, sy - 1.0, channel);
            let weight = f64::from(edge.min(0.5));
            error += f64::from((cgx - gx).abs() + (cgy - gy).abs()) * weight;
            weight_sum += weight;
            samples += 1;
        }
    }
    (samples >= 64 && weight_sum > 0.0).then_some(error / weight_sum)
}

fn fit_channel(rgba: &[u8], width: u32, height: u32, channel: usize) -> Option<(f32, f64, f64)> {
    let baseline = score(rgba, width, height, channel, 0.0)?;
    let mut best = (0.0f32, baseline);
    for step in -24..=24 {
        let delta = step as f32 * 0.0005;
        if let Some(candidate) = score(rgba, width, height, channel, delta) {
            if candidate < best.1 {
                best = (delta, candidate);
            }
        }
    }
    Some((best.0, baseline, best.1))
}

#[must_use]
pub fn estimate(rgba: &[u8], width: u32, height: u32) -> Option<CaRecommendation> {
    let (red, red_zero, red_best) = fit_channel(rgba, width, height, 0)?;
    let (blue, blue_zero, blue_best) = fit_channel(rgba, width, height, 2)?;
    let zero = red_zero + blue_zero;
    let best = red_best + blue_best;
    let improvement = ((zero - best) / zero.max(1e-9)) as f32;
    if improvement < 0.005 || (red.abs() < 0.0005 && blue.abs() < 0.0005) {
        return None;
    }
    Some(CaRecommendation {
        red_scale: red,
        blue_scale: blue,
        improvement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_radially_shifted_color_edges() {
        let (width, height) = (256, 192);
        let mut rgba = vec![255u8; width * height * 4];
        let center = [width as f32 * 0.5, height as f32 * 0.5];
        for y in 0..height {
            for x in 0..width {
                let nx = (x as f32 - center[0]) / center[0];
                let ny = (y as f32 - center[1]) / center[1];
                let r2 = nx * nx + ny * ny;
                let pattern = |warp: f32| {
                    let xx = center[0] + (x as f32 - center[0]) * (1.0 + warp * r2);
                    if ((xx / 12.0).floor() as i32) & 1 == 0 {
                        35
                    } else {
                        220
                    }
                };
                let offset = (y * width + x) * 4;
                rgba[offset] = pattern(0.008);
                rgba[offset + 1] = pattern(0.0);
                rgba[offset + 2] = pattern(-0.006);
                rgba[offset + 3] = 255;
            }
        }
        let result = estimate(&rgba, width as u32, height as u32).expect("coherent CA");
        assert!(result.red_scale.abs() >= 0.002);
        assert!(result.blue_scale.abs() >= 0.002);
        assert!(result.improvement > 0.01);
    }

    #[test]
    fn flat_frame_is_not_actionable() {
        assert!(estimate(&vec![128; 128 * 96 * 4], 128, 96).is_none());
    }
}
