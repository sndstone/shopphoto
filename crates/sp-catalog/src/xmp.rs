//! XMP sidecar interop for annotations and descriptive metadata (BUILD.md §9).
//!
//! ShopPhoto's own `.spedit` sidecar stays the source of truth — it carries
//! the full edit, which no XMP schema can express. This module mirrors
//! interoperable annotations and descriptive metadata. New sidecars use
//! Adobe's extension-replaced `IMG_0001.xmp`; existing full-name
//! `IMG_0001.3FR.xmp` and uppercase `.XMP` conventions are also retained.
//!
//! Mapping, chosen to match what Lightroom/Bridge/darktable already do:
//!
//! | ShopPhoto        | XMP                                    |
//! |------------------|----------------------------------------|
//! | rating 0         | `xmp:Rating="0"`                       |
//! | rating 1–5       | `xmp:Rating="1".."5"`                  |
//! | `Flag::Reject`   | `xmp:Rating="-1"` (Adobe's reject)      |
//! | `Flag::Pick`     | `shopphoto:Flag="Pick"`                |
//!
//! Pick flags have no cross-vendor XMP representation (Lightroom keeps them
//! in its catalog), so they live in our own namespace where they round-trip
//! with us and are ignored elsewhere. Rejects use the `-1` rating because
//! that is the one convention every reader understands.
//!
//! Reading accepts both the attribute form Lightroom writes and the element
//! form Bridge sometimes writes, and never fails on unknown XMP: an
//! unreadable or foreign packet yields `None`, not an error, because a
//! user's existing XMP is not ours to reject.

use crate::{Annotations, DescriptiveMetadata, Flag};
use quick_xml::events::Event;
use sp_core::SpError;
use std::path::{Path, PathBuf};

mod document;

/// Content revision used for optimistic XMP conflict detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmpRevision(Option<blake3::Hash>);

#[derive(Debug, Clone)]
pub struct XmpDocument {
    pub annotations: Option<Annotations>,
    pub metadata: DescriptiveMetadata,
    pub revision: XmpRevision,
}

