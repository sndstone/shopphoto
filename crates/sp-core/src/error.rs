//! Workspace-wide error type.
//!
//! Library crates return `Result<T, SpError>`; the `sp-app` binary converts
//! to `anyhow` at its boundary. Add variants here rather than inventing
//! per-crate error enums, unless a crate genuinely needs private detail.

/// Errors produced by ShopPhoto library crates.
#[derive(Debug, thiserror::Error)]
pub enum SpError {
    /// The file could not be read from disk.
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The RAW decoder rejected the file (corrupt, truncated, or unsupported).
    #[error("cannot decode {path}: {reason}")]
    Decode { path: String, reason: String },

    /// The file decoded but is from a camera we have no color data for.
    #[error("unsupported camera model: {model}")]
    UnsupportedCamera { model: String },

    /// No usable GPU adapter was found. Fatal at startup; there is no CPU
    /// fallback pipeline (BUILD.md §2).
    #[error("no compatible GPU adapter: {0}")]
    NoGpu(String),

    /// A GPU operation failed after initialization (device lost, OOM).
    #[error("gpu error: {0}")]
    Gpu(String),

    /// Sidecar or profile file exists but cannot be parsed. The caller must
    /// quarantine the file and fall back to defaults — never delete it.
    #[error("corrupt data file {path}: {reason}")]
    CorruptData { path: String, reason: String },

    /// A document from a newer app must remain untouched until it can be read.
    #[error("unsupported sidecar version {version} in {path}; this app supports up to {supported}; open it with a newer ShopPhoto")]
    UnsupportedSidecarVersion {
        path: String,
        version: u64,
        supported: u32,
    },

    /// Catalog database error.
    #[error("catalog error: {0}")]
    Catalog(String),

    /// Export/encode failure.
    #[error("export failed: {0}")]
    Export(String),
}

impl SpError {
    /// Convenience constructor for [`SpError::Io`].
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Convenience constructor for [`SpError::Decode`].
    pub fn decode(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Decode {
            path: path.into(),
            reason: reason.into(),
        }
    }
}
