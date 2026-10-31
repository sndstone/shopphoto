//! A scoped command registry for the desktop console, JSON-lines CLI and MCP.
use super::*;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{BufRead, Write},
    path::Path,
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub version: u32,
    pub command: String,
    #[serde(default = "empty_params")]
    pub params: Value,
}
fn empty_params() -> Value {
    json!({})
}

impl Request {
    fn check(&self) -> anyhow::Result<()> {
        if self.version != 1 {
            bail!("Unsupported command version {}", self.version);
        }
        if !self.params.is_object() {
            bail!("Command params must be an object");
        }
        if !registry().iter().any(|spec| spec["id"] == self.command) {
            bail!("Unknown command {}", self.command);
        }
        if matches!(
            self.command.as_str(),
            "engine.commands"
                | "develop.controls"
                | "photo.inspect"
                | "photo.save"
                | "photo.undo"
                | "photo.redo"
        ) && !self.params.as_object().is_some_and(|p| p.is_empty())
        {
            bail!("{} takes no parameters", self.command);
        }
        Ok(())
    }
    fn edit_command(&self) -> anyhow::Result<sp_core::commands::EditCommand> {
        self.check()?;
        Ok(serde_json::from_value(
            json!({"command":self.command,"params":self.params}),
        )?)
    }
}

pub(super) fn registry() -> Vec<Value> {
    [
        ("engine.commands", "List the stable command registry"),
        (
            "develop.controls",
            "List supported scalar controls and current values",
        ),
        (
            "photo.inspect",
            "Inspect the active photo and unsaved edits",
        ),
        (
            "photo.open",
            "Open a supported original; refuses to discard unsaved edits",
        ),
        (
            "photo.save",
            "Explicitly save edits; refuse externally changed sidecars",
        ),
        ("photo.undo", "Undo one edit command"),
        ("photo.redo", "Redo one edit command"),
        ("photo.rate", "Set a star rating from 0 to 5"),
        ("photo.flag", "Set pick, reject or none"),
        ("library.query", "Search the existing local catalog"),
        (
            "develop.set",
            "Set registered controls atomically as one undo step",
        ),
        ("develop.reset", "Reset develop settings as one undo step"),
        (
            "mask.radial",
            "Add a normalized radial mask with local exposure",
        ),
        (
            "mask.linear",
            "Add a normalized linear gradient with local exposure",
        ),
        (
            "mask.remove",
            "Remove a local adjustment by zero-based index",
        ),
        (
            "export.render",
            "Render active edits to a new output file; requires a GPU",
        ),
    ]
    .into_iter()
    .map(|(id, description)| json!({"id":id,"description":description,"version":1,"paramsSchema":params_schema(id)}))
    .collect()
}

