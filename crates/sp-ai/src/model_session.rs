//! Two retained sessions per worker thread. Batch workers reuse compiled
//! networks; threads own their handles and release them on exit. Replacing a
//! model bundle invalidates the session before the next photograph.
use crate::AiError;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

#[derive(PartialEq, Eq)]
struct Key {
    path: PathBuf,
    revision: Vec<(PathBuf, u64, Option<SystemTime>)>,
    compute: crate::ComputeUnits,
}

impl Key {
    fn read(path: &Path, compute: crate::ComputeUnits) -> Result<Self, AiError> {
        fn visit(
            path: &Path,
            files: &mut Vec<(PathBuf, u64, Option<SystemTime>)>,
        ) -> std::io::Result<()> {
            let metadata = std::fs::metadata(path)?;
            if metadata.is_dir() {
                for item in std::fs::read_dir(path)? {
                    visit(&item?.path(), files)?;
                }
            } else {
                files.push((path.to_owned(), metadata.len(), metadata.modified().ok()));
            }
            Ok(())
        }
        let path = path
            .canonicalize()
            .map_err(|e| AiError::CoreMl(e.to_string()))?;
        let mut revision = Vec::new();
        visit(&path, &mut revision).map_err(|e| AiError::CoreMl(e.to_string()))?;
        revision.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Self {
            path,
            revision,
            compute,
        })
    }
}

#[derive(Default)]
struct Cache<T>(Vec<(Key, Rc<T>)>);

impl<T> Cache<T> {
    fn get(
        &mut self,
        key: Key,
        load: impl FnOnce(&Path, crate::ComputeUnits) -> Result<T, AiError>,
    ) -> Result<Rc<T>, AiError> {
        if let Some(index) = self.0.iter().position(|(stored, _)| *stored == key) {
            let entry = self.0.remove(index);
            let model = entry.1.clone();
            self.0.push(entry);
            return Ok(model);
        }
        // Drop stale revisions of this path before loading a replacement.
        self.0.retain(|(stored, _)| stored.path != key.path);
        if self.0.len() == 2 {
            self.0.remove(0);
        }
        let model = Rc::new(load(&key.path, key.compute)?);
        self.0.push((key, model.clone()));
        Ok(model)
    }
}

#[cfg(target_os = "macos")]
pub(super) struct LoadedModel {
    pub handle: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
impl Drop for LoadedModel {
    fn drop(&mut self) {
        unsafe extern "C" {
            fn sp_ai_model_close(model: *mut std::ffi::c_void);
        }
        unsafe { sp_ai_model_close(self.handle) };
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) struct LoadedModel {
    pub model: std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>,
}

impl LoadedModel {
    #[cfg(target_os = "macos")]
    fn load(path: &Path, compute: crate::ComputeUnits) -> Result<Self, AiError> {
        use std::ffi::{c_char, c_void, CStr, CString};
        unsafe extern "C" {
            fn sp_ai_model_open(
                path: *const c_char,
                compute: i32,
                error: *mut c_char,
                len: usize,
            ) -> *mut c_void;
        }
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|e| AiError::CoreMl(e.to_string()))?;
        let mut error = [0i8; 512];
        let handle = unsafe {
            sp_ai_model_open(
                path.as_ptr(),
                compute as i32,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if handle.is_null() {
            return Err(AiError::CoreMl(
                unsafe { CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
        Ok(Self { handle })
    }

    #[cfg(not(target_os = "macos"))]
    fn load(path: &Path, compute: crate::ComputeUnits) -> Result<Self, AiError> {
        let _ = compute;
        use tract_onnx::prelude::*;
        // The export has dynamic height and width; pin the tile shape.
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .and_then(|model| {
                model.with_input_fact(
                    0,
                    f32::fact([1, crate::PLANES, crate::TILE, crate::TILE]).into(),
                )
            })
            .and_then(|model| model.into_optimized())
            .and_then(|model| model.into_runnable())
            .map_err(|error| AiError::CoreMl(format!("{}: {error}", path.display())))?;
        Ok(Self { model })
    }
}

thread_local! { static SESSIONS: RefCell<Cache<LoadedModel>> = const { RefCell::new(Cache(Vec::new())) }; }

pub(super) fn get(path: &Path, compute: crate::ComputeUnits) -> Result<Rc<LoadedModel>, AiError> {
    let key = Key::read(path, compute)?;
    SESSIONS.with(|cache| cache.borrow_mut().get(key, LoadedModel::load))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_changes_replace_sessions_and_preserve_in_flight_handles() {
        use crate::ComputeUnits;
        let key = |compute| Key {
            path: "model".into(),
            revision: vec![],
            compute,
        };
        let mut cache = Cache::<ComputeUnits>::default();
        let old = cache
            .get(key(ComputeUnits::CpuGpu), |_, compute| Ok(compute))
            .unwrap();
        let new = cache
            .get(key(ComputeUnits::CpuNeuralEngine), |_, compute| Ok(compute))
            .unwrap();
        assert!(!Rc::ptr_eq(&old, &new));
        assert_eq!(*old, ComputeUnits::CpuGpu);
        assert_eq!(*new, ComputeUnits::CpuNeuralEngine);
        let reused = cache
            .get(key(ComputeUnits::CpuNeuralEngine), |_, _| {
                panic!("must reuse")
            })
            .unwrap();
        assert!(Rc::ptr_eq(&new, &reused));
        assert_eq!(cache.0.len(), 1);
    }

    #[test]
    fn reuses_sessions_and_invalidates_changed_revisions() {
        let key = |name: &str, revision| Key {
            path: name.into(),
            revision: vec![(name.into(), revision, None)],
            compute: crate::ComputeUnits::Automatic,
        };
        let mut cache = Cache::<u32>::default();
        let first = cache.get(key("a", 1), |_, _| Ok(1)).unwrap();
        let same = cache.get(key("a", 1), |_, _| panic!("must reuse")).unwrap();
        assert!(Rc::ptr_eq(&first, &same));
        let replacement = cache.get(key("a", 2), |_, _| Ok(2)).unwrap();
        assert_eq!(*replacement, 2);
        cache.get(key("b", 1), |_, _| Ok(3)).unwrap();
        cache.get(key("c", 1), |_, _| Ok(4)).unwrap();
        assert_eq!(cache.0.len(), 2);
        assert!(!cache.0.iter().any(|(key, _)| key.path == Path::new("a")));
        assert_eq!(
            *first, 1,
            "borrowed sessions stay alive until the caller finishes"
        );
    }
}
