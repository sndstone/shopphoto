//! Descriptive output metadata, independent of the catalog database.

use crate::ExportSettings;
use sp_core::ImageMeta;

/// User-authored metadata copied to an exported photograph.
/// This is runtime data: export presets must never retain another photo's tags.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PhotoMetadata {
    /// Standard XMP star rating (1–5), or -1 for rejected photographs.
    #[serde(default)]
    pub rating: Option<i8>,
    /// Interoperable XMP color label name, omitted when no label is set.
    #[serde(default)]
    pub label: Option<String>,
    pub caption: String,
    pub keywords: Vec<String>,
    pub creator: String,
    pub copyright: String,
    pub rights: String,
    /// Creator's contact email.
    pub contact: String,
    /// `[latitude, longitude]` in degrees, overriding capture GPS when supplied.
    pub gps: Option<[f64; 2]>,
}

pub(crate) fn xml(value: &str) -> String {
    value
        .chars()
        .filter(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'))
        .fold(String::new(), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                _ => out.push(c),
            }
            out
        })
}

pub(crate) fn gps(settings: &ExportSettings, capture: Option<&ImageMeta>) -> Option<[f64; 2]> {
    if settings.strip_gps || !settings.copy_metadata {
        return None;
    }
    settings
        .photo_metadata
        .gps
        .or_else(|| capture.and_then(|value| value.gps))
        .filter(|[lat, lon]| {
            lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0
        })
}

/// An RDF description, also usable inside Ultra HDR's existing XMP packet.
pub(crate) fn description(
    settings: &ExportSettings,
    capture: Option<&ImageMeta>,
) -> Option<String> {
    let mut fields = String::new();
    if settings.copy_descriptive_metadata {
        let meta = &settings.photo_metadata;
        if let Some(rating) = meta.rating.filter(|value| (-1..=5).contains(value)) {
            fields.push_str(&format!("<xmp:Rating>{rating}</xmp:Rating>"));
        }
        if let Some(label) = meta.label.as_deref().filter(|value| !value.is_empty()) {
            fields.push_str(&format!("<xmp:Label>{}</xmp:Label>", xml(label)));
        }
        for (tag, value) in [
            ("dc:description", &meta.caption),
            ("dc:rights", &meta.copyright),
            ("xmpRights:UsageTerms", &meta.rights),
        ] {
            if !value.is_empty() {
                fields.push_str(&format!(
                    "<{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{tag}>",
                    xml(value)
                ));
            }
        }
        if !meta.creator.is_empty() {
            fields.push_str(&format!(
                "<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>",
                xml(&meta.creator)
            ));
        }
        if !meta.keywords.is_empty() {
            fields.push_str("<dc:subject><rdf:Bag>");
            for keyword in &meta.keywords {
                if !keyword.trim().is_empty() {
                    fields.push_str(&format!("<rdf:li>{}</rdf:li>", xml(keyword)));
                }
            }
            fields.push_str("</rdf:Bag></dc:subject>");
            let hierarchical: Vec<_> = meta.keywords.iter().filter(|k| k.contains('|')).collect();
            if !hierarchical.is_empty() {
                fields.push_str("<lr:hierarchicalSubject><rdf:Bag>");
                for keyword in hierarchical {
                    fields.push_str(&format!("<rdf:li>{}</rdf:li>", xml(keyword)));
                }
                fields.push_str("</rdf:Bag></lr:hierarchicalSubject>");
            }
        }
        if !meta.contact.is_empty() {
            fields.push_str(&format!("<Iptc4xmpCore:CreatorContactInfo rdf:parseType=\"Resource\"><Iptc4xmpCore:CiEmailWork>{}</Iptc4xmpCore:CiEmailWork></Iptc4xmpCore:CreatorContactInfo>", xml(&meta.contact)));
        }
    }
    if let Some([lat, lon]) = gps(settings, capture) {
        for (tag, value, negative, positive) in [
            ("exif:GPSLatitude", lat, 'S', 'N'),
            ("exif:GPSLongitude", lon, 'W', 'E'),
        ] {
            let degrees = value.abs().floor();
            let minutes = (value.abs() - degrees) * 60.0;
            let direction = if value < 0.0 { negative } else { positive };
            fields.push_str(&format!(
                "<{tag}>{degrees:.0},{minutes:.8}{direction}</{tag}>"
            ));
        }
    }
    if fields.is_empty() {
        return None;
    }
    Some(format!(
        r#"<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:xmpRights="http://ns.adobe.com/xap/1.0/rights/" xmlns:Iptc4xmpCore="http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/" xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:lr="http://ns.adobe.com/lightroom/1.0/">{fields}</rdf:Description>"#
    ))
}

