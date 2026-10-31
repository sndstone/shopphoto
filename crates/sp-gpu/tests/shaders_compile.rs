//! CI shader gate (BUILD.md §4): every registered WGSL file must compile on
//! a headless adapter, and all pipelines must build. Skips cleanly on
//! machines with no GPU.
//!
//! BUILD.md §10.2 additionally requires BOTH precision variants of every
//! shader to stay valid, including on machines whose adapter lacks
//! `SHADER_F16`. That check runs through naga directly (no device needed),
//! so an f16-only typo cannot reach a Mac unnoticed from CI runners with
//! software adapters, or vice versa.

use sp_gpu::pipelines::{shader_with_precision, Pipelines, PRECISION_MARKER, SHADER_SOURCES};
use sp_gpu::GpuContext;

fn shader_source(name: &str) -> &'static str {
    SHADER_SOURCES
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, source)| *source)
        .expect("registered shader")
}

fn wgsl_function<'a>(source: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}(");
    let start = source.find(&needle).expect("WGSL function exists");
    let open = source[start..].find('{').expect("function body") + start;
    let mut depth = 0_i32;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated WGSL function {name}")
}

fn validate(name: &str, source: &str, f16: bool) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("shader {name} (f16={f16}) failed to parse: {e:?}"));
    let mut caps = naga::valid::Capabilities::empty();
    if f16 {
        caps |= naga::valid::Capabilities::SHADER_FLOAT16;
    }
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), caps)
        .validate(&module)
        .unwrap_or_else(|e| panic!("shader {name} (f16={f16}) failed validation: {e:?}"));
}

#[test]
fn both_precision_variants_validate() {
    let mut saw_marker = false;
    for (name, source) in SHADER_SOURCES {
        if source.contains(PRECISION_MARKER) {
            saw_marker = true;
        }
        for f16 in [false, true] {
            validate(name, &shader_with_precision(source, f16), f16);
        }
    }
    assert!(
        saw_marker,
        "no shader opts into the precision prelude — §10.2 f16 path is dead"
    );
}

#[test]
fn all_shaders_compile_and_pipelines_build() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter available — skipping shader compile test");
        return;
    };
    eprintln!("adapter SHADER_F16 support: {}", ctx.supports_f16());

    // Compile every registered shader module individually first, so a
    // failure names the file...
    for (name, source) in SHADER_SOURCES {
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let source = shader_with_precision(source, ctx.supports_f16()).into_owned();
        let _module = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "shader {name} failed validation: {err:?}");
    }

    // ...then the full pipeline set (checks bind-group layout compatibility).
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _pipelines = Pipelines::new(&ctx).expect("pipelines");
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "pipeline build failed validation: {err:?}");
}

#[test]
fn oklab_constants_match_across_color_shaders() {
    let source = |name: &str| {
        SHADER_SOURCES
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, source)| *source)
            .expect("registered shader")
    };
    let tone = source("tone_color.wgsl");
    let local = source("local_contrast.wgsl");
    let nr = source("nr_main.wgsl");
    for constant in ["0.4122214708", "0.7936177850", "4.0767416621"] {
        assert!(tone.contains(constant));
        assert!(local.contains(constant));
        assert!(nr.contains(constant));
    }
}

#[test]
fn chroma_warp_luma_matches_sp_core() {
    // The chroma warp divides by this luma on the GPU and the Reference Match
    // fitter divides by `sp_core::color::REC2020_LUMA` on the CPU. If they
    // drift, fitted warps land on the wrong grid nodes and every Tier-3
    // profile is subtly wrong — with nothing visibly broken to notice.
    let source = SHADER_SOURCES
        .iter()
        .find(|(name, _)| *name == "wb_color.wgsl")
        .map(|(_, source)| *source)
        .expect("registered shader");
    for value in sp_core::color::REC2020_LUMA {
        // WGSL literals carry six decimals; compare on that text.
        let literal = format!("{value:.6}");
        assert!(
            source.contains(&literal),
            "wb_color.wgsl is missing luma coefficient {literal}"
        );
    }
}

