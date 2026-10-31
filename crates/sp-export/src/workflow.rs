//! Portable combinations of a selective develop preset and delivery settings.
use crate::ExportSettings;
use serde::{Deserialize, Serialize};
use sp_core::{preset::UserPreset, EditState};
use std::{
    io::{Read, Write},
    path::Path,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoWorkflow {
    pub version: u32,
    pub name: String,
    pub develop: Option<UserPreset>,
    pub export: ExportSettings,
}
impl PhotoWorkflow {
    pub fn apply(&self, edit: &mut EditState) -> Result<(), String> {
        if self.version != 1
            || self.export.version == 0
            || self.export.version > crate::EXPORT_SETTINGS_VERSION
            || self
                .develop
                .as_ref()
                .is_some_and(|p| p.version != UserPreset::VERSION)
        {
            return Err("Unsupported photo workflow version".into());
        }
        if let Some(preset) = &self.develop {
            preset.apply(edit);
        }
        *edit = std::mem::take(edit).clamped();
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err("Workflow exceeds 64 MiB".into());
        }
        let workflow: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        workflow.apply(&mut EditState::default())?;
        Ok(workflow)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.apply(&mut EditState::default())?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut file, self).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        if file.as_file().metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
            return Err("Workflow exceeds 64 MiB".into());
        }
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workflow_reuses_selected_groups_without_capturing_photo_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delivery.spworkflow");
        let mut source = EditState::default();
        source.tone.exposure_ev = 1.0;
        source.geometry.flip_horizontal = true;
        let preset = UserPreset::from_edit(
            "Tone",
            sp_core::preset::EditGroups {
                tone: true,
                geometry: false,
                ..Default::default()
            },
            &source,
        );
        let mut workflow = PhotoWorkflow {
            version: 1,
            name: "Web".into(),
            develop: Some(preset),
            export: ExportSettings::default(),
        };
        workflow.export.photo_metadata.caption = "Private photo identity".into();
        workflow.save(&path).unwrap();
        let loaded = PhotoWorkflow::load(&path).unwrap();
        assert!(loaded.export.photo_metadata.caption.is_empty());
        let mut edit = EditState::default();
        loaded.apply(&mut edit).unwrap();
        assert_eq!(edit.tone.exposure_ev, 1.0);
        assert!(!edit.geometry.flip_horizontal);
        workflow.version = 9;
        assert!(workflow.save(&path).is_err());
        assert_eq!(PhotoWorkflow::load(&path).unwrap().version, 1);
    }
}