pub(crate) fn packet(settings: &ExportSettings, capture: Option<&ImageMeta>) -> Option<String> {
    description(settings, capture).map(|body| format!(r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">{body}</rdf:RDF></x:xmpmeta>"#))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratings_labels_and_rejects_obey_descriptive_metadata_policy() {
        let mut settings = ExportSettings::default();
        assert!(packet(&settings, None).is_none());
        for rating in [-1, 0, 1, 5] {
            settings.photo_metadata.rating = Some(rating);
            settings.photo_metadata.label = Some("Blue & green".into());
            let packet = packet(&settings, None).unwrap();
            assert!(packet.contains(&format!("<xmp:Rating>{rating}</xmp:Rating>")));
            assert!(packet.contains("<xmp:Label>Blue &amp; green</xmp:Label>"));
            assert!(packet.contains("xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\""));
        }
        settings.copy_descriptive_metadata = false;
        assert!(packet(&settings, None).is_none());
        settings.copy_descriptive_metadata = true;
        settings.photo_metadata.rating = Some(99);
        settings.photo_metadata.label = None;
        assert!(packet(&settings, None).is_none());
        let legacy: PhotoMetadata = serde_json::from_str(r#"{"caption":"","keywords":[],"creator":"","copyright":"","rights":"","contact":"","gps":null}"#).unwrap();
        assert_eq!(legacy.rating, None);
        assert_eq!(legacy.label, None);
    }

    #[test]
    fn presets_never_capture_photo_identity() {
        let settings = ExportSettings {
            photo_metadata: PhotoMetadata {
                caption: "Private caption".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("Private caption"));
        let restored: ExportSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.photo_metadata, PhotoMetadata::default());
    }

    #[test]
    fn descriptive_and_location_policies_are_independent() {
        let mut settings = ExportSettings {
            photo_metadata: PhotoMetadata {
                caption: "A < B & C".into(),
                gps: Some([52.25, -4.5]),
                ..Default::default()
            },
            ..Default::default()
        };
        let value = packet(&settings, None).unwrap();
        assert!(value.contains("A &lt; B &amp; C"));
        assert!(value.contains("52,15.00000000N"));
        settings.strip_gps = true;
        assert!(!packet(&settings, None).unwrap().contains("GPSLatitude"));
        settings.copy_descriptive_metadata = false;
        assert!(packet(&settings, None).is_none());
        settings.strip_gps = false;
        settings.copy_metadata = false;
        assert!(packet(&settings, None).is_none());
    }

    #[test]
    fn export_containers_retain_descriptions_and_obey_privacy() {
        use crate::{encode_file_with_metadata, ExportFormat, ExportImage, TiffDepth};
        let dir = tempfile::tempdir().unwrap();
        let image = ExportImage::new(8, 8, vec![160; 8 * 8 * 4]).unwrap();
        let capture = ImageMeta {
            camera_make: "Test Camera".into(),
            gps: Some([52.25, -4.5]),
            ..Default::default()
        };
        for format in [
            ExportFormat::Jpeg {
                quality: 95,
                chroma_444: true,
            },
            ExportFormat::Png { sixteen_bit: false },
            ExportFormat::Tiff {
                bit_depth: TiffDepth::Sixteen,
                compressed: true,
            },
            ExportFormat::WebP { lossless: true },
            ExportFormat::Avif { quality: 80 },
            ExportFormat::JpegXl {
                quality: 95,
                lossless: true,
            },
        ] {
            let mut settings = ExportSettings {
                format,
                photo_metadata: PhotoMetadata {
                    caption: "Canal & café".into(),
                    keywords: vec!["Amsterdam".into()],
                    creator: "Photographer".into(),
                    copyright: "Copyright 2026".into(),
                    rights: "Licensed print".into(),
                    contact: "photo@example.test".into(),
                    gps: None,
                    rating: Some(4),
                    label: Some("Blue".into()),
                },
                ..Default::default()
            };
            let path = dir.path().join(format!("metadata.{}", format.extension()));
            encode_file_with_metadata(&image, &settings, &path, false, Some(&capture)).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            for field in [
                "Canal &amp; café",
                "Amsterdam",
                "Photographer",
                "Copyright 2026",
                "Licensed print",
                "photo@example.test",
                "GPSLatitude",
                "<xmp:Rating>4</xmp:Rating>",
                "<xmp:Label>Blue</xmp:Label>",
            ] {
                assert!(
                    bytes
                        .windows(field.len())
                        .any(|part| part == field.as_bytes()),
                    "missing {field} in {format:?}"
                );
            }
            if matches!(
                format,
                ExportFormat::Jpeg { .. }
                    | ExportFormat::Png { .. }
                    | ExportFormat::Tiff { .. }
                    | ExportFormat::WebP { .. }
            ) {
                let decoded = image::open(&path).unwrap();
                assert_eq!((decoded.width(), decoded.height()), (8, 8));
            }
            settings.strip_gps = true;
            encode_file_with_metadata(&image, &settings, &path, false, Some(&capture)).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.windows(11).any(|part| part == b"GPSLatitude"));
            settings.copy_metadata = false;
            settings.copy_descriptive_metadata = false;
            encode_file_with_metadata(&image, &settings, &path, false, Some(&capture)).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            for field in [
                "Amsterdam",
                "Photographer",
                "Test Camera",
                "photo@example.test",
            ] {
                assert!(
                    !bytes
                        .windows(field.len())
                        .any(|part| part == field.as_bytes()),
                    "leaked {field} in {format:?}"
                );
            }
        }
    }

    #[test]
    fn jpeg_gps_is_readable_by_an_independent_exif_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gps.jpg");
        let image = crate::ExportImage::new(2, 2, vec![100; 16]).unwrap();
        let mut settings = ExportSettings {
            photo_metadata: PhotoMetadata {
                gps: Some([52.25, -4.5]),
                creator: "Photographer".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        crate::encode_file(&image, &settings, &path).unwrap();
        let read = || {
            let file = std::fs::File::open(&path).unwrap();
            exif::Reader::new()
                .read_from_container(&mut std::io::BufReader::new(file))
                .unwrap()
        };
        let tags = read();
        let lat = tags
            .get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY)
            .unwrap();
        let exif::Value::Rational(values) = &lat.value else {
            panic!("GPS latitude must contain rational DMS")
        };
        assert_eq!(values[0].to_f64(), 52.0);
        assert_eq!(values[1].to_f64(), 15.0);
        assert_eq!(values[2].to_f64(), 0.0);
        assert!(tags
            .get_field(exif::Tag::GPSLongitudeRef, exif::In::PRIMARY)
            .unwrap()
            .display_value()
            .to_string()
            .contains('W'));
        assert!(tags
            .get_field(exif::Tag::Artist, exif::In::PRIMARY)
            .unwrap()
            .display_value()
            .to_string()
            .contains("Photographer"));
        settings.strip_gps = true;
        crate::encode_file(&image, &settings, &path).unwrap();
        assert!(read()
            .get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY)
            .is_none());
    }
}
