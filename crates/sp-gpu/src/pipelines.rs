//! Compute pipelines for the develop passes.
//!
//! Every pass shares one bind-group shape:
//!   @binding(0) input texture (sampled, uint or float)
//!   @binding(1) output storage texture (format varies per pass)
//!   @binding(2) uniform params
//!
//! Pipelines are compiled once at startup; per-frame work is just bind
//! groups + dispatch. Uniform structs here MUST match the WGSL layouts in
//! `shaders/` field-for-field (all are 16-byte aligned by construction).

use crate::context::GpuContext;
use crate::{dispatch_size, WORKGROUP_DIM};
use sp_core::SpError;

/// Shader sources compiled into the binary. Names match
/// [`crate::PassId::shader_file`].
pub const SHADER_SOURCES: &[(&str, &str)] = &[
    (
        "demosaic_superpixel.wgsl",
        include_str!("../../../shaders/demosaic_superpixel.wgsl"),
    ),
    (
        "demosaic_malvar.wgsl",
        include_str!("../../../shaders/demosaic_malvar.wgsl"),
    ),
    (
        "demosaic_ahd.wgsl",
        include_str!("../../../shaders/demosaic_ahd.wgsl"),
    ),
    (
        "raw_prepare.wgsl",
        include_str!("../../../shaders/raw_prepare.wgsl"),
    ),
    (
        "wb_color.wgsl",
        include_str!("../../../shaders/wb_color.wgsl"),
    ),
    (
        "downsample.wgsl",
        include_str!("../../../shaders/downsample.wgsl"),
    ),
    (
        "output_transform.wgsl",
        include_str!("../../../shaders/output_transform.wgsl"),
    ),
    (
        "output_transform_16.wgsl",
        include_str!("../../../shaders/output_transform_16.wgsl"),
    ),
    (
        "tone_color.wgsl",
        concat!(
            include_str!("../../../shaders/photographic_color.wgsl"),
            "\n",
            include_str!("../../../shaders/tone_color.wgsl")
        ),
    ),
    (
        "huesat_lut.wgsl",
        include_str!("../../../shaders/huesat_lut.wgsl"),
    ),
    (
        "histogram.wgsl",
        include_str!("../../../shaders/histogram.wgsl"),
    ),
    (
        "nr_main.wgsl",
        include_str!("../../../shaders/nr_main.wgsl"),
    ),
    ("nr_raw.wgsl", include_str!("../../../shaders/nr_raw.wgsl")),
    (
        "local_contrast.wgsl",
        include_str!("../../../shaders/local_contrast.wgsl"),
    ),
    (
        "retouch.wgsl",
        concat!(
            include_str!("../../../shaders/photographic_color.wgsl"),
            "\n",
            include_str!("../../../shaders/retouch.wgsl")
        ),
    ),
    (
        "defringe.wgsl",
        include_str!("../../../shaders/defringe.wgsl"),
    ),
    (
        "sharpen.wgsl",
        include_str!("../../../shaders/sharpen.wgsl"),
    ),
    (
        "output_scrgb.wgsl",
        include_str!("../../../shaders/output_scrgb.wgsl"),
    ),
];

/// Uniforms for `demosaic_superpixel.wgsl` and `raw_prepare.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RawParams {
    pub origin_x: u32,
    pub origin_y: u32,
    pub out_w: u32,
    pub out_h: u32,
    pub black: [f32; 4],
    pub white: [f32; 4],
    pub cfa: u32,
    /// Hot-pixel suppression: 1 = on (raw_prepare honors this; the
    /// superpixel path applies a fixed mild clamp regardless).
    pub hot_enable: u32,
    /// Hot ratio vs same-color neighbors (e.g. 1.5).
    pub hot_thresh: f32,
    /// Lens shading correction: 1 = sample the gain grid at binding 3.
    pub lens_enable: u32,
    /// xy = grid dimensions in cells, zw = optical centre in full-sensor px.
    pub lens_grid: [f32; 4],
    /// x = grid sample spacing in sensor px, y = strength 0..1.
    pub lens_scale: [f32; 4],
}

