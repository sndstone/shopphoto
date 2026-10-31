//! Full-resolution export renders and GPU readbacks.

use super::*;

pub(super) fn decode_export_channel(
    value: f32,
    hdr_pq: bool,
    transfer: sp_core::color::Transfer,
) -> f32 {
    if hdr_pq {
        sp_core::color::pq_eotf(value)
    } else {
        transfer.eotf(value)
    }
}

pub(super) fn encode_export_channel(
    value: f32,
    hdr_pq: bool,
    transfer: sp_core::color::Transfer,
) -> f32 {
    if hdr_pq {
        sp_core::color::pq_oetf(value)
    } else {
        transfer.oetf(value)
    }
}

pub(super) fn box_downsample_rgba8(
    width: u32,
    height: u32,
    input: &[u8],
    factor: u32,
    hdr_pq: bool,
    transfer: sp_core::color::Transfer,
) -> RenderedImage {
    let out_w = (width / factor).max(1);
    let out_h = (height / factor).max(1);
    let mut out = vec![0; (out_w * out_h * 4) as usize];
    for y in 0..out_h {
        for x in 0..out_w {
            let mut sums = [0.0f32; 3];
            let mut count = 0.0;
            for oy in 0..factor {
                for ox in 0..factor {
                    let sx = (x * factor + ox).min(width - 1);
                    let sy = (y * factor + oy).min(height - 1);
                    let offset = ((sy * width + sx) * 4) as usize;
                    for channel in 0..3 {
                        sums[channel] += decode_export_channel(
                            f32::from(input[offset + channel]) / 255.0,
                            hdr_pq,
                            transfer,
                        );
                    }
                    count += 1.0;
                }
            }
            let offset = ((y * out_w + x) * 4) as usize;
            for channel in 0..3 {
                out[offset + channel] = (encode_export_channel(
                    sums[channel] / count,
                    hdr_pq,
                    transfer,
                ) * 255.0)
                    .round() as u8;
            }
            out[offset + 3] = 255;
        }
    }
    RenderedImage {
        width: out_w,
        height: out_h,
        rgba8: out,
        rgba16: None,
    }
}

pub(super) fn box_downsample_rgba16(
    width: u32,
    height: u32,
    input: &[u16],
    factor: u32,
    hdr_pq: bool,
    transfer: sp_core::color::Transfer,
) -> RenderedImage {
    let out_w = (width / factor).max(1);
    let out_h = (height / factor).max(1);
    let mut out = vec![0; (out_w * out_h * 4) as usize];
    for y in 0..out_h {
        for x in 0..out_w {
            let mut sums = [0.0f32; 3];
            let mut count = 0.0;
            for oy in 0..factor {
                for ox in 0..factor {
                    let sx = (x * factor + ox).min(width - 1);
                    let sy = (y * factor + oy).min(height - 1);
                    let offset = ((sy * width + sx) * 4) as usize;
                    for channel in 0..3 {
                        sums[channel] += decode_export_channel(
                            f32::from(input[offset + channel]) / 65535.0,
                            hdr_pq,
                            transfer,
                        );
                    }
                    count += 1.0;
                }
            }
            let offset = ((y * out_w + x) * 4) as usize;
            for channel in 0..3 {
                out[offset + channel] =
                    (encode_export_channel(sums[channel] / count, hdr_pq, transfer) * 65535.0)
                        .round() as u16;
            }
            out[offset + 3] = u16::MAX;
        }
    }
    let rgba8 = out.iter().map(|value| (value / 257) as u8).collect();
    RenderedImage {
        width: out_w,
        height: out_h,
        rgba8,
        rgba16: Some(out),
    }
}

pub(super) fn validate_rgba16(
    pixels: &[u16],
    float: bool,
    opaque_required: bool,
) -> Result<(), SpError> {
    let opaque = if float { 0x3c00 } else { u16::MAX };
    if pixels.chunks_exact(4).any(|pixel| {
        (opaque_required && pixel[3] != opaque)
            || (float && pixel[..3].iter().any(|value| value & 0x7c00 == 0x7c00))
    }) {
        return Err(SpError::Gpu(
            "GPU returned an incomplete or nonfinite image; export was not saved".into(),
        ));
    }
    Ok(())
}

