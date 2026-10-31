//! Persistently mapped staging ring for large CPU→GPU texture uploads
//! (BUILD.md §10.1).
//!
//! On Apple Silicon the CPU and GPU share memory, so the 100 MP CFA upload
//! (~200 MB of `u16`) must not pay for an extra staging hop.
//! [`wgpu::Queue::write_texture`] allocates an internal staging belt and
//! memcpys the whole slice into it, which for a full-sensor Hasselblad frame
//! is a second 200 MB copy plus a 200 MB transient allocation.
//!
//! Instead we keep a small ring of buffers that stay `MAP_WRITE | COPY_SRC`
//! and mapped between uses: rows are written straight into GPU-visible
//! memory and handed to the GPU with `copy_buffer_to_texture`. Chunking keeps
//! the resident staging cost bounded (3 × 32 MB by default) regardless of
//! sensor size, and lets the GPU start consuming the first rows while the CPU
//! is still writing later ones.
//!
//! Buffer→texture copies (unlike `write_texture`) require each row to start
//! on a [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`] boundary, so rows are padded
//! on the way in — see [`StagingRing::upload_texture`].

use crate::context::GpuContext;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Number of buffers in the ring. Three lets the CPU fill slot *n* while the
/// GPU consumes *n-1* and *n-2* is being re-mapped.
const RING_SLOTS: usize = 3;

/// Default per-slot size. A 105 MP CFA frame is ~200 MB, so it uploads in
/// seven chunks with a bounded 96 MB of resident staging memory.
pub const DEFAULT_CHUNK_BYTES: u64 = 32 << 20;

struct Slot {
    buffer: wgpu::Buffer,
    /// Set by the `map_async` callback; `true` means the mapped range is
    /// usable without polling.
    ready: Arc<AtomicBool>,
}

/// A ring of persistently mapped staging buffers.
///
/// Create one per uploader (the renderer owns the image-upload ring) and
/// reuse it: slots are allocated once and recycled, so steady-state uploads
/// perform no buffer allocation.
pub struct StagingRing {
    slots: Vec<Slot>,
    next: usize,
    chunk_bytes: u64,
}

impl StagingRing {
    /// Ring with the default [`DEFAULT_CHUNK_BYTES`] slot size.
    #[must_use]
    pub fn new(ctx: &GpuContext) -> Self {
        Self::with_chunk_bytes(ctx, DEFAULT_CHUNK_BYTES)
    }

    /// Ring with an explicit slot size (tests use small chunks to exercise
    /// the multi-chunk path).
    #[must_use]
    pub fn with_chunk_bytes(ctx: &GpuContext, chunk_bytes: u64) -> Self {
        let align = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let chunk_bytes = chunk_bytes.max(align).next_multiple_of(align);
        Self {
            slots: (0..RING_SLOTS)
                .map(|i| Slot {
                    buffer: create_slot(ctx, chunk_bytes, i),
                    ready: Arc::new(AtomicBool::new(true)),
                })
                .collect(),
            next: 0,
            chunk_bytes,
        }
    }

    /// Current per-slot size in bytes.
    #[must_use]
    pub fn chunk_bytes(&self) -> u64 {
        self.chunk_bytes
    }

    /// Upload tightly packed `data` into `texture`.
    ///
    /// `unpadded_bytes_per_row` is the *source* stride (`width × texel
    /// size`); this function inserts the alignment padding the copy requires.
    /// `data` must hold at least `unpadded_bytes_per_row × height` bytes.
    pub fn upload_texture(
        &mut self,
        ctx: &GpuContext,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        unpadded_bytes_per_row: u32,
        data: &[u8],
    ) -> Result<(), sp_core::SpError> {
        let stride = unpadded_bytes_per_row as usize;
        if data.len() < stride.saturating_mul(height as usize) {
            return Err(sp_core::SpError::Gpu(
                "Upload data is shorter than its declared dimensions".into(),
            ));
        }
        self.upload_texture_rows(
            ctx,
            texture,
            width,
            height,
            unpadded_bytes_per_row,
            |row, mut dst| {
                let start = row as usize * stride;
                dst.copy_from_slice(&data[start..start + stride]);
            },
        )
    }

