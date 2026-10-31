//! Ultra HDR JPEG container assembly.
//!
//! The file is two concatenated JPEGs: an ordinary SDR primary, then the
//! gain map. Two independent mechanisms tell a decoder where the second one
//! starts, and real decoders use both, so we write both:
//!
//! * **MPF** (CIPA Multi-Picture Format) — an APP2 segment in the primary
//!   holding a small TIFF IFD with each image's size and offset. This is
//!   what Android's parser follows.
//! * **GContainer XMP** — an APP1 packet in the primary declaring a
//!   directory of items (`Primary`, then `GainMap` with its byte length).
//!
//! The gain map itself carries an `hdrgm:` XMP packet with the transfer
//! parameters ([`crate::gainmap::GainMapParams`]).
//!
//! Everything before the gain map is a valid baseline JPEG, so any decoder
//! that has never heard of Ultra HDR shows the SDR rendition and ignores
//! the trailing bytes.

use crate::encode::{icc_profile, write_jpeg_segment};
use crate::gainmap::GainMap;
use crate::{ExportColorSpace, ExportError};
use std::io::Write;

/// XMP APP1 namespace prefix, per the XMP specification.
const XMP_NAMESPACE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Size of the MPF APP2 payload we emit: `MPF\0` + MP header + a 3-entry
/// index IFD + two 16-byte MP entries. Fixed, which is what lets us build
/// the primary first and patch the offsets afterwards.
const MPF_PAYLOAD_LEN: usize = 4 + 8 + 42 + 32;

