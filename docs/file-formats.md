# File-format support

Extension acceptance means ShopPhoto will attempt a decode. Camera model,
compression mode, pixel layout and operating-system codec availability still
matter; this is not a claim that every camera/mode has been tested.

## Input

| Input family | Accepted extensions | Capability and boundaries |
|---|---|---|
| Hasselblad / Phase One | 3fr, fff, iiq | Bayer RAW through rawler. Local X2D II, X1D II and Phocus FFF samples have been decoded. |
| Canon | cr2, cr3, crm, crw | Bayer RAW; CR3/CRX decoder exists. Camera and compression modes need representative fixtures. |
| Nikon | nef, nrw | Bayer RAW; support depends on rawler's camera/mode database. |
| Sony | arw | Bayer RAW, including decoder paths for compressed/uncompressed/lossless modes. Reduced-size three-channel ARW is unsupported. |
| Fujifilm | raf | Bayer only. X-Trans sensors are not supported yet. |
| Adobe DNG | dng | Single-channel Bayer mosaic. Linear/three-channel DNG, including Apple ProRAW, is unsupported. Compression variants depend on rawler; JPEG XL compressed DNG needs broader fixture coverage. |
| Other camera RAW | ari, dcr, dcs, erf, kdc, mef, mos, mrw, orf, ori, pef, qtk, raw, rw2, rwl, srw, x3f | Extensions are recognized; decoding still requires a supported Bayer sensor layout and rawler camera/mode. Non-Bayer layouts (including Foveon) are unsupported. |
| Ordinary rendered RGB | jpg, jpeg, png, tif, tiff, webp, bmp | Color-managed raster input; integer 8/16-bit where the format supports it. Embedded RGB ICC is honored; untagged input defaults to sRGB. |
| JPEG XL | jxl | Pure Rust decoder, including high-precision input and codestream orientation. |
| HEIF / HEIC / AVIF on macOS | heif, heic, avif | Available in every macOS build through ImageIO. Full-resolution RGB16 decode retains the decoded image color space/ICC and applies orientation; grid previews use system thumbnails. Actual codec support depends on the installed macOS version. A native-codecs build tries libheif first and falls back to ImageIO. |
| HEIF / HEIC / AVIF on Linux/Windows | heif, heic, avif | Requires the optional `native-codecs` feature, libheif and suitable decoder plugins. |
| Float TIFF | tif, tiff | RGB32F requires an embedded RGB ICC profile. Float16 is unsupported. |

Raster pixels enter the develop pipeline as linear Rec.2020. Non-RGB ICC,
transparency and non-finite samples are rejected. Raster sources are limited
to 150 MP with pixel-allocation limits. TIFF reads one image; unprofiled CMYK
conversion and unusual tiled/planar variants have limited fixture coverage.
HLG needs an explicit display-reference interpretation and is rejected by the
shared color conversion. The ImageIO integer path uses the system's primary
rendition; it does not reconstruct auxiliary HDR gain maps or promise preservation
of their highlight headroom. JPEG Ultra HDR input currently opens its SDR base.
PSD input and sensor layouts other than the four Bayer patterns are unsupported.
Mirrored RAW orientations currently lose the reflection; raster orientations
include reflections.

## Output

| Output | Precision / color |
|---|---|
| JPEG | SDR 8-bit, optional 4:4:4 |
| TIFF | RGB 8/16-bit, optional compression |
| PNG | RGB 8/16-bit; 16-bit PQ+cICP for HDR |
| WebP | SDR 8-bit, lossy or lossless |
| AVIF | SDR or PQ HDR, 10-bit payload |
| JPEG XL | 16-bit lossy/lossless, SDR or PQ HDR |
| Ultra HDR JPEG | SDR JPEG plus gain map and container metadata |
| Linear master TIFF | Separate RGB32F master-writing API; not a normal export format |

Rendered outputs support sRGB, Display P3 and Rec.2020 with output ICC profiles;
HDR uses Rec.2100 PQ signaling where applicable. HEIC, DNG and PSD export are
not implemented. Native HDR/EDR preview uses float scRGB on macOS/Windows;
Linux uses an SDR viewport while HDR export remains available.

## Sidecars and metadata

`.spedit` appends to the original filename and is the source of truth for full
edits. XMP reading recognizes extension-replaced `IMG.xmp` and full-name
`IMG.ARW.xmp`, plus uppercase `.XMP` on case-sensitive volumes. Saving keeps
an existing convention; new files use extension-replaced lowercase `.xmp`.
When multiple sidecars exist, extension-replaced naming wins (lowercase first)
and ambiguity is logged. Same-stem RAW/JPEG files can consequently share an
extension-replaced XMP; full-name sidecars distinguish them.

XMP mirrors stars/rejects, pick flags in ShopPhoto's namespace, the five English
color labels, captions, keywords (including hierarchy), creator, copyright,
rights, contact and GPS. Writes are opt-in, preserve foreign edit XML and refuse
conflicting external changes. Develop/crop settings from other applications
are retained as foreign XML, not applied. Custom/localized labels and language
alternatives do not have full fidelity. Sidecar metadata is adopted when no
ShopPhoto sidecar exists; embedded descriptive XMP/IPTC is not imported.

Export reconstructs selected capture EXIF and descriptive XMP rather than
preserving arbitrary EXIF/IPTC/XMP or MakerNotes. Output dimensions/orientation
and ICC describe the rendered image. Capture/descriptive metadata controls and
GPS stripping apply separately. Check metadata-sensitive delivery in the target
application; cross-application round trips have limited real-app coverage.

Custom proof profiles must be display-class RGB ICCs; printer/device-link
profiles are rejected. See [ShopPhoto RAW Denoise](raw-denoise.md) for its
model, supported input and installation details.