impl RawParams {
    /// Lens fields for "no correction" — a 1×1 grid of ones, disabled.
    #[must_use]
    pub const fn no_lens() -> ([f32; 4], [f32; 4], u32) {
        ([1.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], 0)
    }
}

/// Uniforms for `wb_color.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WbColorParams {
    /// Rows of cam→Rec2020 (w unused).
    pub m0: [f32; 4],
    pub m1: [f32; 4],
    pub m2: [f32; 4],
    /// xyz = WB gains for camera R/G/B, w = linear exposure multiplier.
    pub wb_and_exposure: [f32; 4],
    pub out_w: u32,
    pub out_h: u32,
    /// Chroma warp: 1 = sample the grid at binding 3.
    pub warp_enable: u32,
    pub _pad: u32,
    /// xy = grid dimensions in cells; zw unused.
    pub warp_dims: [f32; 4],
    /// (cb_min, cb_max, cr_min, cr_max).
    pub warp_range: [f32; 4],
    /// x = gray threshold below which the warp fades out.
    pub warp_gray: [f32; 4],
    /// x enabled, y strength, z multi-scale neighbour radius.
    pub recovery: [f32; 4],
    pub negative_base: [f32; 4],
    pub negative_controls: [f32; 4],
}

/// Uniforms for `downsample.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SizeParams {
    pub src_w: u32,
    pub src_h: u32,
    pub out_w: u32,
    pub out_h: u32,
}

/// Uniforms for `tone_color.wgsl`. Layout must match the WGSL struct.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ToneColorParams {
    /// 8 bands × (hue_shift, saturation, luminance, pad), each -100..100.
    pub bands: [[f32; 4]; 8],
    /// Three wheels × (hue degrees, saturation, pad, pad).
    pub grading: [[f32; 4]; 3],
    /// Two packed groups of four monochrome-mixer values.
    pub monochrome_mix: [[f32; 4]; 2],
    /// x = vibrance, y = saturation, z = grading balance, w = monochrome flag.
    pub global_sat: [f32; 4],
    pub out_w: u32,
    pub out_h: u32,
    /// 1 = skip the Oklab HSL detour entirely.
    pub color_neutral: u32,
    pub channel_neutral: u32,
    pub photographic: [[f32; 4]; 14],
}

/// Uniforms for `local_contrast.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LocalContrastGpuParams {
    /// x clarity, y texture, z dehaze, w airlight.
    pub local: [f32; 4],
    pub out_w: u32,
    pub out_h: u32,
    pub working_w: u32,
    pub _pad: u32,
}

/// Uniforms for `defringe.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DefringeGpuParams {
    /// x = purple amount, y = green amount (both 0..1), z = radius pixels,
    /// w = edge selectivity (0..1).
    pub fringe: [f32; 4],
    /// x/y purple hue center/range, z/w green hue center/range. Hue values
    /// are normalized turns; ranges are half-widths.
    pub targets: [f32; 4],
    pub out_w: u32,
    pub out_h: u32,
    /// 1 enables wide chromatic-bloom repair around clipped highlights.
    pub wide_bloom: u32,
    pub _pad: u32,
}

/// Uniforms for `huesat_lut.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ProfileColorParams {
    pub huesat_dims: [u32; 4],
    pub look_dims: [u32; 4],
    pub rgb_dims: [u32; 4],
    /// 0 = pre-tone HueSat map, 1 = post-tone look/RGB LUTs.
    pub stage: u32,
    pub out_w: u32,
    pub out_h: u32,
    pub _pad: u32,
}

/// Uniforms for `demosaic_malvar.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MalvarParams {
    pub out_w: u32,
    pub out_h: u32,
    /// CFA pattern id relative to the TILE origin (use `CfaPattern::shifted`).
    pub cfa: u32,
    pub _pad: u32,
}

