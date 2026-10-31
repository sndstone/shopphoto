//! HDR viewport blit (macOS EDR).
//!
//! When the swapchain is `Rgba16Float` (see the vendored egui-wgpu patch +
//! `SHOPPHOTO_HDR_SURFACE`), the develop pipeline's scRGB display texture
//! is drawn straight into the surface through an egui paint callback —
//! linear values above 1.0 reach CAMetalLayer's extended dynamic range
//! untouched. The 8-bit `register_native_texture` path stays in use for
//! SDR surfaces, thumbnails and the loupe.

use eframe::egui_wgpu::{self, wgpu};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HdrBlitParams {
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    split: f32,
    compare_mode: u32,
    pad: [f32; 2],
    geometry: [[f32; 4]; 3],
}

/// Fullscreen-triangle blit of the scRGB texture into the callback rect.
const BLIT_WGSL: &str = r#"
struct Params {
    uv_min: vec2<f32>,
    uv_max: vec2<f32>,
    split: f32,
    compare_mode: u32,
    _pad: vec2<f32>,
    map0: vec4<f32>,
    map1: vec4<f32>,
    map2: vec4<f32>,
}
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var compare_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) frac: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Oversized triangle covering the viewport; t in 0..2 both axes.
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
    // Flip y: clip-space up vs texture down.
    let frac = vec2<f32>(t.x, 1.0 - t.y);
    out.uv = params.uv_min + (params.uv_max - params.uv_min) * frac;
    out.frac = frac;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Both sources and the surface are linear scRGB. The comparison happens
    // here so HDR headroom survives on both sides of the divider.
    let use_before = params.compare_mode == 2u
        || (params.compare_mode == 1u && in.frac.x <= params.split);
    var uv = in.uv;
    if params.map2.z > 0.5 {
        let delta = in.frac - 0.5;
        let q = vec2<f32>(dot(params.map0.xy,delta),dot(params.map0.zw,delta));
        uv = params.map1.xy + q / max(vec2<f32>(0.25), vec2<f32>(1.0)+q.yx*params.map2.xy) * params.map1.zw;
    }
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { return vec4<f32>(0.0,0.0,0.0,1.0); }
    let rgb = select(
        textureSample(src_tex, src_sampler, uv).rgb,
        textureSample(compare_tex, src_sampler, uv).rgb,
        use_before,
    );
    return vec4<f32>(rgb, 1.0);
}
"#;

/// Which scRGB source a blit samples: the proxy develop output or the 1:1
/// tile. Both stay bound side by side so the viewport can choose per draw.
/// The tile is rendered from the previous frame's view region, so on a frame
/// where a gesture outruns its overscan the panel draws the proxy plus the
/// stale tile where it still covers; a single shared binding would apply
/// proxy UVs to the tile texture on exactly those frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrSource {
    Proxy,
    Tile,
    CullProxy(u8),
    CullTile(u8),
}

/// Uniform slot stride: wgpu's minimum dynamic-offset alignment.
const UNIFORM_STRIDE: u64 = 256;
const UNIFORM_SIZE: u64 = 80;

impl HdrSource {
    fn offset(self) -> u64 {
        match self {
            HdrSource::Proxy => 0,
            HdrSource::Tile => UNIFORM_STRIDE,
            HdrSource::CullProxy(index) => (4 + 2 * u64::from(index.min(3))) * UNIFORM_STRIDE,
            HdrSource::CullTile(index) => (5 + 2 * u64::from(index.min(3))) * UNIFORM_STRIDE,
        }
    }

    /// Uniform slot for one blit. The "second view" slots let a frame
    /// draw the same source twice with different parameters: the
    /// before/after side-by-side view shows the same region in two rects.
    fn slot_offset(self, second_view: bool) -> u64 {
        self.offset()
            + if second_view && matches!(self, Self::Proxy | Self::Tile) {
                2 * UNIFORM_STRIDE
            } else {
                0
            }
    }
}

/// Long-lived GPU objects, created once and stored in egui's callback
/// resources.
pub struct HdrBlitPack {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    /// One 256-byte slot per [`HdrSource`], addressed by dynamic offset so
    /// a proxy blit and a tile blit can coexist in one frame.
    pub uniforms: wgpu::Buffer,
    /// Rebuilt by the app whenever the proxy texture view changes.
    proxy_bind: Option<wgpu::BindGroup>,
    /// Rebuilt whenever a tile is rendered; cleared when the tile is released.
    tile_bind: Option<wgpu::BindGroup>,
    cull_proxy_bind: [Option<wgpu::BindGroup>; 4],
    cull_tile_bind: [Option<wgpu::BindGroup>; 4],
}

