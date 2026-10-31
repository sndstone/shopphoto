//! Disposable, byte-bounded neighbor previews. Never stores full sensor data.
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct PreviewCache {
    entries: VecDeque<(PathBuf, Arc<sp_raw::Preview>)>,
    bytes: usize,
    limit: usize,
}

impl PreviewCache {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }

    pub fn get(&mut self, path: &Path) -> Option<Arc<sp_raw::Preview>> {
        let index = self.entries.iter().position(|(key, _)| key == path)?;
        let entry = self.entries.remove(index)?;
        let preview = entry.1.clone();
        self.entries.push_front(entry);
        Some(preview)
    }

    pub fn insert(&mut self, path: PathBuf, preview: sp_raw::Preview) {
        if let Some(index) = self.entries.iter().position(|(key, _)| *key == path) {
            if let Some((_, old)) = self.entries.remove(index) {
                self.bytes -= old.rgba8.len();
            }
        }
        let bytes = preview.rgba8.len();
        if bytes > self.limit {
            return;
        }
        while self.bytes + bytes > self.limit || self.entries.len() >= 8 {
            if let Some((_, old)) = self.entries.pop_back() {
                self.bytes -= old.rgba8.len();
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.entries.push_front((path, Arc::new(preview)));
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Camera previews can be full-size JPEGs. Resize before retaining them.
pub fn bounded_preview(preview: sp_raw::Preview) -> Option<sp_raw::Preview> {
    let image = image::RgbaImage::from_raw(preview.width, preview.height, preview.rgba8)?;
    let scale = 2048.0 / image.width().max(image.height()).max(1) as f32;
    if scale >= 1.0 {
        return Some(sp_raw::Preview {
            width: image.width(),
            height: image.height(),
            rgba8: image.into_raw(),
        });
    }
    let image = image::imageops::resize(
        &image,
        (image.width() as f32 * scale).round().max(1.0) as u32,
        (image.height() as f32 * scale).round().max(1.0) as u32,
        image::imageops::FilterType::Triangle,
    );
    Some(sp_raw::Preview {
        width: image.width(),
        height: image.height(),
        rgba8: image.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn preview(bytes: usize) -> sp_raw::Preview {
        sp_raw::Preview {
            width: (bytes / 4) as u32,
            height: 1,
            rgba8: vec![0; bytes],
        }
    }
    #[test]
    fn evicts_by_bytes_and_recent_use_and_replaces_without_leaking_accounting() {
        let mut cache = PreviewCache::new(16);
        cache.insert("a".into(), preview(8));
        cache.insert("b".into(), preview(8));
        assert!(cache.get(Path::new("a")).is_some());
        cache.insert("c".into(), preview(8));
        assert!(cache.get(Path::new("b")).is_none());
        cache.insert("a".into(), preview(4));
        assert_eq!(cache.bytes(), 12);
        cache.insert("large".into(), preview(20));
        assert_eq!(cache.bytes(), 12);
    }
}