impl GpuRenderer {
    /// The scRGB display output as 8-bit sRGB: the proxy, or the current
    /// 1:1 tile when `tile` is set. This is the QA capture path on the float
    /// EDR surface, where eframe's swapchain readback cannot run. Linear
    /// values above 1.0 clip to white, so the capture records tonality and
    /// color rather than headroom.
    pub fn read_scrgb_display_srgb8(&self, tile: bool) -> Result<(Vec<u8>, u32, u32), SpError> {
        let (texture, width, height) = if tile {
            let tile = self
                .tile
                .as_ref()
                .ok_or_else(|| SpError::Gpu("no tile is rendered".to_owned()))?;
            (&tile.hdr_display_tex, tile.tex_w, tile.tex_h)
        } else {
            let image = self
                .image
                .as_ref()
                .ok_or_else(|| SpError::Gpu("no image is open".to_owned()))?;
            (&image.hdr_display_tex, image.proxy_w, image.proxy_h)
        };
        let halves = self.read_rgba16_texture(texture, width, height)?;
        validate_rgba16(&halves, true, true)?;
        let encode = |linear: f32| -> u8 {
            let v = linear.clamp(0.0, 1.0);
            let s = if v <= 0.003_130_8 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round() as u8
        };
        let mut rgba = Vec::with_capacity(halves.len());
        for px in halves.chunks_exact(4) {
            for &bits in &px[..3] {
                rgba.push(encode(half::f16::from_bits(bits).to_f32()));
            }
            rgba.push(255);
        }
        Ok((rgba, width, height))
    }

    /// Read the current SDR display proxy for explicit analysis commands.
    pub fn read_display_srgb8(&self) -> Result<(Vec<u8>, u32, u32), SpError> {
        let image = self
            .image
            .as_ref()
            .ok_or_else(|| SpError::Gpu("no image is open".to_owned()))?;
        Ok((
            self.read_rgba8_texture(&image.out_tex, image.proxy_w, image.proxy_h)?,
            image.proxy_w,
            image.proxy_h,
        ))
    }

