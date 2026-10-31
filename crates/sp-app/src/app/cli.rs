//! Headless photo automation: paths in, newline JSON results out.
use super::*;
use anyhow::{bail, Context};
use sp_export::{ExportFormat, ExportImage, ExportSettings};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
const HELP: &str = r#"ShopPhoto photo automation

  shopphoto render INPUT --output FILE [--settings FILE] [--workflow FILE] [--exposure EV]
  shopphoto commands
  shopphoto run INPUT --script FILE.jsonl
  shopphoto mcp
  shopphoto queue QUEUE.json
  shopphoto workflow-init FILE.spworkflow

Render reads saved edits. Settings accepts an export preset/settings JSON;
workflow accepts a .spworkflow selective develop + delivery recipe.
Without settings, the output extension selects JPEG/PNG/TIFF/WebP/AVIF/JXL.
Queue runs pending tasks and saves each result; close any app using that queue.
Render and queue require a GPU, keep originals unchanged, and write JSON results
to stdout. Errors exit nonzero. workflow-init creates an editable recipe template.
"#;
pub(crate) fn try_run() -> Option<anyhow::Result<()>> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match args.first()?.to_str()? {
        "--help" | "-h" => {
            println!("{HELP}");
            Some(Ok(()))
        }
        "mcp" => Some(super::automation::mcp(&args[1..])),
        "run" => Some(super::automation::run_script(&args[1..])),
        "commands" => Some({
            println!("{}", serde_json::json!(super::automation::registry()));
            Ok(())
        }),
        "render" => Some(render_command(&args[1..])),
        "queue" => Some(queue_command(&args[1..])),
        "workflow-init" => Some(workflow_init(&args[1..])),
        _ => None,
    }
}
fn workflow_init(args: &[OsString]) -> anyhow::Result<()> {
    if args.len() != 1 {
        bail!("workflow-init requires one .spworkflow output path");
    }
    let path = PathBuf::from(&args[0]);
    if path.exists() {
        bail!("Workflow already exists; choose another path");
    }
    let workflow = sp_export::workflow::PhotoWorkflow {
        version: 1,
        name: "Photo workflow".into(),
        develop: Some(sp_core::preset::UserPreset::from_edit(
            "Photo workflow",
            Default::default(),
            &sp_core::EditState::default(),
        )),
        export: ExportSettings::default(),
    };
    workflow.save(&path).map_err(anyhow::Error::msg)?;
    println!(
        "{}",
        serde_json::json!({"status":"created","workflow":path})
    );
    Ok(())
}
pub(super) fn format_for_path(path: &Path) -> anyhow::Result<ExportFormat> {
    Ok(
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "jpg" | "jpeg" => ExportFormat::Jpeg {
                quality: 92,
                chroma_444: true,
            },
            "png" => ExportFormat::Png { sixteen_bit: true },
            "tif" | "tiff" => ExportFormat::Tiff {
                bit_depth: sp_export::TiffDepth::Sixteen,
                compressed: true,
            },
            "webp" => ExportFormat::WebP { lossless: true },
            "avif" => ExportFormat::Avif { quality: 85 },
            "jxl" => ExportFormat::JpegXl {
                quality: 95,
                lossless: false,
            },
            _ => bail!("Use a JPEG, PNG, TIFF, WebP, AVIF or JPEG XL output extension"),
        },
    )
}
fn render_command(args: &[OsString]) -> anyhow::Result<()> {
    let source = PathBuf::from(args.first().context("render requires INPUT; use --help")?);
    let (mut output, mut settings_file, mut workflow_file, mut exposure) = (None, None, None, None);
    let mut index = 1;
    while index < args.len() {
        let option = args[index].to_str().context("Invalid option")?;
        let value = args
            .get(index + 1)
            .with_context(|| format!("Missing value for {option}"))?;
        match option {
            "--output" => output = Some(PathBuf::from(value)),
            "--settings" => settings_file = Some(PathBuf::from(value)),
            "--workflow" => workflow_file = Some(PathBuf::from(value)),
            "--exposure" => {
                let ev = value
                    .to_str()
                    .context("Exposure must be numeric")?
                    .parse::<f32>()?;
                if !ev.is_finite() || !(-5.0..=5.0).contains(&ev) {
                    bail!("Exposure must be between -5 and +5 EV");
                }
                exposure = Some(ev);
            }
            _ => bail!("Unknown option {option}"),
        }
        index += 2;
    }
    let output = output.context("render requires --output FILE")?;
    if export::same_file(&source, &output) {
        bail!("Output is the original photo; choose another path");
    }
    let prepared = config::source_cache_dir()
        .map(|cache| sp_catalog::source_cache::prepare(&cache, &source))
        .transpose()?;
    let input = prepared
        .as_ref()
        .map_or(source.as_path(), |p| p.decode_path.as_path());
    let sidecar = sp_catalog::load_sidecar(input)?;
    let (mut edit, metadata) = sidecar.map_or_else(
        || {
            (
                sp_core::EditState::default(),
                sp_export::PhotoMetadata::default(),
            )
        },
        |s| (s.edit, export::export_metadata(&s.metadata, s.annotations)),
    );
    let mut settings = ExportSettings::default();
    if let Some(path) = workflow_file {
        let workflow =
            sp_export::workflow::PhotoWorkflow::load(&path).map_err(anyhow::Error::msg)?;
        workflow.apply(&mut edit).map_err(anyhow::Error::msg)?;
        settings = workflow.export;
    } else if settings_file.is_none() {
        settings.format = format_for_path(&output)?;
    }
    if let Some(path) = settings_file {
        if std::fs::metadata(&path)?.len() > 64 * 1024 * 1024 {
            bail!("Settings exceeds 64 MiB");
        }
        let bytes = std::fs::read(&path)?;
        settings = serde_json::from_slice::<sp_export::ExportPreset>(&bytes)
            .map(|p| p.settings)
            .or_else(|_| serde_json::from_slice(&bytes))?;
    }
    if let Some(ev) = exposure {
        edit.tone.exposure_ev = ev;
    }
    let extension_format = format_for_path(&output)?;
    if std::mem::discriminant(&extension_format) != std::mem::discriminant(&settings.format)
        && !matches!(
            (extension_format, settings.format),
            (ExportFormat::Jpeg { .. }, ExportFormat::UltraHdr { .. })
        )
    {
        bail!(
            "Output extension does not match the workflow/settings format (use .{})",
            settings.format.extension()
        );
    }
    Runtime::new()?.render(input, &output, &edit, metadata, &settings, None)?;
    println!(
        "{}",
        serde_json::json!({"status":"finished","source":source,"output":output})
    );
    Ok(())
}
fn queue_command(args: &[OsString]) -> anyhow::Result<()> {
    if args.len() != 1 {
        bail!("queue requires one export queue JSON path");
    }
    let path = PathBuf::from(&args[0]);
    let _owner = sp_export::queue::QueueLock::acquire(&path).map_err(anyhow::Error::msg)?;
    if !path.is_file() {
        bail!("Export queue does not exist");
    }
    let mut queue = sp_export::queue::ExportQueue::load(&path).map_err(anyhow::Error::msg)?;
    queue.paused = false;
    let mut runtime = Runtime::new()?;
    let mut failures = 0;
    while let Some(task) = queue.start_next() {
        queue.save(&path).map_err(anyhow::Error::msg)?;
        let result = (|| -> anyhow::Result<PathBuf> {
            let prepared = config::source_cache_dir()
                .map(|cache| sp_catalog::source_cache::prepare(&cache, &task.source))
                .transpose()?;
            let input = prepared
                .as_ref()
                .map_or(task.source.as_path(), |p| p.decode_path.as_path());
            let decoded = sp_raw::decode_source(input)?;
            let output = sp_export::output_path(
                &task.source,
                &task.settings,
                Some(&decoded.meta),
                task.sequence,
            );
            if export::same_file(&task.source, &output) {
                bail!("Output is the original photo");
            }
            runtime.render(
                input,
                &output,
                &task.edit,
                task.metadata.clone(),
                &task.settings,
                Some(decoded),
            )?;
            Ok(output)
        })();
        match result {
            Ok(output) => {
                println!(
                    "{}",
                    serde_json::json!({"id":task.id,"status":"finished","output":output})
                );
                queue.finish(task.id, Ok(output));
            }
            Err(error) => {
                failures += 1;
                println!(
                    "{}",
                    serde_json::json!({"id":task.id,"status":"failed","error":error.to_string()})
                );
                queue.finish(task.id, Err(error.to_string()));
            }
        }
        queue.save(&path).map_err(anyhow::Error::msg)?;
    }
    if failures > 0 {
        bail!("{failures} queue tasks failed; details are saved in the queue");
    }
    Ok(())
}
pub(super) struct Runtime {
    renderer: GpuRenderer,
    profiles: Vec<sp_core::profile::CameraProfile>,
    lenses: Vec<sp_core::lens::LensProfile>,
    looks: Vec<(String, sp_core::cube::CubeLut)>,
    cancel: AtomicBool,
}
impl Runtime {
    pub(super) fn new() -> anyhow::Result<Self> {
        Ok(Self {
            renderer: GpuRenderer::new(GpuContext::headless()?)?,
            profiles: config::load_profiles(),
            lenses: config::load_lens_profiles(),
            looks: config::load_custom_looks(),
            cancel: AtomicBool::new(false),
        })
    }
    fn configure(
        &mut self,
        input: &Path,
        edit: &EditState,
        decoded: Option<sp_raw::SourceOutput>,
    ) -> anyhow::Result<sp_core::ImageMeta> {
        let decoded = match decoded {
            Some(decoded) => decoded,
            None => sp_raw::decode_source(input)?,
        };
        let meta = decoded.meta;
        let mut source = decoded.source;
        let neural =
            source.is_raw() && edit.nr.camera_denoise_mode != sp_core::edit::CameraDenoiseMode::Off;
        if neural {
            let root = sp_ai::model_root().context("Selected RAW denoise model is unavailable")?;
            let raw = source.as_raw().context("RAW source missing")?;
            let inferred = crate::inference_cache::infer(raw, &root)?;
            source = sp_core::source::ImageSource::Raw(std::sync::Arc::new(sp_ai::blend(
                raw,
                &inferred,
                edit.nr.camera_denoise_mode.strength(meta.iso),
            )));
        }
        self.renderer.set_capture_metadata(Some(&meta));
        self.renderer.set_external_raw_denoise(neural);
        self.renderer.set_lens_profile(
            config::resolve_lens(&self.lenses, &edit.lens, &meta),
            meta.aperture_f,
        );
        self.renderer.open_source_with_edit(&source, edit)?;
        let profile = match &edit.profile {
            sp_core::edit::ProfileChoice::Named(name) => Some(
                self.profiles
                    .iter()
                    .find(|p| {
                        &p.name == name
                            && config::camera_matches(&p.camera_model, &meta.camera_model)
                    })
                    .with_context(|| format!("Camera profile {name} is unavailable"))?
                    .clone(),
            ),
            sp_core::edit::ProfileChoice::Standard => None,
        };
        self.renderer.set_profile(profile);
        let look = edit
            .look
            .as_ref()
            .map(|name| {
                self.looks
                    .iter()
                    .find(|(n, _)| n == name)
                    .cloned()
                    .with_context(|| format!("Look {name} is unavailable"))
            })
            .transpose()?;
        self.renderer.set_custom_look(look);
        Ok(meta)
    }

