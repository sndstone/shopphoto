//! The GPU develop-pipeline executor.
//!
//! Flow per BUILD.md §3.2/§3.5:
//!
//! ```text
//! open_image:  CFA u16 ──upload──► r16uint tex
//!              superpixel demosaic (fused raw prepare) ──► quarter-res camera RGB
//!              box downsample ──► PROXY source (camera RGB, ≤ PROXY_LONG_EDGE)
//! render:      proxy source ──wb_color(profile matrix + WB + exposure)──► working (Rec2020 f16)
//!              output transform ──► rgba8 (sRGB-encoded) shown by egui
//! ```
//!
//! The proxy is the interactive working image; the CFA remains resident for
//! full-resolution 1:1 and tiled exports. Tone, profile, NR, sharpen and
//! SDR/PQ output passes share the same execution chain at each resolution.

use crate::tone::{build_channel_lut, build_tone_lut};
use sp_core::color::{mat3_mul, Mat3, XYZ_TO_REC2020};
use sp_core::lens::LensProfile;
use sp_core::profile::{HueSatLut, Lut3d};
use sp_core::raw::CfaPattern;
use sp_core::wb::{cct_from_wb_coeffs, wb_gains_for_temp};
use sp_core::{EditState, ImageMeta, RawImage, SpError};
use sp_gpu::pipelines::{
    bind, bind_lookup, bind_retouch, bind_retouch_with_target, make_lookup_texture, make_texture,
    run, AhdParams, DefringeGpuParams, HistogramParams, LocalContrastGpuParams, MalvarParams,
    OutputParams, Pipelines, ProfileColorParams, RawParams, RetouchGpuParams, RetouchGpuRecord,
    SharpenGpuParams, SizeParams, ToneColorParams, WbColorParams, HISTOGRAM_BINS,
};
use sp_gpu::{GpuContext, StagingRing};
use wgpu::util::DeviceExt;

mod buffers;
mod correction;
mod export;
mod linear;
mod proxy;
mod raster;
mod retouch;
mod tile;

const DEFAULT_PROXY_LONG_EDGE: u32 = 2048;

fn cfa_id(p: CfaPattern) -> u32 {
    match p {
        CfaPattern::Rggb => 0,
        CfaPattern::Bggr => 1,
        CfaPattern::Grbg => 2,
        CfaPattern::Gbrg => 3,
    }
}

struct LoadedImage {
    resident_texture_bytes: u64,
    raster: Option<std::sync::Arc<sp_core::source::RasterImage>>,
    /// Proxy-res linear camera RGB — the render source for edit changes.
    proxy_src_view: wgpu::TextureView,
    proxy_src_tex: wgpu::Texture,
    /// Ping textures: working space (Rec2020 f16) at proxy res.
    work_view: wgpu::TextureView,
    work2_view: wgpu::TextureView,
    /// Third working texture for the optional NR hop (Phase 4).
    work_nr_view: wgpu::TextureView,
    /// scRGB float display output for Metal EDR (HDR viewport blit source).
    hdr_display_view: wgpu::TextureView,
    /// The texture behind it, kept for QA readback on the float surface.
    hdr_display_tex: wgpu::Texture,
    /// Final displayable rgba8 (sRGB-encoded values, Rgba8Unorm).
    out_tex: wgpu::Texture,
    out_view: wgpu::TextureView,
    proxy_w: u32,
    proxy_h: u32,
    /// Camera forward matrix, kept for CCT-based WB (temp/tint sliders).
    cam_to_xyz: Mat3,
    wb_coeffs: [f32; 4],
    /// Full-sensor CFA texture view + calibration, kept for 1:1 tile renders.
    cfa_view: wgpu::TextureView,
    cfa_pattern: CfaPattern,
    black_levels: [f32; 4],
    white_levels: [f32; 4],
    active: sp_core::raw::ActiveArea,
    /// Histogram bins (GPU) + tiny staging buffer for readback.
    hist_bins: wgpu::Buffer,
    hist_staging: wgpu::Buffer,
    hist_rx: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    /// Linear airlight estimate used by dehaze, refreshed with histogram readback.
    airlight: f32,
    /// Quarter-resolution superpixel output, retained so the proxy source
    /// can be rebuilt when lens shading changes.
    quarter_view: wgpu::TextureView,
    quarter_w: u32,
    quarter_h: u32,
    /// Which shading state is currently baked into `proxy_src_view`.
    lens_bake: LensBake,
}

