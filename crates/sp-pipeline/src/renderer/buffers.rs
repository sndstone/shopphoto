//! Per-render uniform buffers and bind groups: colour, tone, noise,
//! sharpening, defringe, local adjustments and retouching.

use super::*;

impl GpuRenderer {
    pub(super) fn ahd_params_buffer(
        &self,
        edit: &EditState,
        cfa: CfaPattern,
        out_w: u32,
        out_h: u32,
    ) -> wgpu::Buffer {
        let image = self.image.as_ref().expect("image open");
        let temperature = edit
            .wb
            .temp_kelvin
            .unwrap_or_else(|| cct_from_wb_coeffs(&image.cam_to_xyz, &image.wb_coeffs));
        let resolved = self
            .profile
            .as_ref()
            .map(|profile| profile.resolve(temperature));
        let cam_to_xyz = resolved
            .as_ref()
            .map_or(image.cam_to_xyz, |profile| profile.cam_to_xyz);
        let display_matrix = sp_core::profile::display_cam_to_xyz(
            &cam_to_xyz,
            resolved
                .as_ref()
                .is_some_and(|profile| profile.matrix_is_forward),
        );
        let wb = match edit.wb.temp_kelvin {
            Some(value) => {
                wb_gains_for_temp(&cam_to_xyz, value, edit.wb.tint).unwrap_or_else(|| {
                    let green = image.wb_coeffs[1].max(1e-6);
                    [image.wb_coeffs[0] / green, 1.0, image.wb_coeffs[2] / green]
                })
            }
            None => {
                let green = image.wb_coeffs[1].max(1e-6);
                [image.wb_coeffs[0] / green, 1.0, image.wb_coeffs[2] / green]
            }
        };
        let matrix = display_matrix;
        self.ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("AHD params"),
                contents: bytemuck::bytes_of(&AhdParams {
                    out_w,
                    out_h,
                    cfa: cfa_id(cfa),
                    _pad: 0,
                    m0: [matrix[0][0], matrix[0][1], matrix[0][2], 0.0],
                    m1: [matrix[1][0], matrix[1][1], matrix[1][2], 0.0],
                    m2: [matrix[2][0], matrix[2][1], matrix[2][2], 0.0],
                    wb: [wb[0], wb[1], wb[2], 0.0],
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    /// The `RawParams` shading fields for this edit, and the grid to bind.
    pub(super) fn lens_fields(
        &self,
        edit: &EditState,
    ) -> (u32, [f32; 4], [f32; 4], &wgpu::TextureView) {
        match &self.lens {
            Some(lens) if edit.lens.enable && edit.lens.vignetting > 0.0 => (
                1,
                lens.grid,
                [lens.spacing_px, edit.lens.vignetting / 100.0, 0.0, 0.0],
                &lens.view,
            ),
            _ => {
                let (grid, scale, enable) = RawParams::no_lens();
                (enable, grid, scale, &self.unity_lookup)
            }
        }
    }

    /// The shading state this edit asks for, for comparison against what is
    /// baked into the proxy source.
    pub(super) fn lens_bake(&self, edit: &EditState) -> LensBake {
        let (enable, _, scale, _) = self.lens_fields(edit);
        LensBake {
            generation: self.lens.as_ref().map_or(0, |l| l.generation),
            enabled: enable == 1,
            strength: scale[1].to_bits(),
        }
    }

    /// Build the uniform/storage buffers shared by the proxy and tile
    /// paths: WB+matrix, tone LUT, tone/HSL params, output params.
    pub(super) fn color_buffers(&self, edit: &EditState, output: OutputSpec) -> ColorBuffers {
        let img = self.image.as_ref().expect("image open");
        let ctx = &self.ctx;
        let [out_w, out_h] = output.size;

        // Working WB temperature: the slider when engaged, else an estimate
        // of the as-shot illuminant (drives the standard dual-illuminant
        // profile interpolation described in BUILD.md §3.4).
        let wb_temp = edit
            .wb
            .temp_kelvin
            .unwrap_or_else(|| cct_from_wb_coeffs(&img.cam_to_xyz, &img.wb_coeffs));

        // Resolve matrix and LUTs together on every WB change.
        let resolved = self
            .profile
            .as_ref()
            .filter(|_| img.raster.is_none())
            .map(|p| p.resolve(wb_temp));
        let cam_to_xyz = resolved.as_ref().map_or(img.cam_to_xyz, |p| p.cam_to_xyz);
        let matrix_is_forward = resolved.as_ref().is_some_and(|p| p.matrix_is_forward);
        // White-preserving display matrix (see sp_core::profile docs): the
        // WB'd neutral axis must land exactly on D65 white.
        let display_matrix = sp_core::profile::display_cam_to_xyz(&cam_to_xyz, matrix_is_forward);
        let cam_to_rec2020 = if img.raster.is_some() {
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        } else {
            mat3_mul(&XYZ_TO_REC2020, &display_matrix)
        };

        let wb3 = if img.raster.is_some() {
            sp_core::source::RasterImage::white_balance_gains(edit.wb.temp_kelvin, edit.wb.tint)
        } else {
            match edit.wb.temp_kelvin {
                Some(t) => wb_gains_for_temp(&cam_to_xyz, t, edit.wb.tint).unwrap_or_else(|| {
                    let g = img.wb_coeffs[1].max(1e-6);
                    [img.wb_coeffs[0] / g, 1.0, img.wb_coeffs[2] / g]
                }),
                None => {
                    let g = img.wb_coeffs[1].max(1e-6);
                    [img.wb_coeffs[0] / g, 1.0, img.wb_coeffs[2] / g]
                }
            }
        };
        // Chroma-plane warp: the camera rendering, applied with the matrix.
        // Resolved per WB temperature like the hue/sat maps.
        let warp = resolved.as_ref().and_then(|p| p.chroma_warp.as_ref());
        let (warp_view, warp_enable, warp_dims, warp_range, warp_gray) = match warp {
            Some(w) => {
                let texels: Vec<[f32; 4]> =
                    w.entries.iter().map(|&[r, b]| [r, b, 0.0, 0.0]).collect();
                (
                    make_lookup_texture(ctx, "chroma warp", w.size_r, w.size_b, &texels),
                    1u32,
                    [w.size_r as f32, w.size_b as f32, 0.0, 0.0],
                    [w.r_min, w.r_max, w.b_min, w.b_max],
                    [w.gray_threshold, 0.0, 0.0, 0.0],
                )
            }
            None => (
                self.unity_lookup.clone(),
                0u32,
                [1.0, 1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 1.0],
                [0.0; 4],
            ),
        };

        let m = &cam_to_rec2020;
        let wb_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wb params"),
                contents: bytemuck::bytes_of(&WbColorParams {
                    m0: [m[0][0], m[0][1], m[0][2], 0.0],
                    m1: [m[1][0], m[1][1], m[1][2], 0.0],
                    m2: [m[2][0], m[2][1], m[2][2], 0.0],
                    wb_and_exposure: [wb3[0], wb3[1], wb3[2], 1.0],
                    out_w,
                    out_h,
                    warp_enable,
                    _pad: 0,
                    warp_dims,
                    warp_range,
                    warp_gray,
                    negative_base: [
                        edit.negative.base.0[0],
                        edit.negative.base.0[1],
                        edit.negative.base.0[2],
                        f32::from(edit.negative.enabled),
                    ],
                    negative_controls: [
                        edit.negative.density_range,
                        edit.negative.black_density,
                        edit.negative.gamma,
                        0.0,
                    ],
                    recovery: [
                        f32::from(edit.highlight_recovery.enabled && img.raster.is_none()),
                        edit.highlight_recovery.strength,
                        f32::from(edit.highlight_recovery.radius),
                        0.0,
                    ],
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let creative_look = edit.look.as_deref().and_then(sp_core::profile::Look::named);
        let custom_rgb_look = edit.look.as_deref().and_then(|name| {
            self.custom_look
                .as_ref()
                .filter(|(loaded_name, _)| loaded_name == name)
                .map(|(_, cube)| cube.bake_rgb_lut(33, edit.look_strength.unwrap_or(100.0) / 100.0))
        });
        let standard_curve = crate::tone::standard_base_curve();
        let profile_base_curve = if img.raster.is_some() {
            None
        } else {
            self.profile
                .as_ref()
                .map_or(Some(&standard_curve), |p| p.base_curve.as_ref())
        };
        // Creative look curves replace (never stack with) profile curves.
        let base_curve = creative_look
            .as_ref()
            .and_then(|look| look.base_curve.as_ref())
            .or(profile_base_curve);
        let lut = build_tone_lut(&edit.tone, &edit.curve, base_curve);
        let lut_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tone lut"),
                contents: bytemuck::cast_slice(&lut),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let channel_lut = build_channel_lut(&edit.curve);
        let channel_lut_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("channel curve lut"),
                contents: bytemuck::cast_slice(&channel_lut),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let (huesat_view, huesat_dims) = upload_dcp_lut(
            ctx,
            "profile HueSat LUT",
            resolved.as_ref().and_then(|p| p.huesat.as_ref()),
        );
        let composed_look = sp_core::profile::HueSatLut::compose(
            resolved.as_ref().and_then(|p| p.look.as_ref()),
            creative_look.as_ref().and_then(|look| look.huesat.as_ref()),
        );
        let (look_view, look_dims) =
            upload_dcp_lut(ctx, "profile look LUT", composed_look.as_ref());
        let rgb_lut = sp_core::profile::Lut3d::compose(
            resolved.as_ref().and_then(|p| p.rgb_lut.as_ref()),
            custom_rgb_look.as_ref(),
        );
        let (rgb_view, rgb_dims) = upload_rgb_lut(ctx, rgb_lut.as_ref());
        let active_proof = (output.view_transform
            && self.proof_space == sp_core::edit::ProofSpace::Custom)
            .then_some(())
            .and(self.custom_proof.as_ref())
            .map(|proof| {
                if self.gamut_warning {
                    &proof.gamut_warning
                } else {
                    &proof.simulation
                }
            });
        let (proof_view, proof_dims) = upload_rgb_lut(ctx, active_proof);

        let look_saturation = creative_look
            .as_ref()
            .map_or(1.0, |look| look.base_saturation);
        let dehaze_compensation = edit.local.dehaze * 0.12;
        let effective_saturation = ((1.0 + edit.color.saturation / 100.0) * look_saturation - 1.0)
            * 100.0
            + dehaze_compensation;
        let color_neutral = edit.color.vibrance == 0.0
            && effective_saturation.abs() < f32::EPSILON
            && edit
                .color
                .hsl
                .iter()
                .all(|band| *band == Default::default())
            && edit.color.grading.is_neutral()
            && !edit.color.monochrome;
        let mut bands = [[0.0f32; 4]; 8];
        for (dst, band) in bands.iter_mut().zip(edit.color.hsl) {
            *dst = [band.hue_shift, band.saturation, band.luminance, 0.0];
        }
        let grading = [
            [
                edit.color.grading.shadows.hue_degrees,
                edit.color.grading.shadows.saturation,
                0.0,
                0.0,
            ],
            [
                edit.color.grading.midtones.hue_degrees,
                edit.color.grading.midtones.saturation,
                0.0,
                0.0,
            ],
            [
                edit.color.grading.highlights.hue_degrees,
                edit.color.grading.highlights.saturation,
                0.0,
                0.0,
            ],
        ];
        let monochrome_mix = [
            edit.color.monochrome_mix[0..4]
                .try_into()
                .expect("four monochrome values"),
            edit.color.monochrome_mix[4..8]
                .try_into()
                .expect("four monochrome values"),
        ];
        let tone_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tone params"),
                contents: bytemuck::bytes_of(&ToneColorParams {
                    bands,
                    grading,
                    monochrome_mix,
                    global_sat: [
                        edit.color.vibrance,
                        effective_saturation,
                        edit.color.grading.balance,
                        if edit.color.monochrome { 1.0 } else { 0.0 },
                    ],
                    out_w,
                    out_h,
                    color_neutral: u32::from(color_neutral),
                    channel_neutral: u32::from(edit.curve.channels_are_neutral()),
                    photographic: edit.color.photographic.packed(),
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let profile_params = |stage: u32, label: &str| {
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::bytes_of(&ProfileColorParams {
                        huesat_dims,
                        look_dims,
                        rgb_dims,
                        stage,
                        out_w,
                        out_h,
                        _pad: 0,
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
        };
        let profile_pre_params = profile_params(0, "profile pre params");
        let profile_post_params = profile_params(1, "profile post params");
        let gamut = match (output.view_transform, output.export_space) {
            (true, _) if self.proof_space == sp_core::edit::ProofSpace::DisplayP3 => {
                sp_core::color::REC2020_TO_P3D65
            }
            (true, _) if self.proof_space == sp_core::edit::ProofSpace::Rec2020 => {
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            }
            (false, Some(sp_core::color::OutputColorSpace::DisplayP3)) => {
                sp_core::color::REC2020_TO_P3D65
            }
            (false, Some(sp_core::color::OutputColorSpace::Rec2020)) => {
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            }
            (false, Some(sp_core::color::OutputColorSpace::AdobeRgb)) => {
                sp_core::color::REC2020_TO_ADOBERGB
            }
            _ => sp_core::color::REC2020_TO_REC709,
        };
        // Exports encode with the target space's transfer; the proof view
        // is always sRGB-curve display output.
        let transfer = match (output.view_transform, output.export_space) {
            (false, Some(space)) => space.transfer().shader_id(),
            _ => 0,
        };
        let headroom = if edit.hdr.enabled {
            edit.hdr.headroom_stops
        } else {
            0.0
        };
        let out_params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("out params"),
                contents: bytemuck::bytes_of(&OutputParams {
                    out_w,
                    out_h,
                    mode: output.mode,
                    debug_mode: if output.view_transform {
                        self.clipping_mode
                    } else {
                        0
                    },
                    hdr: [
                        headroom,
                        edit.hdr.highlight_rolloff,
                        edit.hdr.scene_white_ev,
                        0.0,
                    ],
                    gamut0: [gamut[0][0], gamut[0][1], gamut[0][2], 0.0],
                    gamut1: [gamut[1][0], gamut[1][1], gamut[1][2], 0.0],
                    gamut2: [gamut[2][0], gamut[2][1], gamut[2][2], 0.0],
                    vignette: [
                        edit.vignette.amount,
                        edit.vignette.midpoint,
                        edit.vignette.roundness,
                        edit.vignette.feather,
                    ],
                    grain: [
                        edit.grain.amount,
                        edit.grain.size,
                        edit.grain.roughness,
                        edit.grain.seed as f32,
                    ],
                    lens_geometry: [
                        if edit.lens.distortion == [0.0; 3] {
                            self.lens
                                .as_ref()
                                .filter(|_| img.raster.is_none())
                                .map_or(0.0, |lens| lens.distortion[0])
                        } else {
                            edit.lens.distortion[0]
                        },
                        if edit.lens.distortion == [0.0; 3] {
                            self.lens
                                .as_ref()
                                .filter(|_| img.raster.is_none())
                                .map_or(0.0, |lens| lens.distortion[1])
                        } else {
                            edit.lens.distortion[1]
                        },
                        if edit.lens.distortion == [0.0; 3] {
                            self.lens
                                .as_ref()
                                .filter(|_| img.raster.is_none())
                                .map_or(0.0, |lens| lens.distortion[2])
                        } else {
                            edit.lens.distortion[2]
                        },
                        if edit.lens.enable && !self.canonical_mask_input {
                            if edit.lens.crop_to_valid {
                                2.0
                            } else {
                                1.0
                            }
                        } else {
                            0.0
                        },
                    ],
                    lateral_ca: [
                        if edit.lens.lateral_ca == [0.0; 2] {
                            self.lens
                                .as_ref()
                                .filter(|_| img.raster.is_none())
                                .map_or(0.0, |lens| lens.lateral_ca[0])
                        } else {
                            edit.lens.lateral_ca[0]
                        },
                        if edit.lens.lateral_ca == [0.0; 2] {
                            self.lens
                                .as_ref()
                                .filter(|_| img.raster.is_none())
                                .map_or(0.0, |lens| lens.lateral_ca[1])
                        } else {
                            edit.lens.lateral_ca[1]
                        },
                        0.0,
                        0.0,
                    ],
                    crop: edit.geometry.crop,
                    image_rect: [
                        output.origin[0] as f32,
                        output.origin[1] as f32,
                        output.full_size[0] as f32,
                        output.full_size[1] as f32,
                    ],
                    proof_flags: [
                        u32::from(output.view_transform && self.gamut_warning),
                        proof_dims[0] * proof_dims[3],
                        transfer,
                        0,
                    ],
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        ColorBuffers {
            wb_params,
            lut_buf,
            channel_lut_buf,
            tone_params,
            profile_pre_params,
            profile_post_params,
            out_params,
            huesat_view,
            look_view,
            rgb_view,
            proof_view,
            warp_view,
        }
    }

    /// Effective manual or camera/ISO-tuned noise parameters.
    pub(super) fn effective_nr(&self, edit: &EditState) -> [f32; 4] {
        if self
            .image
            .as_ref()
            .is_some_and(|image| image.raster.is_some())
        {
            return [
                edit.nr.luminance,
                edit.nr.luminance_detail,
                edit.nr.chroma,
                edit.nr.chroma_detail,
            ];
        }
        if self.external_raw_denoise
            && edit.nr.camera_denoise_mode != sp_core::edit::CameraDenoiseMode::Off
        {
            return [0.0; 4];
        }
        if edit.nr.camera_denoise_mode != sp_core::edit::CameraDenoiseMode::Off {
            if let Some(tuning) = self.capture_meta.as_ref().and_then(|meta| {
                sp_core::hasselblad::camera_denoise_tuning(
                    &meta.camera_model,
                    meta.iso,
                    edit.nr.camera_denoise_mode,
                )
            }) {
                return [
                    tuning.luminance,
                    tuning.luminance_detail,
                    tuning.chroma,
                    tuning.chroma_detail,
                ];
            }
            // A copied or hand-edited legacy sidecar may request a camera
            // mode for unsupported metadata. Preserve its manual settings
            // instead of silently turning all denoise off.
        }
        [
            edit.nr.luminance,
            edit.nr.luminance_detail,
            edit.nr.chroma,
            edit.nr.chroma_detail,
        ]
    }

    /// Uniform buffer for the NR pass (Phase 4).
    pub(super) fn nr_params_buffer(
        &self,
        edit: &EditState,
        out_w: u32,
        out_h: u32,
    ) -> wgpu::Buffer {
        let nr = self.effective_nr(edit);
        self.ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("nr params"),
                contents: bytemuck::bytes_of(&sp_gpu::pipelines::NrGpuParams {
                    nr,
                    out_w,
                    out_h,
                    _pad: [0; 2],
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    pub(super) fn sharpen_params_buffer(
        &self,
        edit: &EditState,
        out_w: u32,
        out_h: u32,
    ) -> wgpu::Buffer {
        self.ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sharpen params"),
                contents: bytemuck::bytes_of(&SharpenGpuParams {
                    sharpen: [
                        edit.sharpen.amount,
                        edit.sharpen.radius_px,
                        (edit.sharpen.threshold - (edit.sharpen.detail - 25.0) * 0.2)
                            .clamp(0.0, 100.0),
                        edit.sharpen.halo_suppression,
                    ],
                    restoration: [
                        edit.sharpen.restore_blur,
                        edit.sharpen.restore_radius_px,
                        edit.sharpen.restore_regularization,
                        0.0,
                    ],
                    out_w,
                    out_h,
                    preview_mask: u32::from(self.sharpen_mask_preview),
                    _pad: 0,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    pub(super) fn defringe_params_buffer(
        &self,
        edit: &EditState,
        out_w: u32,
        out_h: u32,
        pixel_scale: f32,
    ) -> wgpu::Buffer {
        // Manual radius is expressed at full working resolution. Full-res
        // tiles are already full-scale regardless of their texture width;
        // only the proxy and supersampled modes need a scale conversion.
        let radius = (edit.lens.defringe_radius_px * pixel_scale).max(1.0);
        self.ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("defringe params"),
                contents: bytemuck::bytes_of(&DefringeGpuParams {
                    fringe: [
                        edit.lens.defringe_purple / 100.0,
                        edit.lens.defringe_green / 100.0,
                        radius,
                        edit.lens.defringe_threshold / 100.0,
                    ],
                    targets: [
                        edit.lens.defringe_purple_hue / 360.0,
                        edit.lens.defringe_purple_range / 360.0,
                        edit.lens.defringe_green_hue / 360.0,
                        edit.lens.defringe_green_range / 360.0,
                    ],
                    out_w,
                    out_h,
                    wide_bloom: u32::from(edit.lens.defringe_wide_bloom),
                    _pad: 0,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    pub(super) fn local_params_buffer(
        &self,
        edit: &EditState,
        out_w: u32,
        out_h: u32,
        working_w: u32,
    ) -> wgpu::Buffer {
        let airlight = self.image.as_ref().map_or(0.95, |image| image.airlight);
        self.ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("local contrast params"),
                contents: bytemuck::bytes_of(&LocalContrastGpuParams {
                    local: [
                        edit.local.clarity,
                        edit.local.texture,
                        edit.local.dehaze,
                        airlight,
                    ],
                    out_w,
                    out_h,
                    working_w,
                    _pad: 0,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    pub(super) fn retouch_buffers(
        &self,
        edit: &EditState,
        output: OutputSpec,
    ) -> Option<RetouchBuffers> {
        self.retouch_buffers_with_source(edit, output, None)
    }

    pub(super) fn retouch_buffers_with_source(
        &self,
        edit: &EditState,
        output: OutputSpec,
        source_origin: Option<[u32; 2]>,
    ) -> Option<RetouchBuffers> {
        self.retouch_buffers_internal(edit, output, source_origin, 0, [0, 0])
    }

    pub(super) fn retouch_buffers_internal(
        &self,
        edit: &EditState,
        output: OutputSpec,
        source_origin: Option<[u32; 2]>,
        preview_mask: u32,
        storage_origin: [u32; 2],
    ) -> Option<RetouchBuffers> {
        let local_count = edit
            .local_adjustments
            .iter()
            .filter(|adjustment| adjustment.enabled)
            .count();
        if local_count == 0 && edit.spots.is_empty() {
            return None;
        }

        let mut records = Vec::with_capacity(local_count + edit.spots.len());
        let mut points: Vec<[f32; 4]> = Vec::new();
        for adjustment in edit
            .local_adjustments
            .iter()
            .filter(|adjustment| adjustment.enabled)
        {
            let (mask_offset, mask_count) =
                append_mask_program(&adjustment.mask, output, &mut points);
            let tone_offset = points.len();
            let tone_active = adjustment.curve.parametric != Default::default()
                || !adjustment.curve.luma.is_neutral();
            if tone_active {
                points.extend(
                    crate::tone::build_tone_lut(&Default::default(), &adjustment.curve, None)
                        .into_iter()
                        .map(|v| [v, 0.0, 0.0, 0.0]),
                );
            }
            let channel_offset = points.len();
            let channel_active = !adjustment.curve.channels_are_neutral();
            if channel_active {
                let channels = crate::tone::build_channel_lut(&adjustment.curve);
                for i in 0..256 {
                    points.push([channels[i], channels[256 + i], channels[512 + i], 0.0]);
                }
            }
            let color_offset = points.len();
            let color_active = adjustment
                .color
                .hsl
                .iter()
                .any(|v| *v != Default::default())
                || !adjustment.color.grading.is_neutral()
                || adjustment.color.monochrome_mix.iter().any(|v| *v != 0.0);
            if color_active {
                points.extend(
                    adjustment
                        .color
                        .hsl
                        .iter()
                        .map(|band| [band.hue_shift, band.saturation, band.luminance, 0.0]),
                );
                for wheel in [
                    adjustment.color.grading.shadows,
                    adjustment.color.grading.midtones,
                    adjustment.color.grading.highlights,
                ] {
                    points.push([wheel.hue_degrees, wheel.saturation, 0.0, 0.0]);
                }
                for values in adjustment.color.monochrome_mix.chunks_exact(4) {
                    points.push([values[0], values[1], values[2], values[3]]);
                }
            }
            let photographic_offset = points.len();
            let photographic_active = !adjustment.color.photographic.is_neutral();
            if photographic_active {
                points.extend(adjustment.color.photographic.packed());
            }
            let gains = [
                2.0f32.powf(adjustment.temperature * 0.004),
                2.0f32.powf(-adjustment.tint * 0.003),
                2.0f32.powf(-adjustment.temperature * 0.004),
            ];
            let normalization = gains[0] * 0.2627 + gains[1] * 0.6780 + gains[2] * 0.0593;
            records.push(RetouchGpuRecord {
                header: [7.0, 1.0, 0.0, 0.0],
                a: [0.0; 4],
                b: [0.0; 4],
                tone0: [
                    adjustment.tone.exposure_ev,
                    adjustment.tone.contrast,
                    adjustment.tone.highlights,
                    adjustment.tone.shadows,
                ],
                tone1: [
                    adjustment.tone.whites,
                    adjustment.tone.blacks,
                    adjustment.tone.brightness,
                    0.0,
                ],
                color: [
                    adjustment.color.vibrance,
                    adjustment.color.saturation,
                    f32::from(adjustment.color.monochrome),
                    adjustment.color.grading.balance,
                ],
                mask: [
                    mask_offset as f32,
                    mask_count as f32,
                    f32::from(adjustment.mask_inverted),
                    adjustment.opacity,
                ],
                wb: [
                    gains[0] / normalization,
                    gains[1] / normalization,
                    gains[2] / normalization,
                    0.0,
                ],
                spatial: [
                    adjustment.local.clarity,
                    adjustment.local.texture,
                    adjustment.local.dehaze,
                    adjustment.sharpness,
                ],
                extra: [
                    adjustment.noise_reduction,
                    tone_offset as f32,
                    channel_offset as f32,
                    color_offset as f32,
                ],
                corrective: [
                    adjustment.corrective.red_eye,
                    adjustment.corrective.moire,
                    adjustment.corrective.moire_radius_px * output.full_size[0] as f32
                        / self
                            .image
                            .as_ref()
                            .map_or(output.full_size[0], |image| image.active.width)
                            .max(1) as f32,
                    photographic_offset as f32,
                ],
                flags: [
                    f32::from(tone_active),
                    f32::from(channel_active),
                    f32::from(color_active),
                    f32::from(photographic_active),
                ],
            });
        }
        for spot in &edit.spots {
            let point_offset = points.len();
            points.extend(
                spot.stroke
                    .iter()
                    .map(|point| [point[0], point[1], 0.0, 0.0]),
            );
            records.push(RetouchGpuRecord {
                header: [
                    10.0,
                    f32::from(spot.clone),
                    point_offset as f32,
                    (points.len() - point_offset) as f32,
                ],
                a: [
                    spot.target[0],
                    spot.target[1],
                    spot.source[0],
                    spot.source[1],
                ],
                b: [
                    spot.radius,
                    spot.feather,
                    spot.opacity,
                    f32::from(spot.enabled),
                ],
                tone0: [0.0; 4],
                tone1: [0.0; 4],
                color: [0.0; 4],
                mask: [0.0; 4],
                wb: [1.0; 4],
                spatial: [0.0; 4],
                extra: [0.0; 4],
                flags: [0.0; 4],
                corrective: [
                    spot.heal_texture,
                    8.0 * output.full_size[0] as f32
                        / self
                            .image
                            .as_ref()
                            .map_or(output.full_size[0], |image| image.active.width)
                            .max(1) as f32,
                    0.0,
                    0.0,
                ],
            });
        }
        if points.is_empty() {
            points.push([0.0; 4]);
        }
        let params = self
            .ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("retouch params"),
                contents: bytemuck::bytes_of(&RetouchGpuParams {
                    out_w: output.size[0],
                    out_h: output.size[1],
                    origin_x: output.origin[0],
                    origin_y: output.origin[1],
                    full_w: output.full_size[0],
                    full_h: output.full_size[1],
                    local_count: local_count as u32,
                    spot_count: edit.spots.len() as u32,
                    source_origin_x: source_origin.unwrap_or([
                        output.origin[0] - storage_origin[0],
                        output.origin[1] - storage_origin[1],
                    ])[0],
                    source_origin_y: source_origin.unwrap_or([
                        output.origin[0] - storage_origin[0],
                        output.origin[1] - storage_origin[1],
                    ])[1],
                    patch_source: u32::from(source_origin.is_some()),
                    preview_mask,
                    storage_origin,
                    _region_pad: [0; 2],
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let records = self
            .ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("retouch records"),
                contents: bytemuck::cast_slice(&records),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let points = self
            .ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("retouch points"),
                contents: bytemuck::cast_slice(&points),
                usage: wgpu::BufferUsages::STORAGE,
            });
        Some(RetouchBuffers {
            params,
            records,
            points,
        })
    }

    /// Bind group for the tone pass (extra LUT binding at @binding(3)).
    pub(super) fn tone_bind_group(
        &self,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        bufs: &ColorBuffers,
    ) -> wgpu::BindGroup {
        self.ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tone"),
                layout: &self.pipes.tone_color.layout,
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
                        resource: bufs.tone_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: bufs.lut_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: bufs.channel_lut_buf.as_entire_binding(),
                    },
                ],
            })
    }

    pub(super) fn profile_bind_group(
        &self,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        bufs: &ColorBuffers,
        post_tone: bool,
    ) -> wgpu::BindGroup {
        self.ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(if post_tone {
                    "profile post-tone"
                } else {
                    "profile pre-tone"
                }),
                layout: &self.pipes.profile_color.layout,
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
                        resource: if post_tone {
                            bufs.profile_post_params.as_entire_binding()
                        } else {
                            bufs.profile_pre_params.as_entire_binding()
                        },
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&bufs.huesat_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&bufs.look_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&bufs.rgb_view),
                    },
                ],
            })
    }
}

/// Compile soft-mask expressions to a postfix program; alpha/brush payloads
/// share the existing point buffer. Invalid oversized trees evaluate empty.
fn append_mask_program(
    mask: &sp_core::edit::LocalMask,
    output: OutputSpec,
    points: &mut Vec<[f32; 4]>,
) -> (usize, usize) {
    use sp_core::edit::{LocalMask, MaskOperation};
    fn append(
        mask: &LocalMask,
        output: OutputSpec,
        points: &mut Vec<[f32; 4]>,
        program: &mut Vec<[f32; 4]>,
        depth: usize,
        remaining: &mut usize,
    ) -> bool {
        if depth >= 32 || *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        let offset = points.len();
        let (kind, a, b) = match mask {
            LocalMask::Paint { strokes, settings } => {
                for stroke in strokes {
                    points.extend(stroke.dabs.iter().map(|dab| {
                        [
                            dab.point[0],
                            dab.point[1],
                            dab.pressure,
                            f32::from(stroke.subtract),
                        ]
                    }));
                }
                (
                    8.0,
                    [
                        settings.size,
                        settings.feather,
                        settings.flow,
                        f32::from(settings.pressure),
                    ],
                    [0.0; 4],
                )
            }
            LocalMask::Composite {
                operation,
                left,
                right,
            } => {
                if !append(left, output, points, program, depth + 1, remaining)
                    || !append(right, output, points, program, depth + 1, remaining)
                {
                    return false;
                }
                (
                    match operation {
                        MaskOperation::Add => 20.0,
                        MaskOperation::Subtract => 21.0,
                        MaskOperation::Intersect => 22.0,
                    },
                    [0.0; 4],
                    [0.0; 4],
                )
            }
            LocalMask::Modified {
                mask,
                inverted,
                opacity,
            } => {
                if !append(mask, output, points, program, depth + 1, remaining) {
                    return false;
                }
                (23.0, [f32::from(*inverted), *opacity, 0.0, 0.0], [0.0; 4])
            }
            LocalMask::Alpha(mask) => {
                let sampler = mask.sampler();
                let count = output.size[0] as usize * output.size[1] as usize;
                for first in (0..count).step_by(4) {
                    let mut packed = [0.0; 4];
                    for (channel, value) in packed.iter_mut().enumerate() {
                        let index = first + channel;
                        if index < count {
                            let x = index as u32 % output.size[0];
                            let y = index as u32 / output.size[0];
                            *value = sampler.sample([
                                (output.origin[0] as f32 + x as f32 + 0.5)
                                    / output.full_size[0] as f32,
                                (output.origin[1] as f32 + y as f32 + 0.5)
                                    / output.full_size[1] as f32,
                            ]);
                        }
                    }
                    points.push(packed);
                }
                (6.0, [0.0; 4], [0.0; 4])
            }
            LocalMask::Brush {
                points: brush,
                size,
                feather,
            } => {
                points.extend(brush.iter().map(|p| [p[0], p[1], 0.0, 0.0]));
                let fallback = brush.first().copied().unwrap_or([0.5, 0.5]);
                (1.0, [*size, *feather, fallback[0], fallback[1]], [0.0; 4])
            }
            LocalMask::Radial {
                center,
                radius,
                feather,
            } => (
                2.0,
                [center[0], center[1], radius[0], radius[1]],
                [*feather, 0.0, 0.0, 0.0],
            ),
            LocalMask::LinearGradient { start, end } => {
                (3.0, [start[0], start[1], end[0], end[1]], [0.0; 4])
            }
            LocalMask::LuminanceRange {
                minimum,
                maximum,
                feather,
            } => (4.0, [*minimum, *maximum, *feather, 0.0], [0.0; 4]),
            LocalMask::ColorRange {
                target_ab,
                radius,
                feather,
            } => (
                5.0,
                [target_ab[0], target_ab[1], *radius, *feather],
                [0.0; 4],
            ),
        };
        program.extend([
            [kind, 1.0, offset as f32, (points.len() - offset) as f32],
            a,
            b,
        ]);
        true
    }
    let start = points.len();
    let mut program = Vec::new();
    if !append(mask, output, points, &mut program, 0, &mut 1024) {
        points.truncate(start);
        return (start, 0);
    }
    let offset = points.len();
    let count = program.len() / 3;
    points.extend(program);
    (offset, count)
}