impl HdrBlitPack {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hdr blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hdr blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hdr blit"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hdr blit"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hdr blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hdr blit uv"),
            size: UNIFORM_STRIDE * 12,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            bind_layout,
            sampler,
            uniforms,
            proxy_bind: None,
            tile_bind: None,
            cull_proxy_bind: std::array::from_fn(|_| None),
            cull_tile_bind: std::array::from_fn(|_| None),
        }
    }

    /// Bind `view` as `source`, plus an optional untouched comparison view.
    /// With no comparison the primary is bound twice, keeping the shader
    /// and bind layout stable.
    pub fn bind(
        &mut self,
        device: &wgpu::Device,
        source: HdrSource,
        view: &wgpu::TextureView,
        compare: Option<&wgpu::TextureView>,
    ) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hdr blit"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(compare.unwrap_or(view)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    }),
                },
            ],
        });
        match source {
            HdrSource::Proxy => self.proxy_bind = Some(group),
            HdrSource::Tile => self.tile_bind = Some(group),
            HdrSource::CullProxy(index) => {
                self.cull_proxy_bind[usize::from(index.min(3))] = Some(group)
            }
            HdrSource::CullTile(index) => {
                self.cull_tile_bind[usize::from(index.min(3))] = Some(group)
            }
        }
    }

    /// Drop a source's binding (the tile texture is being released).
    pub fn unbind(&mut self, source: HdrSource) {
        match source {
            HdrSource::Proxy => self.proxy_bind = None,
            HdrSource::Tile => self.tile_bind = None,
            HdrSource::CullProxy(index) => self.cull_proxy_bind[usize::from(index.min(3))] = None,
            HdrSource::CullTile(index) => self.cull_tile_bind[usize::from(index.min(3))] = None,
        }
    }

    fn bind_group(&self, source: HdrSource) -> Option<&wgpu::BindGroup> {
        match source {
            HdrSource::Proxy => self.proxy_bind.as_ref(),
            HdrSource::Tile => self.tile_bind.as_ref(),
            HdrSource::CullProxy(index) => self.cull_proxy_bind[usize::from(index.min(3))].as_ref(),
            HdrSource::CullTile(index) => self.cull_tile_bind[usize::from(index.min(3))].as_ref(),
        }
    }
}

/// The per-frame egui paint callback.
pub struct HdrBlitCallback {
    pub source: HdrSource,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// Divider position as a fraction of the callback rect. May fall outside
    /// 0..1 when a partial tile blit does not straddle the divider.
    pub compare_split: Option<f32>,
    pub show_compare_only: bool,
    /// Use the second uniform slot of `source` (side-by-side views).
    pub second_view: bool,
    pub geometry: Option<sp_core::geometry::GeometryMap>,
}

impl HdrBlitCallback {
    fn params(&self) -> HdrBlitParams {
        HdrBlitParams {
            uv_min: self.uv_min,
            uv_max: self.uv_max,
            split: self.compare_split.unwrap_or(0.0).clamp(-1.0, 2.0),
            compare_mode: if self.show_compare_only {
                2
            } else if self.compare_split.is_some() {
                1
            } else {
                0
            },
            pad: [0.0; 2],
            geometry: self.geometry.map_or([[0.0; 4]; 3], |map| map.packed),
        }
    }
}

impl egui_wgpu::CallbackTrait for HdrBlitCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(pack) = callback_resources.get::<HdrBlitPack>() {
            let data = self.params();
            queue.write_buffer(
                &pack.uniforms,
                self.source.slot_offset(self.second_view),
                bytemuck::bytes_of(&data),
            );
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: eframe::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(pack) = callback_resources.get::<HdrBlitPack>() else {
            return;
        };
        // A missing binding draws nothing rather than sampling the other
        // source with the wrong UVs.
        let Some(bind_group) = pack.bind_group(self.source) else {
            return;
        };
        render_pass.set_pipeline(&pack.pipeline);
        render_pass.set_bind_group(
            0,
            bind_group,
            &[self.source.slot_offset(self.second_view) as u32],
        );
        render_pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::{wgpu, HdrBlitCallback, HdrBlitPack, HdrBlitParams, HdrSource, UNIFORM_STRIDE};

    #[test]
    fn hdr_compare_pipeline_layout_matches_both_shader_stages() {
        let Ok(ctx) = sp_gpu::GpuContext::headless() else {
            return;
        };
        let _pipeline = HdrBlitPack::new(&ctx.device, wgpu::TextureFormat::Rgba16Float);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("HDR comparison pipeline validation");
    }

