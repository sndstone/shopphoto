//! Adobe DNG Camera Profile (.dcp) reader (BUILD.md Phase 3B step 2).
//!
//! A DCP file is a bare TIFF IFD (magic 0x4352 "CR" instead of 42) carrying
//! DNG color-profile tags. We parse only what the render pipeline consumes:
//! ColorMatrix1/2 + ForwardMatrix1/2 + CalibrationIlluminant1/2,
//! ProfileHueSatMapDims/Data1/2,
//! ProfileLookTableDims/Data, ProfileToneCurve, ProfileName,
//! UniqueCameraModel. Embed-policy and copyright are intentionally ignored.
//!
//! The parser is deliberately standalone (~150 lines): rawler's TIFF reader
//! rejects the DCP magic, and this way malformed profiles can never panic
//! the app — every failure maps to `SpError::CorruptData`.

use sp_core::color::{mat3_inverse, Mat3};
use sp_core::edit::{CurvePoint, ToneCurve};
use sp_core::profile::{CameraProfile, HueSatLut, Illuminant, IlluminantMatrix};
use sp_core::SpError;
use std::path::Path;

// DNG tags.
const TAG_UNIQUE_CAMERA_MODEL: u16 = 50708;
const TAG_COLOR_MATRIX_1: u16 = 50721;
const TAG_COLOR_MATRIX_2: u16 = 50722;
const TAG_CALIBRATION_ILLUMINANT_1: u16 = 50778;
const TAG_CALIBRATION_ILLUMINANT_2: u16 = 50779;
const TAG_FORWARD_MATRIX_1: u16 = 50964;
const TAG_FORWARD_MATRIX_2: u16 = 50965;
const TAG_PROFILE_NAME: u16 = 50936;
const TAG_HUESAT_DIMS: u16 = 50937;
const TAG_HUESAT_DATA_1: u16 = 50938;
const TAG_HUESAT_DATA_2: u16 = 50939;
const TAG_TONE_CURVE: u16 = 50940;
const TAG_LOOK_DIMS: u16 = 50981;
const TAG_LOOK_DATA: u16 = 50982;

struct Entry {
    type_: u16,
    count: u32,
    /// Inline value bytes or offset target already resolved to a slice range.
    data_start: usize,
}

struct Reader<'a> {
    bytes: &'a [u8],
    little_endian: bool,
}