#[test]
fn lens_shading_lookup_is_identical_in_both_raw_shaders() {
    // `raw_prepare.wgsl` (full-resolution path) and
    // `demosaic_superpixel.wgsl` (proxy and Super2x2) each carry a copy of
    // `lens_gain`, because WGSL has no includes. A correction that differed
    // between them would make the proxy disagree with the export.
    let source = |name: &str| {
        SHADER_SOURCES
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, source)| *source)
            .expect("registered shader")
    };
    let extract = |src: &str| {
        let start = src
            .find("fn lens_gain(")
            .expect("lens_gain present in both raw shaders");
        let body = &src[start..];
        let end = body.find("\n}").expect("function terminates") + 2;
        body[..end].to_owned()
    };
    assert_eq!(
        extract(source("raw_prepare.wgsl")),
        extract(source("demosaic_superpixel.wgsl")),
        "the two copies of lens_gain have diverged"
    );
}

#[test]
fn eight_and_sixteen_bit_output_helpers_cannot_drift() {
    let output8 = shader_source("output_transform.wgsl");
    let output16 = shader_source("output_transform_16.wgsl");
    for function in ["srgb_oetf", "pq_oetf", "hdr_to_sdr", "tone_map_hdr"] {
        assert_eq!(
            wgsl_function(output8, function),
            wgsl_function(output16, function),
            "{function} differs between 8-bit and 16-bit output"
        );
    }
    assert_eq!(
        wgsl_function(output8, "tone_map_hdr"),
        wgsl_function(shader_source("output_scrgb.wgsl"), "tone_map_hdr"),
        "EDR display shoulder differs from export shoulder"
    );
}

#[test]
fn every_hdr_output_has_the_same_safety_guards() {
    for name in [
        "output_transform.wgsl",
        "output_transform_16.wgsl",
        "output_scrgb.wgsl",
    ] {
        let source = shader_source(name);
        let shoulder = wgsl_function(source, "tone_map_hdr");
        assert!(shoulder.contains("clamp(params.hdr.x, 0.0, 4.0)"), "{name}");
        assert!(shoulder.contains("if ceiling <= 1.001"), "{name}");
        assert!(!shoulder.contains("mapped_luma / max(luma"), "{name}");
        assert!(
            shoulder.contains("var out_rgb = rgb * select(1.0, mapped_luma / luma, luma > knee)"),
            "{name} must apply one luminance-derived shoulder scale to RGB"
        );
        assert!(
            shoulder.contains("out_rgb *= min(1.0, ceiling / max(peak, 1e-6))"),
            "{name} must guard gamut overshoot with one hue-preserving peak scale"
        );
        assert!(
            source.contains("select(0.0, params.hdr.z, params.mode != 0u)"),
            "{name} applies HDR reference white while HDR is disabled"
        );
    }
}

#[test]
fn scrgb_shapes_rec2020_before_converting_primaries() {
    let source = shader_source("output_scrgb.wgsl");
    let shoulder = wgsl_function(source, "tone_map_hdr");
    assert!(
        shoulder.contains("vec3<f32>(0.2627, 0.6780, 0.0593)"),
        "scRGB shoulder must measure Rec.2020 luminance"
    );
    let shoulder_call = source
        .find("let mapped_rec2020 = tone_map_hdr(rec2020)")
        .expect("Rec.2020 is shoulder-mapped");
    let gamut_call = source
        .find("gamut_matrix() * mapped_rec2020")
        .expect("mapped Rec.2020 is converted to scRGB primaries");
    assert!(shoulder_call < gamut_call);
}

#[test]
fn scrgb_sdr_matches_component_clamped_output() {
    let source = SHADER_SOURCES
        .iter()
        .find(|(name, _)| *name == "output_scrgb.wgsl")
        .map(|(_, source)| *source)
        .expect("registered shader");
    assert!(source.contains("if params.mode == 0u"));
    assert!(source.contains("scrgb = min(scrgb, vec3<f32>(1.0))"));
}

#[test]
fn tone_lut_shaper_guards_negative_matrix_values() {
    let source = shader_source("tone_color.wgsl");
    let lut_apply = wgsl_function(source, "lut_apply");
    assert!(lut_apply.contains("let safe_x = max(x, 0.0)"));
    assert!(lut_apply.contains("safe_x / (safe_x + 1.0)"));
}
