//! Bounded, scene-linear source patches for distant heal/clone samples.
//! Target, source and sparse healing statistics have independent coordinates;
//! source distance never expands the working tile to the full photograph.

use super::*;
use sp_core::edit::SpotCorrection;

#[derive(Clone)]
struct ScenePatch {
    texture: wgpu::Texture,
    origin: [u32; 2],
    size: [u32; 2],
}

impl ScenePatch {
    fn contains(&self, pixel: [u32; 2]) -> bool {
        (0..2).all(|axis| {
            pixel[axis] >= self.origin[axis] && pixel[axis] < self.origin[axis] + self.size[axis]
        })
    }
}

fn spot_intersects(spot: &SpotCorrection, rect: [u32; 4], full: [u32; 2]) -> bool {
    if !spot.enabled || spot.opacity <= 0.0 {
        return false;
    }
    let mut lo = spot.target;
    let mut hi = spot.target;
    for point in &spot.stroke {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(point[axis]);
            hi[axis] = hi[axis].max(point[axis]);
        }
    }
    let radius = spot.radius * full[0].min(full[1]) as f32 + 1.0;
    (0..2).all(|axis| {
        hi[axis] * full[axis] as f32 + radius >= rect[axis] as f32
            && lo[axis] * full[axis] as f32 - radius <= (rect[axis] + rect[axis + 2]) as f32
    })
}

/// Source pixel centers use the same floor/clamp rule as the WGSL sampler.
fn source_region(
    spot: &SpotCorrection,
    origin: [u32; 2],
    size: [u32; 2],
    full: [u32; 2],
) -> [u32; 4] {
    let padding = if !spot.clone && spot.heal_texture > 0.0 {
        9.0
    } else {
        1.0
    };
    let mut rect = [0; 4];
    for axis in 0..2 {
        let delta = (spot.source[axis] - spot.target[axis]) * full[axis] as f32;
        let first = (origin[axis] as f32 + delta).floor() - padding;
        let last = (origin[axis] as f32 + size[axis] as f32 + delta).ceil() + padding;
        rect[axis] = first.clamp(0.0, (full[axis] - 1) as f32) as u32;
        let end = last.clamp((rect[axis] + 1) as f32, full[axis] as f32) as u32;
        rect[axis + 2] = end - rect[axis];
    }
    rect
}

impl GpuRenderer {
    fn render_scene_patch(&mut self, edit: &EditState, rect: [u32; 4]) -> Option<ScenePatch> {
        let info = self.render_tile_mode_inner(
            edit,
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            (0, false, false, sp_core::color::OutputColorSpace::Srgb),
            true,
        )?;
        let tile = self.tile.take()?;
        Some(ScenePatch {
            texture: tile.scene_output?,
            origin: [info.x - info.margin_x, info.y - info.margin_y],
            size: [info.tex_w, info.tex_h],
        })
    }

