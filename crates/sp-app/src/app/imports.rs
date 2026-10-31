//! Importing profiles, looks, ICC proofs, presets and Reference Match fits.

use super::*;

pub(super) struct ReferenceJobResult {
    pub(super) profile: sp_core::profile::CameraProfile,
    pub(super) report: ReferenceMatchReport,
}

pub(super) fn run_reference_match(
    raws: Vec<PathBuf>,
    tiff_dir: PathBuf,
) -> Result<ReferenceJobResult, String> {
    let mut tiffs = std::collections::HashMap::new();
    for entry in std::fs::read_dir(&tiff_dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let is_tiff = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "tif" | "tiff"));
        if is_tiff {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                tiffs.insert(stem.to_ascii_lowercase(), path);
            }
        }
    }
    let mut all_pairs = Vec::new();
    let mut model = String::new();
    let mut embedded_matrix = None;
    let mut all_icc = true;
    let per_image = (50_000 / raws.len().max(1)).max(2_000);
    for raw_path in &raws {
        let stem = raw_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                let __l10n_args = (&(raw_path.display()),);
                crate::i18n::format("{0} has no usable file name", &[__l10n_args.0.to_string()])
            })?
            .to_ascii_lowercase();
        let tiff_path = tiffs.get(&stem).ok_or_else(|| {
            crate::i18n::format("No TIFF named {0}.tif/.tiff", std::slice::from_ref(&stem))
        })?;
        let decoded = sp_raw::decode(raw_path).map_err(|e| e.to_string())?;
        if model.is_empty() {
            model = decoded.meta.camera_model.clone();
            embedded_matrix = Some(decoded.raw.cam_to_xyz);
        } else if camera_key(&decoded.meta.camera_model) != camera_key(&model) {
            return Err("All calibration RAWs must come from the same camera model".into());
        }
        let (pairs, summary) =
            sp_pipeline::match_io::sample_pair(&decoded.raw, tiff_path, per_image)
                .map_err(|e| e.to_string())?;
        all_icc &= summary.embedded_icc;
        all_pairs.extend(pairs);
    }
    let fit = sp_pipeline::match_fit::fit_match(&all_pairs)
        .ok_or_else(|| "Calibration samples were not sufficiently varied".to_owned())?;
    let profile_name = crate::i18n::format("Reference Match — {0}", &[model.to_string()]);
    let profile = sp_pipeline::match_fit::make_match_profile(
        &profile_name,
        &model,
        &embedded_matrix.ok_or_else(|| "No RAW was decoded".to_owned())?,
        &fit,
    );
    Ok(ReferenceJobResult {
        profile,
        report: ReferenceMatchReport {
            profile_name,
            paired_images: raws.len(),
            samples: fit.holdout.samples,
            mean_delta_e_00: fit.holdout.mean_delta_e_00,
            p95_delta_e_00: fit.holdout.p95_delta_e_00,
            all_embedded_icc: all_icc,
        },
    })
}

impl ShopPhotoApp {
    pub(super) fn refresh_profile_names(&mut self) {
        let model = self
            .state
            .meta
            .as_ref()
            .map(|m| m.camera_model.as_str())
            .unwrap_or_default();
        self.state.profile_names = self
            .profiles
            .iter()
            .filter(|p| self.state.source_is_raw && camera_matches(&p.camera_model, model))
            .map(|p| p.name.clone())
            .collect();
        self.state.profile_names.sort();
        self.state.profile_names.dedup();

        self.state.lens_names = self.lenses.iter().map(|p| p.name.clone()).collect();
        self.state.lens_names.sort();
        self.state.lens_names.dedup();
        self.state.matched_lens = self
            .state
            .meta
            .as_ref()
            .filter(|_| self.state.source_is_raw)
            .and_then(|meta| {
                sp_raw::lens_store::match_lens(
                    &self.lenses,
                    self.state
                        .edit
                        .lens
                        .lens_model_override
                        .as_deref()
                        .or(meta.lens_model.as_deref()),
                    meta.focal_length_mm,
                )
                .map(|p| p.name.clone())
            });
    }

