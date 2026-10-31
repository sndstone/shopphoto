//! Rendered RGB sources use the same develop/export stages after input
//! conversion, but never pass through RAW sensor preparation/demosaicing.

use super::*;
use sp_core::source::{ImageSource, RasterImage};
use std::sync::Arc;

impl GpuRenderer {
    pub fn open_source_with_edit(
        &mut self,
        source: &ImageSource,
        edit: &EditState,
    ) -> Result<(), SpError> {
        match source {
            ImageSource::Raw(raw) => self.open_image_with_edit(raw, edit),
            ImageSource::Raster(image) => self.open_raster(image.clone(), edit),
        }
    }

    pub fn open_raster(
        &mut self,
        image: Arc<RasterImage>,
        edit: &EditState,
    ) -> Result<(), SpError> {
        if image.width == 0
            || image.height == 0
            || image.linear_rec2020.len() != image.width as usize * image.height as usize
            || image
                .linear_rec2020
                .iter()
                .any(|p| p[3] != 1.0 || p.iter().any(|v| !v.is_finite()))
        {
            return Err(SpError::decode(
                "raster",
                "Invalid opaque linear Rec.2020 pixel buffer",
            ));
        }
        self.preview_base = None;
        self.image = None;
        self.tile = None;
        self.profile = None;
        self.lens = None;
        self.external_raw_denoise = false;
        let ctx = &self.ctx;
        let scale = (self.proxy_long_edge as f32 / image.width.max(image.height) as f32).min(1.0);
        let pw = (image.width as f32 * scale).round().max(1.0) as u32;
        let ph = (image.height as f32 * scale).round().max(1.0) as u32;
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let float_tex = |name, extra| {
            make_texture(
                ctx,
                name,
                pw,
                ph,
                wgpu::TextureFormat::Rgba16Float,
                usage | extra,
            )
        };
        let proxy = float_tex(
            "raster proxy",
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        );
        let mut pixels = Vec::with_capacity(pw as usize * ph as usize);
        // Area sampling averages in linear working RGB, preserving neutral
        // color and avoiding a lossy 8-bit thumbnail as the editing source.
        for y in 0..ph {
            let y0 = y as u64 * image.height as u64 / ph as u64;
            let y1 = ((y + 1) as u64 * image.height as u64 / ph as u64).max(y0 + 1);
            for x in 0..pw {
                let x0 = x as u64 * image.width as u64 / pw as u64;
                let x1 = ((x + 1) as u64 * image.width as u64 / pw as u64).max(x0 + 1);
                let mut sum = [0.0; 4];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let p = image.linear_rec2020[(sy * image.width as u64 + sx) as usize];
                        for c in 0..4 {
                            sum[c] += p[c];
                        }
                    }
                }
                let count = ((x1 - x0) * (y1 - y0)) as f32;
                pixels.push(sum.map(|v| half::f16::from_f32(v / count)));
            }
        }
        self.staging
            .upload_texture(ctx, &proxy, pw, ph, pw * 8, bytemuck::cast_slice(&pixels))?;
        let out_tex = make_texture(
            ctx,
            "raster out",
            pw,
            ph,
            wgpu::TextureFormat::Rgba8Unorm,
            usage | wgpu::TextureUsages::COPY_SRC,
        );
        let hdr = float_tex("raster HDR", wgpu::TextureUsages::COPY_SRC);
        // RAW bind groups are constructed uniformly but never dispatched for
        // a raster source. This unused binding contains no fabricated CFA.
        let unused_cfa = make_texture(
            ctx,
            "unused RAW binding",
            1,
            1,
            wgpu::TextureFormat::R16Uint,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let hist_bins = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist bins"),
            size: (HISTOGRAM_BINS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hist_staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist staging"),
            size: (HISTOGRAM_BINS * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.image = Some(LoadedImage {
            resident_texture_bytes: u64::from(pw) * u64::from(ph) * 44 + 2,
            raster: Some(image.clone()),
            proxy_src_view: proxy.create_view(&Default::default()),
            proxy_src_tex: proxy.clone(),
            work_view: float_tex("raster work", wgpu::TextureUsages::COPY_SRC)
                .create_view(&Default::default()),
            work2_view: float_tex("raster work2", wgpu::TextureUsages::COPY_SRC)
                .create_view(&Default::default()),
            work_nr_view: float_tex("raster NR", wgpu::TextureUsages::COPY_SRC)
                .create_view(&Default::default()),
            hdr_display_view: hdr.create_view(&Default::default()),
            hdr_display_tex: hdr,
            out_view: out_tex.create_view(&Default::default()),
            out_tex,
            proxy_w: pw,
            proxy_h: ph,
            cam_to_xyz: sp_core::color::REC2020_TO_XYZ,
            wb_coeffs: [1.0; 4],
            cfa_view: unused_cfa.create_view(&Default::default()),
            cfa_pattern: CfaPattern::Rggb,
            black_levels: [0.0; 4],
            white_levels: [1.0; 4],
            active: sp_core::raw::ActiveArea {
                x: 0,
                y: 0,
                width: image.width,
                height: image.height,
            },
            hist_bins,
            hist_staging,
            hist_rx: None,
            airlight: 0.95,
            quarter_view: proxy.create_view(&Default::default()),
            quarter_w: pw,
            quarter_h: ph,
            lens_bake: self.lens_bake(edit),
        });
        Ok(())
    }
}