    #[test]
    fn hdr_compare_mode_is_uploaded_as_an_integer() {
        assert_eq!(std::mem::size_of::<HdrBlitParams>(), 80);
        let split = HdrBlitCallback {
            source: HdrSource::Proxy,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            compare_split: Some(0.5),
            show_compare_only: false,
            second_view: false,
            geometry: None,
        };
        assert_eq!(split.params().compare_mode, 1);
        let before = HdrBlitCallback {
            show_compare_only: true,
            ..split
        };
        assert_eq!(before.params().compare_mode, 2);
    }

    #[test]
    fn sources_use_distinct_aligned_uniform_slots() {
        assert_eq!(HdrSource::Proxy.offset(), 0);
        assert_eq!(HdrSource::Tile.offset(), UNIFORM_STRIDE);
        assert_eq!(UNIFORM_STRIDE % 256, 0);
        assert_eq!(HdrSource::Tile.slot_offset(true), 3 * UNIFORM_STRIDE);
    }

    #[test]
    fn a_partial_tile_blit_keeps_an_off_rect_divider() {
        let left = HdrBlitCallback {
            source: HdrSource::Tile,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            compare_split: Some(-0.4),
            show_compare_only: false,
            second_view: false,
            geometry: None,
        };
        assert!(left.params().split < 0.0);
        let right = HdrBlitCallback {
            compare_split: Some(1.6),
            ..left
        };
        assert!(right.params().split > 1.0);
    }
}

#[cfg(test)]
mod culling_gpu_tests {
    use super::*;
    #[test]
    fn four_hdr_comparison_cells_keep_independent_float_sources_and_uniforms() {
        let gpu = match sp_gpu::GpuContext::headless() {
            Ok(context) => context,
            Err(sp_core::SpError::NoGpu(error)) => {
                eprintln!("skipping GPU test: {error}");
                return;
            }
            Err(error) => panic!("GPU initialization failed: {error}"),
        };
        let mut pack = HdrBlitPack::new(&gpu.device, wgpu::TextureFormat::Rgba16Float);
        let mut sources = Vec::new();
        for index in 0..4u8 {
            for tile in [false, true] {
                let source = if tile {
                    HdrSource::CullTile(index)
                } else {
                    HdrSource::CullProxy(index)
                };
                let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("HDR comparison source"),
                    size: wgpu::Extent3d {
                        width: 2,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                // Left texel 2/4/6/8, right texel 4/6/8/10; exact half-floats.
                let levels = [0x4000u16, 0x4400, 0x4600, 0x4800, 0x4900];
                let pixels = [
                    levels[index as usize],
                    0,
                    0,
                    0x3c00,
                    levels[index as usize + 1],
                    0,
                    0,
                    0x3c00,
                ];
                gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&pixels),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(16),
                        rows_per_image: Some(1),
                    },
                    wgpu::Extent3d {
                        width: 2,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                pack.bind(
                    &gpu.device,
                    source,
                    &texture.create_view(&Default::default()),
                    None,
                );
                let uv = if tile { 0.75 } else { 0.25 };
                let callback = HdrBlitCallback {
                    source,
                    uv_min: [uv, 0.5],
                    uv_max: [uv, 0.5],
                    compare_split: None,
                    show_compare_only: false,
                    second_view: false,
                    geometry: None,
                };
                gpu.queue.write_buffer(
                    &pack.uniforms,
                    source.slot_offset(false),
                    bytemuck::bytes_of(&callback.params()),
                );
                sources.push((source, texture));
            }
        }
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HDR comparison target"),
            size: wgpu::Extent3d {
                width: 8,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = target.create_view(&Default::default());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pack.pipeline);
            for (i, (source, _)) in sources.iter().enumerate() {
                pass.set_viewport(i as f32, 0.0, 1.0, 1.0, 0.0, 1.0);
                pass.set_bind_group(
                    0,
                    pack.bind_group(*source).unwrap(),
                    &[source.slot_offset(false) as u32],
                );
                pass.draw(0..3, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 8,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        {
            let bytes = readback.slice(..).get_mapped_range();
            let values: &[u16] = bytemuck::cast_slice(&bytes);
            let expected = [
                0x4000, 0x4400, 0x4400, 0x4600, 0x4600, 0x4800, 0x4800, 0x4900,
            ];
            for (i, v) in expected.into_iter().enumerate() {
                assert_eq!(
                    values[i * 4],
                    v,
                    "cell {i} lost HDR or sampled another source/uniform"
                );
            }
        }
        readback.unmap();
    }
}