/// A rendered 1:1 tile: which full-res region it covers and the margin
/// (in pixels) baked into the texture on each side that the display must
/// crop away via UV.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileInfo {
    /// Visible region origin in active-area coordinates.
    pub x: u32,
    pub y: u32,
    /// Visible region size.
    pub w: u32,
    pub h: u32,
    /// Left/top margin actually present in the texture.
    pub margin_x: u32,
    pub margin_y: u32,
    /// Full texture size (visible + margins on all sides, clamped at edges).
    pub tex_w: u32,
    pub tex_h: u32,
}

/// CPU handoff produced by the tiled export renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved sRGB-encoded RGBA8.
    pub rgba8: Vec<u8>,
    /// High-precision encoded RGBA, present for 16-bit export targets.
    pub rgba16: Option<Vec<u16>>,
}

/// Per-render GPU buffers for the color chain.
struct ColorBuffers {
    wb_params: wgpu::Buffer,
    lut_buf: wgpu::Buffer,
    channel_lut_buf: wgpu::Buffer,
    tone_params: wgpu::Buffer,
    profile_pre_params: wgpu::Buffer,
    profile_post_params: wgpu::Buffer,
    out_params: wgpu::Buffer,
    huesat_view: wgpu::TextureView,
    look_view: wgpu::TextureView,
    rgb_view: wgpu::TextureView,
    /// Final-stage display ICC simulation. A 1×1 dummy when inactive.
    proof_view: wgpu::TextureView,
    /// Chroma-plane warp grid bound alongside `wb_params`; a 1×1 unity
    /// texture when the profile carries no warp.
    warp_view: wgpu::TextureView,
}

struct RetouchBuffers {
    params: wgpu::Buffer,
    records: wgpu::Buffer,
    points: wgpu::Buffer,
}

#[derive(Clone, Copy)]
struct OutputSpec {
    size: [u32; 2],
    mode: u32,
    origin: [u32; 2],
    full_size: [u32; 2],
    view_transform: bool,
    export_space: Option<sp_core::color::OutputColorSpace>,
}

fn make_3d_texture(
    ctx: &GpuContext,
    label: &str,
    width: u32,
    height: u32,
    depth: u32,
    rgba: &[[half::f16; 4]],
) -> wgpu::TextureView {
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: depth,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(rgba),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 8),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: depth,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    })
}

fn upload_dcp_lut(
    ctx: &GpuContext,
    label: &str,
    lut: Option<&HueSatLut>,
) -> (wgpu::TextureView, [u32; 4]) {
    if let Some(lut) = lut.filter(|l| l.entries.len() == l.expected_len()) {
        let data: Vec<[half::f16; 4]> = lut
            .entries
            .iter()
            .map(|&(h, s, v)| {
                [
                    half::f16::from_f32(h),
                    half::f16::from_f32(s),
                    half::f16::from_f32(v),
                    half::f16::ZERO,
                ]
            })
            .collect();
        return (
            make_3d_texture(
                ctx,
                label,
                lut.sat_divisions,
                lut.hue_divisions,
                lut.val_divisions,
                &data,
            ),
            [lut.hue_divisions, lut.sat_divisions, lut.val_divisions, 1],
        );
    }
    let dummy = [[
        half::f16::ZERO,
        half::f16::ONE,
        half::f16::ONE,
        half::f16::ZERO,
    ]];
    (make_3d_texture(ctx, label, 1, 1, 1, &dummy), [1, 1, 1, 0])
}

