//! Two bounded, reusable texture readback slots. Mapping an earlier copy can
//! complete while the GPU renders a later tile on the same queue.
use crate::GpuContext;
use sp_core::SpError;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

struct Pending {
    buffer: wgpu::Buffer,
    receiver: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    width: u32,
    height: u32,
    row_bytes: u32,
    padded_row_bytes: u32,
    submitted: Instant,
}

#[derive(Default)]
pub struct ReadbackRing {
    pending: VecDeque<Pending>,
    recycled: Vec<wgpu::Buffer>,
}

impl ReadbackRing {
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Copy only the requested tile interior, excluding its processing apron.
    pub fn enqueue(
        &mut self,
        ctx: &GpuContext,
        texture: &wgpu::Texture,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<(), SpError> {
        let [width, height] = size;
        let bytes_per_pixel = match texture.format() {
            wgpu::TextureFormat::Rgba8Unorm => 4,
            wgpu::TextureFormat::Rgba16Uint => 8,
            _ => return Err(SpError::Gpu("Unsupported export readback format".into())),
        };
        if texture.dimension() != wgpu::TextureDimension::D2
            || texture.sample_count() != 1
            || !texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || self.pending.len() >= 2
            || width == 0
            || height == 0
            || width > 1024
            || height > 1024
            || u64::from(origin[0]) + u64::from(width) > u64::from(texture.width())
            || u64::from(origin[1]) + u64::from(height) > u64::from(texture.height())
        {
            return Err(SpError::Gpu("Invalid or full export readback ring".into()));
        }
        let row_bytes = width * bytes_per_pixel;
        let padded_row_bytes = row_bytes.div_ceil(256) * 256;
        let bytes = u64::from(padded_row_bytes) * u64::from(height);
        let buffer = if let Some(index) = self
            .recycled
            .iter()
            .position(|buffer| buffer.size() >= bytes)
        {
            self.recycled.swap_remove(index)
        } else {
            // Replace a smaller reusable slot instead of accumulating buffers.
            self.recycled.pop();
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("export readback ring"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("export tile copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        buffer
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.pending.push_back(Pending {
            buffer,
            receiver,
            width,
            height,
            row_bytes,
            padded_row_bytes,
            submitted: Instant::now(),
        });
        Ok(())
    }

    /// Poll without a whole-queue wait, keeping cancellation responsive.
    pub fn collect(&mut self, ctx: &GpuContext, cancel: &AtomicBool) -> Result<Vec<u8>, SpError> {
        let pending = self
            .pending
            .front()
            .ok_or_else(|| SpError::Gpu("No export readback pending".into()))?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(SpError::Export("cancelled".into()));
            }
            if pending.submitted.elapsed() >= Duration::from_secs(30) {
                return Err(SpError::Gpu("Export readback timed out".into()));
            }
            ctx.device
                .poll(wgpu::PollType::Poll)
                .map_err(|e| SpError::Gpu(format!("Export readback poll failed: {e}")))?;
            match pending.receiver.recv_timeout(Duration::from_millis(1)) {
                Ok(result) => {
                    result.map_err(|e| SpError::Gpu(format!("Export readback failed: {e}")))?;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => {
                    return Err(SpError::Gpu(format!(
                        "Export readback callback failed: {error}"
                    )))
                }
            }
        }
        // Presence is established above; no other code mutates the queue.
        let pending = self.pending.pop_front().expect("pending readback");
        let mut pixels = Vec::with_capacity((pending.row_bytes * pending.height) as usize);
        {
            let mapped = pending
                .buffer
                .slice(..u64::from(pending.padded_row_bytes) * u64::from(pending.height))
                .get_mapped_range();
            for y in 0..pending.height {
                let start = (y * pending.padded_row_bytes) as usize;
                pixels.extend_from_slice(&mapped[start..start + pending.row_bytes as usize]);
            }
        }
        debug_assert_eq!(
            pixels.len(),
            (pending.width * pending.height * (pending.row_bytes / pending.width)) as usize
        );
        pending.buffer.unmap();
        self.recycled.push(pending.buffer);
        Ok(pixels)
    }
}

impl Drop for ReadbackRing {
    fn drop(&mut self) {
        for pending in &self.pending {
            pending.buffer.unmap();
        }
    }
}
