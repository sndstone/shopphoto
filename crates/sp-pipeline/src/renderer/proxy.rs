//! Opening an image on the GPU and the interactive proxy render.

use super::*;

impl GpuRenderer {
    /// Upload a decoded RAW and build the proxy chain. Replaces any
    /// previously open image (textures are dropped → freed).
    pub fn open_image(&mut self, raw: &RawImage) -> Result<(), SpError> {
        self.open_image_with_edit(raw, &EditState::default())
    }

    /// Upload a decoded RAW and bake the active lens settings into its first
    /// proxy. Callers that already loaded a sidecar should use this variant
    /// to avoid rebuilding the full-sensor source on the first render.
    pub fn open_image_with_edit(
        &mut self,
        raw: &RawImage,
        edit: &EditState,
    ) -> Result<(), SpError> {
        self.preview_base = None;
        self.image = None; // free the previous image's VRAM first

        // Cloned handle so the staging ring can be borrowed mutably while
        // the rest of the function holds a `&GpuContext` (both are cheap
        // Arc clones of the same device/queue).
        let ctx_handle = self.ctx.clone();
        let ctx = &ctx_handle;
        let cfa_tex = make_texture(
            ctx,
            "cfa",
            raw.width,
            raw.height,
            wgpu::TextureFormat::R16Uint,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        // ~200 MB for a 105 MP frame: goes through the persistently mapped
        // staging ring, never `write_texture` (BUILD.md §10.1).
        self.staging.upload_texture(
            ctx,
            &cfa_tex,
            raw.width,
            raw.height,
            raw.width * 2,
            bytemuck::cast_slice(&raw.cfa_data),
        )?;

        // Superpixel output: one pixel per full Bayer quad of the active area.
        let quarter_w = (raw.active_area.width / 2).max(1);
        let quarter_h = (raw.active_area.height / 2).max(1);
        let quarter_tex = make_texture(
            ctx,
            "quarter",
            quarter_w,
            quarter_h,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        );

        // Proxy: quarter image reduced to the viewport-driven edge budget.
        let scale = (self.proxy_long_edge as f32 / quarter_w.max(quarter_h) as f32).min(1.0);
        let proxy_w = ((quarter_w as f32 * scale) as u32).max(1);
        let proxy_h = ((quarter_h as f32 * scale) as u32).max(1);
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let proxy_src = make_texture(
            ctx,
            "proxy_src",
            proxy_w,
            proxy_h,
            wgpu::TextureFormat::Rgba16Float,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let work = make_texture(
            ctx,
            "work",
            proxy_w,
            proxy_h,
            wgpu::TextureFormat::Rgba16Float,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let work2 = make_texture(
            ctx,
            "work2",
            proxy_w,
            proxy_h,
            wgpu::TextureFormat::Rgba16Float,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let work_nr = make_texture(
            ctx,
            "work_nr",
            proxy_w,
            proxy_h,
            wgpu::TextureFormat::Rgba16Float,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let hdr_display = make_texture(
            ctx,
            "hdr_display",
            proxy_w,
            proxy_h,
            wgpu::TextureFormat::Rgba16Float,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        // COPY_SRC enables readback (golden tests now, export path later).
        let out_tex = make_texture(
            ctx,
            "out",
            proxy_w,
            proxy_h,
            wgpu::TextureFormat::Rgba8Unorm,
            usage | wgpu::TextureUsages::COPY_SRC,
        );

        // Encode: superpixel → downsample. Shading is baked into the proxy
        // source here because it is a pre-demosaic correction; `render`
        // re-runs this pair if the lens settings change afterwards.
        let bake = self.lens_bake(edit);
        let (lens_enable, lens_grid, lens_scale, lens_view) = self.lens_fields(edit);
        let raw_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("raw params"),
                contents: bytemuck::bytes_of(&RawParams {
                    origin_x: raw.active_area.x,
                    origin_y: raw.active_area.y,
                    out_w: quarter_w,
                    out_h: quarter_h,
                    black: raw.black_levels,
                    white: raw.white_levels,
                    cfa: cfa_id(raw.cfa),
                    hot_enable: 1,
                    hot_thresh: 2.0,
                    lens_enable,
                    lens_grid,
                    lens_scale,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let down_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("down params"),
                contents: bytemuck::bytes_of(&SizeParams {
                    src_w: quarter_w,
                    src_h: quarter_h,
                    out_w: proxy_w,
                    out_h: proxy_h,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // cfa_tex itself can drop after this scope: views keep it alive.
        let cfa_view = cfa_tex.create_view(&Default::default());
        let quarter_view = quarter_tex.create_view(&Default::default());
        let proxy_src_view = proxy_src.create_view(&Default::default());

        let bg_super = bind_lookup(
            ctx,
            &self.pipes.superpixel,
            &cfa_view,
            &quarter_view,
            &raw_params,
            lens_view,
        );
        let bg_down = bind(
            ctx,
            &self.pipes.downsample,
            &quarter_view,
            &proxy_src_view,
            &down_params,
        );

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("open_image"),
            });
        run(
            &mut encoder,
            &self.pipes.superpixel,
            &bg_super,
            quarter_w,
            quarter_h,
        );
        run(
            &mut encoder,
            &self.pipes.downsample,
            &bg_down,
            proxy_w,
            proxy_h,
        );
        ctx.queue.submit([encoder.finish()]);

        let hist_bins = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist bins"),
            size: (HISTOGRAM_BINS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hist_staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist staging"),
            size: (HISTOGRAM_BINS * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let out_view = out_tex.create_view(&Default::default());
        self.image = Some(LoadedImage {
            resident_texture_bytes: u64::from(raw.width) * u64::from(raw.height) * 2
                + u64::from(quarter_w) * u64::from(quarter_h) * 8
                + u64::from(proxy_w) * u64::from(proxy_h) * 44,
            raster: None,
            proxy_src_view,
            proxy_src_tex: proxy_src,
            work_view: work.create_view(&Default::default()),
            work2_view: work2.create_view(&Default::default()),
            work_nr_view: work_nr.create_view(&Default::default()),
            hdr_display_view: hdr_display.create_view(&Default::default()),
            hdr_display_tex: hdr_display,
            out_tex,
            out_view,
            proxy_w,
            proxy_h,
            cam_to_xyz: raw.cam_to_xyz,
            wb_coeffs: raw.wb_coeffs,
            cfa_view,
            cfa_pattern: raw.cfa,
            black_levels: raw.black_levels,
            white_levels: raw.white_levels,
            active: raw.active_area,
            hist_bins,
            hist_staging,
            hist_rx: None,
            airlight: 0.95,
            quarter_view,
            quarter_w,
            quarter_h,
            lens_bake: bake,
        });
        self.tile = None; // sized for the previous image
        Ok(())
    }

    /// Re-run superpixel → downsample so the proxy source carries the
    /// current lens shading.
    ///
    /// Shading is a pre-demosaic correction, so it is baked into the source
    /// the interactive renders start from rather than applied per frame.
    /// That means changing it costs one full-sensor pass — the same work
    /// `open_image` already does once — which is why this only runs when the
    /// baked state actually differs.
    pub(super) fn rebuild_proxy_source(&mut self, edit: &EditState) {
        let want = self.lens_bake(edit);
        let Some(img) = &self.image else { return };
        if img.raster.is_some() || img.lens_bake == want {
            return;
        }
        self.preview_base = None;
        let (lens_enable, lens_grid, lens_scale, lens_view) = self.lens_fields(edit);
        let ctx = &self.ctx;
        let raw_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("raw params (lens rebuild)"),
                contents: bytemuck::bytes_of(&RawParams {
                    origin_x: img.active.x,
                    origin_y: img.active.y,
                    out_w: img.quarter_w,
                    out_h: img.quarter_h,
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
        let down_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("down params (lens rebuild)"),
                contents: bytemuck::bytes_of(&SizeParams {
                    src_w: img.quarter_w,
                    src_h: img.quarter_h,
                    out_w: img.proxy_w,
                    out_h: img.proxy_h,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bg_super = bind_lookup(
            ctx,
            &self.pipes.superpixel,
            &img.cfa_view,
            &img.quarter_view,
            &raw_params,
            lens_view,
        );
        let bg_down = bind(
            ctx,
            &self.pipes.downsample,
            &img.quarter_view,
            &img.proxy_src_view,
            &down_params,
        );
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lens rebuild"),
            });
        run(
            &mut encoder,
            &self.pipes.superpixel,
            &bg_super,
            img.quarter_w,
            img.quarter_h,
        );
        run(
            &mut encoder,
            &self.pipes.downsample,
            &bg_down,
            img.proxy_w,
            img.proxy_h,
        );
        ctx.queue.submit([encoder.finish()]);
        self.image.as_mut().expect("image open").lens_bake = want;
    }

    /// Re-run the color passes for the current edit. Cheap (proxy res);
    /// call whenever `EditState` changes. No-op when no image is open.
    /// Returns a previously completed asynchronous histogram, if one was
    /// ready at entry. The histogram scheduled by this render is collected by
    /// a later [`Self::poll_histogram`] call.
    pub fn render(&mut self, edit: &EditState) -> Option<Vec<u32>> {
        self.render_impl(edit, true, false)
    }

    /// Render into the display targets without consuming or scheduling a
    /// histogram readback. Auto Balance uses this immediately after its
    /// neutral analysis pass so the user never sees the analysis frame.
    pub fn render_without_histogram(&mut self, edit: &EditState) {
        let _ = self.render_impl(edit, false, false);
    }

    /// Render an uncorrected frame and populate the two extra fringe-score
    /// histogram planes. The caller should immediately restore the visible
    /// edit with [`Self::render_without_histogram`], just like Auto Balance.
    pub fn render_for_fringe_analysis(&mut self, edit: &EditState) -> Option<Vec<u32>> {
        self.render_impl(edit, true, true)
    }

    pub(super) fn render_impl(
        &mut self,
        edit: &EditState,
        histogram_io: bool,
        fringe_analysis: bool,
    ) -> Option<Vec<u32>> {
        let completed_histogram = histogram_io.then(|| self.poll_histogram()).flatten();
        self.rebuild_proxy_source(edit);
        let Some(img) = &self.image else {
            return None;
        };
        let effective_nr = self.effective_nr(edit);
        let prefix_key = EditState {
            wb: edit.wb,
            negative: edit.negative,
            highlight_recovery: edit.highlight_recovery,
            look: edit.look.clone(),
            look_strength: edit.look_strength,
            profile: edit.profile.clone(),
            lens: edit.lens.clone(),
            ..EditState::default()
        };
        let bytes = u64::from(img.proxy_w) * u64::from(img.proxy_h) * 8;
        let cache_allowed = self.preview_cache_enabled && bytes <= 64 * 1024 * 1024;
        let reuse_prefix = cache_allowed
            && self
                .preview_base
                .as_ref()
                .is_some_and(|base| base.key == prefix_key && base.nr == effective_nr);
        if reuse_prefix {
            self.preview_cache_hits += 1;
        } else {
            self.preview_cache_misses += 1;
            if cache_allowed {
                let same_size = self.preview_base.as_ref().is_some_and(|base| {
                    base.texture.width() == img.proxy_w && base.texture.height() == img.proxy_h
                });
                if same_size {
                    if let Some(base) = &mut self.preview_base {
                        base.key = prefix_key;
                        base.nr = effective_nr;
                    }
                } else {
                    let texture = make_texture(
                        &self.ctx,
                        "preview prefix cache",
                        img.proxy_w,
                        img.proxy_h,
                        wgpu::TextureFormat::Rgba16Float,
                        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    );
                    let view = texture.create_view(&Default::default());
                    self.preview_base = Some(PreviewBase {
                        key: prefix_key,
                        nr: effective_nr,
                        texture,
                        view,
                    });
                }
            } else {
                self.preview_base = None;
            }
        }
        let ctx = &self.ctx;

        let display_mode = u32::from(edit.hdr.enabled);
        let bufs = self.color_buffers(
            edit,
            OutputSpec {
                size: [img.proxy_w, img.proxy_h],
                mode: display_mode,
                origin: [0, 0],
                full_size: [img.proxy_w, img.proxy_h],
                view_transform: true,
                export_space: None,
            },
        );
        let hist_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("hist params"),
                contents: bytemuck::bytes_of(&HistogramParams {
                    src_w: img.proxy_w,
                    src_h: img.proxy_h,
                    fringe_analysis: u32::from(fringe_analysis),
                    working_w: img.active.width,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let bg_wb = bind_lookup(
            ctx,
            &self.pipes.wb_color,
            &img.proxy_src_view,
            &img.work_view,
            &bufs.wb_params,
            &bufs.warp_view,
        );
        let nr_on = effective_nr[0] > 0.0 || effective_nr[2] > 0.0;
        let bg_profile_pre = self.profile_bind_group(&img.work_view, &img.work2_view, &bufs, false);
        let (bg_nr, tone_input) = if nr_on {
            let nr_params = self.nr_params_buffer(edit, img.proxy_w, img.proxy_h);
            (
                Some(bind(
                    ctx,
                    &self.pipes.noise,
                    &img.work2_view,
                    &img.work_nr_view,
                    &nr_params,
                )),
                &img.work_nr_view,
            )
        } else {
            (None, &img.work2_view)
        };
        let original_tone_input = tone_input;
        let tone_input = self
            .preview_base
            .as_ref()
            .map_or(tone_input, |base| &base.view);
        let bg_tone = self.tone_bind_group(tone_input, &img.work_view, &bufs);
        let bg_profile_post = self.profile_bind_group(&img.work_view, &img.work2_view, &bufs, true);
        let defringe_on = edit.lens.enable
            && edit.lens.defringe
            && (edit.lens.defringe_purple > 0.0 || edit.lens.defringe_green > 0.0);
        let mut tail_steps = 0u32;
        let tail_input = |step: u32| match step {
            0 => &img.work2_view,
            step if step % 2 == 1 => &img.work_nr_view,
            _ => &img.work_view,
        };
        let tail_output = |step: u32| {
            if step.is_multiple_of(2) {
                &img.work_nr_view
            } else {
                &img.work_view
            }
        };
        let bg_defringe = defringe_on.then(|| {
            let proxy_scale = img.proxy_w as f32 / img.active.width.max(1) as f32;
            let params = self.defringe_params_buffer(edit, img.proxy_w, img.proxy_h, proxy_scale);
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
            let params = self.local_params_buffer(edit, img.proxy_w, img.proxy_h, img.proxy_w);
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
        let striped_retouch =
            retouch::retouch_stripe_size(edit, [img.proxy_w, img.proxy_h]).is_some();
        let retouch_step = tail_steps;
        let retouch_buffers = if striped_retouch {
            None
        } else {
            self.retouch_buffers(
                edit,
                OutputSpec {
                    size: [img.proxy_w, img.proxy_h],
                    mode: display_mode,
                    origin: [0, 0],
                    full_size: [img.proxy_w, img.proxy_h],
                    view_transform: true,
                    export_space: None,
                },
            )
        };
        // Record coverage before retouch overwrites the ping-pong source.
        // Evaluating earlier records preserves the actual luma/color context.
        let preview_index = self
            .mask_preview_index
            .filter(|&index| edit.local_adjustments.get(index).is_some_and(|a| a.enabled));
        let preview_params = preview_index.map(|index| {
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("local coverage parameters"),
                    contents: bytemuck::bytes_of(&RetouchGpuParams {
                        out_w: img.proxy_w,
                        out_h: img.proxy_h,
                        origin_x: 0,
                        origin_y: 0,
                        full_w: img.proxy_w,
                        full_h: img.proxy_h,
                        local_count: edit.local_adjustments.iter().filter(|a| a.enabled).count()
                            as u32,
                        spot_count: edit.spots.len() as u32,
                        source_origin_x: 0,
                        source_origin_y: 0,
                        patch_source: 0,
                        storage_origin: [0; 2],
                        _region_pad: [0; 2],
                        preview_mask: edit.local_adjustments[..=index]
                            .iter()
                            .filter(|a| a.enabled)
                            .count() as u32,
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
        });
        let preview_texture = preview_params.as_ref().map(|_| {
            make_texture(
                ctx,
                "local coverage",
                img.proxy_w,
                img.proxy_h,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
            )
        });
        let preview_view = preview_texture
            .as_ref()
            .map(|texture| texture.create_view(&Default::default()));
        let bg_preview = preview_params
            .as_ref()
            .zip(preview_view.as_ref())
            .zip(retouch_buffers.as_ref())
            .map(|((params, view), buffers)| {
                bind_retouch(
                    ctx,
                    &self.pipes.retouch,
                    [
                        tail_input(tail_steps),
                        view,
                        tail_input(tail_steps),
                        tail_input(tail_steps),
                    ],
                    [params, &buffers.records, &buffers.points],
                )
            });
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
            let params = self.sharpen_params_buffer(edit, img.proxy_w, img.proxy_h);
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
        let analysis_scale =
            (self.analysis_long_edge as f32 / img.proxy_w.max(img.proxy_h) as f32).min(1.0);
        let analysis_size = [
            (img.proxy_w as f32 * analysis_scale).round().max(1.0) as u32,
            (img.proxy_h as f32 * analysis_scale).round().max(1.0) as u32,
        ];
        let analysis_texture = self.analysis_enabled.then(|| {
            make_texture(
                ctx,
                "scene analysis",
                analysis_size[0],
                analysis_size[1],
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            )
        });
        let bg_analysis = analysis_texture.as_ref().map(|texture| {
            let params = ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scene analysis size"),
                    contents: bytemuck::bytes_of(&SizeParams {
                        src_w: img.proxy_w,
                        src_h: img.proxy_h,
                        out_w: analysis_size[0],
                        out_h: analysis_size[1],
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            bind(
                ctx,
                &self.pipes.downsample,
                output_input,
                &texture.create_view(&Default::default()),
                &params,
            )
        });
        let bg_out = bind_lookup(
            ctx,
            &self.pipes.output,
            output_input,
            &img.out_view,
            &bufs.out_params,
            &bufs.proof_view,
        );
        // scRGB float output for Metal EDR display. Runs for EVERY render
        // on a float surface: SDR edits get a hard 1.0 ceiling (identical
        // look to the 8-bit path but with 16-bit precision on screen), HDR
        // edits get real headroom.
        let bg_scrgb = self.hdr_display.then(|| {
            bind_lookup(
                ctx,
                &self.pipes.output_scrgb,
                output_input,
                &img.hdr_display_view,
                &bufs.out_params,
                &bufs.proof_view,
            )
        });
        let bg_hist = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hist"),
            layout: &self.pipes.histogram.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(output_input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: img.hist_bins.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: hist_params.as_entire_binding(),
                },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render"),
            });
        if !reuse_prefix {
            run(
                &mut encoder,
                &self.pipes.wb_color,
                &bg_wb,
                img.proxy_w,
                img.proxy_h,
            );
            run(
                &mut encoder,
                &self.pipes.profile_color,
                &bg_profile_pre,
                img.proxy_w,
                img.proxy_h,
            );
            if let Some(bg) = &bg_nr {
                run(
                    &mut encoder,
                    &self.pipes.noise,
                    bg,
                    img.proxy_w,
                    img.proxy_h,
                );
            }
            if let Some(base) = &self.preview_base {
                encoder.copy_texture_to_texture(
                    original_tone_input.texture().as_image_copy(),
                    base.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: img.proxy_w,
                        height: img.proxy_h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        run(
            &mut encoder,
            &self.pipes.tone_color,
            &bg_tone,
            img.proxy_w,
            img.proxy_h,
        );
        run(
            &mut encoder,
            &self.pipes.profile_color,
            &bg_profile_post,
            img.proxy_w,
            img.proxy_h,
        );
        if let Some(bg) = &bg_defringe {
            run(
                &mut encoder,
                &self.pipes.defringe,
                bg,
                img.proxy_w,
                img.proxy_h,
            );
        }
        if let Some(bg) = &bg_local {
            run(
                &mut encoder,
                &self.pipes.local_contrast,
                bg,
                img.proxy_w,
                img.proxy_h,
            );
        }
        if let Some(bg) = &bg_preview {
            run(
                &mut encoder,
                &self.pipes.retouch,
                bg,
                img.proxy_w,
                img.proxy_h,
            );
        }
        if let Some(bg) = &bg_retouch {
            run(
                &mut encoder,
                &self.pipes.retouch,
                bg,
                img.proxy_w,
                img.proxy_h,
            );
        }
        if striped_retouch {
            self.run_retouch_stripes(
                &mut encoder,
                edit,
                OutputSpec {
                    size: [img.proxy_w, img.proxy_h],
                    mode: display_mode,
                    origin: [0, 0],
                    full_size: [img.proxy_w, img.proxy_h],
                    view_transform: true,
                    export_space: None,
                },
                [
                    tail_input(retouch_step),
                    tail_output(retouch_step),
                    tail_input(retouch_step),
                    tail_input(retouch_step),
                ],
                preview_index
                    .zip(preview_view.as_ref())
                    .map(|(index, view)| {
                        (
                            edit.local_adjustments[..=index]
                                .iter()
                                .filter(|a| a.enabled)
                                .count() as u32,
                            view,
                        )
                    }),
            );
        }
        if let Some(bg) = &bg_sharpen {
            run(
                &mut encoder,
                &self.pipes.sharpen,
                bg,
                img.proxy_w,
                img.proxy_h,
            );
        }
        if let Some(bg) = &bg_analysis {
            run(
                &mut encoder,
                &self.pipes.downsample,
                bg,
                analysis_size[0],
                analysis_size[1],
            );
        }
        run(
            &mut encoder,
            &self.pipes.output,
            &bg_out,
            img.proxy_w,
            img.proxy_h,
        );
        if let Some(bg) = &bg_scrgb {
            run(
                &mut encoder,
                &self.pipes.output_scrgb,
                bg,
                img.proxy_w,
                img.proxy_h,
            );
        }
        let schedule_histogram = histogram_io && img.hist_rx.is_none();
        if schedule_histogram {
            encoder.clear_buffer(&img.hist_bins, 0, None);
            run(
                &mut encoder,
                &self.pipes.histogram,
                &bg_hist,
                img.proxy_w,
                img.proxy_h,
            );
            encoder.copy_buffer_to_buffer(
                &img.hist_bins,
                0,
                &img.hist_staging,
                0,
                (HISTOGRAM_BINS * 4) as u64,
            );
        }
        ctx.queue.submit([encoder.finish()]);
        if schedule_histogram {
            let (sender, receiver) = std::sync::mpsc::channel();
            img.hist_staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            if let Some(image) = self.image.as_mut() {
                image.hist_rx = Some(receiver);
            }
        }
        self.analysis_texture = analysis_texture;
        self.mask_preview_texture = preview_texture;
        self.mask_preview_view = preview_view;
        completed_histogram
    }

    /// Consume a completed asynchronous histogram readback without stalling
    /// the UI thread. Called every logic frame and before scheduling a new
    /// reduction.
    pub fn poll_histogram(&mut self) -> Option<Vec<u32>> {
        let image = self.image.as_mut()?;
        let receiver = image.hist_rx.as_ref()?;
        let _ = self.ctx.device.poll(wgpu::PollType::Poll);
        match receiver.try_recv() {
            Ok(Ok(())) => {
                let slice = image.hist_staging.slice(..);
                let bins = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
                image.hist_staging.unmap();
                image.hist_rx = None;
                // Conservative min-channel 99.9th-percentile approximation.
                let percentile = |plane: usize| {
                    let slice = &bins[plane * 256..(plane + 1) * 256];
                    let total: u64 = slice.iter().map(|&value| u64::from(value)).sum();
                    let target = (total as f64 * 0.999) as u64;
                    let mut sum = 0_u64;
                    let index = slice
                        .iter()
                        .position(|&value| {
                            sum += u64::from(value);
                            sum >= target
                        })
                        .unwrap_or(223);
                    let encoded = (index.min(223) as f32 / 223.0).clamp(0.0, 1.0);
                    sp_core::color::srgb_eotf(encoded)
                };
                image.airlight = percentile(0)
                    .min(percentile(1))
                    .min(percentile(2))
                    .max(0.05);
                Some(bins)
            }
            Ok(Err(error)) => {
                tracing::warn!("histogram map failed: {error}");
                image.hist_rx = None;
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                image.hist_rx = None;
                None
            }
        }
    }

    /// Whether a histogram reduction/readback is currently in flight.
    #[must_use]
    pub fn histogram_pending(&self) -> bool {
        self.image
            .as_ref()
            .is_some_and(|image| image.hist_rx.is_some())
    }
}