/// Resolve existing extension-replaced or full-name XMP (including `.XMP`).
/// Prefer extension-replaced names when ambiguous; new files use `shot.xmp`.
#[must_use]
pub fn xmp_path(original: &Path) -> PathBuf {
    let appended = |suffix: &str| {
        let mut name = original.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    let candidates = [
        original.with_extension("xmp"),
        original.with_extension("XMP"),
        appended(".xmp"),
        appended(".XMP"),
    ];
    let mut existing = candidates.iter().filter(|path| path.is_file());
    if let Some(preferred) = existing.next() {
        // On case-insensitive volumes the two suffix spellings name one file.
        if existing
            .any(|other| std::fs::canonicalize(other).ok() != std::fs::canonicalize(preferred).ok())
        {
            tracing::warn!(original = %original.display(), chosen = %preferred.display(),
                "Ambiguous XMP sidecars; preferring extension-replaced naming");
        }
        preferred.clone()
    } else {
        candidates[0].clone()
    }
}

/// Our namespace for the fields standard XMP has no slot for.
const SHOPPHOTO_NS: &str = "https://shopphoto.app/ns/1.0/";

/// Serialize `annotations` as a standalone XMP packet.
#[must_use]
pub fn to_packet(annotations: Annotations) -> String {
    to_packet_with_metadata(annotations, &DescriptiveMetadata::default())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Serialize ratings plus IPTC-style description, keywords, label and GPS.
#[must_use]
pub fn to_packet_with_metadata(annotations: Annotations, metadata: &DescriptiveMetadata) -> String {
    // Reject wins over the star rating, matching Adobe: a rejected photo
    // reads as -1 regardless of how many stars it had.
    let rating = match annotations.flag {
        Flag::Reject => -1i32,
        _ => i32::from(annotations.rating.min(5)),
    };
    let flag = match annotations.flag {
        Flag::None => "None",
        Flag::Pick => "Pick",
        Flag::Reject => "Reject",
    };
    let keywords = metadata
        .keywords
        .iter()
        .map(|keyword| format!("     <rdf:li>{}</rdf:li>", xml_escape(keyword)))
        .collect::<Vec<_>>()
        .join("\n");
    let hierarchical_keywords = metadata
        .keywords
        .iter()
        .filter(|keyword| keyword.contains('|'))
        .map(|keyword| format!("<rdf:li>{}</rdf:li>", xml_escape(keyword)))
        .collect::<Vec<_>>()
        .join("");
    let label = match metadata.color_label {
        crate::ColorLabel::None => "",
        crate::ColorLabel::Red => "Red",
        crate::ColorLabel::Yellow => "Yellow",
        crate::ColorLabel::Green => "Green",
        crate::ColorLabel::Blue => "Blue",
        crate::ColorLabel::Purple => "Purple",
    };
    let gps = metadata.gps.map_or_else(String::new, |[lat, lon]| {
        format!(" exif:GPSLatitude=\"{lat:.8}\" exif:GPSLongitude=\"{lon:.8}\"")
    });
    let contact = if metadata.contact.is_empty() {
        String::new()
    } else {
        format!("<Iptc4xmpCore:CreatorContactInfo rdf:parseType=\"Resource\"><Iptc4xmpCore:CiEmailWork>{}</Iptc4xmpCore:CiEmailWork></Iptc4xmpCore:CreatorContactInfo>", xml_escape(&metadata.contact))
    };
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="ShopPhoto {version}">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:shopphoto="{ns}"
    xmp:Rating="{rating}"
    xmp:Label="{label}"{gps}
    shopphoto:Flag="{flag}"/>
  <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:xmpRights="http://ns.adobe.com/xap/1.0/rights/"
    xmlns:Iptc4xmpCore="http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/">
   <dc:description><rdf:Alt><rdf:li xml:lang="x-default">{caption}</rdf:li></rdf:Alt></dc:description>
   <dc:subject><rdf:Bag>
{keywords}
   </rdf:Bag></dc:subject>
   <lr:hierarchicalSubject><rdf:Bag>{hierarchical_keywords}</rdf:Bag></lr:hierarchicalSubject>
   <dc:creator><rdf:Seq><rdf:li>{creator}</rdf:li></rdf:Seq></dc:creator>
   <dc:rights><rdf:Alt><rdf:li xml:lang="x-default">{copyright}</rdf:li></rdf:Alt></dc:rights>
   <xmpRights:UsageTerms><rdf:Alt><rdf:li xml:lang="x-default">{rights}</rdf:li></rdf:Alt></xmpRights:UsageTerms>
   {contact}
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#,
        version = env!("CARGO_PKG_VERSION"),
        ns = SHOPPHOTO_NS,
        caption = xml_escape(&metadata.caption),
        creator = xml_escape(&metadata.creator),
        copyright = xml_escape(&metadata.copyright),
        rights = xml_escape(&metadata.rights),
    )
}

/// Parse ratings/flags out of an XMP packet.
///
/// Returns `None` when the packet carries neither field — a foreign XMP
/// with only camera metadata must not silently reset a photo to unrated.
#[must_use]
pub fn from_packet(xml: &str) -> Option<Annotations> {
    document::read(xml)
        .ok()
        .and_then(|(annotations, _)| annotations)
}

fn parse_flag(value: &str) -> Option<Flag> {
    match value.trim() {
        "Pick" => Some(Flag::Pick),
        "Reject" => Some(Flag::Reject),
        "None" => Some(Flag::None),
        _ => None,
    }
}

/// Read the XMP sidecar for `original`, if present and meaningful.
pub fn load(original: &Path) -> Result<Option<Annotations>, SpError> {
    let path = xmp_path(original);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SpError::io(path.display().to_string(), e)),
    };
    // Foreign XMP may be any encoding; lossy is right here because we only
    // ever look at ASCII field names and numbers.
    Ok(from_packet(&String::from_utf8_lossy(&bytes)))
}

/// Write the XMP sidecar for `original` atomically.
///
/// Only ever called when the user has opted in — writing files into the
/// user's photo folders is not something to do by default.
pub fn save(original: &Path, annotations: Annotations) -> Result<(), SpError> {
    save_impl(
        original,
        annotations,
        &DescriptiveMetadata::default(),
        None,
        false,
    )
    .map(|_| ())
}

pub fn save_with_metadata(
    original: &Path,
    annotations: Annotations,
    metadata: &DescriptiveMetadata,
) -> Result<(), SpError> {
    save_impl(original, annotations, metadata, None, true).map(|_| ())
}

