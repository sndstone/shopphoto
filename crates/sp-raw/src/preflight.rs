//! Cheap structural sanity check run before handing a file to the decoder.
//!
//! Found by the cargo-fuzz `raw_decode` target, five times over: the
//! decoder sizes allocations from TIFF header fields before reading any
//! data, so a truncated or mutated file asks for hundreds of megabytes to
//! gigabytes and the process aborts. A photo editor that dies on a
//! half-copied file is a bug regardless of whether anyone can weaponize it.
//!
//! Two invariants, neither of which a real file can violate:
//!
//! 1. **Nothing declared inside a file may be longer than the file** —
//!    strips, tiles, and any field's value block.
//! 2. **No image exceeds a gigapixel**, far above any sensor that exists.
//!
//! Resist adding a third that a real camera could plausibly trip. The value
//! of these two is that a false positive is impossible, so the check can run
//! on every open without anyone having to trust it.
//!
//! This walks the TIFF structure only — no pixel data is touched — so it
//! costs microseconds. Anything it cannot parse is passed through: this is
//! a guard against absurdity, not a validator, and the real decoder owns
//! the verdict on files it understands better than we do.

use sp_core::SpError;

const TAG_IMAGE_WIDTH: u16 = 0x0100;
const TAG_IMAGE_LENGTH: u16 = 0x0101;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_TILE_BYTE_COUNTS: u16 = 0x0145;
/// The pointer tags the decoder recurses into, verbatim from
/// `GenericTiffReader::wellknown_sub_ifd_tags`. Walking a smaller set than
/// the decoder does is a hole: the sixth OOM hid its oversized field inside
/// an ExifIFD, which a SubIFDs-only walk never reached. Keep this in sync
/// with the vendored decoder.
const SUB_IFD_TAGS: [u16; 4] = [
    0x014A, // SubIFDs
    0x8769, // ExifIFDPointer
    0x8773, // IccProfile
    0x8825, // GPSInfo
];

/// Sensors top out around 100 MP today; a gigapixel is comfortably absurd.
const MAX_PIXELS: u64 = 1_000_000_000;
/// Bound on the IFD chain so a self-referential file cannot spin.
const MAX_IFDS: usize = 64;

