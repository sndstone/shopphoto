//! Optional offscreen visual artifacts also exercise normal egui layout in CI.
use super::*;
type Panel = (&'static str, fn(&egui::Context, &mut AppState));
#[test]
fn photo_delivery_dialogs_layout_and_optional_offscreen_capture() {
    let directory = std::env::var_os("SP_TEST_PANEL_DIR").map(PathBuf::from);
    let gpu = directory
        .as_ref()
        .map(|_| GpuContext::headless().expect("offscreen GPU"));
    if let Some(path) = &directory {
        std::fs::create_dir_all(path).unwrap();
    }
    let panels: [Panel; 13] = [
        ("export", panels::export_dialog),
        ("queue", delivery::queue_window),
        ("workflow", workflow_ui::window),
        ("gallery", gallery::window),
        ("print", printer::print_window),
        ("customize", customize::window),
        ("detail", |ctx, state| {
            egui::Window::new(crate::i18n::text("Photo adjustments"))
                .default_width(theme::RIGHT_PANEL_W)
                .default_height(680.0)
                .show(ctx, |ui| panels::right_panel(ui, state, None));
        }),
        ("ai-runtime", |ctx, state| {
            enhance_runtime::window(ctx, &mut state.enhance_runtime)
        }),
        ("command-console", |ctx, state| {
            automation::console_ui(ctx, &mut state.command_console)
        }),
        ("rail-presets", |ctx, state| {
            state.left_rail_tab = crate::state::LeftRailTab::Presets;
            egui::Window::new("Develop rail")
                .default_width(theme::LEFT_RAIL_W)
                .default_height(680.0)
                .show(ctx, |ui| panels::left_rail(ui, state, &[]));
        }),
        ("rail-ai", |ctx, state| {
            state.left_rail_tab = crate::state::LeftRailTab::Ai;
            egui::Window::new("Develop rail")
                .default_width(theme::LEFT_RAIL_W)
                .default_height(680.0)
                .show(ctx, |ui| panels::left_rail(ui, state, &[]));
        }),
        ("rail-workflow", |ctx, state| {
            state.left_rail_tab = crate::state::LeftRailTab::Workflow;
            egui::Window::new("Develop rail")
                .default_width(theme::LEFT_RAIL_W)
                .default_height(680.0)
                .show(ctx, |ui| panels::left_rail(ui, state, &[]));
        }),
        ("pixel-shift", |ctx, state| {
            multiframe::window(ctx, &mut state.merge)
        }),
    ];
    for (name, panel) in panels {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut state = AppState {
            open_image: Some(PathBuf::from("photo.jpg")),
            active_size: Some((900, 1197)),
            ui_scale: 1.0,
            export_presets: sp_export::ExportPreset::built_ins(),
            ..Default::default()
        };
        if std::env::var("SP_TEST_LANGUAGE").is_ok_and(|value| value == "nl") {
            state.language = crate::i18n::Language::Dutch;
        }
        crate::i18n::set_language(state.language);
        if name == "detail" {
            state.panels.detail = true;
            state.inspector_jump = Some(2);
            state.edit.sharpen.restore_blur = 40.0;
        }
        if name == "ai-runtime" {
            state.enhance_runtime.report = Some(serde_json::json!({
                "python":"3.14.7", "device":"mps", "missing_packages":[],
                "models":{"object":true,"depth":true,"matting":true,"semantic":false}
            }));
            state.enhance_runtime.message =
                "Runtime ready. Model availability is listed below.".into();
        }
        state.merge.kind = sp_core::multiframe::MergeKind::PixelShift;
        state.merge.sources = (1..=4)
            .map(|i| PathBuf::from(format!("capture-{i}.dng")))
            .collect();
        let mut renderer = gpu.as_ref().map(|g| {
            eframe::egui_wgpu::Renderer::new(
                &g.device,
                wgpu::TextureFormat::Rgba8Unorm,
                Default::default(),
            )
        });
        for frame in 0..3 {
            state.export_dialog_open = name == "export";
            state.export_queue_open = name == "queue";
            state.workflow.open = name == "workflow";
            state.gallery.open = name == "gallery";
            state.print_dialog_open = name == "print";
            state.customize.open = name == "customize";
            state.merge.open = name == "pixel-shift";
            state.enhance_runtime.open = name == "ai-runtime";
            state.command_console.open = name == "command-console";
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1024.0, 768.0),
                    )),
                    time: Some(frame as f64),
                    ..Default::default()
                },
                |ui| panel(ui.ctx(), &mut state),
            );
            let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
            if frame == 2 {
                assert!(jobs.iter().any(|job|matches!(&job.primitive,egui::epaint::Primitive::Mesh(mesh) if mesh.vertices.len()>30)),"{name} must produce a visible dialog");
            }
            if let (Some(gpu), Some(renderer), Some(directory)) = (&gpu, &mut renderer, &directory)
            {
                for (id, delta) in output.textures_delta.set {
                    renderer.update_texture(&gpu.device, &gpu.queue, id, &delta);
                }
                if frame == 2 {
                    capture(
                        gpu,
                        renderer,
                        &jobs,
                        output.pixels_per_point,
                        &directory.join(format!("{name}.png")),
                    );
                }
                for id in output.textures_delta.free {
                    renderer.free_texture(&id);
                }
            }
        }
    }
}
fn capture(
    gpu: &GpuContext,
    renderer: &mut eframe::egui_wgpu::Renderer,
    jobs: &[egui::epaint::ClippedPrimitive],
    pixels_per_point: f32,
    path: &std::path::Path,
) {
    let (width, height) = (1024, 768);
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI acceptance"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let screen = eframe::egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point,
    };
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let callbacks = renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, jobs, &screen);
    let view = texture.create_view(&Default::default());
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("UI acceptance"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.07,
                        g: 0.08,
                        b: 0.09,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.render(&mut pass.forget_lifetime(), jobs, &screen);
    }
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI acceptance pixels"),
        size: u64::from(width * height * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue
        .submit(callbacks.into_iter().chain([encoder.finish()]));
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    {
        let pixels = buffer.slice(..).get_mapped_range();
        image::save_buffer(path, &pixels, width, height, image::ColorType::Rgba8).unwrap();
    }
    buffer.unmap();
}
