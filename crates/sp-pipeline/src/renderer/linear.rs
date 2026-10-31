use super::*;

impl GpuRenderer {
    /// Full-resolution working-space pixels before any display encoding.
    /// Callers performing radiometric merges must supply a neutral edit and
    /// a camera profile without a base curve or look.
    pub fn render_scene_linear(
        &mut self,
        edit: &EditState,
        cancelled: &std::sync::atomic::AtomicBool,
        mut progress: impl FnMut(f32),
    ) -> Result<sp_core::source::RasterImage, SpError> {
        let (width, height) = self
            .active_size()
            .ok_or_else(|| SpError::Export("No source open".into()))?;
        let mut pixels = vec![[0.0f32; 4]; width as usize * height as usize];
        const TILE: u32 = 1024;
        let count = width.div_ceil(TILE) * height.div_ceil(TILE);
        let mut completed = 0;
        for y in (0..height).step_by(TILE as usize) {
            for x in (0..width).step_by(TILE as usize) {
                if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(SpError::Export("Merge cancelled".into()));
                }
                let (w, h) = ((width - x).min(TILE), (height - y).min(TILE));
                let info = self
                    .render_tile_mode_inner(
                        edit,
                        x,
                        y,
                        w,
                        h,
                        (0, false, false, sp_core::color::OutputColorSpace::Rec2020),
                        true,
                    )
                    .ok_or_else(|| SpError::Export("Linear source render failed".into()))?;
                let tile = self
                    .tile
                    .as_ref()
                    .ok_or_else(|| SpError::Export("Linear source tile absent".into()))?;
                let output = tile
                    .scene_output
                    .as_ref()
                    .ok_or_else(|| SpError::Export("Linear scene output absent".into()))?;
                let values = self.read_rgba16_texture(output, info.tex_w, info.tex_h)?;
                super::export::validate_rgba16(&values, true, true)?;
                for row in 0..h {
                    for column in 0..w {
                        let source = (((row + info.margin_y) * info.tex_w + column + info.margin_x)
                            * 4) as usize;
                        pixels[((y + row) * width + x + column) as usize] =
                            [0, 1, 2, 3].map(|c| half::f16::from_bits(values[source + c]).to_f32());
                    }
                }
                completed += 1;
                progress(completed as f32 / count as f32);
            }
        }
        Ok(sp_core::source::RasterImage {
            width,
            height,
            linear_rec2020: pixels,
            bits_per_channel: 16,
            embedded_icc: None,
        })
    }
}