fn params_schema(id: &str) -> Value {
    let point = json!({"type":"array","minItems":2,"maxItems":2,"items":{"type":"number","minimum":0,"maximum":1}});
    let ev = json!({"type":"number","minimum":-5,"maximum":5});
    let (properties, required) = match id {
        "develop.set" => (
            json!({"values":{"type":"object","minProperties":1,"additionalProperties":false,"properties":sp_core::commands::CONTROLS.iter().map(|control|(control.id.to_string(),json!({"type":"number","minimum":control.min,"maximum":control.max}))).collect::<serde_json::Map<_,_>>()}}),
            vec!["values"],
        ),
        "mask.radial" => (
            json!({"center":point,"radius":point,"feather":{"type":"number","minimum":0,"maximum":1},"exposure_ev":ev}),
            vec!["center", "radius", "feather", "exposure_ev"],
        ),
        "mask.linear" => (
            json!({"start":point,"end":point,"exposure_ev":ev}),
            vec!["start", "end", "exposure_ev"],
        ),
        "mask.remove" => (
            json!({"index":{"type":"integer","minimum":0}}),
            vec!["index"],
        ),
        "photo.open" => (json!({"path":{"type":"string"}}), vec!["path"]),
        "photo.rate" => (
            json!({"rating":{"type":"integer","minimum":0,"maximum":5}}),
            vec!["rating"],
        ),
        "photo.flag" => (
            json!({"flag":{"type":"string","enum":["pick","reject","none"]}}),
            vec!["flag"],
        ),
        "library.query" => (
            json!({"text":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":1000}}),
            vec![],
        ),
        "export.render" => (
            json!({"path":{"type":"string"},"long_edge_px":{"type":"integer","minimum":64,"maximum":16384}}),
            vec!["path"],
        ),
        _ => (json!({}), vec![]),
    };
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn controls(edit: &EditState) -> Value {
    json!(sp_core::commands::CONTROLS
        .iter()
        .map(|spec| json!({
            "id":spec.id,"min":spec.min,"max":spec.max,"default":spec.default,
            "value":sp_core::commands::control_value(edit,spec.id)
        }))
        .collect::<Vec<_>>())
}

#[derive(Debug, Default)]
pub(crate) struct Console {
    pub open: bool,
    pub request: String,
    pub result: String,
    pub execute: bool,
}

impl ShopPhotoApp {
    pub(super) fn automation_upkeep(&mut self) {
        if !std::mem::take(&mut self.state.command_console.execute) {
            return;
        }
        let result = (|| -> anyhow::Result<Value> {
            let request: Request = serde_json::from_str(&self.state.command_console.request)?;
            request.check()?;
            match request.command.as_str() {
                "engine.commands" => Ok(json!(registry())),
                "develop.controls" => Ok(controls(&self.state.edit)),
                "photo.inspect" => Ok(
                    json!({"source":self.state.open_image,"edit":self.state.edit,"annotations":self.state.annotations,"preview_cache_bytes":self.jobs.preview_cache_bytes()}),
                ),
                _ => {
                    if self.state.open_image.is_none()
                        || self.state.decode_in_flight
                        || self.state.auto_finish_busy
                        || self.state.enhancement_busy
                    {
                        bail!("Open a photo and wait for its current operation to finish");
                    }
                    match request.command.as_str() {
                        "photo.undo" => self.state.undo(),
                        "photo.redo" => self.state.redo(),
                        _ => {
                            let command = request.edit_command()?;
                            self.state.apply_command(&command)?;
                        }
                    }
                    Ok(json!({"status":"applied","command":request.command}))
                }
            }
        })();
        self.state.command_console.result = match result {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_default(),
            Err(error) => error.to_string(),
        };
    }
}

pub(crate) fn console_ui(ctx: &egui::Context, console: &mut Console) {
    if !console.open {
        return;
    }
    if console.request.is_empty() {
        console.request = "{\"version\":1,\"command\":\"develop.set\",\"params\":{\"values\":{\"light.exposure\":0.3}}}".into();
    }
    egui::Window::new(crate::i18n::text("Command console"))
        .open(&mut console.open)
        .default_width(540.0)
        .show(ctx, |ui| {
            ui.label(crate::i18n::text(
                "Develop commands share validation and undo with headless automation.",
            ));
            ui.add(
                egui::TextEdit::multiline(&mut console.request)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY),
            );
            if ui.button(crate::i18n::text("Execute command")).clicked() {
                console.execute = true;
            }
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    ui.monospace(&console.result);
                });
        });
}

struct Document {
    source: PathBuf,
    edit_source: PathBuf,
    baseline: Option<sp_catalog::Sidecar>,
    saved: sp_catalog::Sidecar,
    sidecar: sp_catalog::Sidecar,
    undo: Vec<sp_catalog::Sidecar>,
    redo: Vec<sp_catalog::Sidecar>,
}

struct Session {
    document: Option<Document>,
    runtime: Option<cli::Runtime>,
    catalog_path: Option<PathBuf>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            document: None,
            runtime: None,
            catalog_path: if cfg!(test) {
                None
            } else {
                config::catalog_path()
            },
        }
    }
}

