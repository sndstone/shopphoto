//! Offline proof galleries. Original paths live only in the private receipt.
use crate::{ExportColorSpace, ExportFormat, ExportImage, ExportSettings};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub version: u32,
    pub gallery_id: String,
    pub title: String,
    pub sources: Vec<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub id: usize,
    pub picked: bool,
    pub rating: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Picks {
    pub version: u32,
    pub gallery_id: String,
    pub choices: Vec<Choice>,
}
#[derive(Serialize)]
struct Photo {
    id: usize,
    file: String,
    name: String,
    caption: String,
}
pub struct Gallery {
    staging: tempfile::TempDir,
    destination: PathBuf,
    receipt: Receipt,
    photos: Vec<Photo>,
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Proof metadata exceeds 8 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
impl Picks {
    pub fn load(path: &Path) -> Result<Self, String> {
        read_json(path)
    }
    pub fn validate(&self, receipt: &Receipt) -> Result<(), String> {
        if self.version != 1 || receipt.version != 1 || self.gallery_id != receipt.gallery_id {
            return Err("Picks do not match this gallery receipt".into());
        }
        let mut seen = std::collections::HashSet::new();
        if self
            .choices
            .iter()
            .any(|c| c.id >= receipt.sources.len() || c.rating > 5 || !seen.insert(c.id))
        {
            return Err("Invalid or duplicate photo in returned picks".into());
        }
        Ok(())
    }
    pub fn receipt_filename(&self) -> Result<String, String> {
        if self.gallery_id.is_empty()
            || self.gallery_id.len() > 80
            || !self
                .gallery_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("Invalid gallery identifier".into());
        }
        Ok(format!("{}.json", self.gallery_id))
    }
}
impl Receipt {
    pub fn load(path: &Path) -> Result<Self, String> {
        read_json(path)
    }
}
impl Gallery {
    pub fn new(destination: &Path, title: String) -> Result<Self, String> {
        if destination.exists() {
            return Err("Choose a new folder name for the proof gallery".into());
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let staging = tempfile::Builder::new()
            .prefix(".shopphoto-proof-")
            .tempdir_in(parent)
            .map_err(|e| e.to_string())?;
        let id = format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos(),
            std::process::id()
        );
        Ok(Self {
            staging,
            destination: destination.to_owned(),
            receipt: Receipt {
                version: 1,
                gallery_id: id,
                title,
                sources: Vec::new(),
            },
            photos: Vec::new(),
        })
    }
    pub fn push(
        &mut self,
        image: &ExportImage,
        source: &Path,
        caption: &str,
    ) -> Result<(), String> {
        if self.photos.len() >= 10_000 {
            return Err("A gallery supports at most 10,000 photos".into());
        }
        let id = self.photos.len();
        let file = format!("photo-{:05}.jpg", id + 1);
        let settings = ExportSettings {
            format: ExportFormat::Jpeg {
                quality: 90,
                chroma_444: true,
            },
            color_space: ExportColorSpace::Srgb,
            copy_metadata: false,
            copy_descriptive_metadata: false,
            strip_gps: true,
            ..Default::default()
        };
        crate::encode_file(image, &settings, &self.staging.path().join(&file))
            .map_err(|e| e.to_string())?;
        self.photos.push(Photo {
            id,
            file,
            name: source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            caption: caption.to_owned(),
        });
        self.receipt.sources.push(source.to_owned());
        Ok(())
    }
    pub fn finish(self, private_receipts: &Path) -> Result<PathBuf, String> {
        if self.photos.is_empty() {
            return Err("Gallery has no photos".into());
        }
        let data=serde_json::to_string(&serde_json::json!({"version":1,"gallery_id":self.receipt.gallery_id,"title":self.receipt.title,"photos":self.photos})).map_err(|e|e.to_string())?.replace('<',"\\u003c").replace('>',"\\u003e").replace('&',"\\u0026");
        let html = include_str!("gallery.html").replace("/*PHOTO_DATA*/null", &data);
        std::fs::write(self.staging.path().join("index.html"), html).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(private_receipts).map_err(|e| e.to_string())?;
        let mut receipt =
            tempfile::NamedTempFile::new_in(private_receipts).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut receipt, &self.receipt).map_err(|e| e.to_string())?;
        receipt.flush().map_err(|e| e.to_string())?;
        receipt.as_file().sync_all().map_err(|e| e.to_string())?;
        receipt
            .persist(private_receipts.join(format!("{}.json", self.receipt.gallery_id)))
            .map_err(|e| e.to_string())?;
        if self.destination.exists() {
            return Err("Gallery destination now exists; choose another name".into());
        }
        std::fs::rename(self.staging.path(), &self.destination).map_err(|e| e.to_string())?;
        Ok(self.destination.join("index.html"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proof_gallery_contains_no_source_paths_and_escapes_caption_code() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proof");
        let mut g = Gallery::new(&path, "Summer <trip>".into()).unwrap();
        g.push(
            &ExportImage::new(1, 1, vec![50, 100, 150, 255]).unwrap(),
            Path::new("/private/originals/photo.jpg"),
            "</script><script>bad()</script>",
        )
        .unwrap();
        let html_path = g.finish(&dir.path().join("receipts")).unwrap();
        let html = std::fs::read_to_string(html_path).unwrap();
        assert!(!html.contains("/private/originals"));
        assert!(!html.contains("<script>bad()"));
        assert!(html.contains("\\u003c/script"));
        assert!(path.join("photo-00001.jpg").is_file());
        assert!(Gallery::new(&path, "Overwrite".into()).is_err());
    }
    #[test]
    fn picks_are_bound_to_the_original_receipt_and_validate_every_id() {
        let receipt = Receipt {
            version: 1,
            gallery_id: "123-45".into(),
            title: "x".into(),
            sources: vec!["a.jpg".into()],
        };
        let mut picks = Picks {
            version: 1,
            gallery_id: receipt.gallery_id.clone(),
            choices: vec![Choice {
                id: 0,
                picked: true,
                rating: 5,
            }],
        };
        picks.validate(&receipt).unwrap();
        picks.choices.push(picks.choices[0].clone());
        assert!(picks.validate(&receipt).is_err());
        picks.choices.pop();
        picks.choices[0].rating = 6;
        assert!(picks.validate(&receipt).is_err());
        picks.gallery_id = "../../escape".into();
        assert!(picks.receipt_filename().is_err());
    }
}
