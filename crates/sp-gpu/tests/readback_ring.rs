use sp_gpu::{pipelines::make_texture, GpuContext, ReadbackRing};
use std::sync::atomic::AtomicBool;

#[test]
fn queued_copies_survive_texture_reuse_and_trim_unaligned_regions() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter available — skipping readback ring test");
        return;
    };
    for (format, bpp) in [
        (wgpu::TextureFormat::Rgba8Unorm, 4),
        (wgpu::TextureFormat::Rgba16Uint, 8),
    ] {
        let texture = make_texture(
            &ctx,
            "readback test",
            19,
            11,
            format,
            wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        );
        let mut ring = ReadbackRing::default();
        let write = |offset: u8| {
            let bytes: Vec<u8> = (0..19 * 11 * bpp)
                .map(|i| (i as u8).wrapping_add(offset))
                .collect();
            ctx.queue.write_texture(
                texture.as_image_copy(),
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(19 * bpp),
                    rows_per_image: Some(11),
                },
                wgpu::Extent3d {
                    width: 19,
                    height: 11,
                    depth_or_array_layers: 1,
                },
            );
            bytes
        };
        let first = write(3);
        ring.enqueue(&ctx, &texture, [2, 3], [13, 7]).unwrap();
        let second = write(19);
        ring.enqueue(&ctx, &texture, [1, 1], [17, 9]).unwrap();
        assert!(ring.enqueue(&ctx, &texture, [0, 0], [1, 1]).is_err());
        let extract = |bytes: &[u8], x: usize, y: usize, w: usize, h: usize| {
            (y..y + h)
                .flat_map(|row| {
                    bytes[(row * 19 + x) * bpp as usize..(row * 19 + x + w) * bpp as usize]
                        .iter()
                        .copied()
                })
                .collect::<Vec<_>>()
        };
        let cancel = AtomicBool::new(false);
        assert_eq!(
            ring.collect(&ctx, &cancel).unwrap(),
            extract(&first, 2, 3, 13, 7)
        );
        assert_eq!(
            ring.collect(&ctx, &cancel).unwrap(),
            extract(&second, 1, 1, 17, 9)
        );
        assert!(ring.is_empty());
        assert!(ring.enqueue(&ctx, &texture, [18, 0], [2, 1]).is_err());
        ring.enqueue(&ctx, &texture, [0, 0], [19, 11]).unwrap();
        assert!(ring.collect(&ctx, &AtomicBool::new(true)).is_err());
        drop(ring); // cancels pending mapping before a new export
        let mut recovered = ReadbackRing::default();
        recovered.enqueue(&ctx, &texture, [0, 0], [19, 11]).unwrap();
        assert_eq!(recovered.collect(&ctx, &cancel).unwrap(), second);
    }
}

#[test]
fn texture_without_copy_usage_returns_error_without_queuing_work() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let texture = make_texture(
        &ctx,
        "not a readback source",
        19,
        11,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let mut ring = ReadbackRing::default();
    assert!(ring.enqueue(&ctx, &texture, [0, 0], [19, 11]).is_err());
    assert!(ring.is_empty());
}