    pub(super) fn save_lens_geometry(&mut self) {
        let Some(meta) = &self.state.meta else {
            self.state.push_toast("Open an image first", true);
            return;
        };
        let Some(active) = resolve_lens(&self.lenses, &self.state.edit.lens, meta).cloned() else {
            self.state
                .push_toast("Select an existing .splens profile first", true);
            return;
        };
        let Some(directory) = lenses_dir() else {
            self.state
                .push_toast("The lens-profile directory is unavailable", true);
            return;
        };
        let path = std::fs::read_dir(&directory)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("splens"))
            .find(|path| {
                sp_raw::lens_store::load_lens(path).is_ok_and(|profile| profile.name == active.name)
            })
            .unwrap_or_else(|| {
                let safe: String = active
                    .key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .collect();
                directory.join(format!(
                    "{}{}.splens",
                    if safe.is_empty() { "lens" } else { &safe },
                    active
                        .focal_mm
                        .map_or_else(String::new, |focal| format!("-{focal:.0}mm"))
                ))
            });
        let mut updated = active;
        updated.distortion = self.state.edit.lens.distortion;
        updated.lateral_ca = self.state.edit.lens.lateral_ca;
        match sp_raw::lens_store::save_lens(&path, &updated) {
            Ok(()) => {
                self.lenses = load_lens_profiles();
                self.refresh_profile_names();
                self.state.push_toast(
                    {
                        let __l10n_args = (&(path.display()),);
                        crate::i18n::format("Saved geometry to {0}", &[__l10n_args.0.to_string()])
                    },
                    false,
                );
            }
            Err(error) => self.state.push_toast(error.to_string(), true),
        }
    }

    pub(super) fn import_dcp_dialog(&mut self) {
        if self.dcp_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-dcp-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(crate::i18n::text("DNG Camera Profile"), &["dcp"])
                    .pick_file();
                let _ = tx.send(picked);
            })
            .expect("spawn dcp dialog thread");
        self.dcp_pick = Some(rx);
    }

    pub(super) fn import_cube_dialog(&mut self) {
        if self.cube_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-cube-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(crate::i18n::text("Creative LUT"), &["cube"])
                    .pick_file();
                let _ = tx.send(picked);
            })
            .expect("spawn cube dialog thread");
        self.cube_pick = Some(rx);
    }

    pub(super) fn finish_cube_import(&mut self, path: &std::path::Path) {
        let parsed = std::fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|source| sp_core::cube::parse(&source).map_err(|error| error.to_string()));
        match parsed {
            Ok(cube) => {
                let name = cube
                    .title
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    });
                if let Some(directory) = looks_dir() {
                    let _ = std::fs::create_dir_all(&directory);
                    let destination = directory.join(
                        path.file_name()
                            .unwrap_or_else(|| std::ffi::OsStr::new("imported.cube")),
                    );
                    if let Err(error) = std::fs::copy(path, &destination) {
                        self.state.push_toast(
                            crate::i18n::format(
                                "Could not store creative LUT: {0}",
                                &[error.to_string()],
                            ),
                            true,
                        );
                        return;
                    }
                }
                self.custom_looks
                    .retain(|(candidate, _)| candidate != &name);
                self.custom_looks.push((name.clone(), cube));
                self.custom_looks
                    .sort_by(|left, right| left.0.cmp(&right.0));
                self.state.custom_look_names = self
                    .custom_looks
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect();
                self.state.edit.look = Some(name.clone());
                self.state.edit.look_strength = Some(100.0);
                self.invalidate_profile_render();
                self.state.push_toast(
                    crate::i18n::format("Imported creative LUT {0}", std::slice::from_ref(&name)),
                    false,
                );
            }
            Err(error) => self.state.push_toast(
                crate::i18n::format("Could not import .cube: {0}", std::slice::from_ref(&error)),
                true,
            ),
        }
    }

    pub(super) fn import_icc_dialog(&mut self) {
        if self.icc_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-icc-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(
                        crate::i18n::text("Display or printer ICC profile"),
                        &["icc", "icm"],
                    )
                    .pick_file();
                let _ = tx.send(picked);
            })
            .expect("spawn ICC dialog thread");
        self.icc_pick = Some(rx);
    }

    pub(super) fn finish_icc_import(&mut self, path: &std::path::Path) {
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let result = std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                sp_pipeline::proof::build_custom_proof(name.clone(), &bytes, 33)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(proof) => {
                if let Some(directory) = proof_profiles_dir() {
                    if let Err(error) = std::fs::create_dir_all(&directory).and_then(|()| {
                        std::fs::copy(
                            path,
                            directory.join(
                                path.file_name()
                                    .unwrap_or_else(|| std::ffi::OsStr::new("display.icc")),
                            ),
                        )
                        .map(|_| ())
                    }) {
                        self.state.push_toast(
                            crate::i18n::format(
                                "Could not store ICC profile: {0}",
                                &[error.to_string()],
                            ),
                            true,
                        );
                        return;
                    }
                }
                self.custom_proofs
                    .retain(|candidate| candidate.name != proof.name);
                self.custom_proofs.push(proof);
                self.custom_proofs
                    .sort_by(|left, right| left.name.cmp(&right.name));
                self.state.custom_proof_names = self
                    .custom_proofs
                    .iter()
                    .map(|proof| proof.name.clone())
                    .collect();
                self.state.selected_custom_proof = Some(name.clone());
                self.state.proof_space = sp_core::edit::ProofSpace::Custom;
                self.state.custom_proof_generation =
                    self.state.custom_proof_generation.wrapping_add(1);
                self.state.push_toast(
                    crate::i18n::format("Imported display proof {0}", std::slice::from_ref(&name)),
                    false,
                );
            }
            Err(error) => self.state.push_toast(
                crate::i18n::format("Could not import ICC: {0}", std::slice::from_ref(&error)),
                true,
            ),
        }
    }

    /// Install the ShopPhoto denoise model from its public, checksum-pinned
    /// release (for builds that do not bundle it).
    pub(super) fn start_model_install(&mut self) {
        if self.model_install.is_some() {
            return;
        }
        let Some(destination) = sp_ai::model_install_root() else {
            self.state
                .push_toast("Could not locate the model data directory", true);
            return;
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("sp-model-install".into())
            .spawn(move || {
                let result = sp_ai::distribution::install_shopphoto_model(&destination, |done| {
                    let _ = tx.send(ModelInstallEvent::Progress(done));
                });
                let event = match result {
                    Ok(_) => ModelInstallEvent::Finished("ShopPhoto r4_plain".into()),
                    Err(error) => ModelInstallEvent::Failed(error),
                };
                let _ = tx.send(event);
            })
            .expect("spawn model installer");
        self.model_install = Some(rx);
        self.state.ai_model_install_busy = true;
        self.state.ai_model_install_progress = 0.0;
    }

    pub(super) fn pick_user_preset_import(&mut self) {
        if self.user_preset_import_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-preset-import-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(crate::i18n::text("ShopPhoto presets"), &["json"])
                    .pick_file();
                let _ = tx.send(picked);
            })
            .expect("spawn preset import dialog");
        self.user_preset_import_pick = Some(rx);
    }

    pub(super) fn pick_user_preset_export(&mut self) {
        if self.user_preset_export_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-preset-export-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(crate::i18n::text("ShopPhoto presets"), &["json"])
                    .set_file_name("shopphoto-presets.json")
                    .save_file();
                let _ = tx.send(picked);
            })
            .expect("spawn preset export dialog");
        self.user_preset_export_pick = Some(rx);
    }

    pub(super) fn finish_dcp_import(&mut self, path: &std::path::Path) {
        match sp_raw::dcp::load_dcp(path) {
            Ok(profile) => {
                // Copy into the profile store so it survives restarts.
                if let Some(dir) = profiles_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Some(name) = path.file_name() {
                        let _ = std::fs::copy(path, dir.join(name));
                    }
                }
                let matches = self
                    .state
                    .meta
                    .as_ref()
                    .is_some_and(|m| camera_matches(&profile.camera_model, &m.camera_model));
                self.state.push_toast(
                    if matches {
                        {
                            let __l10n_args = (&(profile.name),);
                            crate::i18n::format(
                                "Imported and selected profile \"{0}\"",
                                std::slice::from_ref(__l10n_args.0),
                            )
                        }
                    } else {
                        {
                            let __l10n_args = (&(profile.name), &(profile.camera_model));
                            crate::i18n::format(
                                "Imported profile \"{0}\" (for {1}; hidden for this camera)",
                                &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                            )
                        }
                    },
                    false,
                );
                if matches {
                    self.state.edit.profile =
                        sp_core::edit::ProfileChoice::Named(profile.name.clone());
                }
                self.profiles.retain(|p| p.name != profile.name);
                self.profiles.push(profile);
                self.refresh_profile_names();
                self.invalidate_profile_render();
            }
            Err(e) => self.state.push_toast(
                crate::i18n::format("DCP import failed: {0}", &[e.to_string()]),
                true,
            ),
        }
    }

    pub(super) fn pick_reference_raws(&mut self) {
        if self.reference_raw_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-reference-raw-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(
                        crate::i18n::text("RAW images"),
                        sp_raw::SUPPORTED_EXTENSIONS,
                    )
                    .pick_files()
                    .unwrap_or_default();
                let _ = tx.send(picked);
            })
            .expect("spawn Reference Match RAW dialog thread");
        self.reference_raw_pick = Some(rx);
    }

    pub(super) fn pick_reference_tiff_folder(&mut self) {
        if self.reference_tiff_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-reference-tiff-dialog".into())
            .spawn(move || {
                let _ = tx.send(rfd::FileDialog::new().pick_folder());
            })
            .expect("spawn Reference Match TIFF dialog thread");
        self.reference_tiff_pick = Some(rx);
    }

    pub(super) fn start_reference_fit(&mut self) {
        if self.reference_fit.is_some() || self.state.reference_fit_busy {
            return;
        }
        let Some(folder) = self.state.reference_tiff_dir.clone() else {
            return;
        };
        let raws = self.state.reference_raws.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-reference-match".into())
            .spawn(move || {
                let _ = tx.send(run_reference_match(raws, folder));
            })
            .expect("spawn reference-match worker");
        self.state.reference_fit_busy = true;
        self.reference_fit = Some(rx);
    }

    pub(super) fn finish_reference_fit(&mut self, result: Result<ReferenceJobResult, String>) {
        self.state.reference_fit_busy = false;
        match result {
            Ok(result) => {
                let save = profiles_dir()
                    .ok_or_else(|| "No profile directory".to_owned())
                    .and_then(|dir| {
                        let safe: String = result
                            .profile
                            .camera_model
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                            .collect();
                        let path = dir.join(format!("ReferenceMatch-{safe}.spprofile"));
                        sp_raw::profile_store::save_profile(&path, &result.profile)
                            .map_err(|e| e.to_string())
                    });
                if let Err(e) = save {
                    self.state.push_toast(
                        crate::i18n::format(
                            "Could not save Reference Match: {0}",
                            std::slice::from_ref(&e),
                        ),
                        true,
                    );
                    return;
                }
                self.profiles.retain(|p| p.name != result.profile.name);
                self.profiles.push(result.profile);
                self.state.edit.profile =
                    sp_core::edit::ProfileChoice::Named(result.report.profile_name.clone());
                let passed = result.report.mean_delta_e_00 < 2.0;
                self.state.push_toast(
                    {
                        let __l10n_args = (
                            &(if passed {
                                "ready"
                            } else {
                                "saved with warning"
                            }),
                            &(result.report.mean_delta_e_00),
                        );
                        crate::i18n::format(
                            "Reference Match {0}: mean ΔE₀₀ {1}",
                            &[__l10n_args.0.to_string(), format!("{:.2}", __l10n_args.1)],
                        )
                    },
                    !passed,
                );
                self.state.reference_report = Some(result.report);
                self.refresh_profile_names();
                self.invalidate_profile_render();
            }
            Err(error) => self.state.push_toast(
                crate::i18n::format("Reference Match failed: {0}", std::slice::from_ref(&error)),
                true,
            ),
        }
    }
}