/// Bytes per value for a TIFF field type. Unknown types count as one byte
/// — the smallest any type can be — so a garbage type with an enormous
/// count is still caught. The third fuzz finding was exactly that: entries
/// with type 0 and counts in the billions, which a stricter table would
/// have skipped.
fn type_size(type_: u16) -> u64 {
    match type_ {
        3 | 8 => 2,           // SHORT, SSHORT
        4 | 9 | 11 | 13 => 4, // LONG, SLONG, FLOAT, IFD
        5 | 10 | 12 => 8,     // RATIONAL, SRATIONAL, DOUBLE
        _ => 1,               // BYTE, ASCII, SBYTE, UNDEFINED, unknown
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    little_endian: bool,
}

impl Reader<'_> {
    fn u16_at(&self, off: usize) -> Option<u16> {
        let b = self.bytes.get(off..off + 2)?;
        Some(if self.little_endian {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    fn u32_at(&self, off: usize) -> Option<u32> {
        let b = self.bytes.get(off..off + 4)?;
        Some(if self.little_endian {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    }

    /// Value `index` of an entry, for SHORT and LONG types only — the byte
    /// counts and dimensions we care about are never rationals.
    fn value_at(&self, type_: u16, count: u32, value_off: usize, index: u32) -> Option<u64> {
        let size = match type_ {
            3 => 2usize, // SHORT
            4 => 4,      // LONG
            _ => return None,
        };
        if index >= count {
            return None;
        }
        // Values up to four bytes live inline in the entry itself.
        let inline = (count as usize).checked_mul(size)? <= 4;
        let base = if inline {
            value_off
        } else {
            self.u32_at(value_off)? as usize
        };
        let at = base.checked_add((index as usize).checked_mul(size)?)?;
        Some(match size {
            2 => u64::from(self.u16_at(at)?),
            _ => u64::from(self.u32_at(at)?),
        })
    }
}

/// Reject files whose TIFF structure describes more data than the file
/// holds, or an impossible image size.
///
/// # Errors
/// [`SpError::CorruptData`] when the structure is self-contradictory.
/// Files that are not TIFF-based, or that cannot be walked at all, return
/// `Ok(())` — the decoder judges those.
pub fn check(path: &str, bytes: &[u8]) -> Result<(), SpError> {
    let corrupt = |reason: &str| SpError::CorruptData {
        path: path.to_owned(),
        reason: reason.to_owned(),
    };

    // Recognize TIFF exactly the way the decoder does, which is NOT what
    // the specification says. `rawler::formats::tiff::reader::parse_file`
    // reads the byte-order marker and then *ignores the magic number* —
    // its `if magic != 42` branch is commented out upstream. Two fuzz
    // findings walked straight past stricter checks into the allocation
    // this function exists to prevent: one with `II*'` instead of `II*\0`,
    // one with a magic of 0x81. So: byte-order marker only.
    //
    // The general rule, worth keeping: a guard in front of a parser must
    // accept exactly what that parser accepts. Anywhere the two disagree
    // is a hole. If the decoder is ever swapped or upstream restores its
    // magic check, revisit this line first.
    let little_endian = match bytes.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        // Not a TIFF-family container at all.
        _ => return Ok(()),
    };
    let reader = Reader {
        bytes,
        little_endian,
    };
    let file_len = bytes.len() as u64;

    let mut queue = match reader.u32_at(4) {
        Some(offset) => vec![offset as usize],
        None => return Ok(()),
    };
    let mut seen = std::collections::HashSet::new();
    let mut visited = 0usize;

    while let Some(ifd_off) = queue.pop() {
        if visited >= MAX_IFDS || !seen.insert(ifd_off) {
            continue;
        }
        visited += 1;
        let Some(count) = reader.u16_at(ifd_off) else {
            continue;
        };
        let mut width: Option<u64> = None;
        let mut height: Option<u64> = None;

        for index in 0..count as usize {
            let entry = ifd_off + 2 + index * 12;
            let (Some(tag), Some(type_), Some(values)) = (
                reader.u16_at(entry),
                reader.u16_at(entry + 2),
                reader.u32_at(entry + 4),
            ) else {
                break;
            };
            let value_off = entry + 8;

            // An entry's value block cannot be larger than the file that
            // contains it. This is the second and third OOM class the
            // fuzzer found: rawler's IFD reader allocates `count ×
            // type_size` up front, so a mutated count of 1.2 billion asks
            // for 4.9 GB before anything has looked at the data.
            let declared = u64::from(values).saturating_mul(type_size(type_));
            if declared > file_len {
                return Err(corrupt(&format!(
                    "tag {tag:#06x} declares {declared} bytes in a {file_len}-byte file"
                )));
            }

            match tag {
                TAG_IMAGE_WIDTH => width = reader.value_at(type_, values, value_off, 0),
                TAG_IMAGE_LENGTH => height = reader.value_at(type_, values, value_off, 0),
                TAG_STRIP_BYTE_COUNTS | TAG_TILE_BYTE_COUNTS => {
                    // Cap the scan: a legitimate file has thousands of
                    // strips at most, and an oversized single strip is
                    // caught by the first few entries anyway.
                    let mut total = 0u64;
                    for i in 0..values.min(4096) {
                        let Some(v) = reader.value_at(type_, values, value_off, i) else {
                            break;
                        };
                        total = total.saturating_add(v);
                        if v > file_len || total > file_len {
                            return Err(corrupt(&format!(
                                "declares {total} bytes of image data in a {file_len}-byte file"
                            )));
                        }
                    }
                }
                tag if SUB_IFD_TAGS.contains(&tag) => {
                    for i in 0..values.min(16) {
                        if let Some(offset) = reader.value_at(type_, values, value_off, i) {
                            queue.push(offset as usize);
                        }
                    }
                }
                _ => {}
            }
        }

        if let (Some(w), Some(h)) = (width, height) {
            if w.saturating_mul(h) > MAX_PIXELS {
                return Err(corrupt(&format!("declares an impossible {w}×{h} image")));
            }
        }

        let next = ifd_off + 2 + count as usize * 12;
        match reader.u32_at(next) {
            Some(0) | None => {}
            // A next-IFD pointer back to an IFD already walked is a cycle:
            // the chain would describe infinitely many images. rawler
            // follows it without a bound and accumulates entries into a map
            // until memory runs out — a 63-byte file whose only IFD points
            // at itself was the fifth OOM. Sub-IFD pointers are only
            // skipped, never rejected: two parents referencing one child is
            // strange but not self-contradictory.
            Some(offset) if seen.contains(&(offset as usize)) => {
                return Err(corrupt("IFD chain loops back on itself"));
            }
            Some(offset) => queue.push(offset as usize),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal little-endian TIFF with one IFD carrying the given entries,
    /// each `(tag, type, count, inline value)`.
    fn tiff(entries: &[(u16, u16, u32, u32)]) -> Vec<u8> {
        let mut out = b"II\x2a\x00".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, type_, count, value) in entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&type_.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out
    }

    #[test]
    fn plausible_file_passes() {
        let mut bytes = tiff(&[
            (TAG_IMAGE_WIDTH, 4, 1, 64),
            (TAG_IMAGE_LENGTH, 4, 1, 64),
            (TAG_STRIP_BYTE_COUNTS, 4, 1, 4096),
        ]);
        bytes.resize(8192, 0);
        assert!(check("f.dng", &bytes).is_ok());
    }

    #[test]
    fn strip_longer_than_the_file_is_corrupt() {
        // The fuzz finding: a truncated copy that still advertises the
        // original's 200 MB strip.
        let mut bytes = tiff(&[(TAG_STRIP_BYTE_COUNTS, 4, 1, 210_857_984)]);
        bytes.resize(200_000, 0);
        let err = check("f.3fr", &bytes).expect_err("must reject");
        assert!(matches!(err, SpError::CorruptData { .. }));
    }

    #[test]
    fn entry_value_block_larger_than_the_file_is_corrupt() {
        // The second fuzz finding: a mutated count makes the decoder's IFD
        // reader allocate `count × 4` bytes before reading anything.
        let bytes = tiff(&[(0x0115, 4, 1_229_539_329, 0)]);
        let err = check("f.3fr", &bytes).expect_err("must reject");
        assert!(matches!(err, SpError::CorruptData { .. }));
    }

    #[test]
    fn unknown_field_type_with_an_enormous_count_is_corrupt() {
        // Type 0 is not a TIFF type at all, but the decoder still sizes an
        // allocation from the count. Unknown types must not be a way past
        // the check.
        let bytes = tiff(&[(0x0001, 0, 3_439_329_286, 0)]);
        let err = check("f.3fr", &bytes).expect_err("must reject");
        assert!(matches!(err, SpError::CorruptData { .. }));
    }

    #[test]
    fn gigapixel_dimensions_are_corrupt() {
        let bytes = tiff(&[
            (TAG_IMAGE_WIDTH, 4, 1, 100_000),
            (TAG_IMAGE_LENGTH, 4, 1, 100_000),
        ]);
        let err = check("f.dng", &bytes).expect_err("must reject");
        assert!(matches!(err, SpError::CorruptData { .. }));
    }

    #[test]
    fn non_tiff_and_truncated_input_pass_through() {
        assert!(check("f.dng", b"").is_ok());
        assert!(check("f.dng", b"not a tiff at all").is_ok());
        assert!(check("f.dng", b"II\x2a\x00\x08").is_ok());
    }

    #[test]
    fn any_magic_number_is_still_checked() {
        // Fourth and fifth fuzz findings: `II*'` and `II\x81\x00`. The
        // decoder ignores the magic entirely, so a check keyed on it let
        // both through.
        for magic in [[0x2a, b'\''], [0x81, 0x00], [0x2b, 0x00]] {
            let mut bytes = tiff(&[(0x0001, 0, 3_882_297_095, 0)]);
            bytes[2] = magic[0];
            bytes[3] = magic[1];
            let err = check("f.3fr", &bytes).expect_err("must reject");
            assert!(matches!(err, SpError::CorruptData { .. }));
        }
    }

    #[test]
    fn self_referential_ifd_chain_is_corrupt() {
        // IFD at 8 whose "next IFD" points back at itself. Terminating our
        // own walk is not enough — the decoder follows it forever.
        let mut bytes = b"II\x2a\x00".to_vec();
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&8u32.to_le_bytes());
        let err = check("f.dng", &bytes).expect_err("must reject");
        assert!(matches!(err, SpError::CorruptData { .. }));
    }

    #[test]
    fn a_normal_two_ifd_chain_is_fine() {
        // IFD at 8 chaining to a second IFD, which terminates.
        let mut bytes = b"II\x2a\x00".to_vec();
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // IFD 1: no entries
        bytes.extend_from_slice(&16u32.to_le_bytes()); // -> IFD 2
        bytes.extend_from_slice(&0u16.to_le_bytes()); // IFD 2: no entries
        bytes.extend_from_slice(&0u32.to_le_bytes()); // end of chain
        assert!(check("f.dng", &bytes).is_ok());
    }
}