    pub(super) fn preview(&mut self, input: &Path, edit: &EditState) -> anyhow::Result<Vec<u8>> {
        self.renderer.set_proxy_long_edge(1024);
        self.configure(input, edit, None)?;
        self.renderer.render_without_histogram(edit);
        let (rgba, width, height) = self.renderer.read_display_srgb8()?;
        let pixels = ExportImage::new(width, height, rgba).context("Invalid preview pixels")?;
        let settings = ExportSettings {
            size: sp_export::SizeMode::LongEdgePx(1024),
            ..Default::default()
        };
        let pixels = enhance::prepare_export(pixels, edit.geometry, &settings, false, &self.cancel)
            .map_err(anyhow::Error::msg)?;
        let image = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba8)
            .context("Invalid prepared preview pixels")?;
        let mut output = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut output, image::ImageFormat::Png)?;
        Ok(output.into_inner())
    }

    pub(super) fn render(
        &mut self,
        input: &Path,
        output: &Path,
        edit: &sp_core::EditState,
        metadata: sp_export::PhotoMetadata,
        settings: &ExportSettings,
        decoded: Option<sp_raw::SourceOutput>,
    ) -> anyhow::Result<()> {
        if export::same_file(input, output) {
            bail!("Output is the original photo");
        }
        let meta = self.configure(input, edit, decoded)?;
        let mut settings = settings.clone();
        settings.photo_metadata = metadata;
        let hdr = edit.hdr.enabled
            && edit.hdr.target == sp_core::edit::HdrTarget::Hdr
            && matches!(
                settings.format,
                ExportFormat::Avif { .. }
                    | ExportFormat::Png { sixteen_bit: true }
                    | ExportFormat::JpegXl { .. }
            );
        if hdr || settings.output_profile.is_some() {
            settings.color_space = sp_export::ExportColorSpace::Rec2020;
        }
        let precise = settings.format.supports_sixteen_bit()
            || hdr
            || settings.upscale_model.is_some()
            || settings.output_profile.is_some();
        let image = handoff(self.renderer.render_export(
            edit,
            hdr,
            precise,
            settings.color_space,
            &self.cancel,
            |_| {},
        )?)?;
        let prepared = enhance::prepare_export(image, edit.geometry, &settings, hdr, &self.cancel)
            .map_err(anyhow::Error::msg)?;
        if matches!(settings.format, ExportFormat::UltraHdr { .. }) {
            let hdr_image = handoff(self.renderer.render_export(
                edit,
                true,
                true,
                sp_export::ExportColorSpace::Rec2020,
                &self.cancel,
                |_| {},
            )?)?;
            let mut hdr_settings = settings.clone();
            hdr_settings.color_space = sp_export::ExportColorSpace::Rec2020;
            let hdr = enhance::prepare_export(
                hdr_image,
                edit.geometry,
                &hdr_settings,
                true,
                &self.cancel,
            )
            .map_err(anyhow::Error::msg)?;
            sp_export::encode_ultra_hdr(&prepared, &hdr, &settings, output, Some(&meta))?;
        } else {
            sp_export::encode_file_with_metadata(&prepared, &settings, output, hdr, Some(&meta))?;
        }
        Ok(())
    }
}
fn handoff(image: sp_pipeline::renderer::RenderedImage) -> anyhow::Result<ExportImage> {
    ExportImage::from_buffers(image.width, image.height, image.rgba8, image.rgba16)
        .context("GPU returned invalid export pixels")
}
