//! Small source-region readbacks for photographic calibration pickers.
use super::*;
impl GpuRenderer {
    /// Read exact selected coverage and the scene immediately before this local
    /// adjustment. Both use canonical image coordinates, independent of crop.
    pub fn mask_refinement_input(
        &mut self,
        edit: &EditState,
        index: usize,
    ) -> Result<(sp_core::mask::AlphaMask, sp_core::source::RasterImage), SpError> {
        let adjustment = edit
            .local_adjustments
            .get(index)
            .ok_or_else(|| SpError::Gpu("Select a mask first".into()))?;
        let mut source = edit.clone();
        source.local_adjustments.truncate(index + 1);
        source.local_adjustments[index] = sp_core::edit::LocalAdjustment {
            mask: adjustment.coverage_mask(),
            ..Default::default()
        };
        source.sharpen = Default::default();
        let enabled = self.analysis_enabled;
        let edge = self.analysis_long_edge;
        let preview = self.mask_preview_index;
        self.analysis_enabled = true;
        self.analysis_long_edge = 1536;
        self.mask_preview_index = Some(index);
        self.render_without_histogram(&source);
        let result = (|| {
            Ok((
                self.read_mask_preview()?
                    .ok_or_else(|| SpError::Gpu("Mask coverage unavailable".into()))?,
                self.read_analysis_frame()?
                    .ok_or_else(|| SpError::Gpu("Guidance unavailable".into()))?,
            ))
        })();
        self.analysis_enabled = enabled;
        self.analysis_long_edge = edge;
        self.mask_preview_index = preview;
        self.render_without_histogram(edit);
        result
    }

    /// Bounded pre-retouch scene for donor/dust review. The displayed edit and
    /// optional scope/coverage textures are restored before returning.
    pub fn retouch_analysis_frame(
        &mut self,
        edit: &EditState,
    ) -> Result<sp_core::source::RasterImage, SpError> {
        if self.image.is_none() {
            return Err(SpError::Gpu("No image is open".into()));
        }
        let enabled = self.analysis_enabled;
        let edge = self.analysis_long_edge;
        let mut source = edit.clone();
        source.spots.clear();
        source.local_adjustments.clear();
        source.sharpen = Default::default();
        self.analysis_enabled = true;
        self.analysis_long_edge = 1024;
        self.render_without_histogram(&source);
        let result = self
            .read_analysis_frame()
            .and_then(|v| v.ok_or_else(|| SpError::Gpu("No scene analysis is available".into())));
        self.analysis_enabled = enabled;
        self.analysis_long_edge = edge;
        self.render_without_histogram(edit);
        result
    }

    /// Average a 7x7 source patch before film inversion/WB/color development.
    /// Coordinates belong to the canonical pre-lens active-image frame.
    pub fn sample_film_base(
        &self,
        uv: [f32; 2],
    ) -> Result<sp_core::correction::LinearSourceRgb, SpError> {
        let img = self
            .image
            .as_ref()
            .ok_or_else(|| SpError::Gpu("No image is open".into()))?;
        let x = (uv[0].clamp(0.0, 1.0) * img.proxy_w.saturating_sub(1) as f32).round() as u32;
        let y = (uv[1].clamp(0.0, 1.0) * img.proxy_h.saturating_sub(1) as f32).round() as u32;
        let left = x.saturating_sub(3);
        let top = y.saturating_sub(3);
        let width = (x + 4).min(img.proxy_w) - left;
        let height = (y + 4).min(img.proxy_h) - top;
        let buffer = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("film base sample"),
            size: 256 * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.ctx.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &img.proxy_src_tex,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: left,
                    y: top,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        self.ctx.map_read_buffer(&buffer)?;
        let mapped = slice.get_mapped_range();
        let mut sum = [0.0; 3];
        for row in 0..height as usize {
            for column in 0..width as usize {
                for (channel, total) in sum.iter_mut().enumerate() {
                    let offset = row * 256 + column * 8 + channel * 2;
                    *total += half::f16::from_bits(u16::from_le_bytes([
                        mapped[offset],
                        mapped[offset + 1],
                    ]))
                    .to_f32();
                }
            }
        }
        drop(mapped);
        buffer.unmap();
        let sample = sum.map(|v| v / (width * height) as f32);
        if sample.iter().any(|v| !v.is_finite() || *v < 0.0001) {
            return Err(SpError::Gpu(
                "Choose an evenly lit clear film border".into(),
            ));
        }
        Ok(sp_core::correction::LinearSourceRgb(sample))
    }
}
