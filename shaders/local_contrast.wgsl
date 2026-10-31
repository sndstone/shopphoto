// Spatial texture, clarity, and dehaze. Operates on perceptual luminance
// while preserving hue by scaling linear Rec.2020 RGB. The Oklab-related
// Rec.2020 luminance constants must match tone_color.wgsl/nr_main.wgsl.

struct Params {
    // x clarity, y texture, z dehaze (-100..100), w airlight (linear).
    local: vec4<f32>,
    out_w: u32,
    out_h: u32,
    working_w: u32,
    _pad0: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;

const LUMA_2020 = vec3<f32>(0.2627, 0.6780, 0.0593);
// Must match tone_color.wgsl and nr_main.wgsl.
const REC2020_TO_REC709 = mat3x3<f32>(
    vec3<f32>(1.660491, -0.124550, -0.018151),
    vec3<f32>(-0.587641, 1.132900, -0.100579),
    vec3<f32>(-0.072850, -0.008349, 1.118730));
const REC709_TO_REC2020 = mat3x3<f32>(
    vec3<f32>(0.627404, 0.069097, 0.016391),
    vec3<f32>(0.329283, 0.919540, 0.088013),
    vec3<f32>(0.043313, 0.011362, 0.895595));
const OK_M1 = mat3x3<f32>(
    vec3<f32>(0.4122214708, 0.2119034982, 0.0883024619),
    vec3<f32>(0.5363325363, 0.6806995451, 0.2817188376),
    vec3<f32>(0.0514459929, 0.1073969566, 0.6299787005));
const OK_M2 = mat3x3<f32>(
    vec3<f32>(0.2104542553, 1.9779984951, 0.0259040371),
    vec3<f32>(0.7936177850, -2.4285922050, 0.7827717662),
    vec3<f32>(-0.0040720468, 0.4505937099, -0.8086757660));
const OK_M2_INV = mat3x3<f32>(
    vec3<f32>(1.0, 1.0, 1.0),
    vec3<f32>(0.3963377774, -0.1055613458, -0.0894841775),
    vec3<f32>(0.2158037573, -0.0638541728, -1.2914855480));
const OK_M1_INV = mat3x3<f32>(
    vec3<f32>(4.0767416621, -1.2684380046, -0.0041960863),
    vec3<f32>(-3.3077115913, 2.6097574011, -0.7034186147),
    vec3<f32>(0.2309699292, -0.3413193965, 1.7076147010));

fn load_rgb(x: i32, y: i32) -> vec3<f32> {
    return textureLoad(src_tex, vec2<i32>(
        clamp(x, 0, i32(params.out_w) - 1),
        clamp(y, 0, i32(params.out_h) - 1)), 0).rgb;
}

fn oklab_of(rgb: vec3<f32>) -> vec3<f32> {
    let linear709 = max(REC2020_TO_REC709 * rgb, vec3<f32>(0.0));
    let lms = OK_M1 * linear709;
    return OK_M2 * pow(max(lms, vec3<f32>(0.0)), vec3<f32>(1.0 / 3.0));
}

fn rgb_of(lab: vec3<f32>) -> vec3<f32> {
    let lms_c = OK_M2_INV * lab;
    let linear709 = OK_M1_INV * (lms_c * lms_c * lms_c);
    return max(REC709_TO_REC2020 * linear709, vec3<f32>(0.0));
}

fn cross_blur(x: i32, y: i32, radius: i32) -> f32 {
    var value = oklab_of(load_rgb(x, y)).x * 4.0;
    value += oklab_of(load_rgb(x - radius, y)).x;
    value += oklab_of(load_rgb(x + radius, y)).x;
    value += oklab_of(load_rgb(x, y - radius)).x;
    value += oklab_of(load_rgb(x, y + radius)).x;
    value += oklab_of(load_rgb(x - radius, y - radius)).x;
    value += oklab_of(load_rgb(x + radius, y - radius)).x;
    value += oklab_of(load_rgb(x - radius, y + radius)).x;
    value += oklab_of(load_rgb(x + radius, y + radius)).x;
    return value / 12.0;
}

fn srgb_oetf(x: f32) -> f32 {
    if x <= 0.0031308 { return 12.92 * max(x, 0.0); }
    return 1.055 * pow(max(x, 0.0), 1.0 / 2.4) - 0.055;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h { return; }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let rgb = load_rgb(x, y);
    let center_lab = oklab_of(rgb);
    let l = center_lab.x;
    let small = cross_blur(x, y, 2);
    let large_radius = max(4, min(32, i32(round(f32(params.working_w) / 40.0))));
    let large = cross_blur(x, y, large_radius);

    let texture_high = l - small;
    let edge = abs(oklab_of(load_rgb(x + 1, y)).x - oklab_of(load_rgb(x - 1, y)).x)
        + abs(oklab_of(load_rgb(x, y + 1)).x - oklab_of(load_rgb(x, y - 1)).x);
    let texture_mask = 1.0 - smoothstep(0.035, 0.12, edge);
    var adjusted_l = l + texture_high * params.local.y / 100.0 * 0.6 * texture_mask;

    let encoded = srgb_oetf(clamp(l * l * l, 0.0, 1.0));
    let midtone = 1.0 - pow(abs(2.0 * encoded - 1.0), 1.5);
    adjusted_l += (l - large) * params.local.x / 100.0 * 0.75 * midtone;
    adjusted_l = clamp(adjusted_l, 0.0, pow(1.1, 1.0 / 3.0));

    var out_rgb = rgb_of(vec3<f32>(adjusted_l, center_lab.yz));
    var linear_luma = max(dot(out_rgb, LUMA_2020), 0.0);
    let dehaze = params.local.z / 100.0 * 0.35;
    if abs(dehaze) > 1e-6 {
        let denominator = max(1.0 - dehaze, 0.2);
        linear_luma = max((linear_luma - params.local.w * dehaze) / denominator, 0.0);
    }
    let original_luma = max(dot(out_rgb, LUMA_2020), 1e-6);
    let ratio = clamp(linear_luma / original_luma, 0.1, 8.0);
    out_rgb *= ratio;
    textureStore(out_tex, vec2<i32>(x, y), vec4<f32>(max(out_rgb, vec3<f32>(0.0)), 1.0));
}
