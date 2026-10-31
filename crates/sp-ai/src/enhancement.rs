//! Native, named-tensor enhancement sessions. Public segmentation/restoration
//! models never share handles, manifests or directories with camera denoise.
use crate::ComputeUnits;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TensorSpec {
    pub name: String,
    pub shape: Vec<usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flexible_axes: BTreeMap<usize, [usize; 2]>,
}
impl TensorSpec {
    fn accepts(&self, actual: &Self) -> bool {
        self.name == actual.name
            && self.shape.len() == actual.shape.len()
            && self
                .shape
                .iter()
                .zip(&actual.shape)
                .enumerate()
                .all(|(axis, (expected, actual))| {
                    self.flexible_axes
                        .get(&axis)
                        .map_or(*expected == *actual, |[min, max]| {
                            (*min..=*max).contains(actual)
                        })
                })
    }

    fn maximum_count(&self) -> Result<usize, String> {
        self.count()?;
        let mut maximum = self.clone();
        for (&axis, &[_, max]) in &self.flexible_axes {
            maximum.shape[axis] = max;
        }
        maximum.count()
    }

    pub fn count(&self) -> Result<usize, String> {
        if self.name.is_empty()
            || self.name.as_bytes().contains(&0)
            || self.shape.is_empty()
            || self.shape.len() > 6
        {
            return Err("Invalid native tensor name or rank".into());
        }
        for (&axis, &[min, max]) in &self.flexible_axes {
            if axis >= self.shape.len()
                || min == 0
                || min > max
                || max > 32
                || !(min..=max).contains(&self.shape[axis])
            {
                return Err("Invalid native flexible tensor axis".into());
            }
        }
        self.shape.iter().try_fold(1usize, |count, &axis| {
            count
                .checked_mul(axis)
                .filter(|&n| axis > 0 && n <= 32_000_000)
                .ok_or_else(|| "Native tensor exceeds memory limit or has an empty axis".into())
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub id: String,
    pub task: String,
    pub model: String,
    pub inputs: Vec<TensorSpec>,
    pub outputs: Vec<TensorSpec>,
    #[serde(default)]
    pub scale: usize,
    #[serde(default)]
    pub tile: usize,
    #[serde(default)]
    pub halo: usize,
    #[serde(default)]
    pub classes: BTreeMap<String, usize>,
    pub license: String,
    pub source: String,
    pub revision: String,
    pub files: BTreeMap<String, String>,
}

fn relative(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Used to ensure a named legacy checkpoint really matches its native port.
/// Renaming another checkpoint must never silently change the selected model.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

impl Manifest {
    pub fn read(bundle: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(bundle.join("manifest.json")).map_err(|e| e.to_string())?;
        if bytes.len() > 1_000_000 {
            return Err("Native manifest exceeds limit".into());
        }
        let manifest: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid native manifest: {e}"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || !relative(&self.model)
            || !self.model.ends_with(".mlmodelc")
            || self.id.is_empty()
            || self.license.is_empty()
            || self.source.is_empty()
            || self.revision.is_empty()
            || self.inputs.is_empty()
            || self.outputs.is_empty()
            || self.inputs.len() > 8
            || self.outputs.len() > 8
            || self.files.is_empty()
            || self.files.len() > 2048
        {
            return Err("Invalid or unsupported native model manifest".into());
        }
        for specs in [&self.inputs, &self.outputs] {
            let mut names = std::collections::BTreeSet::new();
            let mut total = 0usize;
            for spec in specs {
                total += spec.maximum_count()?;
                if !names.insert(&spec.name) || total > 48_000_000 {
                    return Err("Duplicate native tensors or excessive tensor memory".into());
                }
            }
        }
        Ok(())
    }

    fn verify_files(&self, bundle: &Path) -> Result<(), String> {
        use std::io::Read;
        let root = bundle.canonicalize().map_err(|e| e.to_string())?;
        for (name, expected) in &self.files {
            if !relative(name) || expected.len() != 64 {
                return Err("Invalid native model file record".into());
            }
            let path = bundle
                .join(name)
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if !path.starts_with(&root) {
                return Err("Native model file escapes its bundle".into());
            }
            let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            if format!("{:x}", hash.finalize()) != *expected {
                return Err(format!("Native model checksum failed: {name}"));
            }
        }
        // Every compiled-model file must be pinned, not just one convenient
        // metadata file. Extra license/readme files need not be graph inputs.
        fn visit(path: &Path, root: &Path, files: &BTreeMap<String, String>) -> Result<(), String> {
            if path.is_dir() {
                for child in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                    visit(&child.map_err(|e| e.to_string())?.path(), root, files)?;
                }
            } else {
                let name = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy();
                if !files.contains_key(name.as_ref()) {
                    return Err(format!("Unpinned native model file: {name}"));
                }
            }
            Ok(())
        }
        visit(&bundle.join(&self.model), bundle, &self.files)
    }
}

#[derive(Debug)]
pub struct Tensor {
    pub spec: TensorSpec,
    pub values: Vec<f32>,
}
impl Tensor {
    pub fn new(spec: TensorSpec, values: Vec<f32>) -> Result<Self, String> {
        if spec.count()? != values.len() || values.iter().any(|v| !v.is_finite()) {
            return Err("Native tensor pixels are nonfinite or misaligned".into());
        }
        Ok(Self { spec, values })
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::{
        cell::RefCell,
        ffi::{c_char, c_void, CStr, CString},
        rc::Rc,
        time::SystemTime,
    };
    #[repr(C)]
    struct FfiTensor {
        name: *const c_char,
        shape: *const usize,
        rank: usize,
        data: *mut f32,
        count: usize,
    }
    unsafe extern "C" {
        fn sp_ai_model_open(
            path: *const c_char,
            compute: i32,
            error: *mut c_char,
            len: usize,
        ) -> *mut c_void;
        fn sp_ai_model_close(handle: *mut c_void);
        fn sp_enhance_predict(
            handle: *mut c_void,
            inputs: *const FfiTensor,
            input_count: usize,
            outputs: *mut FfiTensor,
            output_count: usize,
            error: *mut c_char,
            len: usize,
        ) -> i32;
    }
    struct Session {
        manifest: Manifest,
        handle: *mut c_void,
    }
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe { sp_ai_model_close(self.handle) };
        }
    }
    type Stamp = Vec<(PathBuf, u64, Option<SystemTime>)>;
    fn stamp(path: &Path, result: &mut Stamp) -> Result<(), String> {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Native model symlinks are not supported".into());
        }
        if metadata.is_dir() {
            for item in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                stamp(&item.map_err(|e| e.to_string())?.path(), result)?;
            }
        } else {
            result.push((path.into(), metadata.len(), metadata.modified().ok()));
        }
        Ok(())
    }
    struct Entry {
        path: PathBuf,
        stamp: Stamp,
        units: ComputeUnits,
        session: Rc<Session>,
    }
    thread_local! { static SESSIONS: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) }; }
    pub fn clear() {
        SESSIONS.with(|s| s.borrow_mut().clear());
    }
    pub fn predict(
        bundle: &Path,
        inputs: Vec<Tensor>,
        units: ComputeUnits,
    ) -> Result<Vec<Tensor>, String> {
        let path = bundle.canonicalize().map_err(|e| e.to_string())?;
        let mut revision = Vec::new();
        stamp(&path, &mut revision)?;
        revision.sort_by(|a, b| a.0.cmp(&b.0));
        let session = SESSIONS.with(|slot| -> Result<Rc<Session>, String> {
            let mut cache = slot.borrow_mut();
            if let Some(index) = cache
                .iter()
                .position(|e| e.path == path && e.stamp == revision && e.units == units)
            {
                let entry = cache.remove(index);
                let session = entry.session.clone();
                cache.push(entry);
                return Ok(session);
            }
            cache.retain(|e| e.path != path);
            if cache.len() >= 3 {
                cache.remove(0);
            }
            let manifest = Manifest::read(&path)?;
            manifest.verify_files(&path)?;
            let model = CString::new(path.join(&manifest.model).as_os_str().as_encoded_bytes())
                .map_err(|e| e.to_string())?;
            let mut error = [0i8; 1024];
            let handle = unsafe {
                sp_ai_model_open(
                    model.as_ptr(),
                    units as i32,
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if handle.is_null() {
                return Err(unsafe { CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned());
            }
            let session = Rc::new(Session { manifest, handle });
            cache.push(Entry {
                path,
                stamp: revision,
                units,
                session: session.clone(),
            });
            Ok(session)
        })?;
        if inputs.iter().map(|input| input.values.len()).sum::<usize>() > 48_000_000
            || inputs.len() != session.manifest.inputs.len()
            || inputs.iter().zip(&session.manifest.inputs).any(|(a, b)| {
                !b.accepts(&a.spec)
                    || a.values.len() != a.spec.count().unwrap_or(0)
                    || a.values.iter().any(|v| !v.is_finite())
            })
        {
            return Err("Native model input differs from manifest".into());
        }
        let mut outputs: Vec<_> = session
            .manifest
            .outputs
            .iter()
            .map(|spec| Tensor {
                spec: spec.clone(),
                values: vec![0.; spec.count().expect("validated manifest")],
            })
            .collect();
        let names: Vec<_> = inputs
            .iter()
            .chain(&outputs)
            .map(|t| CString::new(t.spec.name.as_str()).expect("validated name"))
            .collect();
        let ffi_inputs: Vec<_> = inputs
            .iter()
            .zip(&names)
            .map(|(t, n)| FfiTensor {
                name: n.as_ptr(),
                shape: t.spec.shape.as_ptr(),
                rank: t.spec.shape.len(),
                data: t.values.as_ptr().cast_mut(),
                count: t.values.len(),
            })
            .collect();
        let mut ffi_outputs: Vec<_> = outputs
            .iter_mut()
            .zip(names.iter().skip(inputs.len()))
            .map(|(t, n)| FfiTensor {
                name: n.as_ptr(),
                shape: t.spec.shape.as_ptr(),
                rank: t.spec.shape.len(),
                data: t.values.as_mut_ptr(),
                count: t.values.len(),
            })
            .collect();
        let mut error = [0i8; 1024];
        if unsafe {
            sp_enhance_predict(
                session.handle,
                ffi_inputs.as_ptr(),
                ffi_inputs.len(),
                ffi_outputs.as_mut_ptr(),
                ffi_outputs.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        } == 0
        {
            return Err(unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .into_owned());
        }
        if outputs
            .iter()
            .any(|t| t.values.iter().any(|v| !v.is_finite()))
        {
            return Err("Native model returned nonfinite pixels".into());
        }
        Ok(outputs)
    }
}

pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn sp_enhance_available() -> i32;
        }
        unsafe { sp_enhance_available() != 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn clear_sessions() {
    #[cfg(target_os = "macos")]
    macos::clear();
}
pub fn predict(
    bundle: &Path,
    inputs: Vec<Tensor>,
    units: ComputeUnits,
) -> Result<Vec<Tensor>, String> {
    #[cfg(target_os = "macos")]
    {
        macos::predict(bundle, inputs, units)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (bundle, inputs, units);
        Err("Native enhancement requires macOS; use the optional portable runtime".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flexible_tensor_contracts_validate_actual_dimensions_and_maximum_memory() {
        let spec = TensorSpec {
            name: "points".into(),
            shape: vec![1, 1, 2, 2],
            flexible_axes: BTreeMap::from([(2, [1, 32])]),
        };
        assert_eq!(spec.maximum_count().unwrap(), 64);
        for count in [1, 4, 32] {
            let mut actual = spec.clone();
            actual.shape[2] = count;
            assert!(spec.accepts(&actual));
        }
        for count in [0, 33] {
            let mut actual = spec.clone();
            actual.shape[2] = count;
            assert!(!spec.accepts(&actual));
        }
        let oversized = TensorSpec {
            name: "huge".into(),
            shape: vec![1, 2_000_000],
            flexible_axes: BTreeMap::from([(0, [1, 32])]),
        };
        assert!(oversized.count().is_ok());
        assert!(
            oversized.maximum_count().is_err(),
            "variable axes must not bypass tensor memory limits"
        );
        let manifest:Manifest=serde_json::from_value(serde_json::json!({"version":1,"id":"test","task":"test","model":"model.mlmodelc","license":"MIT","source":"test","revision":"test","files":{"model.mlmodelc/weight.bin":"0".repeat(64)},"inputs":[{"name":"a","shape":[1,1000000],"flexible_axes":{"0":[1,32]}},{"name":"b","shape":[1,1000000],"flexible_axes":{"0":[1,32]}}],"outputs":[{"name":"out","shape":[1]}]})).unwrap();
        assert!(
            manifest.validate().is_err(),
            "aggregate maximum variable tensors exceed 48M floats"
        );
    }

    #[test]
    fn rejects_tensor_overflow_and_nonfinite_values() {
        for shape in [vec![usize::MAX, 2], vec![1, 0, 2], vec![32_000_001], vec![]] {
            assert!(TensorSpec {
                name: "pixels".into(),
                shape,
                flexible_axes: BTreeMap::new(),
            }
            .count()
            .is_err());
        }
        let spec = TensorSpec {
            name: "pixels".into(),
            shape: vec![1, 2],
            flexible_axes: BTreeMap::new(),
        };
        assert!(Tensor::new(spec.clone(), vec![0., f32::NAN]).is_err());
        assert!(Tensor::new(spec.clone(), vec![0.]).is_err());
        assert!(Tensor::new(spec, vec![0., 1.]).is_ok());
        assert!(!relative("../model.mlmodelc"));
        assert!(!relative("/model.mlmodelc"));
    }
}
