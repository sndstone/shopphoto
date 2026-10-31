//! Shared image-coordinate transforms. Masks and retouch remain in canonical
//! pre-lens coordinates; viewport and export transform their presentation.
use crate::edit::GeometryParams;

#[derive(Debug, Clone, Copy)]
pub struct GeometryMap {
    pub image_size: [u32; 2],
    pub output_size: [u32; 2],
    pub packed: [[f32; 4]; 3],
}

impl GeometryMap {
    pub fn new(image_size: [u32; 2], params: GeometryParams) -> Self {
        let [width, height] = image_size.map(|v| v.max(1) as f32);
        let [left, top, right, bottom] = params.crop;
        let crop_w = ((right - left).max(0.01) * width).round().max(1.0);
        let crop_h = ((bottom - top).max(0.01) * height).round().max(1.0);
        let turn = params.quarter_turns % 4;
        let (sin, cos) = params.rotate_deg.to_radians().sin_cos();
        let (base_w, base_h) = if turn.is_multiple_of(2) {
            (crop_w, crop_h)
        } else {
            (crop_h, crop_w)
        };
        let output_size = [
            (base_w * cos.abs() + base_h * sin.abs()).round().max(1.0) as u32,
            (base_h * cos.abs() + base_w * sin.abs()).round().max(1.0) as u32,
        ];
        let scale = (params.scale / 100.0).max(0.1);
        let transform = |x: f32, y: f32| {
            let dx =
                x * output_size[0] as f32 / scale * if params.flip_horizontal { -1.0 } else { 1.0 };
            let dy =
                y * output_size[1] as f32 / scale * if params.flip_vertical { -1.0 } else { 1.0 };
            let (rx, ry) = (cos * dx + sin * dy, -sin * dx + cos * dy);
            match turn {
                1 => [-ry, rx],
                2 => [-rx, -ry],
                3 => [ry, -rx],
                _ => [rx, ry],
            }
        };
        let x = transform(1.0, 0.0);
        let y = transform(0.0, 1.0);
        Self {
            image_size,
            output_size,
            packed: [
                [x[0], y[0], x[1], y[1]],
                [
                    left + crop_w * 0.5 / width,
                    top + crop_h * 0.5 / height,
                    1.0 / width,
                    1.0 / height,
                ],
                [
                    params.perspective_vertical / 100.0 / crop_h,
                    params.perspective_horizontal / 100.0 / crop_w,
                    1.0,
                    0.0,
                ],
            ],
        }
    }

    /// Output pixel-center UV to the input pixel-center UV used by export.
    pub fn source_uv(&self, output_uv: [f32; 2]) -> [f32; 2] {
        let [m00, m01, m10, m11] = self.packed[0];
        let x = output_uv[0] - 0.5;
        let y = output_uv[1] - 0.5;
        let qx = m00 * x + m01 * y;
        let qy = m10 * x + m11 * y;
        let [cx, cy, sx, sy] = self.packed[1];
        [
            cx + qx / (1.0 + qy * self.packed[2][0]).max(0.25) * sx,
            cy + qy / (1.0 + qx * self.packed[2][1]).max(0.25) * sy,
        ]
    }

    pub fn output_uv(&self, source_uv: [f32; 2]) -> [f32; 2] {
        invert_map(source_uv, |uv| self.source_uv(uv))
    }
}

/// Green/luminance geometry used by all three output shaders. Chromatic
/// offsets affect color samples, while mask coverage follows green geometry.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LensMap {
    pub distortion: [f32; 3],
    pub lateral_ca: [f32; 2],
    pub enabled: bool,
    pub crop_to_valid: bool,
}