impl<'a> Reader<'a> {
    fn u16_at(&self, off: usize) -> Option<u16> {
        let b: [u8; 2] = self.bytes.get(off..off + 2)?.try_into().ok()?;
        Some(if self.little_endian {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    fn u32_at(&self, off: usize) -> Option<u32> {
        let b: [u8; 4] = self.bytes.get(off..off + 4)?.try_into().ok()?;
        Some(if self.little_endian {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    fn i32_at(&self, off: usize) -> Option<i32> {
        self.u32_at(off).map(|v| v as i32)
    }
    fn f32_at(&self, off: usize) -> Option<f32> {
        self.u32_at(off).map(f32::from_bits)
    }

    fn type_size(type_: u16) -> usize {
        match type_ {
            1 | 2 | 6 | 7 => 1, // BYTE/ASCII/SBYTE/UNDEFINED
            3 | 8 => 2,         // SHORT/SSHORT
            4 | 9 | 11 => 4,    // LONG/SLONG/FLOAT
            5 | 10 | 12 => 8,   // RATIONAL/SRATIONAL/DOUBLE
            _ => 0,
        }
    }

    fn parse_ifd(&self, ifd_off: usize) -> Option<Vec<(u16, Entry)>> {
        let count = self.u16_at(ifd_off)? as usize;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let e = ifd_off + 2 + i * 12;
            let tag = self.u16_at(e)?;
            let type_ = self.u16_at(e + 2)?;
            let count = self.u32_at(e + 4)?;
            let size = Self::type_size(type_).checked_mul(count as usize)?;
            let data_start = if size <= 4 {
                e + 8
            } else {
                self.u32_at(e + 8)? as usize
            };
            if data_start.checked_add(size)? > self.bytes.len() {
                return None; // truncated file
            }
            entries.push((
                tag,
                Entry {
                    type_,
                    count,
                    data_start,
                },
            ));
        }
        Some(entries)
    }
}

fn illuminant_from_light_source(code: u16) -> Illuminant {
    match code {
        3 | 17 | 24 => Illuminant::StdA, // tungsten / StdA / ISO tungsten
        23 => Illuminant::D50,
        20 => Illuminant::D55,
        _ => Illuminant::D65, // 21, daylight, flash, unknown
    }
}

/// Parse a DCP from raw bytes. `fallback_name` is used when the profile
/// carries no ProfileName (typically the file stem).
pub fn parse_dcp(bytes: &[u8], fallback_name: &str) -> Result<CameraProfile, SpError> {
    let corrupt = |reason: &str| SpError::CorruptData {
        path: fallback_name.to_owned(),
        reason: reason.to_owned(),
    };

    let little_endian = match bytes.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err(corrupt("not a TIFF/DCP byte-order header")),
    };
    let r = Reader {
        bytes,
        little_endian,
    };
    let magic = r.u16_at(2).ok_or_else(|| corrupt("truncated header"))?;
    if magic != 0x4352 && magic != 42 {
        return Err(corrupt("wrong magic (not a DCP)"));
    }
    let ifd_off = r.u32_at(4).ok_or_else(|| corrupt("truncated header"))? as usize;
    let entries = r.parse_ifd(ifd_off).ok_or_else(|| corrupt("bad IFD"))?;
    let get = |tag: u16| entries.iter().find(|(t, _)| *t == tag).map(|(_, e)| e);

    let read_string = |e: &Entry| -> Option<String> {
        let raw = bytes.get(e.data_start..e.data_start + e.count as usize)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Some(String::from_utf8_lossy(&raw[..end]).into_owned())
    };
    let read_srational_mat3 = |e: &Entry| -> Option<Mat3> {
        if e.count < 9 || e.type_ != 10 {
            return None;
        }
        let mut m = [[0.0f32; 3]; 3];
        for i in 0..9 {
            let off = e.data_start + i * 8;
            let num = r.i32_at(off)? as f32;
            let den = r.i32_at(off + 4)? as f32;
            if den == 0.0 {
                return None;
            }
            m[i / 3][i % 3] = num / den;
        }
        Some(m)
    };
    let read_floats = |e: &Entry| -> Option<Vec<f32>> {
        if e.type_ != 11 {
            return None;
        }
        (0..e.count as usize)
            .map(|i| r.f32_at(e.data_start + i * 4))
            .collect()
    };
    let read_u32s = |e: &Entry| -> Option<Vec<u32>> {
        (0..e.count as usize)
            .map(|i| match e.type_ {
                3 => r.u16_at(e.data_start + i * 2).map(u32::from),
                4 => r.u32_at(e.data_start + i * 4),
                _ => None,
            })
            .collect()
    };

    // Matrices (XYZ→cam in the file; we store cam→XYZ).
    let cm1 = get(TAG_COLOR_MATRIX_1)
        .and_then(read_srational_mat3)
        .ok_or_else(|| corrupt("missing/invalid ColorMatrix1"))?;
    let ill1 = get(TAG_CALIBRATION_ILLUMINANT_1)
        .and_then(read_u32s)
        .and_then(|v| v.first().copied())
        .map_or(Illuminant::D65, |c| illuminant_from_light_source(c as u16));
    let cam1 = mat3_inverse(&cm1).ok_or_else(|| corrupt("singular ColorMatrix1"))?;

    let mut matrices = vec![IlluminantMatrix {
        illuminant: ill1,
        cam_to_xyz: cam1,
    }];
    let mut forward_matrices = Vec::new();
    if let Some(fm1) = get(TAG_FORWARD_MATRIX_1).and_then(read_srational_mat3) {
        forward_matrices.push(IlluminantMatrix {
            illuminant: ill1,
            cam_to_xyz: fm1,
        });
    }
    let mut huesat_pair: Vec<(Illuminant, Option<HueSatLut>)> = Vec::new();

    let huesat_dims = get(TAG_HUESAT_DIMS).and_then(read_u32s);
    let make_lut = |dims: &Option<Vec<u32>>, data: Option<Vec<f32>>| -> Option<HueSatLut> {
        let dims = dims.as_ref()?;
        let data = data?;
        if dims.len() < 3 {
            return None;
        }
        let lut = HueSatLut {
            hue_divisions: dims[0],
            sat_divisions: dims[1],
            val_divisions: dims[2],
            entries: data.chunks_exact(3).map(|c| (c[0], c[1], c[2])).collect(),
        };
        (data.len() % 3 == 0 && lut.is_valid()).then_some(lut)
    };
    let hs1 = make_lut(&huesat_dims, get(TAG_HUESAT_DATA_1).and_then(read_floats));
    huesat_pair.push((ill1, hs1));

    if let Some(cm2) = get(TAG_COLOR_MATRIX_2).and_then(read_srational_mat3) {
        let ill2 = get(TAG_CALIBRATION_ILLUMINANT_2)
            .and_then(read_u32s)
            .and_then(|v| v.first().copied())
            .map_or(Illuminant::D65, |c| illuminant_from_light_source(c as u16));
        let cam2 = mat3_inverse(&cm2).ok_or_else(|| corrupt("singular ColorMatrix2"))?;
        matrices.push(IlluminantMatrix {
            illuminant: ill2,
            cam_to_xyz: cam2,
        });
        if let Some(fm2) = get(TAG_FORWARD_MATRIX_2).and_then(read_srational_mat3) {
            forward_matrices.push(IlluminantMatrix {
                illuminant: ill2,
                cam_to_xyz: fm2,
            });
        }
        let hs2 = make_lut(&huesat_dims, get(TAG_HUESAT_DATA_2).and_then(read_floats));
        huesat_pair.push((ill2, hs2));
    }

    // Sort by CCT ascending; keep the hue/sat maps attached to their
    // illuminants (huesat1 belongs to the lowest CCT after sorting).
    matrices.sort_by(|a, b| {
        a.illuminant
            .cct_kelvin()
            .total_cmp(&b.illuminant.cct_kelvin())
    });
    forward_matrices.sort_by(|a, b| {
        a.illuminant
            .cct_kelvin()
            .total_cmp(&b.illuminant.cct_kelvin())
    });
    // A lone forward matrix in a dual-illuminant profile cannot be
    // interpolated consistently; fall back to the complete color matrices.
    if forward_matrices.len() != matrices.len() {
        forward_matrices.clear();
    }
    huesat_pair.sort_by(|a, b| a.0.cct_kelvin().total_cmp(&b.0.cct_kelvin()));
    let mut hs_iter = huesat_pair.into_iter().map(|(_, h)| h);
    let huesat1 = hs_iter.next().flatten();
    let huesat2 = hs_iter.next().flatten();

    let look_dims = get(TAG_LOOK_DIMS).and_then(read_u32s);
    let look = make_lut(&look_dims, get(TAG_LOOK_DATA).and_then(read_floats));

    let base_curve = get(TAG_TONE_CURVE).and_then(read_floats).and_then(|pts| {
        if pts.len() < 4 || pts.len() % 2 != 0 {
            return None;
        }
        let points: Vec<CurvePoint> = pts
            .chunks_exact(2)
            .map(|c| CurvePoint { x: c[0], y: c[1] })
            .collect();
        Some(ToneCurve { points })
    });

    let name = get(TAG_PROFILE_NAME)
        .and_then(read_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_name.to_owned());
    let camera_model = get(TAG_UNIQUE_CAMERA_MODEL)
        .and_then(read_string)
        .unwrap_or_default();

    let profile = CameraProfile {
        name,
        camera_model,
        matrices,
        forward_matrices,
        huesat1,
        huesat2,
        look,
        rgb_lut: None,
        // DCP has no chroma-warp equivalent; Adobe's rendering is carried
        // entirely by the hue/sat maps and look table above.
        chroma_warp1: None,
        chroma_warp2: None,
        base_curve,
    };
    if !profile.is_valid() {
        return Err(corrupt(
            "invalid camera profile matrices, values or LUT shape",
        ));
    }
    Ok(profile)
}

/// Load and parse a `.dcp` file.
pub fn load_dcp(path: &Path) -> Result<CameraProfile, SpError> {
    let bytes = std::fs::read(path).map_err(|e| SpError::io(path.display().to_string(), e))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("profile");
    parse_dcp(&bytes, stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny DCP writer for tests (little-endian, everything via offsets).
    struct DcpBuilder {
        entries: Vec<(u16, u16, u32, Vec<u8>)>, // tag, type, count, payload
    }
    impl DcpBuilder {
        fn new() -> Self {
            Self { entries: vec![] }
        }
        fn srationals(&mut self, tag: u16, vals: &[(i32, i32)]) -> &mut Self {
            let mut p = vec![];
            for (n, d) in vals {
                p.extend_from_slice(&n.to_le_bytes());
                p.extend_from_slice(&d.to_le_bytes());
            }
            self.entries.push((tag, 10, vals.len() as u32, p));
            self
        }
        fn shorts(&mut self, tag: u16, vals: &[u16]) -> &mut Self {
            let mut p = vec![];
            for v in vals {
                p.extend_from_slice(&v.to_le_bytes());
            }
            self.entries.push((tag, 3, vals.len() as u32, p));
            self
        }
        fn longs(&mut self, tag: u16, vals: &[u32]) -> &mut Self {
            let mut p = vec![];
            for v in vals {
                p.extend_from_slice(&v.to_le_bytes());
            }
            self.entries.push((tag, 4, vals.len() as u32, p));
            self
        }
        fn floats(&mut self, tag: u16, vals: &[f32]) -> &mut Self {
            let mut p = vec![];
            for v in vals {
                p.extend_from_slice(&v.to_le_bytes());
            }
            self.entries.push((tag, 11, vals.len() as u32, p));
            self
        }
        fn ascii(&mut self, tag: u16, s: &str) -> &mut Self {
            let mut p = s.as_bytes().to_vec();
            p.push(0);
            self.entries.push((tag, 2, p.len() as u32, p));
            self
        }
        fn build(&mut self) -> Vec<u8> {
            self.entries.sort_by_key(|e| e.0);
            let n = self.entries.len();
            let mut out = vec![b'I', b'I'];
            out.extend_from_slice(&0x4352u16.to_le_bytes());
            out.extend_from_slice(&8u32.to_le_bytes()); // IFD at 8
            out.extend_from_slice(&(n as u16).to_le_bytes());
            let mut heap_off = 8 + 2 + n * 12 + 4;
            let mut heap = vec![];
            for (tag, type_, count, payload) in &self.entries {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&type_.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
                if payload.len() <= 4 {
                    let mut v = payload.clone();
                    v.resize(4, 0);
                    out.extend_from_slice(&v);
                } else {
                    out.extend_from_slice(&(heap_off as u32).to_le_bytes());
                    heap.extend_from_slice(payload);
                    heap_off += payload.len();
                }
            }
            out.extend_from_slice(&0u32.to_le_bytes()); // next IFD
            out.extend_from_slice(&heap);
            out
        }
    }

    /// Identity-ish XYZ→cam matrix as rationals.
    fn ident_rat(scale: i32) -> Vec<(i32, i32)> {
        vec![
            (scale, 1000),
            (0, 1000),
            (0, 1000),
            (0, 1000),
            (scale, 1000),
            (0, 1000),
            (0, 1000),
            (0, 1000),
            (scale, 1000),
        ]
    }

    #[test]
    fn oversized_and_zero_huesat_dimensions_do_not_panic() {
        for dims in [
            [u32::MAX, u32::MAX, u32::MAX],
            [2, 2, 0],
            [0, 2, 1],
            [2, 1, 1],
        ] {
            let bytes = DcpBuilder::new()
                .srationals(TAG_COLOR_MATRIX_1, &ident_rat(1000))
                .longs(TAG_HUESAT_DIMS, &dims)
                .floats(TAG_HUESAT_DATA_1, &[0.0, 1.0, 1.0])
                .build();
            let profile = parse_dcp(&bytes, "bad dimensions").unwrap();
            assert!(profile.huesat1.is_none());
        }
    }

    #[test]
    fn parses_full_dcp_round_trip() {
        let hs1: Vec<f32> = (0..4).flat_map(|_| [5.0, 1.2, 1.0]).collect();
        let hs2: Vec<f32> = (0..4).flat_map(|_| [-5.0, 0.8, 1.0]).collect();
        let bytes = DcpBuilder::new()
            .ascii(TAG_PROFILE_NAME, "Test Profile")
            .ascii(TAG_UNIQUE_CAMERA_MODEL, "Hasselblad X2D II 100C")
            .srationals(TAG_COLOR_MATRIX_1, &ident_rat(2000))
            .srationals(TAG_COLOR_MATRIX_2, &ident_rat(1000))
            .srationals(TAG_FORWARD_MATRIX_1, &ident_rat(750))
            .srationals(TAG_FORWARD_MATRIX_2, &ident_rat(1250))
            .shorts(TAG_CALIBRATION_ILLUMINANT_1, &[17])
            .shorts(TAG_CALIBRATION_ILLUMINANT_2, &[21])
            .longs(TAG_HUESAT_DIMS, &[2, 2, 1])
            .floats(TAG_HUESAT_DATA_1, &hs1)
            .floats(TAG_HUESAT_DATA_2, &hs2)
            .floats(TAG_TONE_CURVE, &[0.0, 0.0, 0.5, 0.6, 1.0, 1.0])
            .build();

        let p = parse_dcp(&bytes, "fallback").expect("parse");
        assert_eq!(p.name, "Test Profile");
        assert_eq!(p.camera_model, "Hasselblad X2D II 100C");
        // Sorted by CCT: StdA first. XYZ→cam scale 2 ⇒ cam→XYZ scale 0.5.
        assert_eq!(p.matrices[0].illuminant, Illuminant::StdA);
        assert!((p.matrices[0].cam_to_xyz[0][0] - 0.5).abs() < 1e-4);
        assert_eq!(p.matrices[1].illuminant, Illuminant::D65);
        assert!((p.matrices[1].cam_to_xyz[0][0] - 1.0).abs() < 1e-4);
        // HueSat maps follow their illuminants.
        assert!((p.huesat1.as_ref().unwrap().entries[0].0 - 5.0).abs() < 1e-4);
        assert!((p.huesat2.as_ref().unwrap().entries[0].0 + 5.0).abs() < 1e-4);
        // Base curve.
        let curve = p.base_curve.as_ref().expect("curve");
        assert_eq!(curve.points.len(), 3);
        assert!((curve.points[1].y - 0.6).abs() < 1e-4);
        // Forward matrices are preferred when present.
        assert!((p.forward_matrices[0].cam_to_xyz[0][0] - 0.75).abs() < 1e-4);
        assert!((p.forward_matrices[1].cam_to_xyz[0][0] - 1.25).abs() < 1e-4);
        // Resolve midway: matrix between the two forward matrices.
        let m = p.resolve_matrix(4500.0);
        assert!(m[0][0] > 0.75 && m[0][0] < 1.25);
        let hs = p.resolve_huesat(4500.0).expect("huesat");
        assert!(hs.entries[0].0.abs() < 5.0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_dcp(b"not a dcp at all", "x").is_err());
        assert!(parse_dcp(b"II\x52\x43\xff\xff\xff\xff", "x").is_err());
    }
}