/// Uniforms for the edge-directed AHD demosaic. The camera-to-XYZ matrix
/// and white balance are used only for the CIELAB homogeneity decision; the
/// shader still outputs unbalanced linear camera RGB for the normal pipeline.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AhdParams {
    pub out_w: u32,
    pub out_h: u32,
    pub cfa: u32,
    pub _pad: u32,
    pub m0: [f32; 4],
    pub m1: [f32; 4],
    pub m2: [f32; 4],
    pub wb: [f32; 4],
}

/// Uniforms for `nr_main.wgsl` (Phase 4 noise reduction).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NrGpuParams {
    /// x = luminance, y = luminance_detail, z = chroma, w = chroma_detail
    /// (all 0..100, straight from `NrParams`).
    pub nr: [f32; 4],
    pub out_w: u32,
    pub out_h: u32,
    pub _pad: [u32; 2],
}

/// Uniforms for `sharpen.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SharpenGpuParams {
    /// x = amount, y = radius, z = threshold, w = halo suppression.
    pub sharpen: [f32; 4],
    /// Strength, sample radius, regularization, reserved.
    pub restoration: [f32; 4],
    pub out_w: u32,
    pub out_h: u32,
    pub preview_mask: u32,
    pub _pad: u32,
}

/// Uniforms for `retouch.wgsl`. Variable-sized records and points are
/// supplied through the two read-only storage bindings.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RetouchGpuParams {
    pub out_w: u32,
    pub out_h: u32,
    pub origin_x: u32,
    pub origin_y: u32,
    pub full_w: u32,
    pub full_h: u32,
    pub local_count: u32,
    pub spot_count: u32,
    pub source_origin_x: u32,
    pub source_origin_y: u32,
    /// Independent source patch and eight ring samples are bound when set.
    pub patch_source: u32,
    /// One-based local record index to render as coverage; zero develops normally.
    pub preview_mask: u32,
    pub storage_origin: [u32; 2],
    pub _region_pad: [u32; 2],
}

/// One packed retouch record. All fields are vec4-shaped so Rust/WGSL
/// storage-buffer layout is identical on every backend.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RetouchGpuRecord {
    pub header: [f32; 4],
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub tone0: [f32; 4],
    pub tone1: [f32; 4],
    pub color: [f32; 4],
    /// Postfix mask program offset/count, invert, coverage opacity.
    pub mask: [f32; 4],
    /// Relative Rec.2020 white-balance gains.
    pub wb: [f32; 4],
    /// Clarity, texture, dehaze, sharpness.
    pub spatial: [f32; 4],
    /// Noise reduction, tone LUT offset, channel LUT offset, color-table offset.
    pub extra: [f32; 4],
    /// Tone curve, channel curves, extended color active; reserved.
    pub flags: [f32; 4],
    pub corrective: [f32; 4],
}

/// Uniforms for `histogram.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HistogramParams {
    pub src_w: u32,
    pub src_h: u32,
    /// 0 = normal live histogram, 1 = also build purple/green fringe-score
    /// planes for the explicit Auto Defringe command.
    pub fringe_analysis: u32,
    /// Full-resolution active width, used to report a scale-independent
    /// recommended fringe radius from proxy analysis.
    pub working_w: u32,
}

/// Eight 256-bin planes: display R/G/B, true luminance, opt-in purple/green
/// confidence, and matching purple/green full-resolution width estimates.
pub const HISTOGRAM_BINS: usize = 2048;

