//! Explicit per-request Core ML hardware policy. CPU fallback is always allowed.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
pub enum ComputeUnits {
    #[default]
    Automatic = 0,
    CpuGpu = 1,
    CpuNeuralEngine = 2,
    CpuOnly = 3,
}

impl ComputeUnits {
    /// Compatibility policy for CLI callers that have no saved app preferences.
    pub fn from_environment() -> Self {
        Self::from_override(std::env::var("SHOPPHOTO_COREML_UNITS").ok().as_deref())
    }

    fn from_override(value: Option<&str>) -> Self {
        match value {
            Some("cpu_gpu") => Self::CpuGpu,
            Some("cpu_ane") => Self::CpuNeuralEngine,
            Some("cpu") => Self::CpuOnly,
            _ => Self::Automatic,
        }
    }

    pub const fn supported(self) -> bool {
        match self {
            Self::Automatic | Self::CpuOnly => true,
            Self::CpuGpu => cfg!(target_os = "macos"),
            Self::CpuNeuralEngine => cfg!(all(target_os = "macos", target_arch = "aarch64")),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic (recommended)",
            Self::CpuGpu => "GPU + CPU",
            Self::CpuNeuralEngine => "Neural Engine (NPU) + CPU",
            Self::CpuOnly => "CPU only",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_environment_policy_defaults_and_supported_devices() {
        assert_eq!(ComputeUnits::from_override(None), ComputeUnits::Automatic);
        assert_eq!(
            ComputeUnits::from_override(Some("all")),
            ComputeUnits::Automatic
        );
        assert_eq!(
            ComputeUnits::from_override(Some("unknown")),
            ComputeUnits::Automatic
        );
        assert_eq!(
            ComputeUnits::from_override(Some("cpu_ane")),
            ComputeUnits::CpuNeuralEngine
        );
        assert_eq!(
            ComputeUnits::from_override(Some("cpu_gpu")),
            ComputeUnits::CpuGpu
        );
        assert_eq!(
            ComputeUnits::from_override(Some("cpu")),
            ComputeUnits::CpuOnly
        );
        assert_eq!(
            ComputeUnits::CpuNeuralEngine.supported(),
            cfg!(all(target_os = "macos", target_arch = "aarch64"))
        );
    }
}
