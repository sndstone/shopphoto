// Luminance unsharp mask.
//
// Input/output: rgba16float, display-shaped linear Rec.2020. This pass runs
// after tone/profile-look and before the output transform. Chroma is
// preserved by scaling RGB by the sharpened/original luma ratio.

struct Params {
    // x amount, y radius, z threshold, w halo suppression (UI units).
    sharpen: vec4<f32>,
    restoration: vec4<f32>,
    out_w: u32,
    out_h: u32,
    preview_mask: u32,
    _pad0: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;

const LUMA_2020 = vec3<f32>(0.2627, 0.6780, 0.0593);

fn load_rgb(x: i32, y: i32) -> vec3<f32> {
    let p = vec2<i32>(
        clamp(x, 0, i32(params.out_w) - 1),
        clamp(y, 0, i32(params.out_h) - 1),
    );
    return textureLoad(src_tex, p, 0).rgb;
}

// Binomial PSF K=[1,2,1]/4 in both axes; K²=[1,4,6,4,1]/16.
// Two terms of the damped Neumann inverse: y+a(I-K)y+a²(I-K)²y.
// Finite support (4r+1) keeps proxy/tile/export memory independent of image size.
fn restore_luma(p: vec2<i32>, y: f32) -> f32 {
    if params.restoration.x <= 0.0 { return y; }
    let r = clamp(i32(round(params.restoration.y)), 1, 2);
    let w = array<f32,3>(0.25,0.5,0.25);
    let w2 = array<f32,5>(0.0625,0.25,0.375,0.25,0.0625);
    var ky = 0.0;
    var k2y = 0.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            let v = dot(load_rgb(p.x+i*r,p.y+j*r),LUMA_2020);
            k2y += v*w2[u32(i+2)]*w2[u32(j+2)];
            if abs(i)<=1 && abs(j)<=1 { ky += v*w[u32(i+1)]*w[u32(j+1)]; }
        }
    }
    let regularization = params.restoration.z/100.0;
    let a = params.restoration.x/100.0*mix(0.85,0.45,regularization);
    let high = y-ky;
    let floor = mix(0.0002,0.012,regularization)*max(1.0,abs(y));
    let gate = smoothstep(floor,floor*3.0,abs(high));
    let correction = a*high+a*a*(y-2.0*ky+k2y);
    let limit = mix(0.3,0.08,regularization)*max(0.05,abs(y));
    return max(0.0,y+clamp(correction,-limit,limit)*gate);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let rgb = load_rgb(p.x, p.y);
    let luma = dot(rgb, LUMA_2020);
    let r = max(1, i32(round(params.sharpen.y)));
    var blur = luma * 4.0;
    blur += dot(load_rgb(p.x - r, p.y), LUMA_2020);
    blur += dot(load_rgb(p.x + r, p.y), LUMA_2020);
    blur += dot(load_rgb(p.x, p.y - r), LUMA_2020);
    blur += dot(load_rgb(p.x, p.y + r), LUMA_2020);
    blur += dot(load_rgb(p.x - r, p.y - r), LUMA_2020);
    blur += dot(load_rgb(p.x + r, p.y - r), LUMA_2020);
    blur += dot(load_rgb(p.x - r, p.y + r), LUMA_2020);
    blur += dot(load_rgb(p.x + r, p.y + r), LUMA_2020);
    blur /= 12.0;

    let high = luma - blur;
    let threshold = mix(0.0005, 0.018, params.sharpen.z / 100.0);
    let mask = smoothstep(threshold, threshold * 3.0, abs(high));
    if params.preview_mask == 1u {
        textureStore(out_tex, p, vec4<f32>(vec3<f32>(mask), 1.0));
        return;
    }
    // Bound overshoot increasingly tightly as halo suppression rises.
    let halo_limit = mix(0.20, 0.025, params.sharpen.w / 100.0);
    let detail = clamp(high, -halo_limit, halo_limit);
    let sharpened = max(0.0, restore_luma(p,luma) + detail * (params.sharpen.x / 100.0) * mask);
    let ratio = select(1.0, sharpened / luma, luma > 1e-6);
    textureStore(out_tex, p, vec4<f32>(max(rgb * ratio, vec3<f32>(0.0)), 1.0));
}