    /// Upload `height` rows produced on demand by `fill`, which receives the
    /// row index and an exactly `unpadded_bytes_per_row`-long destination
    /// **inside the mapped staging buffer**. Producers that can generate
    /// rows directly (decoders) should use this and skip the intermediate
    /// `Vec` entirely.
    ///
    /// Mapped upload memory can be write-combining, so the destination is a
    /// [`wgpu::WriteOnly`] slice: write it with `copy_from_slice`,
    /// `write_iter` or `fill`, never read it back.
    pub fn upload_texture_rows(
        &mut self,
        ctx: &GpuContext,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        unpadded_bytes_per_row: u32,
        mut fill: impl FnMut(u32, wgpu::WriteOnly<'_, [u8]>),
    ) -> Result<(), sp_core::SpError> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        // Reject malformed layouts before filling rows or issuing a GPU copy:
        // wgpu validation errors otherwise panic with the default error handler.
        let format = texture.format();
        let texel_bytes = format
            .block_copy_size(Some(wgpu::TextureAspect::All))
            .filter(|_| format.block_dimensions() == (1, 1) && !format.is_depth_stencil_format())
            .ok_or_else(|| sp_core::SpError::Gpu("Unsupported staging upload format".into()))?;
        let minimum_stride = width.checked_mul(texel_bytes);
        if texture.dimension() != wgpu::TextureDimension::D2
            || texture.sample_count() != 1
            || !texture.usage().contains(wgpu::TextureUsages::COPY_DST)
            || width > texture.width()
            || height > texture.height()
            || minimum_stride.is_none_or(|minimum| unpadded_bytes_per_row < minimum)
            || !unpadded_bytes_per_row.is_multiple_of(texel_bytes)
        {
            return Err(sp_core::SpError::Gpu(
                "Invalid staging upload layout or texture".into(),
            ));
        }
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded_bytes_per_row
            .checked_next_multiple_of(align)
            .ok_or_else(|| sp_core::SpError::Gpu("Upload row stride overflows".into()))?;
        if u64::from(padded) > ctx.device.limits().max_buffer_size {
            return Err(sp_core::SpError::Gpu(
                "Upload row exceeds the device buffer limit".into(),
            ));
        }

        // A single row must fit in a slot; grow the ring if a future sensor
        // ever exceeds the default chunk (32 MB ≙ a 16 M-pixel-wide row).
        if u64::from(padded) > self.chunk_bytes {
            *self = Self::with_chunk_bytes(ctx, u64::from(padded));
        }
        let rows_per_chunk = (self.chunk_bytes / u64::from(padded)).max(1) as u32;

        let mut row = 0u32;
        while row < height {
            let rows = rows_per_chunk.min(height - row);
            let used = u64::from(padded) * u64::from(rows);
            let slot_index = self.next;
            self.next = (self.next + 1) % RING_SLOTS;

            {
                let slot = &self.slots[slot_index];
                wait_for_map(ctx, slot)?;
                let mut view = slot.buffer.slice(0..used).get_mapped_range_mut();
                for r in 0..rows {
                    let base = r as usize * padded as usize;
                    fill(
                        row + r,
                        view.slice(base..base + unpadded_bytes_per_row as usize),
                    );
                }
            }
            let slot = &self.slots[slot_index];
            slot.buffer.unmap();

            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("staging ring upload"),
                });
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &slot.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(rows),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
            ctx.queue.submit(Some(encoder.finish()));

            // Re-map immediately; `map_async` resolves once the copy above
            // has retired, so the CPU only blocks if it laps the GPU.
            slot.ready.store(false, Ordering::Release);
            let ready = Arc::clone(&slot.ready);
            slot.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Write, move |r| {
                    if r.is_ok() {
                        ready.store(true, Ordering::Release);
                    } else {
                        // Device/buffer teardown can cancel an outstanding
                        // remap. Nothing can consume this slot after its
                        // ring has been dropped, so never panic from the
                        // asynchronous callback.
                        tracing::debug!("staging ring remap cancelled during teardown");
                    }
                });

            row += rows;
        }
        Ok(())
    }
}

fn create_slot(ctx: &GpuContext, size: u64, index: usize) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("staging ring slot {index}")),
        size,
        usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    })
}

fn wait_for_map(ctx: &GpuContext, slot: &Slot) -> Result<(), sp_core::SpError> {
    if !slot.ready.load(Ordering::Acquire) {
        ctx.wait_for_gpu()?;
    }
    if !slot.ready.load(Ordering::Acquire) {
        return Err(sp_core::SpError::Gpu(
            "Staging buffer remap failed or its callback was not delivered".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn chunk_size_is_row_aligned_and_never_zero() {
        let align = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        for requested in [0u64, 1, 255, 256, 257, 32 << 20] {
            let rounded = requested.max(align).next_multiple_of(align);
            assert_eq!(rounded % align, 0);
            assert!(rounded >= align);
        }
    }

    #[test]
    fn row_padding_matches_copy_alignment() {
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        // X2D II: 11904 px × 2 B is already aligned; X1D II: 8280 × 2 is not.
        assert_eq!((11904u32 * 2).div_ceil(align) * align, 23808);
        assert_eq!((8280u32 * 2).div_ceil(align) * align, 16640);
    }
}
