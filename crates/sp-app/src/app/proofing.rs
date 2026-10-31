//! Rebuild printer-proof LUTs off the UI thread when view options change.
use super::*;

impl ShopPhotoApp {
    pub(super) fn sync_proof_options(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.proof_job.as_ref().and_then(|job| job.try_recv().ok()) {
            self.proof_job = None;
            self.state.proof_busy = false;
            match result {
                Ok(proof) => {
                    if let Some(current) = self
                        .custom_proofs
                        .iter_mut()
                        .find(|p| p.name == proof.name && p.icc == proof.icc)
                    {
                        *current = proof;
                        self.state.custom_proof_generation =
                            self.state.custom_proof_generation.wrapping_add(1);
                    }
                }
                Err(error) => {
                    if let Some(current) = self.custom_proof_for_view() {
                        self.state.proof_options = current.options;
                    }
                    self.state.push_toast(
                        crate::i18n::format(
                            "Could not apply proof options: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    );
                }
            }
        }
        if self.proof_job.is_some() || self.state.proof_space != sp_core::edit::ProofSpace::Custom {
            return;
        }
        let Some(proof) = self
            .custom_proof_for_view()
            .filter(|p| p.options != self.state.proof_options)
        else {
            return;
        };
        let options = self.state.proof_options;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let context = ctx.clone();
        std::thread::spawn(move || {
            let result = sp_pipeline::proof::build_custom_proof_with_options(
                proof.name, &proof.icc, 33, options,
            )
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
            context.request_repaint();
        });
        self.proof_job = Some(rx);
        self.state.proof_busy = true;
    }
}