fn upload_rgb_lut(ctx: &GpuContext, lut: Option<&Lut3d>) -> (wgpu::TextureView, [u32; 4]) {
    if let Some(lut) = lut.filter(|l| l.size >= 2 && l.entries.len() == l.expected_len()) {
        let data: Vec<[half::f16; 4]> = lut
            .entries
            .iter()
            .map(|&rgb| {
                [
                    half::f16::from_f32(rgb[0]),
                    half::f16::from_f32(rgb[1]),
                    half::f16::from_f32(rgb[2]),
                    half::f16::ONE,
                ]
            })
            .collect();
        return (
            make_3d_texture(ctx, "profile RGB LUT", lut.size, lut.size, lut.size, &data),
            [lut.size, 0, 0, 1],
        );
    }
    let dummy = [[half::f16::ZERO; 4]];
    (
        make_3d_texture(ctx, "profile RGB LUT", 1, 1, 1, &dummy),
        [1, 0, 0, 0],
    )
}

/// A lens shading profile resolved for the open image: the uploaded gain
/// grid plus the geometry the shaders index it with.
struct ResolvedLens {
    view: wgpu::TextureView,
    /// `[grid_w, grid_h, center_x, center_y]`, matching `RawParams.lens_grid`.
    grid: [f32; 4],
    spacing_px: f32,
    /// Bumped on every upload. The proxy source is built once at open with
    /// shading baked in, so it must be rebuilt when this changes.
    generation: u64,
    distortion: [f32; 3],
    lateral_ca: [f32; 2],
}

/// Identifies the shading state baked into the proxy source. When the
/// current state differs, `render` rebuilds the source before using it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LensBake {
    generation: u64,
    enabled: bool,
    /// Strength as raw bits so the key stays `Eq`.
    strength: u32,
}

struct TileResources {
    work_nr: wgpu::TextureView,
    work_nr_tex: wgpu::Texture,
    prepared: wgpu::TextureView,
    /// Second mosaic-domain texture for the pre-demosaic NR hop.
    prepared_nr: wgpu::TextureView,
    demosaiced: wgpu::TextureView,
    demosaiced_tex: wgpu::Texture,
    work: wgpu::TextureView,
    work_tex: wgpu::Texture,
    work2: wgpu::TextureView,
    work2_tex: wgpu::Texture,
    scene_output: Option<wgpu::Texture>,
    out_tex: wgpu::Texture,
    out_view: wgpu::TextureView,
    hdr_display_view: wgpu::TextureView,
    hdr_display_tex: wgpu::Texture,
    out16_tex: wgpu::Texture,
    out16_view: wgpu::TextureView,
    tex_w: u32,
    tex_h: u32,
}

struct PreviewBase {
    key: EditState,
    nr: [f32; 4],
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub struct GpuRenderer {
    ctx: GpuContext,
    pipes: std::sync::Arc<Pipelines>,
    image: Option<LoadedImage>,
    preview_base: Option<PreviewBase>,
    preview_cache_enabled: bool,
    preview_cache_hits: u64,
    preview_cache_misses: u64,
    tile: Option<TileResources>,
    /// scRGB display output requested (float EDR surface present).
    hdr_display: bool,
    /// Override camera profile (Tier 2 DCP / Tier 3 Reference Match). `None`
    /// = Tier 1 (embedded matrix). Part of every render's dirty key via
    /// `EditState.profile`, so the app sets this before rendering.
    profile: Option<sp_core::profile::CameraProfile>,
    /// Imported `.cube` selected by the app; kept outside EditState so
    /// sidecars store a stable name instead of tens of thousands of values.
    custom_look: Option<(String, sp_core::cube::CubeLut)>,
    custom_proof: Option<crate::proof::CustomProof>,
    /// Lens shading for the open image. `None` = no matching profile, which
    /// is not an error: the passes bind a 1×1 unity grid and skip the work.
    lens: Option<ResolvedLens>,
    /// 1×1 unity textures bound when lens shading / the chroma warp are
    /// inactive, so every bind group stays complete.
    unity_lookup: wgpu::TextureView,
    /// Monotonic counter behind `ResolvedLens::generation`.
    lens_generation: u64,
    proxy_long_edge: u32,
    /// Capture metadata used to resolve the allow-listed, ISO-dependent
    /// ShopPhoto camera-aware tuning. Generic RAWs leave this empty.
    capture_meta: Option<ImageMeta>,
    /// The source CFA has already been processed by the optional neural RAW
    /// denoiser. Avoid applying the interactive fallback NR a second time.
    external_raw_denoise: bool,
    proof_space: sp_core::edit::ProofSpace,
    gamut_warning: bool,
    clipping_mode: u32,
    sharpen_mask_preview: bool,
    mask_preview_index: Option<usize>,
    mask_preview_texture: Option<wgpu::Texture>,
    mask_preview_view: Option<wgpu::TextureView>,
    canonical_mask_input: bool,
    analysis_enabled: bool,
    analysis_long_edge: u32,
    analysis_texture: Option<wgpu::Texture>,
    /// Persistently mapped upload ring for the full-sensor CFA (BUILD.md
    /// §10.1) — allocated once, recycled for every image opened.
    staging: StagingRing,
}

/// The app builds renderers on worker threads and adopts them on the UI
/// thread; keep that possible.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<GpuRenderer>();
};