    /// Copy four original target and four original source samples. Usually
    /// they lie in the existing patches; distant samples render as 1x1 tiles
    /// with the normal demosaic/filter halo, never as a giant connecting apron.
    fn healing_rings(
        &mut self,
        edit: &EditState,
        spot: &SpotCorrection,
        target: &ScenePatch,
        source: &ScenePatch,
        full: [u32; 2],
    ) -> Option<wgpu::Texture> {
        let texture = make_texture(
            &self.ctx,
            "heal ring samples",
            8,
            1,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let radius = spot.radius * 1.25 * full[0].min(full[1]) as f32;
        for (index, (center, offset)) in [spot.target, spot.source]
            .into_iter()
            .flat_map(|center| {
                [[radius, 0.0], [-radius, 0.0], [0.0, radius], [0.0, -radius]]
                    .into_iter()
                    .map(move |offset| (center, offset))
            })
            .enumerate()
        {
            let pixel = std::array::from_fn(|axis| {
                ((center[axis] + offset[axis] / full[axis] as f32) * full[axis] as f32)
                    .floor()
                    .clamp(0.0, (full[axis] - 1) as f32) as u32
            });
            let temporary;
            let patch = if target.contains(pixel) {
                target
            } else if source.contains(pixel) {
                source
            } else {
                temporary = self.render_scene_patch(edit, [pixel[0], pixel[1], 1, 1])?;
                &temporary
            };
            let mut encoder =
                self.ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("heal ring sample"),
                    });
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &patch.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: pixel[0] - patch.origin[0],
                        y: pixel[1] - patch.origin[1],
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: index as u32,
                        y: 0,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            self.ctx.queue.submit([encoder.finish()]);
        }
        Some(texture)
    }

    pub(super) fn render_tile_with_spot_sources(
        &mut self,
        edit: &EditState,
        rect: [u32; 4],
        output: (u32, bool, bool, sp_core::color::OutputColorSpace),
    ) -> Option<TileInfo> {
        let full = self.active_size().map(|(w, h)| [w, h])?;
        let mut base_edit = edit.clone();
        base_edit.spots.clear();
        base_edit.local_adjustments.clear();
        base_edit.sharpen.amount = 0.0;
        // Retain the local spatial halo even though those adjustments run
        // after retouch. It supplies their neighborhood at the visible edge.
        let local_margin = if edit.local_adjustments.iter().any(|a| {
            a.enabled
                && (!a.local.is_neutral()
                    || a.sharpness > 0.0
                    || a.noise_reduction > 0.0
                    || a.corrective.moire > 0.0)
        }) {
            tile::LOCAL_FILTER_RADIUS
        } else {
            0
        } + if edit.sharpen.amount > 0.0 {
            edit.sharpen.radius_px.ceil() as u32
        } else {
            0
        };
        let local_margin = local_margin
            + if edit
                .spots
                .iter()
                .any(|spot| spot.enabled && !spot.clone && spot.heal_texture > 0.0)
            {
                8
            } else {
                0
            };
        let x = rect[0].min(full[0] - 1);
        let y = rect[1].min(full[1] - 1);
        let w = rect[2].clamp(1, full[0] - x);
        let h = rect[3].clamp(1, full[1] - y);
        let expanded_x = x.saturating_sub(local_margin);
        let expanded_y = y.saturating_sub(local_margin);
        let expanded_w = (x + w + local_margin).min(full[0]) - expanded_x;
        let expanded_h = (y + h + local_margin).min(full[1]) - expanded_y;
        let mut info = self.render_tile_mode_inner(
            &base_edit, expanded_x, expanded_y, expanded_w, expanded_h, output, true,
        )?;
        info.margin_x += x - expanded_x;
        info.margin_y += y - expanded_y;
        info.x = x;
        info.y = y;
        info.w = w;
        info.h = h;
        let target_tile = self.tile.take()?;
        let result = (|| {
            let target = ScenePatch {
                texture: target_tile.scene_output.as_ref()?.clone(),
                origin: [info.x - info.margin_x, info.y - info.margin_y],
                size: [info.tex_w, info.tex_h],
            };
            let target_rect = [
                target.origin[0],
                target.origin[1],
                target.size[0],
                target.size[1],
            ];
            let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
            let ping = make_texture(
                &self.ctx,
                "retouch target ping",
                info.tex_w,
                info.tex_h,
                wgpu::TextureFormat::Rgba16Float,
                usage,
            );
            let pong = make_texture(
                &self.ctx,
                "retouch target pong",
                info.tex_w,
                info.tex_h,
                wgpu::TextureFormat::Rgba16Float,
                usage,
            );
            let ping_view = ping.create_view(&Default::default());
            let pong_view = pong.create_view(&Default::default());
            let original_target = target.texture.create_view(&Default::default());
            let mut current = original_target.clone();
            let mut steps = 0usize;
            let spec = OutputSpec {
                size: target.size,
                mode: output.0,
                origin: target.origin,
                full_size: full,
                view_transform: output.2,
                export_space: (!output.2).then_some(output.3),
            };
            for spot in edit
                .spots
                .iter()
                .filter(|spot| spot_intersects(spot, target_rect, full))
            {
                let source = self.render_scene_patch(
                    &base_edit,
                    source_region(spot, target.origin, target.size, full),
                )?;
                let source_view = source.texture.create_view(&Default::default());
                let rings = if spot.clone {
                    None
                } else {
                    Some(self.healing_rings(&base_edit, spot, &target, &source, full)?)
                };
                let ring_view = rings
                    .as_ref()
                    .map(|texture| texture.create_view(&Default::default()));
                let one = EditState {
                    spots: vec![spot.clone()],
                    ..Default::default()
                };
                let buffers = self.retouch_buffers_with_source(&one, spec, Some(source.origin))?;
                let next = if steps.is_multiple_of(2) {
                    &ping_view
                } else {
                    &pong_view
                };
                let group = bind_retouch_with_target(
                    &self.ctx,
                    &self.pipes.retouch,
                    [
                        &current,
                        next,
                        &source_view,
                        ring_view.as_ref().unwrap_or(&source_view),
                        &original_target,
                    ],
                    [&buffers.params, &buffers.records, &buffers.points],
                );
                let mut encoder =
                    self.ctx
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("bounded retouch correction"),
                        });
                run(
                    &mut encoder,
                    &self.pipes.retouch,
                    &group,
                    info.tex_w,
                    info.tex_h,
                );
                self.ctx.queue.submit([encoder.finish()]);
                // Finish this correction before recycling scratch textures;
                // queued source renders otherwise retain unbounded memory.
                self.ctx.wait_for_gpu().ok()?;
                current = next.clone();
                steps += 1;
            }
            let mut encoder =
                self.ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("retouch finish"),
                    });
            let locals = EditState {
                local_adjustments: edit.local_adjustments.clone(),
                ..Default::default()
            };
            if !locals.local_adjustments.is_empty() {
                let next = if steps.is_multiple_of(2) {
                    &ping_view
                } else {
                    &pong_view
                };
                if retouch_stripe_size(&locals, spec.size).is_some() {
                    self.run_retouch_stripes(
                        &mut encoder,
                        &locals,
                        spec,
                        [&current, next, &current, &current],
                        None,
                    );
                } else if let Some(buffers) = self.retouch_buffers(&locals, spec) {
                    let group = bind_retouch(
                        &self.ctx,
                        &self.pipes.retouch,
                        [&current, next, &current, &current],
                        [&buffers.params, &buffers.records, &buffers.points],
                    );
                    run(
                        &mut encoder,
                        &self.pipes.retouch,
                        &group,
                        info.tex_w,
                        info.tex_h,
                    );
                }
                current = next.clone();
                steps += 1;
            }
            if edit.sharpen.amount > 0.0 {
                let params = self.sharpen_params_buffer(edit, info.tex_w, info.tex_h);
                let next = if steps.is_multiple_of(2) {
                    &ping_view
                } else {
                    &pong_view
                };
                let group = bind(&self.ctx, &self.pipes.sharpen, &current, next, &params);
                run(
                    &mut encoder,
                    &self.pipes.sharpen,
                    &group,
                    info.tex_w,
                    info.tex_h,
                );
                current = next.clone();
            }
            let buffers = self.color_buffers(edit, spec);
            let pass = if output.1 {
                &self.pipes.output16
            } else {
                &self.pipes.output
            };
            let out = if output.1 {
                &target_tile.out16_view
            } else {
                &target_tile.out_view
            };
            let group = bind_lookup(
                &self.ctx,
                pass,
                &current,
                out,
                &buffers.out_params,
                &buffers.proof_view,
            );
            run(&mut encoder, pass, &group, info.tex_w, info.tex_h);
            if self.hdr_display {
                let group = bind_lookup(
                    &self.ctx,
                    &self.pipes.output_scrgb,
                    &current,
                    &target_tile.hdr_display_view,
                    &buffers.out_params,
                    &buffers.proof_view,
                );
                run(
                    &mut encoder,
                    &self.pipes.output_scrgb,
                    &group,
                    info.tex_w,
                    info.tex_h,
                );
            }
            self.ctx.queue.submit([encoder.finish()]);
            Some(info)
        })();
        // Restore target output even when source rendering fails.
        self.tile = Some(target_tile);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distant_source_region_size_is_independent_of_distance() {
        let spot = SpotCorrection {
            target: [0.9, 0.5],
            source: [0.1, 0.5],
            radius: 0.02,
            ..Default::default()
        };
        assert!(spot_intersects(
            &spot,
            [8400, 3500, 1024, 1024],
            [10000, 8000]
        ));
        let source = source_region(&spot, [8400, 3500], [1024, 1024], [10000, 8000]);
        assert!(source[2] <= 1027 && source[3] <= 1027, "{source:?}");
        assert!(
            source[0] < 1000,
            "source patch did not move to the distant source: {source:?}"
        );
        assert!(!spot_intersects(&spot, [0, 0, 1024, 1024], [10000, 8000]));
        let disabled = SpotCorrection {
            enabled: false,
            ..spot
        };
        assert!(!spot_intersects(
            &disabled,
            [8400, 3500, 1024, 1024],
            [10000, 8000]
        ));
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use sp_core::color::REC2020_TO_XYZ;
    use sp_core::raw::{ActiveArea, CfaPattern, Orientation};

    fn wide_raw() -> RawImage {
        let (width, height) = (4096, 128);
        let cfa = CfaPattern::Rggb;
        let data = (0..width * height)
            .map(|i| {
                let x = i % width;
                let y = i / width;
                let channel = cfa.color_at(x, y) as usize;
                let value = [0.07, 0.13, 0.09][channel]
                    + 0.28 * x as f32 / width as f32
                    + ((x / 2 + y / 2) % 7) as f32 * 0.003;
                (value * 65535.0) as u16
            })
            .collect();
        RawImage {
            cfa_data: data,
            width,
            height,
            cfa,
            black_levels: [0.0; 4],
            white_levels: [65535.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: REC2020_TO_XYZ,
            active_area: ActiveArea {
                x: 0,
                y: 0,
                width,
                height,
            },
            orientation: Orientation::Normal,
        }
    }

    #[test]
    fn far_heal_and_clone_match_full_scene_without_large_target_tiles() {
        let Ok(ctx) = GpuContext::headless() else {
            return;
        };
        let mut renderer = GpuRenderer::new(ctx).unwrap();
        renderer.open_image(&wide_raw()).unwrap();
        let output = (0, false, false, sp_core::color::OutputColorSpace::Srgb);
        for (clone, radius, heal_texture) in [
            (true, 0.25, 0.0),
            (false, 0.25, 0.0),
            (false, 0.49, 0.0),
            (false, 0.25, 100.0),
        ] {
            let mut edit = EditState::default();
            edit.spots.push(SpotCorrection {
                target: [0.9, 0.5],
                source: [0.05, 0.35],
                radius,
                feather: 0.3,
                opacity: 0.65,
                heal_texture,
                clone,
                ..Default::default()
            });
            // Independent full-scene reference uses the original one-pass
            // sampler, with no source-patch orchestration.
            let all = renderer
                .render_tile_mode_inner(&edit, 0, 0, 4096, 128, output, false)
                .unwrap();
            let reference = renderer
                .read_rgba8_texture(
                    &renderer.tile.as_ref().unwrap().out_tex,
                    all.tex_w,
                    all.tex_h,
                )
                .unwrap();
            let info = renderer
                .render_tile_mode(&edit, 3600, 16, 160, 96, output)
                .unwrap();
            assert!(
                info.tex_w <= 184 && info.tex_h <= 120,
                "source distance expanded target tile: {info:?}"
            );
            let pixels = renderer
                .read_rgba8_texture(
                    &renderer.tile.as_ref().unwrap().out_tex,
                    info.tex_w,
                    info.tex_h,
                )
                .unwrap();
            let mut maximum = 0;
            for y in 0..96 {
                for x in 0..160 {
                    for c in 0..3 {
                        let actual = pixels[(((y + info.margin_y) * info.tex_w + x + info.margin_x)
                            * 4
                            + c) as usize];
                        let expected = reference[(((y + 16) * 4096 + x + 3600) * 4 + c) as usize];
                        maximum = maximum.max(actual.abs_diff(expected));
                    }
                }
            }
            assert!(
                maximum <= 2,
                "clone={clone}, texture={heal_texture}, far patch differs by {maximum} code values"
            );
        }
    }

    #[test]
    fn disabled_and_zero_opacity_spots_are_exact_bypasses() {
        let Ok(ctx) = GpuContext::headless() else {
            return;
        };
        let mut renderer = GpuRenderer::new(ctx).unwrap();
        renderer.open_image(&wide_raw()).unwrap();
        let output = (0, false, false, sp_core::color::OutputColorSpace::Srgb);
        let read = |renderer: &mut GpuRenderer, edit: &EditState| {
            let info = renderer
                .render_tile_mode(edit, 3600, 16, 160, 96, output)
                .unwrap();
            renderer
                .read_rgba8_texture(
                    &renderer.tile.as_ref().unwrap().out_tex,
                    info.tex_w,
                    info.tex_h,
                )
                .unwrap()
        };
        let baseline = read(&mut renderer, &EditState::default());
        for (enabled, opacity) in [(false, 1.0), (true, 0.0)] {
            let edit = EditState {
                spots: vec![SpotCorrection {
                    target: [0.9, 0.5],
                    source: [0.05, 0.5],
                    radius: 0.25,
                    enabled,
                    opacity,
                    clone: true,
                    ..Default::default()
                }],
                ..Default::default()
            };
            assert_eq!(read(&mut renderer, &edit), baseline);
        }
    }
}

/// Keep transient raster-alpha storage below 8 MiB per dispatch, independently
/// of image resolution or mask count. Source/output textures remain full region
/// textures, so stripe borders cannot alter color or neighborhood sampling.
pub(super) fn retouch_stripe_size(edit: &EditState, size: [u32; 2]) -> Option<[u32; 2]> {
    fn leaves(mask: &sp_core::edit::LocalMask, depth: usize) -> u64 {
        use sp_core::edit::LocalMask;
        if depth >= 32 {
            return 0;
        }
        match mask {
            LocalMask::Alpha(_) => 1,
            LocalMask::Composite { left, right, .. } => {
                leaves(left, depth + 1) + leaves(right, depth + 1)
            }
            LocalMask::Modified { mask, .. } => leaves(mask, depth + 1),
            _ => 0,
        }
    }
    let count = edit
        .local_adjustments
        .iter()
        .filter(|a| a.enabled)
        .map(|a| leaves(&a.mask, 0))
        .sum::<u64>();
    let max_pixels = (8 * 1024 * 1024 / (count * 4).max(1)).max(1);
    if count == 0 || u64::from(size[0]) * u64::from(size[1]) <= max_pixels {
        return None;
    }
    let width = u64::from(size[0]).min(max_pixels) as u32;
    Some([
        width,
        (max_pixels / u64::from(width)).min(u64::from(size[1])) as u32,
    ])
}
impl GpuRenderer {
    pub(super) fn run_retouch_stripes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        edit: &EditState,
        spec: OutputSpec,
        textures: [&wgpu::TextureView; 4],
        preview: Option<(u32, &wgpu::TextureView)>,
    ) {
        let stripe = retouch_stripe_size(edit, spec.size).unwrap_or(spec.size);
        for y in (0..spec.size[1]).step_by(stripe[1] as usize) {
            for x in (0..spec.size[0]).step_by(stripe[0] as usize) {
                let region = OutputSpec {
                    size: [
                        stripe[0].min(spec.size[0] - x),
                        stripe[1].min(spec.size[1] - y),
                    ],
                    origin: [spec.origin[0] + x, spec.origin[1] + y],
                    ..spec
                };
                let Some(buffers) = self.retouch_buffers_internal(edit, region, None, 0, [x, y])
                else {
                    return;
                };
                if let Some((index, view)) = preview {
                    let params = RetouchGpuParams {
                        out_w: region.size[0],
                        out_h: region.size[1],
                        origin_x: region.origin[0],
                        origin_y: region.origin[1],
                        full_w: spec.full_size[0],
                        full_h: spec.full_size[1],
                        local_count: edit.local_adjustments.iter().filter(|a| a.enabled).count()
                            as u32,
                        spot_count: edit.spots.len() as u32,
                        source_origin_x: spec.origin[0],
                        source_origin_y: spec.origin[1],
                        patch_source: 0,
                        preview_mask: index,
                        storage_origin: [x, y],
                        _region_pad: [0; 2],
                    };
                    let uniform =
                        self.ctx
                            .device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("stripe coverage parameters"),
                                contents: bytemuck::bytes_of(&params),
                                usage: wgpu::BufferUsages::UNIFORM,
                            });
                    let group = bind_retouch(
                        &self.ctx,
                        &self.pipes.retouch,
                        [textures[0], view, textures[2], textures[3]],
                        [&uniform, &buffers.records, &buffers.points],
                    );
                    run(
                        encoder,
                        &self.pipes.retouch,
                        &group,
                        region.size[0],
                        region.size[1],
                    );
                }
                let group = bind_retouch(
                    &self.ctx,
                    &self.pipes.retouch,
                    textures,
                    [&buffers.params, &buffers.records, &buffers.points],
                );
                run(
                    encoder,
                    &self.pipes.retouch,
                    &group,
                    region.size[0],
                    region.size[1],
                );
                let done = std::mem::replace(
                    encoder,
                    self.ctx.device.create_command_encoder(&Default::default()),
                );
                self.ctx.queue.submit([done.finish()]);
                let _ = self.ctx.wait_for_gpu();
            }
        }
    }
}