/// Read all supported XMP fields and a revision for conflict-aware saving.
/// Unlike the legacy annotation reader, this refuses malformed/non-UTF-8 XML.
pub fn load_with_metadata(original: &Path) -> Result<Option<XmpDocument>, SpError> {
    let path = xmp_path(original);
    let Some(bytes) = read_existing(&path)? else {
        return Ok(None);
    };
    let xml = std::str::from_utf8(&bytes).map_err(|e| SpError::CorruptData {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    let (annotations, metadata) = document::read(xml)?;
    Ok(Some(XmpDocument {
        annotations,
        metadata,
        revision: XmpRevision(Some(blake3::hash(&bytes))),
    }))
}

pub fn revision(original: &Path) -> Result<XmpRevision, SpError> {
    Ok(XmpRevision(
        read_existing(&xmp_path(original))?.map(|bytes| blake3::hash(&bytes)),
    ))
}

/// Refuse to overwrite changes made since the caller read the supplied revision.
pub fn save_with_metadata_if_unchanged(
    original: &Path,
    annotations: Annotations,
    metadata: &DescriptiveMetadata,
    expected: &XmpRevision,
) -> Result<XmpRevision, SpError> {
    save_impl(original, annotations, metadata, Some(expected), true)
}

fn read_existing(path: &Path) -> Result<Option<Vec<u8>>, SpError> {
    use std::io::Read;
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| SpError::io(path.display().to_string(), error))?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(SpError::CorruptData {
                    path: path.display().to_string(),
                    reason: "XMP exceeds 16 MiB".into(),
                });
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SpError::io(path.display().to_string(), error)),
    }
}

