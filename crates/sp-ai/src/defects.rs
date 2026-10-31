//! Stuck-photosite pre-pass for the ShopPhoto model, ported from
//! `denoise_lab.defects.correct_defects(k=8, abs_floor=0.01,
//! adjacency_ratio=0.1)`, the `rawdenoise` CLI default.
//!
//! A photosite is replaced by the median of its eight same-colour neighbours
//! when it lies more than `k·σ` beyond all of them while its four
//! mosaic-adjacent (other-colour) photosites show at most a tenth of that
//! deviation. Real detail and point lights spread over adjacent photosites
//! through the lens; stuck photosites do not. σ comes from a per-plane noise
//! envelope over eight intensity bins. Statistics always read the original
//! samples.

use crate::{mirror, PLANES, PLANE_OFFSETS};
use sp_core::RawImage;

const K: f32 = 8.0;
const ABS_FLOOR: f32 = 0.01;
const ADJACENCY_RATIO: f32 = 0.1;
const MAD_TO_SIGMA: f32 = 0.674_489_75;

/// Mosaic-adjacent photosites of each plane (R, G1, B, G2) as
/// `(plane, dy, dx)` in packed coordinates, clamped at the edges.
const ADJACENT: [[(usize, isize, isize); 4]; PLANES] = [
    [(1, 0, 0), (1, 0, -1), (3, 0, 0), (3, -1, 0)],
    [(0, 0, 0), (0, 0, 1), (2, 0, 0), (2, -1, 0)],
    [(3, 0, 0), (3, 0, 1), (1, 0, 0), (1, 1, 0)],
    [(2, 0, 0), (2, 0, -1), (0, 0, 0), (0, 1, 0)],
];

/// Normalized R/G1/B/G2 view of a red-topped Bayer mosaic.
struct Planes<'a> {
    data: &'a [u16],
    stride: usize,
    origin: (usize, usize),
    width: usize,
    height: usize,
    black: [f32; PLANES],
    range: [f32; PLANES],
}

impl Planes<'_> {
    fn index(&self, c: usize, y: usize, x: usize) -> usize {
        let (dx, dy) = PLANE_OFFSETS[c];
        (self.origin.1 + 2 * y + dy) * self.stride + self.origin.0 + 2 * x + dx
    }

    fn at(&self, c: usize, y: usize, x: usize) -> f32 {
        (f32::from(self.data[self.index(c, y, x)]) - self.black[c]) / self.range[c]
    }

    /// The eight same-plane neighbours, mirrored at the edges.
    fn neighbours(&self, c: usize, y: usize, x: usize) -> [f32; 8] {
        let mut values = [0.0; 8];
        let mut n = 0;
        for dy in -1..=1isize {
            for dx in -1..=1isize {
                if dy == 0 && dx == 0 {
                    continue;
                }
                values[n] = self.at(
                    c,
                    mirror(y as isize + dy, self.height),
                    mirror(x as isize + dx, self.width),
                );
                n += 1;
            }
        }
        values
    }

    fn median(&self, c: usize, y: usize, x: usize) -> f32 {
        let mut values = self.neighbours(c, y, x);
        values.sort_unstable_by(f32::total_cmp);
        (values[3] + values[4]) * 0.5
    }

    fn excess(&self, c: usize, y: isize, x: isize) -> f32 {
        let y = y.clamp(0, self.height as isize - 1) as usize;
        let x = x.clamp(0, self.width as isize - 1) as usize;
        self.at(c, y, x) - self.median(c, y, x)
    }
}

/// Lower median, matching `torch.median`.
fn lower_median(values: &mut [f32]) -> f32 {
    let middle = (values.len() - 1) / 2;
    *values.select_nth_unstable_by(middle, f32::total_cmp).1
}

fn mad(values: &[f32]) -> f32 {
    let mut values = values.to_vec();
    let center = lower_median(&mut values);
    for value in &mut values {
        *value = (*value - center).abs();
    }
    lower_median(&mut values) / MAD_TO_SIGMA
}

/// Nondecreasing noise σ over eight equal-width neighbour-median bins.
struct SigmaModel {
    edges: [f32; 7],
    scales: [f32; 8],
}

impl SigmaModel {
    fn fit(planes: &Planes, c: usize) -> Self {
        let (h, w) = (planes.height, planes.width);
        let step = ((h * w) as f64 / 65536.0).sqrt().ceil().max(1.0) as usize;
        let mut medians = Vec::new();
        let mut residuals = Vec::new();
        for y in (0..h).step_by(step) {
            for x in (0..w).step_by(step) {
                let median = planes.median(c, y, x);
                medians.push(median);
                residuals.push(planes.at(c, y, x) - median);
            }
        }
        let fallback = mad(&residuals);
        let low = medians.iter().copied().fold(f32::INFINITY, f32::min);
        let high = medians.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let edges = std::array::from_fn(|i| low + (high - low) * ((i + 1) as f32 / 8.0));
        let mut model = Self {
            edges,
            scales: [fallback; 8],
        };
        let mut bins: [Vec<f32>; 8] = Default::default();
        for (&median, &residual) in medians.iter().zip(&residuals) {
            bins[model.bin(median)].push(residual);
        }
        let mut running = f32::NEG_INFINITY;
        for (scale, bin) in model.scales.iter_mut().zip(&bins) {
            if bin.len() >= 32 {
                *scale = mad(bin);
            }
            running = running.max(*scale);
            *scale = running;
        }
        model
    }

