// Box downsample for building the proxy working image.
//
// Input:  rgba16float at source resolution (linear, any space).
// Output: rgba16float at output resolution (same space).
//
// Averages the axis-aligned source footprint of each output pixel
// (integer box; correct in linear light, no ringing). Used for
// superpixel→proxy reduction. Super3x3/4x4 export uses
// `supersample.wgsl`, which shares this kernel shape.

struct Params {
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let sx0 = gid.x * params.src_w / params.out_w;
    let sx1 = max((gid.x + 1u) * params.src_w / params.out_w, sx0 + 1u);
    let sy0 = gid.y * params.src_h / params.out_h;
    let sy1 = max((gid.y + 1u) * params.src_h / params.out_h, sy0 + 1u);

    var acc = vec3<f32>(0.0);
    var count = 0.0;
    for (var y = sy0; y < sy1; y++) {
        for (var x = sx0; x < sx1; x++) {
            acc += textureLoad(src_tex, vec2<i32>(i32(x), i32(y)), 0).rgb;
            count += 1.0;
        }
    }
    textureStore(
        out_tex,
        vec2<i32>(i32(gid.x), i32(gid.y)),
        vec4<f32>(acc / max(count, 1.0), 1.0),
    );
}