/// GContainer XMP for the primary image, declaring the trailing gain map.
fn container_xmp(gain_map_len: usize) -> String {
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="ShopPhoto {version}">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:Container="http://ns.google.com/photos/1.0/container/"
    xmlns:Item="http://ns.google.com/photos/1.0/container/item/"
    xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/"
    hdrgm:Version="1.0">
   <Container:Directory>
    <rdf:Seq>
     <rdf:li rdf:parseType="Resource">
      <Container:Item Item:Semantic="Primary" Item:Mime="image/jpeg"/>
     </rdf:li>
     <rdf:li rdf:parseType="Resource">
      <Container:Item Item:Semantic="GainMap" Item:Mime="image/jpeg" Item:Length="{gain_map_len}"/>
     </rdf:li>
    </rdf:Seq>
   </Container:Directory>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// Build the MPF APP2 payload with zeroed sizes/offsets, to be patched by
/// [`patch_mpf`] once the primary's final length is known.
fn mpf_payload() -> Vec<u8> {
    let mut out = Vec::with_capacity(MPF_PAYLOAD_LEN);
    out.extend_from_slice(b"MPF\0");
    // MP header: little-endian TIFF, first IFD at offset 8.
    out.extend_from_slice(b"II\x2a\0");
    out.extend_from_slice(&8u32.to_le_bytes());
    // MP Index IFD: three entries.
    out.extend_from_slice(&3u16.to_le_bytes());
    // 0xB000 MPFVersion, UNDEFINED, count 4, inline "0100".
    out.extend_from_slice(&0xB000u16.to_le_bytes());
    out.extend_from_slice(&7u16.to_le_bytes());
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(b"0100");
    // 0xB001 NumberOfImages, LONG, count 1, value 2.
    out.extend_from_slice(&0xB001u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    // 0xB002 MPEntry, UNDEFINED, count 32, offset past the IFD.
    // 8 (header) + 42 (IFD) = 50 bytes from the MP header start.
    out.extend_from_slice(&0xB002u16.to_le_bytes());
    out.extend_from_slice(&7u16.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&50u32.to_le_bytes());
    // No next IFD.
    out.extend_from_slice(&0u32.to_le_bytes());
    // Two MP entries: attribute, size, offset, two dependency fields.
    // Primary: MP Type 0x030000 (Baseline MP Primary Image), representative.
    out.extend_from_slice(&0x0003_0000u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // size, patched
    out.extend_from_slice(&0u32.to_le_bytes()); // offset: always 0 for #1
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    // Gain map: undefined MP type, it is identified by the XMP.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // size, patched
    out.extend_from_slice(&0u32.to_le_bytes()); // offset, patched
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    debug_assert_eq!(out.len(), MPF_PAYLOAD_LEN);
    out
}

/// Fill in the MP entry sizes and the gain map's offset.
///
/// Per the MPF specification, offsets are counted from the first byte of
/// the MP header (the endian marker), not from the start of the file — so
/// the primary's own offset is 0 and the gain map's is
/// `primary_len - mp_header_start`.
fn patch_mpf(primary: &mut [u8], mp_header_start: usize, gain_map_len: usize) {
    let entries = mp_header_start + 50;
    let primary_len = primary.len();
    primary[entries + 4..entries + 8].copy_from_slice(&(primary_len as u32).to_le_bytes());
    primary[entries + 20..entries + 24].copy_from_slice(&(gain_map_len as u32).to_le_bytes());
    let offset = (primary_len - mp_header_start) as u32;
    primary[entries + 24..entries + 28].copy_from_slice(&offset.to_le_bytes());
}

fn xmp_segment(writer: &mut impl Write, packet: &str) -> std::io::Result<()> {
    let mut payload = XMP_NAMESPACE.to_vec();
    payload.extend_from_slice(packet.as_bytes());
    write_jpeg_segment(writer, 0xe1, &payload)
}

/// Encode the gain map as a standalone grayscale JPEG carrying its
/// `hdrgm:` metadata.
fn encode_gain_map(map: &GainMap, quality: u8) -> Result<Vec<u8>, ExportError> {
    let mut encoded = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut encoded, quality);
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    encoder
        .encode(
            &map.data,
            u16::try_from(map.width).map_err(|_| ExportError::Encode {
                path: Default::default(),
                reason: "gain map wider than 65535 px".into(),
            })?,
            u16::try_from(map.height).map_err(|_| ExportError::Encode {
                path: Default::default(),
                reason: "gain map taller than 65535 px".into(),
            })?,
            jpeg_encoder::ColorType::Luma,
        )
        .map_err(|error| ExportError::Encode {
            path: Default::default(),
            reason: error.to_string(),
        })?;

    let mut out = Vec::with_capacity(encoded.len() + 512);
    out.extend_from_slice(&encoded[..2]); // SOI
    xmp_segment(&mut out, &map.params.xmp_packet()).map_err(|error| ExportError::Encode {
        path: Default::default(),
        reason: error.to_string(),
    })?;
    out.extend_from_slice(&encoded[2..]);
    Ok(out)
}

/// Assemble a complete Ultra HDR file from an already-encoded baseline SDR
/// JPEG body and a gain map.
///
/// `sdr_jpeg` must be the raw encoder output (starting at SOI, no metadata
/// segments); this function owns segment order: EXIF, XMP, ICC, MPF.
#[cfg(test)]
pub fn assemble(
    sdr_jpeg: &[u8],
    map: &GainMap,
    color_space: ExportColorSpace,
    exif: Option<&[u8]>,
    map_quality: u8,
) -> Result<Vec<u8>, ExportError> {
    assemble_with_description(sdr_jpeg, map, color_space, exif, None, map_quality)
}

/// Attach photographic metadata inside the primary container XMP packet so
/// the gain-map directory and MPF offsets remain valid.
pub fn assemble_with_description(
    sdr_jpeg: &[u8],
    map: &GainMap,
    color_space: ExportColorSpace,
    exif: Option<&[u8]>,
    description: Option<&str>,
    map_quality: u8,
) -> Result<Vec<u8>, ExportError> {
    if !sdr_jpeg.starts_with(&[0xff, 0xd8]) {
        return Err(ExportError::Encode {
            path: Default::default(),
            reason: "JPEG encoder omitted SOI marker".into(),
        });
    }
    let gain_map = encode_gain_map(map, map_quality)?;
    let icc = icc_profile(color_space)?;

    let io = |error: std::io::Error| ExportError::Encode {
        path: Default::default(),
        reason: error.to_string(),
    };

    let mut primary: Vec<u8> = Vec::with_capacity(sdr_jpeg.len() + gain_map.len() + 4096);
    primary.extend_from_slice(&sdr_jpeg[..2]);
    if let Some(exif) = exif {
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(exif);
        write_jpeg_segment(&mut primary, 0xe1, &payload).map_err(io)?;
    }
    let mut container = container_xmp(gain_map.len());
    if let Some(description) = description {
        container = container.replace("</rdf:RDF>", &format!("{description}</rdf:RDF>"));
    }
    xmp_segment(&mut primary, &container).map_err(io)?;

    const ICC_HEADER: usize = 14;
    const ICC_CHUNK: usize = u16::MAX as usize - 2 - ICC_HEADER;
    let count = icc.len().div_ceil(ICC_CHUNK).min(255);
    for (index, chunk) in icc.chunks(ICC_CHUNK).enumerate() {
        let mut payload = b"ICC_PROFILE\0".to_vec();
        payload.push((index + 1) as u8);
        payload.push(count as u8);
        payload.extend_from_slice(chunk);
        write_jpeg_segment(&mut primary, 0xe2, &payload).map_err(io)?;
    }

    // MPF must be written before the entropy-coded data, and its offsets
    // are patched once the primary's length is final.
    let payload = mpf_payload();
    write_jpeg_segment(&mut primary, 0xe2, &payload).map_err(io)?;
    // Marker (2) + length (2) + "MPF\0" (4) precede the MP header.
    let mp_header_start = primary.len() - MPF_PAYLOAD_LEN + 4;
    primary.extend_from_slice(&sdr_jpeg[2..]);
    patch_mpf(&mut primary, mp_header_start, gain_map.len());

    primary.extend_from_slice(&gain_map);
    Ok(primary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gainmap::GainMapParams;

    fn tiny_map() -> GainMap {
        GainMap {
            width: 4,
            height: 4,
            data: vec![128; 16],
            params: GainMapParams {
                min_log2: 0.0,
                max_log2: 2.0,
                gamma: 1.0,
                offset_sdr: 1.0 / 64.0,
                offset_hdr: 1.0 / 64.0,
                hdr_capacity_min: 0.0,
                hdr_capacity_max: 2.0,
            },
        }
    }

    fn tiny_sdr() -> Vec<u8> {
        let mut encoded = Vec::new();
        jpeg_encoder::Encoder::new(&mut encoded, 90)
            .encode(&[128u8; 8 * 8 * 3], 8, 8, jpeg_encoder::ColorType::Rgb)
            .expect("encode");
        encoded
    }

    /// Walk the JPEG segment chain, returning (marker, payload) pairs until
    /// the start of scan.
    fn segments(jpeg: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 2;
        while i + 4 <= jpeg.len() && jpeg[i] == 0xff {
            let marker = jpeg[i + 1];
            if marker == 0xda {
                break;
            }
            let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
            out.push((marker, jpeg[i + 4..i + 2 + len].to_vec()));
            i += 2 + len;
        }
        out
    }

    #[test]
    fn file_starts_as_a_plain_jpeg_and_ends_with_the_gain_map() {
        let file =
            assemble(&tiny_sdr(), &tiny_map(), ExportColorSpace::Srgb, None, 90).expect("assemble");
        assert_eq!(&file[..2], &[0xff, 0xd8], "must open with SOI");
        // Two EOI markers: the primary's and the gain map's.
        let eoi_count = file.windows(2).filter(|w| w == &[0xff, 0xd9]).count();
        assert!(eoi_count >= 2, "gain map JPEG is missing");
    }

    #[test]
    fn primary_carries_container_xmp_and_icc() {
        let file =
            assemble(&tiny_sdr(), &tiny_map(), ExportColorSpace::Srgb, None, 90).expect("assemble");
        let segs = segments(&file);
        let xmp = segs
            .iter()
            .find(|(marker, payload)| *marker == 0xe1 && payload.starts_with(XMP_NAMESPACE))
            .expect("XMP APP1");
        let text = String::from_utf8_lossy(&xmp.1);
        assert!(text.contains(r#"Item:Semantic="GainMap""#), "{text}");
        assert!(text.contains(r#"hdrgm:Version="1.0""#));
        assert!(segs
            .iter()
            .any(|(marker, payload)| *marker == 0xe2 && payload.starts_with(b"ICC_PROFILE\0")));
    }

    #[test]
    fn mpf_offsets_point_at_the_real_images() {
        let file =
            assemble(&tiny_sdr(), &tiny_map(), ExportColorSpace::Srgb, None, 90).expect("assemble");
        let segs = segments(&file);
        let mpf = segs
            .iter()
            .find(|(marker, payload)| *marker == 0xe2 && payload.starts_with(b"MPF\0"))
            .expect("MPF APP2");
        // Locate the MP header inside the file to convert relative offsets.
        let mp_header_start = file
            .windows(8)
            .position(|w| w == [b'M', b'P', b'F', 0, b'I', b'I', 0x2a, 0])
            .expect("MP header")
            + 4;
        let entries = &mpf.1[4 + 50..];
        let primary_size =
            u32::from_le_bytes([entries[4], entries[5], entries[6], entries[7]]) as usize;
        let map_size =
            u32::from_le_bytes([entries[20], entries[21], entries[22], entries[23]]) as usize;
        let map_offset =
            u32::from_le_bytes([entries[24], entries[25], entries[26], entries[27]]) as usize;

        assert_eq!(
            primary_size + map_size,
            file.len(),
            "sizes must account for the whole file"
        );
        let map_start = mp_header_start + map_offset;
        assert_eq!(map_start, primary_size, "gain map must follow the primary");
        assert_eq!(
            &file[map_start..map_start + 2],
            &[0xff, 0xd8],
            "gain map offset must land on an SOI"
        );
        assert_eq!(map_start + map_size, file.len());
    }

    #[test]
    fn gain_map_image_carries_hdrgm_metadata() {
        let file =
            assemble(&tiny_sdr(), &tiny_map(), ExportColorSpace::Srgb, None, 90).expect("assemble");
        let mp_header_start = file
            .windows(8)
            .position(|w| w == [b'M', b'P', b'F', 0, b'I', b'I', 0x2a, 0])
            .expect("MP header")
            + 4;
        let segs = segments(&file);
        let mpf = segs
            .iter()
            .find(|(marker, payload)| *marker == 0xe2 && payload.starts_with(b"MPF\0"))
            .expect("MPF");
        let entries = &mpf.1[4 + 50..];
        let map_offset =
            u32::from_le_bytes([entries[24], entries[25], entries[26], entries[27]]) as usize;
        let map = &file[mp_header_start + map_offset..];
        let map_segments = segments(map);
        let text = String::from_utf8_lossy(&map_segments[0].1);
        assert!(text.contains(r#"hdrgm:GainMapMax="2""#), "{text}");
        assert!(text.contains(r#"hdrgm:BaseRenditionIsHDR="False""#));
    }
}

#[cfg(test)]
mod dump {
    use super::*;
    use crate::gainmap::GainMapParams;

    /// `SP_ULTRAHDR_DUMP=/path/out.jpg cargo test -p sp-export dump_sample`
    /// writes a real file for inspection in an OS viewer.
    #[test]
    fn dump_sample() {
        let Some(path) = std::env::var_os("SP_ULTRAHDR_DUMP") else {
            return;
        };
        let mut encoded = Vec::new();
        let pixels: Vec<u8> = (0..64 * 64)
            .flat_map(|i| {
                let v = if (i % 64) < 32 { 60u8 } else { 250 };
                [v, v, v]
            })
            .collect();
        jpeg_encoder::Encoder::new(&mut encoded, 92)
            .encode(&pixels, 64, 64, jpeg_encoder::ColorType::Rgb)
            .expect("encode");
        let map = GainMap {
            width: 32,
            height: 32,
            data: (0..32 * 32)
                .map(|i| if (i % 32) < 16 { 0u8 } else { 220 })
                .collect(),
            params: GainMapParams {
                min_log2: 0.0,
                max_log2: 2.0,
                gamma: 1.0,
                offset_sdr: 1.0 / 64.0,
                offset_hdr: 1.0 / 64.0,
                hdr_capacity_min: 0.0,
                hdr_capacity_max: 2.0,
            },
        };
        let file = assemble(&encoded, &map, ExportColorSpace::Srgb, None, 95).expect("assemble");
        std::fs::write(&path, file).expect("write");
    }
}