fn save_impl(
    original: &Path,
    annotations: Annotations,
    metadata: &DescriptiveMetadata,
    expected: Option<&XmpRevision>,
    full_metadata: bool,
) -> Result<XmpRevision, SpError> {
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _guard = WRITER
        .lock()
        .map_err(|_| SpError::Catalog("XMP writer lock unavailable".into()))?;
    let path = xmp_path(original);
    let before = read_existing(&path)?;
    let actual = XmpRevision(before.as_ref().map(|bytes| blake3::hash(bytes)));
    let conflict = || {
        SpError::Catalog(format!(
            "XMP changed externally; reload metadata before saving {}",
            path.display()
        ))
    };
    if expected.is_some_and(|expected| *expected != actual) {
        return Err(conflict());
    }
    let packet = if let Some(bytes) = &before {
        let xml = std::str::from_utf8(bytes).map_err(|e| SpError::CorruptData {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        document::merge(xml, annotations, metadata, full_metadata)?
    } else {
        to_packet_with_metadata(annotations, metadata)
    };
    // Validate generated values too (control characters are not legal XML).
    document::read(&packet)?;
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = path.with_extension(format!("xmp.{}.{sequence}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| SpError::io(temp.display().to_string(), e))?;
        output
            .write_all(packet.as_bytes())
            .and_then(|_| output.sync_all())
            .map_err(|e| SpError::io(temp.display().to_string(), e))?;
        drop(output);
        // Optimistic concurrency: cooperating ShopPhoto writes are serialized;
        // foreign writers are checked immediately before atomic publication.
        if read_existing(&path)? != before {
            return Err(conflict());
        }
        std::fs::rename(&temp, &path).map_err(|e| SpError::io(path.display().to_string(), e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map(|()| XmpRevision(Some(blake3::hash(packet.as_bytes()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_retains_existing_naming_conventions() {
        for name in ["IMG.xmp", "IMG.XMP", "IMG.ARW.xmp", "IMG.ARW.XMP"] {
            let directory = tempfile::tempdir().unwrap();
            let original = directory.path().join("IMG.ARW");
            let sidecar = directory.path().join(name);
            let annotations = Annotations {
                rating: 3,
                flag: Flag::Pick,
            };
            std::fs::write(&sidecar, to_packet(annotations)).unwrap();
            assert_eq!(load(&original).unwrap(), Some(annotations));
            assert_eq!(
                load_with_metadata(&original).unwrap().unwrap().annotations,
                Some(annotations)
            );
            save(
                &original,
                Annotations {
                    rating: 5,
                    ..annotations
                },
            )
            .unwrap();
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
            assert_eq!(
                from_packet(&std::fs::read_to_string(sidecar).unwrap())
                    .unwrap()
                    .rating,
                5
            );
        }
    }

    #[test]
    fn extension_replaced_wins_when_both_conventions_exist() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("IMG.ARW");
        let replaced = directory.path().join("IMG.XMP");
        let appended = directory.path().join("IMG.ARW.xmp");
        std::fs::write(
            &replaced,
            to_packet(Annotations {
                rating: 2,
                flag: Flag::None,
            }),
        )
        .unwrap();
        let foreign = to_packet(Annotations {
            rating: 4,
            flag: Flag::None,
        });
        std::fs::write(&appended, &foreign).unwrap();
        assert_eq!(load(&original).unwrap().unwrap().rating, 2);
        save(&original, Annotations::default()).unwrap();
        assert_eq!(std::fs::read_to_string(appended).unwrap(), foreign);
    }

    #[test]
    fn hierarchical_subject_import_and_save_retains_nested_keywords() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.dng");
        std::fs::write(xmp_path(&photo),r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:lr="http://ns.adobe.com/lightroom/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/"><lr:hierarchicalSubject><rdf:Bag><rdf:li>People|Family|Alice</rdf:li></rdf:Bag></lr:hierarchicalSubject><dc:subject><rdf:Bag><rdf:li>Alice</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF>"#).unwrap();
        let document = load_with_metadata(&photo).unwrap().unwrap();
        assert!(document
            .metadata
            .keywords
            .contains(&"People|Family|Alice".into()));
        save_with_metadata(&photo, Annotations::default(), &document.metadata).unwrap();
        let packet = std::fs::read_to_string(xmp_path(&photo)).unwrap();
        assert!(packet.contains("<lr:hierarchicalSubject>"));
        assert_eq!(
            load_with_metadata(&photo).unwrap().unwrap().metadata,
            document.metadata
        );
    }

    const FOREIGN: &str = r#"<?xpacket begin=""?>
<m:xmpmeta xmlns:m="adobe:ns:meta/">
<r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <r:Description r:about="" xmlns:a="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:other="urn:other" a:Rating="2" crs:Exposure2012="1.20" other:Rating="99">
 <!-- Preserve foreign edits -->
 <crs:Look><r:Description crs:Name="My &amp; look"/></crs:Look>
 </r:Description>
</r:RDF></m:xmpmeta><?xpacket end="w"?>"#;

    #[test]
    fn merge_preserves_foreign_edit_namespaces_and_prefix_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("test.dng");
        std::fs::write(xmp_path(&source), FOREIGN).unwrap();
        let metadata = DescriptiveMetadata {
            caption: "Water & sky".into(),
            keywords: vec!["Travel".into(), "Amsterdam".into()],
            creator: "Photographer".into(),
            copyright: "© 2026".into(),
            rights: "Licensed to client".into(),
            contact: "photo@example.com".into(),
            gps: Some([52.3676, 4.9041]),
            color_label: crate::ColorLabel::Blue,
        };
        let annotations = Annotations {
            rating: 5,
            flag: Flag::Pick,
        };
        save_with_metadata(&source, annotations, &metadata).unwrap();
        let bytes = std::fs::read_to_string(xmp_path(&source)).unwrap();
        assert!(bytes.contains("crs:Exposure2012=\"1.20\""));
        assert!(bytes.contains("other:Rating=\"99\""));
        assert!(bytes.contains("<!-- Preserve foreign edits -->"));
        assert!(bytes.contains("<crs:Look><r:Description crs:Name=\"My &amp; look\"/></crs:Look>"));
        assert!(!bytes.contains("a:Rating=\"2\""));
        let loaded = load_with_metadata(&source).unwrap().unwrap();
        assert_eq!(loaded.annotations, Some(annotations));
        assert_eq!(loaded.metadata, metadata);
        // Repeated saves must not accumulate duplicate owned properties.
        save_with_metadata(&source, annotations, &metadata).unwrap();
        let again = std::fs::read_to_string(xmp_path(&source)).unwrap();
        assert_eq!(again.matches("<dc:creator>").count(), 1);
    }

    #[test]
    fn annotation_only_save_retains_descriptive_and_foreign_contact_fields() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("test.dng");
        let metadata = DescriptiveMetadata {
            caption: "Keep me".into(),
            contact: "old@example.com".into(),
            ..Default::default()
        };
        let packet = to_packet_with_metadata(Annotations::default(), &metadata)
            .replace("</Iptc4xmpCore:CreatorContactInfo>", "<Iptc4xmpCore:CiAdrCity>Amsterdam</Iptc4xmpCore:CiAdrCity></Iptc4xmpCore:CreatorContactInfo>");
        std::fs::write(xmp_path(&source), packet).unwrap();
        save(
            &source,
            Annotations {
                rating: 3,
                flag: Flag::None,
            },
        )
        .unwrap();
        assert_eq!(
            load_with_metadata(&source).unwrap().unwrap().metadata,
            metadata
        );
        let updated = DescriptiveMetadata {
            contact: "new@example.com".into(),
            ..metadata
        };
        save_with_metadata(&source, Annotations::default(), &updated).unwrap();
        let saved = std::fs::read_to_string(xmp_path(&source)).unwrap();
        assert!(saved.contains("<Iptc4xmpCore:CiAdrCity>Amsterdam</Iptc4xmpCore:CiAdrCity>"));
        assert!(!saved.contains("old@example.com"));
        assert_eq!(
            load_with_metadata(&source)
                .unwrap()
                .unwrap()
                .metadata
                .contact,
            "new@example.com"
        );
    }

    #[test]
    fn malformed_or_concurrently_changed_xmp_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("test.dng");
        for malformed in [
            "<unclosed>",
            "<one/><two/>",
            "<x:undeclared/>",
            "<root>&undefined;</root>",
            "<!DOCTYPE root><root/>",
        ] {
            std::fs::write(xmp_path(&source), malformed).unwrap();
            assert!(save(&source, Annotations::default()).is_err());
            assert_eq!(
                std::fs::read_to_string(xmp_path(&source)).unwrap(),
                malformed
            );
        }
        std::fs::write(xmp_path(&source), FOREIGN).unwrap();
        let expected = revision(&source).unwrap();
        let changed = FOREIGN.replace("1.20", "2.50");
        std::fs::write(xmp_path(&source), &changed).unwrap();
        assert!(save_with_metadata_if_unchanged(
            &source,
            Annotations::default(),
            &DescriptiveMetadata::default(),
            &expected
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(xmp_path(&source)).unwrap(), changed);
    }

    #[test]
    fn unrelated_rating_namespace_does_not_change_annotations() {
        let packet = FOREIGN.replace("a:Rating=\"2\"", "");
        assert!(from_packet(&packet).is_none());
    }

    #[test]
    fn path_replaces_the_extension() {
        // New sidecars retain the Adobe extension-replaced default.
        assert_eq!(
            xmp_path(Path::new("/a/shot.3FR")),
            PathBuf::from("/a/shot.xmp")
        );
    }

    #[test]
    fn round_trips_every_flag_and_rating() {
        for flag in [Flag::None, Flag::Pick, Flag::Reject] {
            for rating in 0..=5u8 {
                let value = Annotations { rating, flag };
                let back = from_packet(&to_packet(value)).expect("parsed");
                assert_eq!(back.flag, flag, "flag lost for {rating} stars");
                let expected_rating = if flag == Flag::Reject { 0 } else { rating };
                assert_eq!(back.rating, expected_rating);
            }
        }
    }

    #[test]
    fn reject_is_written_as_adobe_negative_rating() {
        let packet = to_packet(Annotations {
            rating: 4,
            flag: Flag::Reject,
        });
        assert!(packet.contains(r#"xmp:Rating="-1""#), "{packet}");
    }

    #[test]
    fn reads_lightroom_attribute_form() {
        let lr = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 6.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmp:Rating="3" xmp:CreatorTool="Adobe Photoshop Lightroom"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        let value = from_packet(lr).expect("parsed");
        assert_eq!(value.rating, 3);
        assert_eq!(value.flag, Flag::None);
    }

    #[test]
    fn reads_bridge_element_form_and_negative_reject() {
        let bridge = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/">
   <xmp:Rating>-1</xmp:Rating>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let value = from_packet(bridge).expect("parsed");
        assert_eq!(value.rating, 0);
        assert_eq!(value.flag, Flag::Reject, "-1 must read back as a reject");
    }

    #[test]
    fn foreign_xmp_without_rating_is_not_an_annotation() {
        let camera_only = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    tiff:Make="Hasselblad"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        assert!(
            from_packet(camera_only).is_none(),
            "must not reset ratings from unrelated XMP"
        );
    }

    #[test]
    fn malformed_xml_does_not_panic_or_error() {
        assert!(from_packet("<x:xmpmeta><rdf:RDF unclosed").is_none());
        assert!(from_packet("").is_none());
        assert!(from_packet("\u{0}\u{1}not xml at all").is_none());
    }

    #[test]
    fn save_and_load_round_trip_on_disk() {
        let dir = std::env::temp_dir().join("sp-catalog-test-xmp");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let original = dir.join("shot.3FR");
        assert!(load(&original).expect("load none").is_none());

        let value = Annotations {
            rating: 5,
            flag: Flag::Pick,
        };
        save(&original, value).expect("save");
        assert!(dir.join("shot.xmp").exists());
        assert_eq!(load(&original).expect("load"), Some(value));
        std::fs::remove_dir_all(&dir).ok();
    }
}