impl GpuRenderer {
    pub fn new(ctx: GpuContext) -> Result<Self, SpError> {
        let pipes = std::sync::Arc::new(Pipelines::new(&ctx)?);
        Self::with_pipelines(ctx, pipes)
    }

    /// Compiled pipelines shared with this renderer, so a second renderer
    /// on the same device (export, denoise comparison, a background image
    /// open) skips shader compilation.
    #[must_use]
    pub fn shared_pipelines(&self) -> std::sync::Arc<Pipelines> {
        self.pipes.clone()
    }

    /// Build a renderer around already compiled pipelines. Safe to call
    /// from a worker thread; the renderer is `Send`, so the app can build
    /// one off the UI thread and adopt it once the image is uploaded.
    pub fn with_pipelines(
        ctx: GpuContext,
        pipes: std::sync::Arc<Pipelines>,
    ) -> Result<Self, SpError> {
        let staging = StagingRing::new(&ctx);
        let unity_lookup = make_lookup_texture(&ctx, "unity lookup", 1, 1, &[[1.0; 4]]);
        Ok(Self {
            staging,
            ctx,
            pipes,
            image: None,
            preview_base: None,
            preview_cache_enabled: true,
            preview_cache_hits: 0,
            preview_cache_misses: 0,
            hdr_display: false,
            tile: None,
            profile: None,
            custom_look: None,
            custom_proof: None,
            lens: None,
            unity_lookup,
            lens_generation: 0,
            proxy_long_edge: DEFAULT_PROXY_LONG_EDGE,
            capture_meta: None,
            external_raw_denoise: false,
            proof_space: sp_core::edit::ProofSpace::DisplayNative,
            gamut_warning: false,
            clipping_mode: 0,
            sharpen_mask_preview: false,
            mask_preview_index: None,
            mask_preview_texture: None,
            mask_preview_view: None,
            canonical_mask_input: false,
            analysis_enabled: false,
            analysis_long_edge: 512,
            analysis_texture: None,
        })
    }

    pub fn set_analysis_enabled(&mut self, enabled: bool) {
        self.analysis_enabled = enabled;
    }
    /// Bounded scene-linear proxy after local edits and sharpening, before
    /// lens/output encoding. The UI calls this only after pointer interaction.
    pub fn read_analysis_frame(&self) -> Result<Option<sp_core::source::RasterImage>, SpError> {
        let Some(texture) = &self.analysis_texture else {
            return Ok(None);
        };
        let width = texture.width();
        let height = texture.height();
        let pixels = self.read_rgba16_texture(texture, width, height)?;
        export::validate_rgba16(&pixels, true, true)?;
        Ok(Some(sp_core::source::RasterImage {
            width,
            height,
            linear_rec2020: pixels
                .chunks_exact(4)
                .map(|v| std::array::from_fn(|c| half::f16::from_bits(v[c]).to_f32()))
                .collect(),
            bits_per_channel: 16,
            embedded_icc: None,
        }))
    }

