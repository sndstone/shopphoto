//! The staging ring must deliver bit-exact pixels for row strides that are
//! NOT 256-byte aligned (X1D II: 8280 px × 2 B) and for uploads that span
//! several ring slots. Skips cleanly with no GPU adapter.

use sp_gpu::pipelines::make_texture;
use sp_gpu::{GpuContext, StagingRing};

fn readback_r16(ctx: &GpuContext, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u16> {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = (w * 2).div_ceil(align) * align;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit(Some(encoder.finish()));
    buf.slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let view = buf.slice(..).get_mapped_range();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        let row = &view[y * padded as usize..y * padded as usize + (w * 2) as usize];
        out.extend(
            row.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]])),
        );
    }
    drop(view);
    buf.unmap();
    out
}

fn expected(w: u32, h: u32) -> Vec<u16> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| ((y * 7919 + x * 31) % 65535) as u16))
        .collect()
}

#[test]
fn uploads_unaligned_rows_across_multiple_slots() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter available — skipping staging ring test");
        return;
    };
    // 301 × 2 = 602 bytes per row: not a multiple of 256, so every row needs
    // padding. A 4 KB chunk holds 5 padded rows → 26 chunks over 3 slots,
    // which forces the re-map wait path.
    let (w, h) = (301u32, 128u32);
    let tex = make_texture(
        &ctx,
        "staging test",
        w,
        h,
        wgpu::TextureFormat::R16Uint,
        wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
    );
    let data = expected(w, h);
    let mut ring = StagingRing::with_chunk_bytes(&ctx, 4096);
    ring.upload_texture(&ctx, &tex, w, h, w * 2, bytemuck::cast_slice(&data))
        .unwrap();
    assert_eq!(readback_r16(&ctx, &tex, w, h), data);
}

#[test]
fn reuses_slots_across_uploads_and_matches_write_texture() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter available — skipping staging ring test");
        return;
    };
    let (w, h) = (64u32, 64u32);
    let usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
    let via_ring = make_texture(&ctx, "ring", w, h, wgpu::TextureFormat::R16Uint, usage);
    let via_queue = make_texture(&ctx, "queue", w, h, wgpu::TextureFormat::R16Uint, usage);
    let data = expected(w, h);

    let mut ring = StagingRing::new(&ctx);
    // Several uploads in a row exercise slot recycling.
    for _ in 0..4 {
        ring.upload_texture(&ctx, &via_ring, w, h, w * 2, bytemuck::cast_slice(&data))
            .unwrap();
    }
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &via_queue,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 2),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    assert_eq!(
        readback_r16(&ctx, &via_ring, w, h),
        readback_r16(&ctx, &via_queue, w, h)
    );
}

#[test]
fn row_callback_writes_directly_into_mapped_memory() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter available — skipping staging ring test");
        return;
    };
    let (w, h) = (100u32, 40u32);
    let tex = make_texture(
        &ctx,
        "rows",
        w,
        h,
        wgpu::TextureFormat::R16Uint,
        wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
    );
    let mut ring = StagingRing::with_chunk_bytes(&ctx, 2048);
    ring.upload_texture_rows(&ctx, &tex, w, h, w * 2, |y, dst| {
        // Written straight into mapped (write-only) memory, no scratch row.
        dst.write_iter(
            (0..w).flat_map(move |x| (((y * 7919 + x * 31) % 65535) as u16).to_le_bytes()),
        );
    })
    .unwrap();
    assert_eq!(readback_r16(&ctx, &tex, w, h), expected(w, h));
}

#[test]
fn invalid_layouts_return_errors_before_filling_rows_and_keep_ring_usable() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let texture = make_texture(
        &ctx,
        "validated upload",
        19,
        11,
        wgpu::TextureFormat::R16Uint,
        wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
    );
    let mut ring = StagingRing::with_chunk_bytes(&ctx, 4096);
    for (width, height, stride) in [
        (20, 11, 40),           // copy extends beyond texture
        (19, 12, 38),           // too many rows
        (19, 11, 36),           // row cannot contain all texels
        (19, 11, 39),           // stride splits a texel
        (19, 11, u32::MAX - 1), // alignment overflows
    ] {
        assert!(ring
            .upload_texture_rows(&ctx, &texture, width, height, stride, |_, _| {
                panic!("invalid upload must not call the row producer");
            })
            .is_err());
    }
    let no_copy = make_texture(
        &ctx,
        "not an upload destination",
        19,
        11,
        wgpu::TextureFormat::R16Uint,
        wgpu::TextureUsages::TEXTURE_BINDING,
    );
    assert!(ring
        .upload_texture_rows(&ctx, &no_copy, 19, 11, 38, |_, _| {
            panic!("invalid texture must not call the row producer");
        })
        .is_err());
    let data = expected(19, 11);
    ring.upload_texture(&ctx, &texture, 19, 11, 38, bytemuck::cast_slice(&data))
        .unwrap();
    assert_eq!(readback_r16(&ctx, &texture, 19, 11), data);
}