    pub(super) fn read_rgba8_texture(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, SpError> {
        let bytes_per_row = (width * 4).div_ceil(256) * 256;
        let staging = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("export readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("export readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        self.ctx.map_read_buffer(&staging)?;
        let mapped = slice.get_mapped_range();
        let mut rgba8 = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let start = (y * bytes_per_row) as usize;
            rgba8.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
        }
        drop(mapped);
        staging.unmap();
        if rgba8.chunks_exact(4).any(|pixel| pixel[3] != 255) {
            return Err(SpError::Gpu(
                "GPU returned an incomplete image; export was not saved".into(),
            ));
        }
        Ok(rgba8)
    }

    pub(super) fn read_rgba16_texture(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Result<Vec<u16>, SpError> {
        let bytes_per_row = (width * 8).div_ceil(256) * 256;
        let staging = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("16-bit export readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("16-bit export readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        self.ctx.map_read_buffer(&staging)?;
        let mapped = slice.get_mapped_range();
        let mut rgba16 = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let start = (y * bytes_per_row) as usize;
            for bytes in mapped[start..start + (width * 8) as usize].chunks_exact(2) {
                rgba16.push(u16::from_le_bytes([bytes[0], bytes[1]]));
            }
        }
        drop(mapped);
        staging.unmap();
        validate_rgba16(
            &rgba16,
            texture.format() == wgpu::TextureFormat::Rgba16Float,
            texture.format() != wgpu::TextureFormat::Rgba16Float,
        )?;
        Ok(rgba16)
    }

    pub(super) fn render_superpixel_export(
        &mut self,
        edit: &EditState,
        output_mode: u32,
        high_precision: bool,
        output_space: sp_core::color::OutputColorSpace,
    ) -> Result<RenderedImage, SpError> {
        let img = self
            .image
            .as_ref()
            .ok_or_else(|| SpError::Export("no image is open".into()))?;
        let (width, height) = (
            (img.active.width / 2).max(1),
            (img.active.height / 2).max(1),
        );
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let make = |label| {
            make_texture(
                &self.ctx,
                label,
                width,
                height,
                wgpu::TextureFormat::Rgba16Float,
                usage,
            )
        };
        let quarter = make("export super2 source");
        let quarter_v = quarter.create_view(&Default::default());
        let work = make("export super2 work");
        let work2 = make("export super2 work2");
        let work_nr = make("export super2 scratch");
        let out = make_texture(
            &self.ctx,
            "export super2 output",
            width,
            height,
            if high_precision {
                wgpu::TextureFormat::Rgba16Uint
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let (work_v, work2_v, scratch_v, out_v) = (
            work.create_view(&Default::default()),
            work2.create_view(&Default::default()),
            work_nr.create_view(&Default::default()),
            out.create_view(&Default::default()),
        );
        let bufs = self.color_buffers(
            edit,
            OutputSpec {
                size: [width, height],
                mode: output_mode,
                origin: [0, 0],
                full_size: [width, height],
                view_transform: false,
                export_space: Some(output_space),
            },
        );
        let (lens_enable, lens_grid, lens_scale, lens_view) = self.lens_fields(edit);
        let img = self.image.as_ref().expect("checked above");
        let raw_params = self
            .ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("export super2 raw params"),
                contents: bytemuck::bytes_of(&RawParams {
                    origin_x: img.active.x,
                    origin_y: img.active.y,
                    out_w: width,
                    out_h: height,
                    black: img.black_levels,
                    white: img.white_levels,
                    cfa: cfa_id(img.cfa_pattern),
                    hot_enable: 1,
                    hot_thresh: 2.0,
                    lens_enable,
                    lens_grid,
                    lens_scale,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let superpixel = bind_lookup(
            &self.ctx,
            &self.pipes.superpixel,
            &img.cfa_view,
            &quarter_v,
            &raw_params,
            lens_view,
        );
        let wb = bind_lookup(
            &self.ctx,
            &self.pipes.wb_color,
            &quarter_v,
            &work_v,
            &bufs.wb_params,
            &bufs.warp_view,
        );
        let pre = self.profile_bind_group(&work_v, &work2_v, &bufs, false);
        let effective_nr = self.effective_nr(edit);
        let nr_on = effective_nr[0] > 0.0 || effective_nr[2] > 0.0;
        let nr = nr_on.then(|| {
            let params = self.nr_params_buffer(edit, width, height);
            bind(&self.ctx, &self.pipes.noise, &work2_v, &scratch_v, &params)
        });
        let tone_input = if nr_on { &scratch_v } else { &work2_v };
        let tone = self.tone_bind_group(tone_input, &work_v, &bufs);
        let post = self.profile_bind_group(&work_v, &work2_v, &bufs, true);
        let defringe_on = edit.lens.enable
            && edit.lens.defringe
            && (edit.lens.defringe_purple > 0.0 || edit.lens.defringe_green > 0.0);
        let mut tail_steps = 0u32;
        let tail_input = |step: u32| match step {
            0 => &work2_v,
            step if step % 2 == 1 => &scratch_v,
            _ => &work_v,
        };
        let tail_output = |step: u32| {
            if step.is_multiple_of(2) {
                &scratch_v
            } else {
                &work_v
            }
        };
        let defringe = defringe_on.then(|| {
            let params = self.defringe_params_buffer(edit, width, height, 0.5);
            let group = bind(
                &self.ctx,
                &self.pipes.defringe,
                tail_input(tail_steps),
                tail_output(tail_steps),
                &params,
            );
            tail_steps += 1;
            group
        });
        let local_on = !edit.local.is_neutral();
        let local = local_on.then(|| {
            let params = self.local_params_buffer(edit, width, height, width);
            let group = bind(
                &self.ctx,
                &self.pipes.local_contrast,
                tail_input(tail_steps),
                tail_output(tail_steps),
                &params,
            );
            tail_steps += 1;
            group
        });
        let retouch_buffers = self.retouch_buffers(
            edit,
            OutputSpec {
                size: [width, height],
                mode: output_mode,
                origin: [0, 0],
                full_size: [width, height],
                view_transform: false,
                export_space: Some(output_space),
            },
        );
        let retouch = retouch_buffers.as_ref().map(|buffers| {
            let group = bind_retouch(
                &self.ctx,
                &self.pipes.retouch,
                [
                    tail_input(tail_steps),
                    tail_output(tail_steps),
                    tail_input(tail_steps),
                    tail_input(tail_steps),
                ],
                [&buffers.params, &buffers.records, &buffers.points],
            );
            tail_steps += 1;
            group
        });
        let sharpen_on = edit.sharpen.amount > 0.0 || edit.sharpen.restore_blur > 0.0;
        let sharpen = sharpen_on.then(|| {
            let params = self.sharpen_params_buffer(edit, width, height);
            let group = bind(
                &self.ctx,
                &self.pipes.sharpen,
                tail_input(tail_steps),
                tail_output(tail_steps),
                &params,
            );
            tail_steps += 1;
            group
        });
        let output_input = tail_input(tail_steps);
        let output_pass = if high_precision {
            &self.pipes.output16
        } else {
            &self.pipes.output
        };
        let output = bind_lookup(
            &self.ctx,
            output_pass,
            output_input,
            &out_v,
            &bufs.out_params,
            &bufs.proof_view,
        );
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("export super2"),
            });
        run(
            &mut encoder,
            &self.pipes.superpixel,
            &superpixel,
            width,
            height,
        );
        run(&mut encoder, &self.pipes.wb_color, &wb, width, height);
        run(&mut encoder, &self.pipes.profile_color, &pre, width, height);
        if let Some(group) = &nr {
            run(&mut encoder, &self.pipes.noise, group, width, height);
        }
        run(&mut encoder, &self.pipes.tone_color, &tone, width, height);
        run(
            &mut encoder,
            &self.pipes.profile_color,
            &post,
            width,
            height,
        );
        if let Some(group) = &defringe {
            run(&mut encoder, &self.pipes.defringe, group, width, height);
        }
        if let Some(group) = &local {
            run(
                &mut encoder,
                &self.pipes.local_contrast,
                group,
                width,
                height,
            );
        }
        if let Some(group) = &retouch {
            run(&mut encoder, &self.pipes.retouch, group, width, height);
        }
        if let Some(group) = &sharpen {
            run(&mut encoder, &self.pipes.sharpen, group, width, height);
        }
        run(&mut encoder, output_pass, &output, width, height);
        self.ctx.queue.submit([encoder.finish()]);
        let rgba16 = high_precision
            .then(|| self.read_rgba16_texture(&out, width, height))
            .transpose()?;
        let rgba8 = if let Some(values) = &rgba16 {
            values.iter().map(|value| (value / 257) as u8).collect()
        } else {
            self.read_rgba8_texture(&out, width, height)?
        };
        Ok(RenderedImage {
            width,
            height,
            rgba8,
            rgba16,
        })
    }

    /// Render the complete working-resolution image. Full/3×3/4×4 modes
    /// iterate 1024² Malvar tiles; Super2x2 builds an exact Bayer superpixel
    /// image on demand. Cancellation is checked between tiles.
    pub fn render_export(
        &mut self,
        edit: &EditState,
        hdr_pq: bool,
        high_precision: bool,
        output_space: sp_core::color::OutputColorSpace,
        cancelled: &std::sync::atomic::AtomicBool,
        mut progress: impl FnMut(f32),
    ) -> Result<RenderedImage, SpError> {
        use std::sync::atomic::Ordering;
        if edit.resolution.mode == sp_core::edit::ResolutionMode::Super2x2
            && self
                .image
                .as_ref()
                .is_some_and(|image| image.raster.is_none())
        {
            if cancelled.load(Ordering::Relaxed) {
                return Err(SpError::Export("cancelled".into()));
            }
            let output_mode = if hdr_pq {
                2
            } else {
                u32::from(edit.hdr.enabled)
            };
            let out =
                self.render_superpixel_export(edit, output_mode, high_precision, output_space)?;
            progress(1.0);
            return Ok(out);
        }
        let (width, height) = self
            .active_size()
            .ok_or_else(|| SpError::Export("no image is open".into()))?;
        let mut rgba8 = if high_precision {
            Vec::new()
        } else {
            vec![0; (width * height * 4) as usize]
        };
        let mut rgba16 = high_precision.then(|| vec![0; (width * height * 4) as usize]);
        const TILE: u32 = 1024;
        let tiles_x = width.div_ceil(TILE);
        let tiles_y = height.div_ceil(TILE);
        let total = tiles_x * tiles_y;
        let mut completed = 0;
        let slots = std::env::var("SHOPPHOTO_EXPORT_READBACK_SLOTS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(2)
            .clamp(1, 2);
        let mut readbacks = sp_gpu::ReadbackRing::default();
        let mut regions = std::collections::VecDeque::new();
        let readback_ctx = self.ctx.clone();
        let collect = |readbacks: &mut sp_gpu::ReadbackRing,
                       regions: &mut std::collections::VecDeque<TileInfo>,
                       rgba8: &mut Vec<u8>,
                       rgba16: &mut Option<Vec<u16>>|
         -> Result<(), SpError> {
            let bytes = readbacks.collect(&readback_ctx, cancelled)?;
            let info = regions
                .pop_front()
                .ok_or_else(|| SpError::Gpu("Export region missing".into()))?;
            if let Some(output) = rgba16 {
                let values: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|v| u16::from_le_bytes([v[0], v[1]]))
                    .collect();
                validate_rgba16(&values, false, true)?;
                for row in 0..info.h {
                    let src = (row * info.w * 4) as usize;
                    let dst = (((info.y + row) * width + info.x) * 4) as usize;
                    output[dst..dst + (info.w * 4) as usize]
                        .copy_from_slice(&values[src..src + (info.w * 4) as usize]);
                }
            } else {
                if bytes.chunks_exact(4).any(|pixel| pixel[3] != 255) {
                    return Err(SpError::Gpu(
                        "GPU returned an incomplete export tile".into(),
                    ));
                }
                for row in 0..info.h {
                    let src = (row * info.w * 4) as usize;
                    let dst = (((info.y + row) * width + info.x) * 4) as usize;
                    rgba8[dst..dst + (info.w * 4) as usize]
                        .copy_from_slice(&bytes[src..src + (info.w * 4) as usize]);
                }
            }
            Ok(())
        };
        for tile_y in 0..tiles_y {
            for tile_x in 0..tiles_x {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(SpError::Export("cancelled".into()));
                }
                let x = tile_x * TILE;
                let y = tile_y * TILE;
                let w = TILE.min(width - x);
                let h = TILE.min(height - y);
                let output_mode = if hdr_pq {
                    2
                } else {
                    u32::from(edit.hdr.enabled)
                };
                let info = self
                    .render_tile_mode(
                        edit,
                        x,
                        y,
                        w,
                        h,
                        (output_mode, high_precision, false, output_space),
                    )
                    .ok_or_else(|| SpError::Export("tile render failed".into()))?;
                let tile = self
                    .tile
                    .as_ref()
                    .ok_or_else(|| SpError::Export("tile output missing".into()))?;
                readbacks.enqueue(
                    &self.ctx,
                    if high_precision {
                        &tile.out16_tex
                    } else {
                        &tile.out_tex
                    },
                    [info.margin_x, info.margin_y],
                    [info.w, info.h],
                )?;
                regions.push_back(info);
                if readbacks.len() == slots {
                    collect(&mut readbacks, &mut regions, &mut rgba8, &mut rgba16)?;
                    completed += 1;
                    progress(completed as f32 / total as f32);
                }
            }
        }
        while !readbacks.is_empty() {
            collect(&mut readbacks, &mut regions, &mut rgba8, &mut rgba16)?;
            completed += 1;
            progress(completed as f32 / total as f32);
        }

        let factor = edit.resolution.mode.factor();
        if factor == 1 {
            if let Some(values) = &rgba16 {
                rgba8 = values.iter().map(|value| (value / 257) as u8).collect();
            }
            return Ok(RenderedImage {
                width,
                height,
                rgba8,
                rgba16,
            });
        }
        if let Some(values) = rgba16 {
            Ok(box_downsample_rgba16(
                width,
                height,
                &values,
                factor,
                hdr_pq,
                output_space.transfer(),
            ))
        } else {
            Ok(box_downsample_rgba8(
                width,
                height,
                &rgba8,
                factor,
                hdr_pq,
                output_space.transfer(),
            ))
        }
    }

    /// Read the displayed sRGB pixel at normalized active-image coordinates.
    /// This intentionally samples what the photographer sees, so the
    /// defringe eyedropper targets the visible halo after the active look.
    /// It is synchronous but copies only four bytes and runs only on an
    /// explicit click, never during interactive rendering.
    pub fn sample_display_rgb(&self, u: f32, v: f32) -> Result<[u8; 3], SpError> {
        let image = self
            .image
            .as_ref()
            .ok_or_else(|| SpError::Gpu("no image is open".to_owned()))?;
        let x = (u.clamp(0.0, 1.0) * image.proxy_w.saturating_sub(1) as f32).round() as u32;
        let y = (v.clamp(0.0, 1.0) * image.proxy_h.saturating_sub(1) as f32).round() as u32;
        let staging = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fringe eyedropper readback"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fringe eyedropper readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &image.out_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        self.ctx.map_read_buffer(&staging)?;
        let mapped = slice.get_mapped_range();
        let rgb = [mapped[0], mapped[1], mapped[2]];
        drop(mapped);
        staging.unmap();
        Ok(rgb)
    }
}

#[cfg(test)]
mod readback_tests {
    use super::*;
    #[test]
    fn black_images_are_valid_but_unwritten_pixels_and_nonfinite_float_are_rejected() {
        assert!(validate_rgba16(&[0, 0, 0, u16::MAX], false, true).is_ok());
        assert!(validate_rgba16(&[0, 0, 0, 0x3c00], true, true).is_ok());
        assert!(validate_rgba16(&[0, 0, 0, 0], false, true).is_err());
        assert!(validate_rgba16(&[0x7e00, 0, 0, 0x3c00], true, true).is_err());
        assert!(validate_rgba16(
            &[half::f16::from_f32(4.0).to_bits(), 0, 0, 0x3c00],
            true,
            true
        )
        .is_ok());
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use sp_core::color::Transfer;

    #[test]
    fn supersampling_averages_in_the_selected_linear_space() {
        let input8 = [
            0, 0, 0, 255, 64, 64, 64, 255, 128, 128, 128, 255, 255, 255, 255, 255,
        ];
        let input16: Vec<u16> = input8.iter().map(|v| u16::from(*v) * 257).collect();
        for transfer in [Transfer::Srgb, Transfer::AdobeRgb] {
            let mean = [0.0, 64.0 / 255.0, 128.0 / 255.0, 1.0]
                .map(|v| transfer.eotf(v))
                .iter()
                .sum::<f32>()
                / 4.0;
            let expected = transfer.oetf(mean);
            let result8 = box_downsample_rgba8(2, 2, &input8, 2, false, transfer);
            let result16 = box_downsample_rgba16(2, 2, &input16, 2, false, transfer);
            assert_eq!(result8.rgba8[0], (expected * 255.0).round() as u8);
            assert_eq!(
                result16.rgba16.unwrap()[0],
                (expected * 65535.0).round() as u16
            );
        }
        for transfer in [Transfer::Srgb, Transfer::AdobeRgb] {
            let value = 0.4;
            assert_eq!(
                decode_export_channel(value, true, transfer),
                sp_core::color::pq_eotf(value)
            );
        }
    }
}