    pub fn lens_coordinate_map(&self, edit: &EditState) -> sp_core::geometry::LensMap {
        let profile = self.lens.as_ref().filter(|_| {
            self.image
                .as_ref()
                .is_some_and(|image| image.raster.is_none())
        });
        sp_core::geometry::LensMap {
            distortion: if edit.lens.distortion == [0.0; 3] {
                profile.map_or([0.0; 3], |lens| lens.distortion)
            } else {
                edit.lens.distortion
            },
            lateral_ca: if edit.lens.lateral_ca == [0.0; 2] {
                profile.map_or([0.0; 2], |lens| lens.lateral_ca)
            } else {
                edit.lens.lateral_ca
            },
            enabled: edit.lens.enable && !self.canonical_mask_input,
            crop_to_valid: edit.lens.crop_to_valid,
        }
    }

    /// Render exact local coverage without changing the developed output.
    pub fn set_mask_preview(&mut self, index: Option<usize>) {
        self.mask_preview_index = index;
    }

    pub fn mask_preview_view(&self) -> Option<&wgpu::TextureView> {
        self.mask_preview_view.as_ref()
    }

    /// Inference mattes belong to pre-lens image coordinates. Shading and
    /// fringe correction still run, but the final geometric warp is bypassed.
    pub fn set_canonical_mask_input(&mut self, enabled: bool) {
        self.canonical_mask_input = enabled;
    }

    pub fn read_mask_preview(&self) -> Result<Option<sp_core::mask::AlphaMask>, SpError> {
        let Some(texture) = &self.mask_preview_texture else {
            return Ok(None);
        };
        let width = texture.width();
        let height = texture.height();
        let pixels = self.read_rgba16_texture(texture, width, height)?;
        Ok(Some(sp_core::mask::AlphaMask {
            width,
            height,
            alpha: pixels
                .chunks_exact(4)
                .map(|pixel| {
                    (half::f16::from_bits(pixel[0]).to_f32().clamp(0.0, 1.0) * 65535.0).round()
                        as u16
                })
                .collect(),
            source: "Local coverage preview".into(),
            inverted: false,
            repairs: vec![],
        }))
    }

    /// Set the proxy budget before opening an image. The app supplies at
    /// most twice the physical viewport edge, avoiding invisible pixels.
    pub fn set_proxy_long_edge(&mut self, edge: u32) {
        self.proxy_long_edge = edge.max(1);
    }

    /// Set capture metadata before rendering an image. The GPU denoise
    /// fallback only overrides manual NR for sensors it has tuning for.
    pub fn set_capture_metadata(&mut self, meta: Option<&ImageMeta>) {
        self.preview_base = None;
        self.capture_meta = meta.cloned();
    }

    pub fn set_external_raw_denoise(&mut self, enabled: bool) {
        if self.external_raw_denoise != enabled {
            self.preview_base = None;
        }
        self.external_raw_denoise = enabled;
    }

    /// Enable the extra scRGB display output (Metal EDR surface active).
    pub fn set_hdr_display(&mut self, enabled: bool) {
        self.hdr_display = enabled;
    }

    /// Set non-editing preview options. Export calls always use their own
    /// explicit output transform and ignore these values.
    pub fn set_view_options(
        &mut self,
        proof_space: sp_core::edit::ProofSpace,
        gamut_warning: bool,
        clipping_mode: u32,
    ) {
        self.proof_space = proof_space;
        self.gamut_warning = gamut_warning;
        self.clipping_mode = clipping_mode.min(3);
    }

    /// Set the imported display ICC used when `ProofSpace::Custom` is active.
    pub fn set_custom_proof(&mut self, proof: Option<crate::proof::CustomProof>) {
        self.custom_proof = proof;
    }

    /// Transient Alt-preview for the sharpening edge mask.
    pub fn set_sharpen_mask_preview(&mut self, enabled: bool) {
        self.sharpen_mask_preview = enabled;
    }

    /// scRGB display texture view; valid after `render` when HDR display is
    /// enabled and the edit has HDR on.
    pub fn hdr_display_view(&self) -> Option<&wgpu::TextureView> {
        self.image.as_ref().map(|i| &i.hdr_display_view)
    }

