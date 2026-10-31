//! Small dependency-free visual-regression comparator for CI screenshots.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let reference = arguments
        .next()
        .ok_or("usage: visual_compare REFERENCE ACTUAL [rmse]")?;
    let actual = arguments
        .next()
        .ok_or("usage: visual_compare REFERENCE ACTUAL [rmse]")?;
    let threshold = arguments
        .next()
        .and_then(|value| value.to_string_lossy().parse::<f64>().ok())
        .unwrap_or(3.0);
    let changed_limit = arguments
        .next()
        .and_then(|value| value.to_string_lossy().parse::<f64>().ok())
        .unwrap_or(0.02);
    let reference = image::open(reference)?.to_rgba8();
    let actual = image::open(actual)?.to_rgba8();
    if reference.dimensions() != actual.dimensions() {
        return Err(format!(
            "screenshot dimensions differ: reference {:?}, actual {:?}",
            reference.dimensions(),
            actual.dimensions()
        )
        .into());
    }
    let mut squared = 0f64;
    let mut changed = 0usize;
    for (left, right) in reference.pixels().zip(actual.pixels()) {
        let mut pixel_peak = 0u8;
        for channel in 0..3 {
            let delta = left[channel].abs_diff(right[channel]);
            squared += f64::from(delta).powi(2);
            pixel_peak = pixel_peak.max(delta);
        }
        changed += usize::from(pixel_peak > 12);
    }
    let rmse = (squared / (reference.width() as f64 * reference.height() as f64 * 3.0)).sqrt();
    let changed_fraction = changed as f64 / (reference.width() as f64 * reference.height() as f64);
    println!(
        "visual RMSE {rmse:.3}; pixels over 12 levels {:.3}%",
        changed_fraction * 100.0
    );
    if rmse > threshold || changed_fraction > changed_limit {
        return Err("visual regression exceeded tolerance".into());
    }
    Ok(())
}
