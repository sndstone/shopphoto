//! GPU device/queue handles.
//!
//! In the app, the device comes from eframe's shared wgpu render state (one
//! device for UI and pipeline — required so the pipeline's output texture
//! can be sampled by egui directly). Tests use [`GpuContext::headless`].

use sp_core::SpError;
use std::sync::{Arc, Mutex};

/// One-shot signal installed on the shared device. The app consumes it and
/// restarts the graphics process so wgpu, the surface, and every resource are
/// recreated together.
#[derive(Clone, Default)]
pub struct DeviceLossMonitor {
    message: Arc<Mutex<Option<String>>>,
}

impl DeviceLossMonitor {
    pub fn install(&self, device: &wgpu::Device) {
        let monitor = self.clone();
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                monitor.mark(format!("{reason:?}: {message}"));
            }
        });
    }

    pub fn take(&self) -> Option<String> {
        self.message.lock().ok()?.take()
    }

    fn mark(&self, message: String) {
        if let Ok(mut slot) = self.message.lock() {
            *slot = Some(message);
        }
    }

    #[doc(hidden)]
    pub fn simulate(&self, message: impl Into<String>) {
        self.mark(message.into());
    }
}

/// Cloneable handles to the wgpu device and queue.
#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Known only when this crate created the adapter ([`GpuContext::headless`]).
    /// The app's context comes from eframe, which owns adapter selection.
    adapter_info: Option<wgpu::AdapterInfo>,
}

impl GpuContext {
    /// Platform defaults avoid loading unrelated GPU drivers during enumeration.
    /// SHOPPHOTO_GPU_BACKEND (or WGPU_BACKEND) may name comma-separated backends.
    pub fn configured_backends() -> Result<wgpu::Backends, SpError> {
        let configured = std::env::var("SHOPPHOTO_GPU_BACKEND")
            .or_else(|_| std::env::var("WGPU_BACKEND"))
            .ok();
        Self::parse_backends(configured.as_deref())
    }

    fn parse_backends(value: Option<&str>) -> Result<wgpu::Backends, SpError> {
        let Some(value) = value else {
            return Ok(if cfg!(target_os = "macos") {
                wgpu::Backends::METAL
            } else if cfg!(target_os = "windows") {
                wgpu::Backends::DX12
            } else {
                wgpu::Backends::VULKAN
            });
        };
        let mut backends = wgpu::Backends::empty();
        for name in value.split(',').map(str::trim) {
            backends |= match name.to_ascii_lowercase().as_str() {
                "auto" | "all" => wgpu::Backends::all(),
                "metal" => wgpu::Backends::METAL,
                "dx12" => wgpu::Backends::DX12,
                "vulkan" => wgpu::Backends::VULKAN,
                "gl" => wgpu::Backends::GL,
                _ => {
                    return Err(SpError::Gpu(format!(
                        "Unknown GPU backend {name:?}; use metal, dx12, vulkan, gl or auto"
                    )))
                }
            };
        }
        Ok(backends)
    }

    /// Bound synchronous worker waits so a lost driver cannot hang an export.
    pub fn wait_for_gpu(&self) -> Result<(), SpError> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .map_err(|error| SpError::Gpu(format!("GPU wait failed: {error}")))?;
        Ok(())
    }

    /// Map a submitted readback, checking both the GPU wait and map callback.
    /// A failure cancels the pending mapping; callers must not read its range.
    pub fn map_read_buffer(&self, buffer: &wgpu::Buffer) -> Result<(), SpError> {
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        let result = self.wait_for_gpu().and_then(|()| {
            receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .map_err(|error| SpError::Gpu(format!("GPU map callback failed: {error}")))?
                .map_err(|error| SpError::Gpu(format!("GPU readback failed: {error}")))
        });
        if result.is_err() {
            buffer.unmap();
        }
        result
    }

    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            adapter_info: None,
        }
    }

    /// True when this context is known to run on a software rasterizer
    /// (lavapipe, WARP). Absolute timings there are meaningless, so the
    /// perf-budget test skips rather than reporting a false regression.
    /// `false` when the adapter is unknown.
    #[must_use]
    pub fn is_software(&self) -> bool {
        self.adapter_info
            .as_ref()
            .is_some_and(|i| i.device_type == wgpu::DeviceType::Cpu)
    }

    /// Whether this device can run the `f16` shader variants (BUILD.md
    /// §10.2). Every Apple GPU reports it; adapters that do not get the
    /// f32 fallback compiled from the same source.
    #[must_use]
    pub fn supports_f16(&self) -> bool {
        self.device.features().contains(wgpu::Features::SHADER_F16)
    }

    /// Features every device path must request: `SHADER_F16` when the
    /// adapter has it. Both device-creation sites (here and the app's
    /// eframe descriptor) go through this so they cannot drift.
    #[must_use]
    pub fn preferred_features(adapter_features: wgpu::Features) -> wgpu::Features {
        adapter_features & wgpu::Features::SHADER_F16
    }

    /// Create a headless context on any available adapter. Returns an error
    /// when the machine/CI runner has no usable GPU — callers in tests
    /// should skip, the app treats it as fatal (BUILD.md §2).
    pub fn headless() -> Result<Self, SpError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: Self::configured_backends()?,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|e| SpError::NoGpu(e.to_string()))?;
        // Match the app's device config: full-sensor CFA textures are wider
        // than the 8192 default (X2D II: 11904), so request the adapter's
        // real 2D-texture limit.
        let adapter_limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("sp-gpu headless"),
            required_features: Self::preferred_features(adapter.features()),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: adapter_limits.max_texture_dimension_2d.max(8192),
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }))
        .map_err(|e| SpError::NoGpu(e.to_string()))?;
        Ok(Self {
            device,
            queue,
            adapter_info: Some(adapter.get_info()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_loss_signal_is_one_shot() {
        let monitor = DeviceLossMonitor::default();
        monitor.simulate("validation-scope simulated loss");
        assert_eq!(
            monitor.take().as_deref(),
            Some("validation-scope simulated loss")
        );
        assert!(monitor.take().is_none());
    }

    #[test]
    fn backend_override_is_explicit_and_rejects_typos_or_cpu_fallback() {
        assert_eq!(
            GpuContext::parse_backends(Some("dx12, vulkan")).unwrap(),
            wgpu::Backends::DX12 | wgpu::Backends::VULKAN
        );
        for bad in ["", "off", "metla"] {
            assert!(GpuContext::parse_backends(Some(bad)).is_err());
        }
    }
}