    /// Set the active camera profile override (`None` = embedded Tier 1).
    pub fn set_profile(&mut self, profile: Option<sp_core::profile::CameraProfile>) {
        if self.profile != profile {
            self.preview_base = None;
        }
        self.profile = profile;
    }

    pub fn set_custom_look(&mut self, look: Option<(String, sp_core::cube::CubeLut)>) {
        if self.custom_look != look {
            self.preview_base = None;
        }
        self.custom_look = look;
    }

    /// Upload the lens shading grid for the open image, interpolated to the
    /// shot's `f_number`.
    ///
    /// `None` — or a structurally invalid profile — clears the correction,
    /// which is the normal state for an uncalibrated lens and never an
    /// error. Call this whenever the matched profile or the aperture
    /// changes; the proxy source rebuilds itself on the next render.
    pub fn set_lens_profile(&mut self, profile: Option<&LensProfile>, f_number: Option<f32>) {
        let Some(profile) = profile.filter(|p| p.is_valid()) else {
            self.lens = None;
            return;
        };
        let gains = profile.gains_for_aperture(f_number);
        self.lens_generation += 1;
        self.lens = Some(ResolvedLens {
            view: make_lookup_texture(
                &self.ctx,
                "lens shading",
                profile.grid_w,
                profile.grid_h,
                &gains,
            ),
            grid: [
                profile.grid_w as f32,
                profile.grid_h as f32,
                profile.center_x,
                profile.center_y,
            ],
            spacing_px: profile.spacing_px,
            generation: self.lens_generation,
            distortion: profile.distortion,
            lateral_ca: profile.lateral_ca,
        });
    }

    /// Whether a lens shading grid is loaded for the open image.
    #[must_use]
    pub fn has_lens_profile(&self) -> bool {
        self.lens.is_some()
    }

    /// Active-area dimensions of the open image.
    pub fn active_size(&self) -> Option<(u32, u32)> {
        self.image
            .as_ref()
            .map(|i| (i.active.width, i.active.height))
    }

    /// Disable prefix retention for memory troubleshooting or equivalence QA.
    pub fn set_preview_cache_enabled(&mut self, enabled: bool) {
        self.preview_cache_enabled = enabled;
        if !enabled {
            self.preview_base = None;
        }
    }

    /// Completed prefix cache hits/misses for diagnostics and benchmarks.
    #[must_use]
    pub fn preview_cache_stats(&self) -> (u64, u64) {
        (self.preview_cache_hits, self.preview_cache_misses)
    }

    /// Approximate live texture bytes, used by the F12 diagnostics overlay.
    #[must_use]
    /// Texel payload of resident develop image/tile/coverage/analysis textures.
    /// Excludes allocation padding, LUT/lens lookups, transient patch textures,
    /// buffers/staging, other renderers, export/AI and CPU source/alpha storage.
    pub fn estimated_texture_bytes(&self) -> u64 {
        let image_bytes = self
            .image
            .as_ref()
            .map_or(0, |image| image.resident_texture_bytes);
        let tile_bytes = self
            .tile
            .as_ref()
            .map_or(0, |tile| u64::from(tile.tex_w) * u64::from(tile.tex_h) * 60);
        let extra = [
            self.mask_preview_texture.as_ref(),
            self.analysis_texture.as_ref(),
            self.preview_base.as_ref().map(|base| &base.texture),
        ]
        .into_iter()
        .flatten()
        .map(|texture| u64::from(texture.width()) * u64::from(texture.height()) * 8)
        .sum::<u64>();
        image_bytes + tile_bytes + extra
    }

    #[must_use]
    pub fn proxy_size(&self) -> Option<(u32, u32)> {
        self.image
            .as_ref()
            .map(|image| (image.proxy_w, image.proxy_h))
    }

    /// The displayable output texture, if an image is open.
    pub fn output(&self) -> Option<(&wgpu::Texture, u32, u32)> {
        self.image
            .as_ref()
            .map(|i| (&i.out_tex, i.proxy_w, i.proxy_h))
    }

    /// View of the displayable output texture.
    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.image.as_ref().map(|i| &i.out_view)
    }
}