/// Uniforms for `output_transform.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OutputParams {
    pub out_w: u32,
    pub out_h: u32,
    /// 0 = normal SDR, 1 = HDR edit tone-mapped to SDR, 2 = PQ Rec.2020.
    pub mode: u32,
    /// 0 normal, 1 clipping threshold view.
    pub debug_mode: u32,
    /// x = headroom stops, y = rolloff 0..100, z = scene-white EV.
    pub hdr: [f32; 4],
    /// Rows of the working→proof gamut matrix.
    pub gamut0: [f32; 4],
    pub gamut1: [f32; 4],
    pub gamut2: [f32; 4],
    /// amount, midpoint, roundness, feather.
    pub vignette: [f32; 4],
    /// amount, size, roughness, seed.
    pub grain: [f32; 4],
    /// Radial distortion k1..k3, w = enabled.
    pub lens_geometry: [f32; 4],
    /// Lateral CA red/blue radial scale deltas.
    pub lateral_ca: [f32; 4],
    /// Normalized edit crop.
    pub crop: [f32; 4],
    /// Tile origin x/y and full working width/height.
    pub image_rect: [f32; 4],
    /// x = gamut warning enabled, y = custom proof LUT size (0 disables).
    pub proof_flags: [u32; 4],
}

/// One compiled pass: pipeline + its bind group layout.
pub struct Pass {
    pub pipeline: wgpu::ComputePipeline,
    pub layout: wgpu::BindGroupLayout,
}

/// All compiled develop passes.
pub struct Pipelines {
    pub superpixel: Pass,
    pub raw_prepare: Pass,
    pub malvar: Pass,
    pub ahd: Pass,
    /// Pre-demosaic noise reduction; full-resolution paths only.
    pub raw_denoise: Pass,
    pub wb_color: Pass,
    pub tone_color: Pass,
    pub noise: Pass,
    pub defringe: Pass,
    pub local_contrast: Pass,
    pub retouch: Pass,
    pub sharpen: Pass,
    pub profile_color: Pass,
    pub downsample: Pass,
    pub output: Pass,
    /// scRGB float output for Metal EDR display (HDR viewport blit source).
    pub output_scrgb: Pass,
    pub output16: Pass,
    pub histogram: Pass,
}

pub fn shader_source(name: &str) -> &'static str {
    SHADER_SOURCES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| unreachable!("shader {name} not registered in SHADER_SOURCES"))
}

/// Marker line a shader places before its first declaration to opt into the
/// precision prelude (BUILD.md §10.2). Sources without it are used verbatim.
pub const PRECISION_MARKER: &str = "//#SP_PRECISION";

const PRELUDE_F32: &str = "\
alias fp = f32;
alias fp2 = vec2<f32>;
alias fp3 = vec3<f32>;
alias fp4 = vec4<f32>;";

const PRELUDE_F16: &str = "\
enable f16;
alias fp = f16;
alias fp2 = vec2<f16>;
alias fp3 = vec3<f16>;
alias fp4 = vec4<f16>;";

/// Substitute the precision prelude into a shader source.
///
/// `fp`/`fp3`/`fp4` become `f16` variants when `f16` is true, so one source
/// file compiles to both the native-half path (Apple GPUs run it at double
/// rate with half the register/shared-memory footprint) and the portable
/// f32 fallback. Both variants are validated in CI —
/// `sp-gpu/tests/shaders_compile.rs`.
#[must_use]
pub fn shader_with_precision(source: &str, f16: bool) -> std::borrow::Cow<'_, str> {
    if !source.contains(PRECISION_MARKER) {
        return std::borrow::Cow::Borrowed(source);
    }
    let prelude = if f16 { PRELUDE_F16 } else { PRELUDE_F32 };
    std::borrow::Cow::Owned(source.replace(PRECISION_MARKER, prelude))
}

fn shader_module(ctx: &GpuContext, name: &str) -> wgpu::ShaderModule {
    let source = shader_with_precision(shader_source(name), ctx.supports_f16());
    ctx.device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::ShaderSource::Wgsl(source.into_owned().into()),
        })
}

fn make_pass(
    ctx: &GpuContext,
    name: &str,
    input_sample_type: wgpu::TextureSampleType,
    output_format: wgpu::TextureFormat,
) -> Pass {
    let module = shader_module(ctx, name);

    let layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(name),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: input_sample_type,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: output_format,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(name),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(name),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

    Pass { pipeline, layout }
}