impl LensMap {
    pub fn source_uv(self, output_uv: [f32; 2]) -> [f32; 2] {
        if !self.enabled {
            return output_uv;
        }
        let [k1, k2, k3] = self.distortion;
        let scale = if self.crop_to_valid {
            let radial = (1.0 + k1 * 2.0 + k2 * 4.0 + k3 * 8.0).abs();
            let ca = (1.0 + self.lateral_ca[0] * 2.0)
                .abs()
                .max((1.0 + self.lateral_ca[1] * 2.0).abs());
            (radial * ca).max(1.0)
        } else {
            1.0
        };
        let x = (output_uv[0] * 2.0 - 1.0) / scale;
        let y = (output_uv[1] * 2.0 - 1.0) / scale;
        let r2 = x * x + y * y;
        let radial = 1.0 + k1 * r2 + k2 * r2 * r2 + k3 * r2 * r2 * r2;
        [(x * radial + 1.0) * 0.5, (y * radial + 1.0) * 0.5]
    }
    pub fn output_uv(self, source_uv: [f32; 2]) -> [f32; 2] {
        if !self.enabled {
            return source_uv;
        }
        invert_map(source_uv, |uv| self.source_uv(uv))
    }
}

fn invert_map(target: [f32; 2], map: impl Fn([f32; 2]) -> [f32; 2]) -> [f32; 2] {
    let mut uv = target;
    for _ in 0..20 {
        let value = map(uv);
        let error = [value[0] - target[0], value[1] - target[1]];
        if error[0].abs().max(error[1].abs()) < 1e-6 {
            break;
        }
        let dx = map([uv[0] + 0.0001, uv[1]]);
        let dy = map([uv[0], uv[1] + 0.0001]);
        let a = (dx[0] - value[0]) * 10000.0;
        let b = (dy[0] - value[0]) * 10000.0;
        let c = (dx[1] - value[1]) * 10000.0;
        let d = (dy[1] - value[1]) * 10000.0;
        let determinant = a * d - b * c;
        if determinant.abs() < 1e-8 {
            break;
        }
        uv[0] -= ((d * error[0] - b * error[1]) / determinant).clamp(-1.0, 1.0);
        uv[1] -= ((a * error[1] - c * error[0]) / determinant).clamp(-1.0, 1.0);
    }
    uv
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_roundtrip_keeps_canonical_mask_coordinates() {
        for turn in 0..4 {
            for flip in [false, true] {
                let map = GeometryMap::new(
                    [6016, 4016],
                    GeometryParams {
                        crop: [0.1, 0.2, 0.8, 0.9],
                        quarter_turns: turn,
                        rotate_deg: 11.0,
                        flip_horizontal: flip,
                        flip_vertical: !flip,
                        perspective_vertical: 23.0,
                        perspective_horizontal: -17.0,
                        scale: 113.0,
                    },
                );
                for point in [[0.3, 0.4], [0.5, 0.5], [0.7, 0.6]] {
                    let round = map.source_uv(map.output_uv(point));
                    assert!(
                        (point[0] - round[0]).abs() < 2e-5 && (point[1] - round[1]).abs() < 2e-5,
                        "{point:?} {round:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn neutral_and_quarter_turn_pixel_centers_match_export() {
        let map = GeometryMap::new([6, 4], GeometryParams::default());
        assert_eq!(map.output_size, [6, 4]);
        assert_eq!(map.source_uv([0.25, 0.375]), [0.25, 0.375]);
        let map = GeometryMap::new(
            [6, 4],
            GeometryParams {
                quarter_turns: 1,
                ..Default::default()
            },
        );
        assert_eq!(map.output_size, [4, 6]);
        let uv = map.source_uv([0.125, 1.0 / 12.0]);
        assert!((uv[0] - 11.0 / 12.0).abs() < 1e-6 && (uv[1] - 0.125).abs() < 1e-6);
    }
    #[test]
    fn lens_roundtrip_preserves_mask_points_when_correction_changes() {
        for crop_to_valid in [false, true] {
            for k1 in [-0.05, 0.08] {
                let lens = LensMap {
                    distortion: [k1, 0.01, 0.001],
                    lateral_ca: [0.004, -0.003],
                    enabled: true,
                    crop_to_valid,
                };
                for p in [[0.12, 0.2], [0.5, 0.5], [0.9, 0.72]] {
                    let round = lens.source_uv(lens.output_uv(p));
                    assert!((round[0] - p[0]).abs() < 2e-5 && (round[1] - p[1]).abs() < 2e-5);
                }
            }
        }
    }
}
