//! Parser and sampler for the common `.cube` 1-D/3-D look format.

use crate::profile::Lut3d;

#[derive(Debug, Clone, PartialEq)]
pub struct CubeLut {
    pub title: Option<String>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub one_d: Option<Vec<[f32; 3]>>,
    pub three_d: Option<Lut3d>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CubeError {
    #[error("invalid .cube directive on line {line}: {reason}")]
    Invalid { line: usize, reason: String },
    #[error(".cube has no LUT data")]
    Empty,
}

fn triple(words: &[&str], line: usize) -> Result<[f32; 3], CubeError> {
    if words.len() != 3 {
        return Err(CubeError::Invalid {
            line,
            reason: "expected three numbers".into(),
        });
    }
    let mut values = [0.0f32; 3];
    for (index, word) in words.iter().enumerate() {
        values[index] = word.parse::<f32>().map_err(|_| CubeError::Invalid {
            line,
            reason: format!("invalid number {word}"),
        })?;
        if !values[index].is_finite() {
            return Err(CubeError::Invalid {
                line,
                reason: "non-finite value".into(),
            });
        }
    }
    Ok(values)
}

pub fn parse(source: &str) -> Result<CubeLut, CubeError> {
    let mut title = None;
    let mut domain_min = [0.0; 3];
    let mut domain_max = [1.0; 3];
    let mut size_1d = None;
    let mut size_3d = None;
    let mut values = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<_> = line.split_whitespace().collect();
        match words[0] {
            "TITLE" => title = Some(line[5..].trim().trim_matches('"').to_owned()),
            "DOMAIN_MIN" => domain_min = triple(&words[1..], line_no)?,
            "DOMAIN_MAX" => domain_max = triple(&words[1..], line_no)?,
            "LUT_1D_SIZE" | "LUT_3D_SIZE" => {
                let size: u32 = words
                    .get(1)
                    .ok_or_else(|| CubeError::Invalid {
                        line: line_no,
                        reason: "missing size".into(),
                    })?
                    .parse()
                    .map_err(|_| CubeError::Invalid {
                        line: line_no,
                        reason: "invalid size".into(),
                    })?;
                if !(2..=256).contains(&size) {
                    return Err(CubeError::Invalid {
                        line: line_no,
                        reason: "size must be 2..=256".into(),
                    });
                }
                if words[0] == "LUT_1D_SIZE" {
                    size_1d = Some(size);
                } else {
                    size_3d = Some(size);
                }
            }
            _ => values.push(triple(&words, line_no)?),
        }
    }
    if domain_min.iter().zip(domain_max).any(|(lo, hi)| *lo >= hi) {
        return Err(CubeError::Invalid {
            line: 0,
            reason: "invalid domain".into(),
        });
    }
    let one_len = size_1d.unwrap_or(0) as usize;
    let three_len = size_3d.map_or(0, |size| size.saturating_pow(3) as usize);
    if one_len + three_len == 0 {
        return Err(CubeError::Empty);
    }
    if values.len() != one_len + three_len {
        return Err(CubeError::Invalid {
            line: 0,
            reason: format!(
                "expected {} values, found {}",
                one_len + three_len,
                values.len()
            ),
        });
    }
    let one_d = size_1d.map(|_| values[..one_len].to_vec());
    let three_d = size_3d.map(|size| {
        let file = &values[one_len..];
        let mut entries = vec![[0.0; 3]; three_len];
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let source_index = (b * size * size + g * size + r) as usize;
                    let target_index = (r * size * size + g * size + b) as usize;
                    entries[target_index] = file[source_index];
                }
            }
        }
        Lut3d { size, entries }
    });
    Ok(CubeLut {
        title,
        domain_min,
        domain_max,
        one_d,
        three_d,
    })
}

impl CubeLut {
    #[must_use]
    pub fn sample(&self, rgb: [f32; 3], strength: f32) -> [f32; 3] {
        let normalized = std::array::from_fn(|i| {
            ((rgb[i] - self.domain_min[i]) / (self.domain_max[i] - self.domain_min[i]))
                .clamp(0.0, 1.0)
        });
        let after_1d = self.one_d.as_ref().map_or(normalized, |table| {
            std::array::from_fn(|channel| {
                let f = normalized[channel] * (table.len() - 1) as f32;
                let lo = f.floor() as usize;
                let hi = (lo + 1).min(table.len() - 1);
                table[lo][channel] + (table[hi][channel] - table[lo][channel]) * (f - lo as f32)
            })
        });
        let looked = self
            .three_d
            .as_ref()
            .map_or(after_1d, |lut| lut.sample(after_1d));
        let amount = strength.clamp(0.0, 1.0);
        std::array::from_fn(|i| rgb[i] + (looked[i] - rgb[i]) * amount)
    }

    /// Bake any 1-D pre-LUT, domain transform and 3-D table into the regular
    /// RGB LUT shape consumed by the renderer.
    #[must_use]
    pub fn bake_rgb_lut(&self, size: u32, strength: f32) -> Lut3d {
        let size = size.clamp(2, 65);
        let denominator = (size - 1) as f32;
        let mut entries = Vec::with_capacity(size.saturating_pow(3) as usize);
        for red in 0..size {
            for green in 0..size {
                for blue in 0..size {
                    entries.push(self.sample(
                        [
                            red as f32 / denominator,
                            green as f32 / denominator,
                            blue as f32 / denominator,
                        ],
                        strength,
                    ));
                }
            }
        }
        Lut3d { size, entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_red_fast_cube_and_applies_strength() {
        let cube = parse("TITLE \"Swap\"\nLUT_3D_SIZE 2\n0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n").unwrap();
        assert_eq!(cube.title.as_deref(), Some("Swap"));
        assert_eq!(cube.sample([1.0, 0.0, 0.0], 1.0), [0.0, 0.0, 1.0]);
        assert_eq!(cube.sample([1.0, 0.0, 0.0], 0.0), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn bakes_one_and_three_dimensional_tables_at_requested_strength() {
        let cube = parse(
            "LUT_1D_SIZE 2\n0 0 0\n0.5 1 1\nLUT_3D_SIZE 2\n0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n",
        )
        .unwrap();
        let baked = cube.bake_rgb_lut(2, 0.5);
        assert_eq!(baked.size, 2);
        assert_eq!(baked.entries.len(), 8);
        assert_eq!(baked.sample([1.0, 0.0, 0.0]), [0.5, 0.0, 0.25]);
    }

    #[test]
    fn rejects_invalid_domains_and_non_finite_values() {
        assert!(
            parse("DOMAIN_MIN 1 0 0\nDOMAIN_MAX 1 1 1\nLUT_1D_SIZE 2\n0 0 0\n1 1 1\n").is_err()
        );
        assert!(parse("LUT_1D_SIZE 2\n0 0 0\nNaN 1 1\n").is_err());
    }
}