impl Session {
    fn execute(&mut self, request: Request) -> anyhow::Result<Value> {
        request.check()?;
        match request.command.as_str() {
            "engine.commands" => return Ok(json!(registry())),
            "library.query" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Query {
                    #[serde(default)]
                    text: String,
                    #[serde(default = "query_limit")]
                    limit: usize,
                }
                let query: Query = serde_json::from_value(request.params)?;
                if query.limit == 0 || query.limit > 1000 {
                    bail!("Query limit must be 1..=1000");
                }
                let path = self
                    .catalog_path
                    .as_ref()
                    .context("Catalog path unavailable")?;
                if !path.is_file() {
                    bail!("No existing ShopPhoto catalog");
                }
                let photos = sp_catalog::CatalogDb::open(path)?.photos(
                    &sp_catalog::LibraryFilter {
                        text: query.text,
                        ..Default::default()
                    },
                    sp_catalog::LibrarySort::CaptureTime,
                )?;
                return Ok(json!(photos.into_iter().take(query.limit).map(|photo| json!({"id":photo.id,"path":photo.path,"rating":photo.annotations.rating,"flag":photo.annotations.flag})).collect::<Vec<_>>()));
            }
            "photo.open" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Open {
                    path: PathBuf,
                }
                let params: Open = serde_json::from_value(request.params)?;
                if self
                    .document
                    .as_ref()
                    .is_some_and(|doc| doc.sidecar != doc.saved)
                {
                    bail!("Save unsaved edits before opening another photo");
                }
                if !params.path.is_file() || !sp_raw::is_supported_image(&params.path) {
                    bail!("Choose an existing supported photograph");
                }
                let source = params.path.canonicalize()?;
                let prepared = config::source_cache_dir()
                    .map(|cache| sp_catalog::source_cache::prepare(&cache, &source))
                    .transpose()?;
                let edit_source = prepared.map_or_else(|| source.clone(), |p| p.decode_path);
                let baseline = sp_catalog::load_sidecar(&edit_source)?;
                let mut sidecar = baseline.clone().unwrap_or_else(|| sp_catalog::Sidecar {
                    version: sp_core::edit::EDIT_SCHEMA_VERSION,
                    ..Default::default()
                });
                if baseline.is_none() {
                    if let Some(xmp) = sp_catalog::xmp::load_with_metadata(&edit_source)? {
                        sidecar.metadata = xmp.metadata;
                        if let Some(annotations) = xmp.annotations {
                            sidecar.annotations = annotations;
                        }
                    }
                }
                self.document = Some(Document {
                    source,
                    edit_source,
                    baseline,
                    saved: sidecar.clone(),
                    sidecar,
                    undo: vec![],
                    redo: vec![],
                });
                return self.inspect();
            }
            "export.render" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Render {
                    path: PathBuf,
                    #[serde(default)]
                    long_edge_px: Option<u32>,
                }
                let params: Render = serde_json::from_value(request.params)?;
                let doc = self.document.as_ref().context("Open a photo first")?;
                if params.path.exists() {
                    bail!("Output exists; choose a new file");
                }
                if export::same_file(&doc.source, &params.path)
                    || export::same_file(&doc.edit_source, &params.path)
                {
                    bail!("Output is the original photo");
                }
                let mut settings = sp_export::ExportSettings {
                    format: cli::format_for_path(&params.path)?,
                    ..Default::default()
                };
                if let Some(edge) = params.long_edge_px {
                    if !(64..=16384).contains(&edge) {
                        bail!("Long edge must be 64..=16384 pixels");
                    }
                    settings.size = sp_export::SizeMode::LongEdgePx(edge);
                }
                if self.runtime.is_none() {
                    self.runtime = Some(cli::Runtime::new()?);
                }
                self.runtime
                    .as_mut()
                    .context("Renderer unavailable")?
                    .render(
                        &doc.edit_source,
                        &params.path,
                        &doc.sidecar.edit,
                        export::export_metadata(&doc.sidecar.metadata, doc.sidecar.annotations),
                        &settings,
                        None,
                    )?;
                return Ok(json!({"status":"finished","output":params.path}));
            }
            _ => {}
        }
        let doc = self.document.as_mut().context("Open a photo first")?;
        let before = doc.sidecar.clone();
        match request.command.as_str() {
            "develop.controls" => return Ok(controls(&doc.sidecar.edit)),
            "photo.inspect" => return self.inspect(),
            "photo.save" => {
                if sp_catalog::load_sidecar(&doc.edit_source)? != doc.baseline {
                    bail!("Sidecar changed externally; reopen before saving");
                }
                sp_catalog::save_sidecar(&doc.edit_source, &doc.sidecar)?;
                doc.baseline = Some(doc.sidecar.clone());
                doc.saved = doc.sidecar.clone();
                if let Some(catalog) = self.catalog_path.as_ref().filter(|path| path.is_file()) {
                    let mirrored = sp_catalog::CatalogDb::open(catalog).and_then(|db| {
                        db.set_annotations(&doc.source, doc.sidecar.annotations)?;
                        db.set_descriptive_metadata(&doc.source, &doc.sidecar.metadata)
                    });
                    if let Err(error) = mirrored {
                        tracing::warn!(%error, "sidecar saved but catalog mirror failed");
                    }
                }
                if doc.edit_source != doc.source {
                    sp_catalog::source_cache::sync_sidecar(&doc.edit_source, &doc.source)?;
                }
                return Ok(json!({"status":"saved","source":doc.source}));
            }
            "photo.rate" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Rating {
                    rating: u8,
                }
                let params: Rating = serde_json::from_value(request.params)?;
                if params.rating > 5 {
                    bail!("Rating must be 0..=5");
                }
                doc.sidecar.annotations.rating = params.rating;
            }
            "photo.flag" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Flag {
                    flag: String,
                }
                let params: Flag = serde_json::from_value(request.params)?;
                doc.sidecar.annotations.flag = match params.flag.as_str() {
                    "pick" => sp_catalog::Flag::Pick,
                    "reject" => sp_catalog::Flag::Reject,
                    "none" => sp_catalog::Flag::None,
                    _ => bail!("Flag must be pick, reject or none"),
                };
            }
            "photo.undo" => {
                if let Some(previous) = doc.undo.pop() {
                    doc.redo.push(std::mem::replace(&mut doc.sidecar, previous));
                }
                return Ok(json!({"status":"applied","command":request.command}));
            }
            "photo.redo" => {
                if let Some(next) = doc.redo.pop() {
                    doc.undo.push(std::mem::replace(&mut doc.sidecar, next));
                }
                return Ok(json!({"status":"applied","command":request.command}));
            }
            _ => {
                let command = request.edit_command()?;
                let next = command.apply(&doc.sidecar.edit)?;
                doc.sidecar.edit = next;
            }
        }
        if doc.sidecar != before {
            doc.undo.push(before);
            if doc.undo.len() > 100 {
                doc.undo.remove(0);
            }
            doc.redo.clear();
        }
        Ok(json!({"status":"applied","command":request.command}))
    }

    fn inspect(&self) -> anyhow::Result<Value> {
        let doc = self.document.as_ref().context("Open a photo first")?;
        Ok(
            json!({"source":doc.source,"edit":doc.sidecar.edit,"annotations":doc.sidecar.annotations,"unsaved":doc.sidecar!=doc.saved}),
        )
    }
}
fn query_limit() -> usize {
    100
}