    /// `torch.bucketize(value, edges)`: boundaries strictly below `value`.
    fn bin(&self, value: f32) -> usize {
        self.edges.iter().filter(|&&edge| edge < value).count()
    }

    fn sigma(&self, median: f32) -> f32 {
        self.scales[self.bin(median)]
    }
}

/// Correct isolated stuck photosites inside the `size` (packed quads) area
/// whose first red-topped quad starts at sensor coordinate `origin`.
pub(crate) fn correct(raw: &mut RawImage, origin: (usize, usize), size: (usize, usize)) {
    let (width, height) = size;
    if width == 0 || height == 0 {
        return;
    }
    let calibration = [0, 1, 2, 1];
    let planes = Planes {
        data: &raw.cfa_data,
        stride: raw.width as usize,
        origin,
        width,
        height,
        black: calibration.map(|channel| raw.black_levels[channel]),
        range: calibration
            .map(|channel| (raw.white_levels[channel] - raw.black_levels[channel]).max(1.0)),
    };
    let models: [SigmaModel; PLANES] = std::array::from_fn(|c| SigmaModel::fit(&planes, c));
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(height);
    let rows = height.div_ceil(threads);
    let fixes: Vec<(usize, u16)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..height)
            .step_by(rows)
            .map(|start| {
                let (planes, models) = (&planes, &models);
                scope.spawn(move || scan(planes, models, start..(start + rows).min(height)))
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("defect scan worker"))
            .collect()
    });
    for (index, value) in fixes {
        raw.cfa_data[index] = value;
    }
}

fn scan(
    planes: &Planes,
    models: &[SigmaModel; PLANES],
    rows: std::ops::Range<usize>,
) -> Vec<(usize, u16)> {
    let mut fixes = Vec::new();
    for y in rows {
        for x in 0..planes.width {
            for c in 0..PLANES {
                let value = planes.at(c, y, x);
                let mut neighbours = planes.neighbours(c, y, x);
                let low = neighbours.iter().copied().fold(f32::INFINITY, f32::min);
                let high = neighbours.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                // The margin never drops below the floor, so nearly every
                // photosite is settled without sorting.
                if value - high <= ABS_FLOOR && low - value <= ABS_FLOOR {
                    continue;
                }
                neighbours.sort_unstable_by(f32::total_cmp);
                let median = (neighbours[3] + neighbours[4]) * 0.5;
                let margin = (K * models[c].sigma(median)).max(ABS_FLOOR);
                let hot = value - high > margin;
                let dead = low - value > margin;
                if !hot && !dead {
                    continue;
                }
                let excess = value - median;
                let adjacent = ADJACENT[c]
                    .map(|(a, dy, dx)| planes.excess(a, y as isize + dy, x as isize + dx));
                let isolated = if hot {
                    adjacent.iter().copied().fold(f32::NEG_INFINITY, f32::max)
                        < ADJACENCY_RATIO * excess
                } else {
                    adjacent.iter().copied().fold(f32::INFINITY, f32::min)
                        > ADJACENCY_RATIO * excess
                };
                if isolated {
                    let code = median * planes.range[c] + planes.black[c];
                    fixes.push((
                        planes.index(c, y, x),
                        code.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                    ));
                }
            }
        }
    }
    fixes
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_core::raw::{ActiveArea, CfaPattern, Orientation};

    fn noisy(size: u32) -> RawImage {
        let mut state = 0x2545_f491_u32;
        let cfa_data = (0..size * size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                1000 + (state % 40) as u16
            })
            .collect();
        RawImage {
            cfa_data,
            width: size,
            height: size,
            cfa: CfaPattern::Rggb,
            black_levels: [0.0; 4],
            white_levels: [16383.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            active_area: ActiveArea {
                x: 0,
                y: 0,
                width: size,
                height: size,
            },
            orientation: Orientation::Normal,
        }
    }

    #[test]
    fn replaces_isolated_stuck_photosites_only() {
        let mut raw = noisy(64);
        let clean = raw.clone();
        let hot = 20 * 64 + 21; // G1 photosite
        let dead = 41 * 64 + 41; // B photosite
        raw.cfa_data[hot] = 9000;
        raw.cfa_data[dead] = 0;
        // A point light covers a whole quad, adjacent colours included.
        for (y, x) in [(30, 30), (30, 31), (31, 30), (31, 31)] {
            raw.cfa_data[y * 64 + x] = 9000;
        }
        correct(&mut raw, (0, 0), (32, 32));
        assert!((1000..1040).contains(&raw.cfa_data[hot]));
        assert!((1000..1040).contains(&raw.cfa_data[dead]));
        assert_eq!(raw.cfa_data[30 * 64 + 30], 9000);
        assert_eq!(raw.cfa_data[31 * 64 + 31], 9000);
        let changed = raw
            .cfa_data
            .iter()
            .zip(&clean.cfa_data)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            changed, 6,
            "only the two defects and the point light differ"
        );
    }

    #[test]
    fn lower_median_matches_torch() {
        assert_eq!(lower_median(&mut [4.0, 1.0, 3.0, 2.0]), 2.0);
        assert_eq!(lower_median(&mut [5.0, 1.0, 3.0]), 3.0);
    }
}