/// Output pass with a final-stage 3-D ICC proof LUT.
fn make_output_pass(ctx: &GpuContext, name: &str, output_format: wgpu::TextureFormat) -> Pass {
    let module = shader_module(ctx, name);
    let entries = [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: output_format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        },
    ];
    finish_pass(ctx, name, module, &entries)
}

/// Like [`make_pass`] but with a fourth binding: a sampled 2-D lookup
/// texture. Used by the passes that carry calibration data alongside the
/// image — lens shading grids and the chroma warp.
fn make_lookup_pass(
    ctx: &GpuContext,
    name: &str,
    input_sample_type: wgpu::TextureSampleType,
    output_format: wgpu::TextureFormat,
) -> Pass {
    let module = shader_module(ctx, name);
    let entries = [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: input_sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: output_format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
    ];
    finish_pass(ctx, name, module, &entries)
}

impl Pipelines {
    pub fn new(ctx: &GpuContext) -> Result<Self, SpError> {
        use wgpu::TextureFormat as TF;
        use wgpu::TextureSampleType as ST;
        Ok(Self {
            superpixel: make_lookup_pass(
                ctx,
                "demosaic_superpixel.wgsl",
                ST::Uint,
                TF::Rgba16Float,
            ),
            raw_prepare: make_lookup_pass(ctx, "raw_prepare.wgsl", ST::Uint, TF::R32Float),
            malvar: make_pass(
                ctx,
                "demosaic_malvar.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            ahd: make_pass(
                ctx,
                "demosaic_ahd.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            raw_denoise: make_pass(
                ctx,
                "nr_raw.wgsl",
                ST::Float { filterable: false },
                TF::R32Float,
            ),
            wb_color: make_lookup_pass(
                ctx,
                "wb_color.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            tone_color: make_lut_pass(ctx, "tone_color.wgsl", TF::Rgba16Float),
            noise: make_pass(
                ctx,
                "nr_main.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            defringe: make_pass(
                ctx,
                "defringe.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            local_contrast: make_pass(
                ctx,
                "local_contrast.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            retouch: make_lut_pass(ctx, "retouch.wgsl", TF::Rgba16Float),
            sharpen: make_pass(
                ctx,
                "sharpen.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            profile_color: make_profile_pass(ctx),
            downsample: make_pass(
                ctx,
                "downsample.wgsl",
                ST::Float { filterable: false },
                TF::Rgba16Float,
            ),
            output: make_output_pass(ctx, "output_transform.wgsl", TF::Rgba8Unorm),
            output_scrgb: make_output_pass(ctx, "output_scrgb.wgsl", TF::Rgba16Float),
            output16: make_output_pass(ctx, "output_transform_16.wgsl", TF::Rgba16Uint),
            histogram: make_histogram_pass(ctx),
        })
    }
}

/// Like [`make_pass`] but with two read-only storage buffers: the combined
/// tone LUT and the packed per-channel curve LUT.
fn make_lut_pass(ctx: &GpuContext, name: &str, output_format: wgpu::TextureFormat) -> Pass {
    let module = shader_module(ctx, name);
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: output_format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ];
    if name == "retouch.wgsl" {
        for binding in 5..=7 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
    }
    finish_pass(ctx, name, module, &entries)
}

/// Profile pass: the standard image bindings plus three read-only 3-D LUTs.
fn make_profile_pass(ctx: &GpuContext) -> Pass {
    let name = "huesat_lut.wgsl";
    let module = shader_module(ctx, name);
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ];
    for binding in 3..=5 {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        });
    }
    finish_pass(ctx, name, module, &entries)
}

/// Histogram pass: sampled texture + read-write storage bins + uniform.
fn make_histogram_pass(ctx: &GpuContext) -> Pass {
    let name = "histogram.wgsl";
    let module = shader_module(ctx, name);
    let entries = [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ];
    finish_pass(ctx, name, module, &entries)
}

fn finish_pass(
    ctx: &GpuContext,
    name: &str,
    module: wgpu::ShaderModule,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> Pass {
    let layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(name),
            entries,
        });
    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(name),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(name),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    Pass { pipeline, layout }
}