fn line(reader: &mut impl BufRead) -> anyhow::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let count = std::io::Read::take(&mut *reader, 65537).read_until(b'\n', &mut bytes)?;
    if count == 0 {
        return Ok(None);
    }
    if count > 65536 {
        bail!("Command line exceeds 64 KiB");
    }
    Ok(Some(bytes))
}

pub(super) fn run_script(args: &[std::ffi::OsString]) -> anyhow::Result<()> {
    if args.len() != 3 || args[1] != "--script" {
        bail!(
            "Use shopphoto run INPUT --script FILE.jsonl; save with an explicit photo.save command"
        );
    }
    let mut session = Session::default();
    session.execute(Request {
        version: 1,
        command: "photo.open".into(),
        params: json!({"path":PathBuf::from(&args[0])}),
    })?;
    let file = std::fs::File::open(Path::new(&args[2]))?;
    if file.metadata()?.len() > 8 * 1024 * 1024 {
        bail!("Script exceeds 8 MiB");
    }
    let mut reader = std::io::BufReader::new(file);
    while let Some(bytes) = line(&mut reader)? {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let request: Request = serde_json::from_slice(&bytes)?;
        // Stop at the first error so dependent commands cannot save partial work.
        let result = session.execute(request)?;
        println!("{}", result);
    }
    Ok(())
}

