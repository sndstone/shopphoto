//! Preset thumbnails: the open photo rendered through each built-in preset,
//! one per idle frame, for the presets rail.

use super::*;

/// Long edge of a preset thumbnail in pixels (2× the rail tile).
const THUMB_EDGE: u32 = 240;

/// Box-downscale an RGBA8 image so its long edge is at most `edge`.
fn downscale_rgba(rgba: &[u8], width: u32, height: u32, edge: u32) -> (Vec<u8>, u32, u32) {
    let step = (width.max(height) / edge).max(1);
    let (tw, th) = ((width / step).max(1), (height / step).max(1));
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    let n = step * step;
    for ty in 0..th {
        for tx in 0..tw {
            let mut sum = [0u32; 4];
            for dy in 0..step {
                let row = ((ty * step + dy) * width + tx * step) as usize * 4;
                for dx in 0..step as usize {
                    for (c, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(rgba[row + dx * 4 + c]);
                    }
                }
            }
            out.extend(sum.map(|total| (total / n) as u8));
        }
    }
    (out, tw, th)
}

impl ShopPhotoApp {
    /// The inputs a preset thumbnail depends on: presets replace the rest of
    /// the edit, so only the fields they preserve (and the photo) matter.
    fn preset_thumb_key(&self) -> Option<String> {
        let path = self.state.open_image.as_ref()?;
        let base = self.state.builtin_preset_base();
        let preserved = (
            &base.profile,
            &base.wb,
            base.tone.exposure_ev,
            base.tone.brightness,
            base.tone.whites,
            base.tone.blacks,
            &base.nr,
            &base.lens,
            &base.hdr,
        );
        Some(format!(
            "{}|{}",
            path.display(),
            serde_json::to_string(&preserved).unwrap_or_default()
        ))
    }

    /// Render at most one missing preset thumbnail per frame while the
    /// photographer is idle. The develop output is re-rendered in the same
    /// frame afterwards, so the viewport never shows the preset render.
    pub(super) fn update_preset_thumbnails(&mut self, ctx: &egui::Context) {
        let visible = self.state.mode == Mode::Develop
            && self.state.left_rail_tab == crate::state::LeftRailTab::Presets
            && !self.state.rails_hidden
            && self.state.culling.is_none();
        let Some(key) = self.preset_thumb_key() else {
            return;
        };
        if key != self.preset_thumbs_key {
            // Keep stale thumbnails on screen until replaced; only the key
            // changes, so tiles do not flash empty after every edit.
            self.preset_thumbs_key = key;
            self.preset_thumbs_done = vec![false; self.preset_thumbs.len()];
            ctx.request_repaint_after(std::time::Duration::from_millis(900));
            return;
        }
        let idle = self
            .state
            .last_edit_change
            .is_none_or(|t| t.elapsed().as_secs_f32() > 0.8);
        let busy = self.state.decode_in_flight
            || self.state.ai_denoise_busy
            || self.state.auto_in_flight
            || self.state.export_busy
            || self.state.enhancement_busy
            || self.state.preset_preview.is_some()
            || ctx.input(|i| i.pointer.any_down());
        if !visible || !idle || busy {
            if visible && !idle {
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }
            return;
        }
        let Some(index) = self.preset_thumbs_done.iter().position(|done| !done) else {
            return;
        };
        let preset = sp_core::preset::DevelopPreset::ALL[index];
        let mut edit = self.state.builtin_preset_base();
        preset.apply(&mut edit);
        let profile = self.profile_for(&edit);
        let look = self.custom_look_for(&edit);
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        if gpu.tex_id.is_none() {
            return;
        }
        gpu.renderer.set_profile(profile);
        gpu.renderer.set_custom_look(look);
        gpu.renderer.render_without_histogram(&edit);
        let pixels = gpu.renderer.read_display_srgb8();
        // Force the develop render back before this frame paints.
        gpu.last_rendered = None;
        self.preset_thumbs_done[index] = true;
        match pixels {
            Ok((rgba, width, height)) => {
                let (small, tw, th) = downscale_rgba(&rgba, width, height, THUMB_EDGE);
                self.preset_thumbs[index] = Some(ctx.load_texture(
                    format!("preset-thumb-{index}"),
                    egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &small),
                    egui::TextureOptions::LINEAR,
                ));
            }
            Err(error) => tracing::warn!("preset thumbnail failed: {error}"),
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::downscale_rgba;

    #[test]
    fn downscale_averages_blocks_and_keeps_aspect() {
        let (w, h) = (8, 4);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [if i % 2 == 0 { 0 } else { 200 }, 10, 20, 255])
            .collect();
        let (out, tw, th) = downscale_rgba(&rgba, w, h, 4);
        assert_eq!((tw, th), (4, 2));
        assert_eq!(&out[..4], &[100, 10, 20, 255]);
    }
}
