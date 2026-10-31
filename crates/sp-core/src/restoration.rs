//! Reference for the bounded, regularized two-term inverse used by the GPU.
//! Assumes a small separable binomial PSF; it cannot infer motion or lost detail.
use crate::edit::SharpenParams;

#[must_use]
pub fn restore_luma(
    x: i32,
    y: i32,
    params: &SharpenParams,
    sample: impl Fn(i32, i32) -> f32,
) -> f32 {
    let value = sample(x, y);
    if params.restore_blur <= 0.0 {
        return value;
    }
    let radius = params.restore_radius_px.round().clamp(1.0, 2.0) as i32;
    let w = [0.25, 0.5, 0.25];
    let w2 = [0.0625, 0.25, 0.375, 0.25, 0.0625];
    let (mut blur, mut twice) = (0.0, 0.0);
    for j in -2i32..=2 {
        for i in -2i32..=2 {
            let v = sample(x + i * radius, y + j * radius);
            twice += v * w2[(i + 2) as usize] * w2[(j + 2) as usize];
            if i.abs() <= 1 && j.abs() <= 1 {
                blur += v * w[(i + 1) as usize] * w[(j + 1) as usize];
            }
        }
    }
    let guard = params.restore_regularization.clamp(0.0, 100.0) / 100.0;
    let amount = params.restore_blur.clamp(0.0, 100.0) / 100.0 * (0.85 - 0.4 * guard);
    let high = value - blur;
    let floor = (0.0002 + 0.0118 * guard) * value.abs().max(1.0);
    let gate = ((high.abs() - floor) / (2.0 * floor)).clamp(0.0, 1.0);
    let gate = gate * gate * (3.0 - 2.0 * gate);
    let correction = amount * high + amount * amount * (value - 2.0 * blur + twice);
    let limit = (0.3 - 0.22 * guard) * value.abs().max(0.05);
    (value + correction.clamp(-limit, limit) * gate).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reduces_known_blur_error_and_preserves_flat_hdr_and_low_noise() {
        let original = |x: i32| 0.5 + 0.2 * (x as f32 * 0.9).sin();
        let blurred =
            |x: i32, _: i32| 0.25 * original(x - 1) + 0.5 * original(x) + 0.25 * original(x + 1);
        let params = SharpenParams {
            restore_blur: 100.0,
            restore_regularization: 10.0,
            ..Default::default()
        };
        let before: f32 = (0..100)
            .map(|x| (blurred(x, 0) - original(x)).powi(2))
            .sum();
        let after: f32 = (0..100)
            .map(|x| (restore_luma(x, 0, &params, blurred) - original(x)).powi(2))
            .sum();
        assert!(after < before * 0.2, "{before} -> {after}");
        assert_eq!(restore_luma(0, 0, &params, |_, _| 4.0), 4.0);
        let noisy = |x: i32, y: i32| 0.5 + if (x + y) % 2 == 0 { 0.0003 } else { -0.0003 };
        assert_eq!(restore_luma(0, 0, &params, noisy), noisy(0, 0));
    }
    #[test]
    fn sharpness_preserves_restoration_and_older_sidecars_default_to_off() {
        let mut params = SharpenParams {
            restore_blur: 42.0,
            restore_radius_px: 2.0,
            ..Default::default()
        };
        params.apply_sharpness(30.0);
        assert_eq!(params.restore_blur, 42.0);
        assert_eq!(params.restore_radius_px, 2.0);
        let old = serde_json::json!({"amount":20.0,"radius_px":1.0,"detail":25.0});
        let params: SharpenParams = serde_json::from_value(old).unwrap();
        assert_eq!(params.restore_blur, 0.0);
    }
}