pub(super) fn mcp(args: &[std::ffi::OsString]) -> anyhow::Result<()> {
    if !args.is_empty() {
        bail!("Use shopphoto mcp; open photos with the execute tool");
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(&mut stdin.lock(), &mut stdout.lock())
}

fn tools_list() -> Value {
    json!({"tools":[
        {"name":"execute","description":"Execute a ShopPhoto command. Start with engine.commands, then photo.open. Edits stay in memory until photo.save. Each develop command is undoable.","inputSchema":{"type":"object","properties":{"version":{"type":"integer","const":1},"command":{"type":"string"},"params":{"type":"object"}},"required":["version","command"],"additionalProperties":false}},
        {"name":"render_preview","description":"Return an sRGB PNG of the active photo's current edits. Requires a GPU. Preserves originals and sidecars.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}
    ]})
}

fn serve(reader: &mut impl BufRead, writer: &mut impl Write) -> anyhow::Result<()> {
    use base64::Engine;
    let mut session = Session::default();
    while let Some(bytes) = line(reader)? {
        let parsed = serde_json::from_slice::<Value>(&bytes);
        let message = match parsed {
            Ok(message) => message,
            Err(error) => {
                writeln!(
                    writer,
                    "{}",
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":error.to_string()}})
                )?;
                writer.flush()?;
                continue;
            }
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let result = (|| -> anyhow::Result<Value> {
            if message["jsonrpc"] != "2.0" {
                bail!("Expected JSON-RPC 2.0");
            }
            match message["method"].as_str().unwrap_or("") {
                "initialize" => Ok(
                    json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"shopphoto","version":env!("CARGO_PKG_VERSION")}}),
                ),
                "ping" => Ok(json!({})),
                "tools/list" => Ok(tools_list()),
                "tools/call" => {
                    let args = message["params"]
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(empty_params);
                    let call = match message["params"]["name"].as_str().unwrap_or("") {
                        "execute" => serde_json::from_value::<Request>(args).map_err(anyhow::Error::from).and_then(|request| session.execute(request)).map(|value| json!({"content":[{"type":"text","text":value.to_string()}]})),
                        "render_preview" => (|| -> anyhow::Result<Value> {
                            if !args.as_object().is_some_and(|object| object.is_empty()) { bail!("render_preview takes no arguments"); }
                            let doc = session.document.as_ref().context("Open a photo first")?;
                            if session.runtime.is_none() { session.runtime = Some(cli::Runtime::new()?); }
                            let png = session.runtime.as_mut().context("Renderer unavailable")?.preview(&doc.edit_source, &doc.sidecar.edit)?;
                            Ok(json!({"content":[{"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(png)}]}))
                        })(),
                        _ => bail!("Unknown tool"),
                    };
                    Ok(call.unwrap_or_else(|error| json!({"isError":true,"content":[{"type":"text","text":error.to_string()}]})))
                }
                _ => bail!("Method not found"),
            }
        })();
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":error.to_string()}})
            }
        };
        writeln!(writer, "{response}")?;
        writer.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(command: &str, params: Value) -> Request {
        Request {
            version: 1,
            command: command.into(),
            params,
        }
    }

    #[test]
    fn desktop_command_is_one_history_step_and_validation_is_atomic() {
        let mut state = AppState::default();
        let original = state.edit.clone();
        let command = request(
            "develop.set",
            json!({"values":{"light.exposure":0.7,"light.shadows":18.0}}),
        )
        .edit_command()
        .unwrap();
        state.apply_command(&command).unwrap();
        let edited = state.edit.clone();
        assert_eq!(edited.tone.exposure_ev, 0.7);
        state.undo();
        assert_eq!(state.edit, original);
        state.redo();
        assert_eq!(state.edit, edited);
        let invalid = request(
            "develop.set",
            json!({"values":{"light.exposure":1.0,"light.shadows":101.0}}),
        )
        .edit_command()
        .unwrap();
        assert!(state.apply_command(&invalid).is_err());
        assert_eq!(state.edit, edited);
        state.undo();
        assert_eq!(state.edit, original);
    }

    #[test]
    fn jsonl_runner_accepts_native_paths_with_spaces() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("photo with spaces.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([100, 120, 140, 255]))
            .save(&source)
            .unwrap();
        let script = directory.path().join("inspect.jsonl");
        std::fs::write(
            &script,
            "{\"version\":1,\"command\":\"photo.inspect\",\"params\":{}}\n",
        )
        .unwrap();
        run_script(&[
            source.clone().into_os_string(),
            "--script".into(),
            script.into_os_string(),
        ])
        .unwrap();
        assert!(!sp_catalog::sidecar_path(&source).exists());
    }

    #[test]
    fn command_edit_undo_and_conflict_checked_save_preserve_originals() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("photo.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([100, 120, 140, 255]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let catalog = directory.path().join("catalog.sqlite");
        sp_catalog::CatalogDb::open(&catalog).unwrap();
        let mut session = Session {
            catalog_path: Some(catalog),
            ..Default::default()
        };
        session
            .execute(request("photo.open", json!({"path":source})))
            .unwrap();
        session
            .execute(request(
                "develop.set",
                json!({"values":{"light.exposure":1.0,"light.shadows":20.0}}),
            ))
            .unwrap();
        assert!(!sp_catalog::sidecar_path(&source).exists());
        session.execute(request("photo.undo", json!({}))).unwrap();
        assert_eq!(
            session
                .document
                .as_ref()
                .unwrap()
                .sidecar
                .edit
                .tone
                .exposure_ev,
            0.0
        );
        session.execute(request("photo.redo", json!({}))).unwrap();
        session.execute(request("photo.save", json!({}))).unwrap();
        assert_eq!(
            sp_catalog::load_sidecar(&source)
                .unwrap()
                .unwrap()
                .edit
                .tone
                .exposure_ev,
            1.0
        );
        let mut foreign = sp_catalog::load_sidecar(&source).unwrap().unwrap();
        foreign.annotations.rating = 5;
        sp_catalog::save_sidecar(&source, &foreign).unwrap();
        assert!(session.execute(request("photo.save", json!({}))).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert_eq!(sp_catalog::load_sidecar(&source).unwrap().unwrap(), foreign);
    }

    #[test]
    fn cached_save_mirrors_to_source_even_if_catalog_is_unreadable() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("photo.png");
        let cached = directory.path().join("cached.png");
        image::RgbaImage::new(8, 8).save(&source).unwrap();
        let catalog = directory.path().join("broken.sqlite");
        std::fs::write(&catalog, b"not a database").unwrap();
        let mut session = Session {
            catalog_path: Some(catalog),
            ..Default::default()
        };
        session
            .execute(request("photo.open", json!({"path":source})))
            .unwrap();
        let doc = session.document.as_mut().unwrap();
        sp_catalog::save_sidecar(&source, &doc.sidecar).unwrap();
        doc.edit_source = cached.clone();
        doc.sidecar.edit.tone.exposure_ev = 1.25;
        session.execute(request("photo.save", json!({}))).unwrap();
        let local = sp_catalog::load_sidecar(&cached).unwrap().unwrap();
        assert_eq!(local.edit.tone.exposure_ev, 1.25);
        assert_eq!(sp_catalog::load_sidecar(&source).unwrap().unwrap(), local);
    }

    #[test]
    fn mcp_handshake_tools_and_validation_are_protocol_messages_only() {
        let input = concat!("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"execute\",\"arguments\":{\"version\":99,\"command\":\"develop.set\"}}}\n");
        let mut output = Vec::new();
        serve(&mut input.as_bytes(), &mut output).unwrap();
        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "shopphoto");
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 2);
        assert_eq!(lines[2]["result"]["isError"], true);
    }
}
