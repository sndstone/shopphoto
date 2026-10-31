// Pre-optimization resampler retained as an independent quality reference.
use super::*;
pub(super) fn resize_linear(
    image: &ExportImage,
    out_w: u32,
    out_h: u32,
    transfer: Transfer,
) -> ExportImage {
    if (out_w, out_h) == (image.width, image.height) {
        return image.clone();
    }
    let mut rgba8 = vec![0; (out_w * out_h * 4) as usize];
    let mut rgba16 = image
        .rgba16
        .as_ref()
        .map(|_| vec![0; (out_w * out_h * 4) as usize]);
    let sx = image.width as f32 / out_w as f32;
    let sy = image.height as f32 / out_h as f32;
    for oy in 0..out_h {
        for ox in 0..out_w {
            let cx = (ox as f32 + 0.5) * sx - 0.5;
            let cy = (oy as f32 + 0.5) * sy - 0.5;
            let rx = sx.max(1.0);
            let ry = sy.max(1.0);
            let x0 = (cx - 3.0 * rx).floor() as i32;
            let x1 = (cx + 3.0 * rx).ceil() as i32;
            let y0 = (cy - 3.0 * ry).floor() as i32;
            let y1 = (cy + 3.0 * ry).ceil() as i32;
            let mut sum = [0.0f32; 4];
            let mut weights = 0.0;
            for iy in y0..=y1 {
                let wy = lanczos((iy as f32 - cy) / ry);
                for ix in x0..=x1 {
                    let weight = wy * lanczos((ix as f32 - cx) / rx);
                    if weight == 0.0 {
                        continue;
                    }
                    let sample = bilinear(image, ix as f32, iy as f32, transfer);
                    for channel in 0..4 {
                        sum[channel] += sample[channel] * weight;
                    }
                    weights += weight;
                }
            }
            let off = ((oy * out_w + ox) * 4) as usize;
            let denom = weights.max(1e-6);
            for channel in 0..3 {
                let encoded = encode_channel(sum[channel] / denom, transfer).clamp(0.0, 1.0);
                rgba8[off + channel] = (encoded * 255.0).round() as u8;
                if let Some(pixels) = &mut rgba16 {
                    pixels[off + channel] = (encoded * 65535.0).round() as u16;
                }
            }
            rgba8[off + 3] = ((sum[3] / denom).clamp(0.0, 1.0) * 255.0).round() as u8;
            if let Some(pixels) = &mut rgba16 {
                pixels[off + 3] = ((sum[3] / denom).clamp(0.0, 1.0) * 65535.0).round() as u16;
            }
        }
    }
    ExportImage {
        width: out_w,
        height: out_h,
        rgba8,
        rgba16,
    }
}