/// Create the standard bind group for a pass.
pub fn bind(
    ctx: &GpuContext,
    pass: &Pass,
    input: &wgpu::TextureView,
    output: &wgpu::TextureView,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pass.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(input),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}

/// Bind group for a `make_lookup_pass` pass: the standard three bindings
/// plus a sampled lookup texture at binding 3.
pub fn bind_lookup(
    ctx: &GpuContext,
    pass: &Pass,
    input: &wgpu::TextureView,
    output: &wgpu::TextureView,
    uniforms: &wgpu::Buffer,
    lookup: &wgpu::TextureView,
) -> wgpu::BindGroup {
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pass.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(input),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(lookup),
            },
        ],
    })
}

/// Bind the image/uniform trio plus two read-only storage buffers.
pub fn bind_storage2(
    ctx: &GpuContext,
    pass: &Pass,
    input: &wgpu::TextureView,
    output: &wgpu::TextureView,
    uniforms: &wgpu::Buffer,
    records: &wgpu::Buffer,
    points: &wgpu::Buffer,
) -> wgpu::BindGroup {
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pass.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(input),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: records.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: points.as_entire_binding(),
            },
        ],
    })
}

/// Retouch bindings: target input/output, independent source patch and
/// healing-ring samples; uniforms, operation records and mask payloads.
pub fn bind_retouch(
    ctx: &GpuContext,
    pass: &Pass,
    textures: [&wgpu::TextureView; 4],
    buffers: [&wgpu::Buffer; 3],
) -> wgpu::BindGroup {
    bind_retouch_with_target(
        ctx,
        pass,
        [
            textures[0],
            textures[1],
            textures[2],
            textures[3],
            textures[0],
        ],
        buffers,
    )
}

/// Independent immutable target context keeps overlapping heals consistent
/// with the complete-image proxy while correction outputs accumulate.
pub fn bind_retouch_with_target(
    ctx: &GpuContext,
    pass: &Pass,
    textures: [&wgpu::TextureView; 5],
    buffers: [&wgpu::Buffer; 3],
) -> wgpu::BindGroup {
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("retouch sources"),
        layout: &pass.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(textures[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(textures[1]),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffers[0].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: buffers[1].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: buffers[2].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(textures[2]),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(textures[3]),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(textures[4]),
            },
        ],
    })
}

/// Upload an RGBA f32 lookup grid as an `Rgba32Float` 2-D texture.
///
/// `data` is `width * height` texels in row-major order. Used for lens
/// shading grids (RGBA = the four CFA quad slots) and the chroma warp
/// (rg = the warped chroma pair). f32 rather than f16 because shading gains
/// in deep corners and chroma coordinates near the grid edge both want more
/// precision than half gives, and these textures are tiny.
pub fn make_lookup_texture(
    ctx: &GpuContext,
    label: &str,
    width: u32,
    height: u32,
    data: &[[f32; 4]],
) -> wgpu::TextureView {
    debug_assert_eq!(data.len(), (width as usize) * (height as usize));
    let texture = make_texture(
        ctx,
        label,
        width,
        height,
        wgpu::TextureFormat::Rgba32Float,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 16),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&Default::default())
}

/// Encode a dispatch covering `out_w` × `out_h` pixels.
pub fn run(
    encoder: &mut wgpu::CommandEncoder,
    pass: &Pass,
    bind_group: &wgpu::BindGroup,
    out_w: u32,
    out_h: u32,
) {
    let (gx, gy) = dispatch_size(out_w, out_h);
    let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: None,
        timestamp_writes: None,
    });
    cpass.set_pipeline(&pass.pipeline);
    cpass.set_bind_group(0, bind_group, &[]);
    cpass.dispatch_workgroups(gx, gy, 1);
    // All shaders in `shaders/` hardcode @workgroup_size(16, 16).
    const { assert!(WORKGROUP_DIM == 16) };
}

/// Create a 2-D texture with common defaults.
pub fn make_texture(
    ctx: &GpuContext,
    label: &str,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}
