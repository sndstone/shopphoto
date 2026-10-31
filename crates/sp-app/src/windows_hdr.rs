//! Windows DX12 HDR display capability query.

use windows::core::Interface;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};

/// Largest attached-output scRGB headroom relative to 203-nit diffuse white.
pub fn display_headroom() -> f32 {
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return 1.0;
        };
        let mut best = 1.0f32;
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                if let Ok(output) = output.cast::<IDXGIOutput6>() {
                    if let Ok(description) = output.GetDesc1() {
                        if description.AttachedToDesktop.as_bool() && description.BitsPerColor >= 10
                        {
                            best = best.max(description.MaxLuminance / 203.0);
                        }
                    }
                }
                output_index += 1;
            }
            adapter_index += 1;
        }
        best.max(1.0)
    }
}
