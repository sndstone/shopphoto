//! Full-resolution 1:1 tiles for the zoomed viewport and the noise loupe.

use super::*;

/// Demosaic margin around a tile so Malvar/AHD neighbourhoods never read
/// synthetic edge clamps inside the visible region.
pub(super) const DEMOSAIC_TILE_MARGIN: u32 = 4;

pub(super) const LOCAL_FILTER_RADIUS: u32 = 32;

impl GpuRenderer {
    pub(super) fn ensure_tile_resources(&mut self, tex_w: u32, tex_h: u32) {
        let needs_new = self
            .tile
            .as_ref()
            .is_none_or(|t| t.tex_w != tex_w || t.tex_h != tex_h);
        if !needs_new {
            return;
        }
        let ctx = &self.ctx;
        let usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let prepared = make_texture(
            ctx,
            "tile_prep",
            tex_w,
            tex_h,
            wgpu::TextureFormat::R32Float,
            usage,
        );
        let prepared_nr = make_texture(
            ctx,
            "tile_prep_nr",
            tex_w,
            tex_h,
            wgpu::TextureFormat::R32Float,
            usage,
        );
        let demosaiced = make_texture(
            ctx,
            "tile_demo",
            tex_w,
            tex_h,
            wgpu::TextureFormat::Rgba16Float,
            usage | wgpu::TextureUsages::COPY_DST,
        );
        let work = make_texture(
            ctx,
            "tile_work",
            tex_w,
            tex_h,
            wgpu::TextureFormat::Rgba16Float,
            usage,
        );
        let work2 = make_texture(
            ctx,
            "tile_work2",
            tex_w,
            tex_h,
            wgpu::TextureFormat::Rgba16Float,
            usage,
        );
        let out_tex = make_texture(
            ctx,
            "tile_out",
            tex_w,
            tex_h,
            wgpu::TextureFormat::Rgba8Unorm,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let out16_tex = make_texture(
            ctx,
            "tile_out16",
            tex_w,
            tex_h,
            wgpu::TextureFormat::Rgba16Uint,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let hdr_display = make_texture(
            ctx,
            "tile_hdr_display",
            tex_w,
            tex_h,
            wgpu::TextureFormat::Rgba16Float,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let work_nr = make_texture(
            ctx,
            "tile_work_nr",
            tex_w,
            tex_h,
            wgpu::TextureFormat::Rgba16Float,
            usage,
        );
        let out_view = out_tex.create_view(&Default::default());
        let out16_view = out16_tex.create_view(&Default::default());
        self.tile = Some(TileResources {
            work_nr: work_nr.create_view(&Default::default()),
            work_nr_tex: work_nr,
            prepared: prepared.create_view(&Default::default()),
            prepared_nr: prepared_nr.create_view(&Default::default()),
            demosaiced: demosaiced.create_view(&Default::default()),
            demosaiced_tex: demosaiced,
            work: work.create_view(&Default::default()),
            work_tex: work,
            work2: work2.create_view(&Default::default()),
            work2_tex: work2,
            scene_output: None,
            out_tex,
            out_view,
            hdr_display_view: hdr_display.create_view(&Default::default()),
            hdr_display_tex: hdr_display,
            out16_tex,
            out16_view,
            tex_w,
            tex_h,
        });
    }

    /// Render a full-resolution tile covering the visible
    /// region `(x, y, w, h)` in active-area coordinates. Returns the
    /// realized tile geometry; `None` if no image is open.
    ///
    /// The tile texture includes the required demosaic/spatial extra pixels
    /// so the demosaic window is honest inside the visible region — the
    /// caller crops margins via UV using the returned [`TileInfo`].
    pub fn render_tile(
        &mut self,
        edit: &EditState,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) -> Option<TileInfo> {
        self.render_tile_mode(
            edit,
            x,
            y,
            w,
            h,
            (
                u32::from(edit.hdr.enabled),
                false,
                true,
                sp_core::color::OutputColorSpace::Srgb,
            ),
        )
    }

    pub(super) fn render_tile_mode(
        &mut self,
        edit: &EditState,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        output: (u32, bool, bool, sp_core::color::OutputColorSpace),
    ) -> Option<TileInfo> {
        if edit
            .spots
            .iter()
            .any(|spot| spot.enabled && spot.opacity > 0.0)
        {
            self.render_tile_with_spot_sources(edit, [x, y, w, h], output)
        } else {
            self.render_tile_mode_inner(edit, x, y, w, h, output, false)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_tile_mode_inner(
        &mut self,
        edit: &EditState,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        output: (u32, bool, bool, sp_core::color::OutputColorSpace),
        scene_only: bool,
    ) -> Option<TileInfo> {
        let (output_mode, high_precision, view_transform, output_space) = output;
        let img = self.image.as_ref()?;
        let (aw, ah) = (img.active.width, img.active.height);
        let x = x.min(aw.saturating_sub(1));
        let y = y.min(ah.saturating_sub(1));
        let w = w.clamp(1, aw - x);
        let h = h.clamp(1, ah - y);

        // Padded region, clamped to the active area. Spatial contrast needs
        // its full sampling apron so tiled and untiled results agree.
        let defringe_on = edit.lens.enable
            && edit.lens.defringe
            && (edit.lens.defringe_purple > 0.0 || edit.lens.defringe_green > 0.0);
        let fringe_margin = if defringe_on {
            edit.lens.defringe_radius_px.ceil() as u32 + 1
        } else {
            0
        };
        // Sequential spatial passes need the sum of their reaches, rather
        // than only the largest radius: each pass consumes the previous halo.
        let nr = self.effective_nr(edit);
        let raw_nr_margin = if img.raster.is_none() && nr[0] > 0.0 {
            4
        } else {
            0
        };
        let rgb_nr_margin = if nr[0] > 0.0 || nr[2] > 0.0 { 3 } else { 0 };
        let global_local_margin = if edit.local.is_neutral() {
            0
        } else {
            LOCAL_FILTER_RADIUS
        };
        let masked_local_margin = if edit.local_adjustments.iter().any(|a| {
            a.enabled
                && (!a.local.is_neutral()
                    || a.sharpness > 0.0
                    || a.noise_reduction > 0.0
                    || a.corrective.moire > 0.0)
        }) {
            LOCAL_FILTER_RADIUS
        } else {
            0
        };
        let sharpen_margin = if edit.sharpen.amount > 0.0 {
            edit.sharpen.radius_px.ceil() as u32
        } else {
            0
        };
        let tile_margin = DEMOSAIC_TILE_MARGIN
            + raw_nr_margin
            + rgb_nr_margin
            + fringe_margin
            + global_local_margin
            + masked_local_margin
            + sharpen_margin
            + if edit.sharpen.restore_blur > 0.0 {
                2 * edit.sharpen.restore_radius_px.ceil() as u32
            } else {
                0
            }
            + if edit
                .spots
                .iter()
                .any(|spot| spot.enabled && !spot.clone && spot.heal_texture > 0.0)
            {
                8
            } else {
                0
            };
        let px0 = x.saturating_sub(tile_margin);
        let py0 = y.saturating_sub(tile_margin);
        let px1 = (x + w + tile_margin).min(aw);
        let py1 = (y + h + tile_margin).min(ah);
        let (tex_w, tex_h) = (px1 - px0, py1 - py0);
        let info = TileInfo {
            x,
            y,
            w,
            h,
            margin_x: x - px0,
            margin_y: y - py0,
            tex_w,
            tex_h,
        };

        self.ensure_tile_resources(tex_w, tex_h);
        // Shading indexes absolute sensor coordinates, which raw_prepare
        // already reconstructs from `origin_*`, so tiles need no special
        // handling — a tile lands on the same grid cells as the full frame.
        let (lens_enable, lens_grid, lens_scale, lens_view) = self.lens_fields(edit);
        let img = self.image.as_ref().expect("checked above");
        let ctx = &self.ctx;

        // Raw prepare over the padded region (absolute sensor coords).
        let prep_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tile prep params"),
                contents: bytemuck::bytes_of(&RawParams {
                    origin_x: img.active.x + px0,
                    origin_y: img.active.y + py0,
                    out_w: tex_w,
                    out_h: tex_h,
                    black: img.black_levels,
                    white: img.white_levels,
                    cfa: cfa_id(img.cfa_pattern),
                    hot_enable: u32::from(edit.nr.hot_pixels),
                    hot_thresh: 1.5,
                    lens_enable,
                    lens_grid,
                    lens_scale,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        // Malvar works in tile-local coords: shift the pattern to the tile
        // origin's parity (absolute sensor position of the tile corner).
        let local_cfa = img
            .cfa_pattern
            .shifted((img.active.x + px0) & 1, (img.active.y + py0) & 1);
        let demosaic_pass = if edit.high_detail_demosaic {
            &self.pipes.ahd
        } else {
            &self.pipes.malvar
        };
        let demosaic_params = if edit.high_detail_demosaic {
            self.ahd_params_buffer(edit, local_cfa, tex_w, tex_h)
        } else {
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tile malvar params"),
                    contents: bytemuck::bytes_of(&MalvarParams {
                        out_w: tex_w,
                        out_h: tex_h,
                        cfa: cfa_id(local_cfa),
                        _pad: 0,
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
        };
        let bufs = self.color_buffers(
            edit,
            OutputSpec {
                size: [tex_w, tex_h],
                mode: output_mode,
                origin: [px0, py0],
                full_size: [aw, ah],
                view_transform,
                export_space: (!view_transform).then_some(output_space),
            },
        );
        let img = self.image.as_ref().expect("still open");
        let tile = self.tile.as_ref().expect("still allocated");

        let bg_prep = bind_lookup(
            ctx,
            &self.pipes.raw_prepare,
            &img.cfa_view,
            &tile.prepared,
            &prep_params,
            lens_view,
        );
        // Pre-demosaic NR (`plan_passes` schedules it whenever luminance NR
        // is engaged): raw_prepare → nr_raw → Malvar, so the demosaic
        // interpolates denoised photosites.
        let raw_nr_on = img.raster.is_none() && self.effective_nr(edit)[0] > 0.0;
        let bg_raw_nr = raw_nr_on.then(|| {
            let params = self.nr_params_buffer(edit, tex_w, tex_h);
            bind(
                ctx,
                &self.pipes.raw_denoise,
                &tile.prepared,
                &tile.prepared_nr,
                &params,
            )
        });
        let bg_demosaic = bind(
            ctx,
            demosaic_pass,
            if raw_nr_on {
                &tile.prepared_nr
            } else {
                &tile.prepared
            },
            &tile.demosaiced,
            &demosaic_params,
        );
        let bg_wb = bind_lookup(
            ctx,
            &self.pipes.wb_color,
            &tile.demosaiced,
            &tile.work,
            &bufs.wb_params,
            &bufs.warp_view,
        );
        let effective_nr = self.effective_nr(edit);
        let nr_on = effective_nr[0] > 0.0 || effective_nr[2] > 0.0;
        let bg_profile_pre = self.profile_bind_group(&tile.work, &tile.work2, &bufs, false);
        let (bg_nr, tone_input) = if nr_on {
            let nr_params = self.nr_params_buffer(edit, tex_w, tex_h);
            (
                Some(bind(
                    ctx,
                    &self.pipes.noise,
                    &tile.work2,
                    &tile.work_nr,
                    &nr_params,
                )),
                &tile.work_nr,
            )
        } else {
            (None, &tile.work2)
        };
        let bg_tone = self.tone_bind_group(tone_input, &tile.work, &bufs);
        let bg_profile_post = self.profile_bind_group(&tile.work, &tile.work2, &bufs, true);
        let mut tail_steps = 0u32;
        let tail_input = |step: u32| match step {
            0 => &tile.work2,
            step if step % 2 == 1 => &tile.work_nr,
            _ => &tile.work,
        };
        let tail_output = |step: u32| {
            if step.is_multiple_of(2) {
                &tile.work_nr
            } else {
                &tile.work
            }
        };
        let bg_defringe = defringe_on.then(|| {
            let params = self.defringe_params_buffer(edit, tex_w, tex_h, 1.0);
            let group = bind(
                ctx,
                &self.pipes.defringe,
                tail_input(tail_steps),
                tail_output(tail_steps),
                &params,
            );
            tail_steps += 1;
            group
        });
        let local_on = !edit.local.is_neutral();
        let bg_local = local_on.then(|| {
            let params = self.local_params_buffer(edit, tex_w, tex_h, aw);
            let group = bind(
                ctx,
                &self.pipes.local_contrast,
                tail_input(tail_steps),
                tail_output(tail_steps),
                &params,
            );
            tail_steps += 1;
            group
        });
        let striped_retouch = retouch::retouch_stripe_size(edit, [tex_w, tex_h]).is_some();
        let retouch_step = tail_steps;
        let retouch_buffers = if striped_retouch {
            None
        } else {
            self.retouch_buffers(
                edit,
                OutputSpec {
                    size: [tex_w, tex_h],
                    mode: output_mode,
                    origin: [px0, py0],
                    full_size: [aw, ah],
                    view_transform,
                    export_space: (!view_transform).then_some(output_space),
                },
            )
        };
        let bg_retouch = retouch_buffers.as_ref().map(|buffers| {
            let group = bind_retouch(
                ctx,
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
        if striped_retouch {
            tail_steps += 1;
        }
        let sharpen_on = edit.sharpen.amount > 0.0 || edit.sharpen.restore_blur > 0.0;
        let bg_sharpen = sharpen_on.then(|| {
            let params = self.sharpen_params_buffer(edit, tex_w, tex_h);
            let group = bind(
                ctx,
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
        let output_view = if high_precision {
            &tile.out16_view
        } else {
            &tile.out_view
        };
        let bg_out = bind_lookup(
            ctx,
            output_pass,
            output_input,
            output_view,
            &bufs.out_params,
            &bufs.proof_view,
        );
        let bg_scrgb = self.hdr_display.then(|| {
            bind_lookup(
                ctx,
                &self.pipes.output_scrgb,
                output_input,
                &tile.hdr_display_view,
                &bufs.out_params,
                &bufs.proof_view,
            )
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("tile"),
            });
        if let Some(raster) = &img.raster {
            // Source pixels already carry their ICC-converted RGB values.
            // Upload the requested upright region directly, without sensor
            // preparation, CFA denoise or demosaicing.
            let mut pixels = Vec::with_capacity(tex_w as usize * tex_h as usize);
            for row in py0..py0 + tex_h {
                let start = row as usize * raster.width as usize + px0 as usize;
                pixels.extend(
                    raster.linear_rec2020[start..start + tex_w as usize]
                        .iter()
                        .map(|p| p.map(half::f16::from_f32)),
                );
            }
            self.staging
                .upload_texture(
                    ctx,
                    &tile.demosaiced_tex,
                    tex_w,
                    tex_h,
                    tex_w * 8,
                    bytemuck::cast_slice(&pixels),
                )
                .ok()?;
        } else {
            run(
                &mut encoder,
                &self.pipes.raw_prepare,
                &bg_prep,
                tex_w,
                tex_h,
            );
            if let Some(bg) = &bg_raw_nr {
                run(&mut encoder, &self.pipes.raw_denoise, bg, tex_w, tex_h);
            }
            run(&mut encoder, demosaic_pass, &bg_demosaic, tex_w, tex_h);
        }
        run(&mut encoder, &self.pipes.wb_color, &bg_wb, tex_w, tex_h);
        run(
            &mut encoder,
            &self.pipes.profile_color,
            &bg_profile_pre,
            tex_w,
            tex_h,
        );
        if let Some(bg) = &bg_nr {
            run(&mut encoder, &self.pipes.noise, bg, tex_w, tex_h);
        }
        run(&mut encoder, &self.pipes.tone_color, &bg_tone, tex_w, tex_h);
        run(
            &mut encoder,
            &self.pipes.profile_color,
            &bg_profile_post,
            tex_w,
            tex_h,
        );
        if let Some(bg) = &bg_defringe {
            run(&mut encoder, &self.pipes.defringe, bg, tex_w, tex_h);
        }
        if let Some(bg) = &bg_local {
            run(&mut encoder, &self.pipes.local_contrast, bg, tex_w, tex_h);
        }
        if let Some(bg) = &bg_retouch {
            run(&mut encoder, &self.pipes.retouch, bg, tex_w, tex_h);
        }
        if striped_retouch {
            self.run_retouch_stripes(
                &mut encoder,
                edit,
                OutputSpec {
                    size: [tex_w, tex_h],
                    mode: output_mode,
                    origin: [px0, py0],
                    full_size: [aw, ah],
                    view_transform,
                    export_space: (!view_transform).then_some(output_space),
                },
                [
                    tail_input(retouch_step),
                    tail_output(retouch_step),
                    tail_input(retouch_step),
                    tail_input(retouch_step),
                ],
                None,
            );
        }
        if let Some(bg) = &bg_sharpen {
            run(&mut encoder, &self.pipes.sharpen, bg, tex_w, tex_h);
        }
        if !scene_only {
            run(&mut encoder, output_pass, &bg_out, tex_w, tex_h);
            if let Some(bg) = &bg_scrgb {
                run(&mut encoder, &self.pipes.output_scrgb, bg, tex_w, tex_h);
            }
        }
        let scene = match tail_steps {
            0 => tile.work2_tex.clone(),
            step if !step.is_multiple_of(2) => tile.work_nr_tex.clone(),
            _ => tile.work_tex.clone(),
        };
        ctx.queue.submit([encoder.finish()]);
        self.tile.as_mut()?.scene_output = Some(scene);
        Some(info)
    }

    /// The tile output texture view (valid after `render_tile`).
    pub fn tile_view(&self) -> Option<&wgpu::TextureView> {
        self.tile.as_ref().map(|t| &t.out_view)
    }

    pub fn tile_hdr_display_view(&self) -> Option<&wgpu::TextureView> {
        self.tile.as_ref().map(|tile| &tile.hdr_display_view)
    }

    /// The tile output texture (readback in tests/export).
    pub fn tile_texture(&self) -> Option<&wgpu::Texture> {
        self.tile.as_ref().map(|t| &t.out_tex)
    }
}
