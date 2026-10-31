# Platform and hardware capability reporting

Tools → System & photo capabilities reports the active GPU, backend, surface
format, texture/buffer limits, current proxy size, tracked texture estimate,
compiled native-codec support and loaded profile counts. The texture estimate
does not represent total process memory or AI/driver/export allocations.

The existing macOS Metal EDR and Windows DX12 paths remain. Linux now has an
explicit experimental Wayland/Vulkan path:

```sh
SHOPPHOTO_LINUX_HDR=1 shopphoto --hdr-check
```

This opts into Vulkan and asks for float scRGB only if the surface advertises
it. The bundled wgpu 29 Vulkan backend maps `Rgba16Float` exclusively from
`EXTENDED_SRGB_LINEAR_EXT` and creates that color-space swapchain. If a compatible
Vulkan surface offers only SDR, the normal SDR format is retained. Systems without
a Vulkan adapter should use the default launch path. `SHOPPHOTO_SDR=1` disables
the opt-in. X11 is not opted into automatically.

No display peak is inferred from the availability of a float format. The Linux
UI and HDR check report unknown compositor headroom and experimental support.
The compositor/display must be configured for HDR, and visible HDR requires
hardware acceptance on that specific system. This change was developed on macOS;
Linux/Windows builds and physical HDR behavior are not claimed as locally tested.
The repository's CI matrix retains macOS, Windows and Linux compile/test checks.

Ordinary raster decoding is capped at 150 MP. Multi-photo operations cap inputs
at 150 MP each and 320 MP combined; panorama output is capped at 150 MP. Exports
use the existing tiled path. The adapter's actual texture limit is requested,
including adapters below 8192 pixels, instead of raising the request above what
the adapter reports. Detailed mask/retouch allocation constraints and regressions
are recorded in `phase-local-masks.md`.

AVIF/HEIF input remains a `native-codecs` build option that requires libheif and
appropriate decoder plugins. Default JPEG/PNG/TIFF/WebP/BMP/JPEG XL input does not
require them. Optional codec packaging must include its runtime dependencies.
AI runtime readiness is described in `ai-runtime.md`; RAW denoise input and
model details are described in `raw-denoise.md`.
