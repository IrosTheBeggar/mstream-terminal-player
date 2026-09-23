//! The compile matrix: every vendored preset, every pass, every backend.
//!
//! A pass that naga cannot compile is a preset that shows nothing, on some
//! machine nobody here is sitting at. This finds that out on every run of
//! the tests, for all four targets wgpu translates to, without a GPU:
//! naga's frontend and validator, then the Metal, HLSL, SPIR-V and GLSL
//! writers, each behind `catch_unwind` because a writer can panic where it
//! should have returned an error — one does, below.
//!
//! The canaries at the bottom pin the naga behaviours the translation in
//! `glsl.rs` exists to work around. When a naga upgrade fixes one, its
//! canary fails, and the workaround it names can go.

use naga::ShaderStage;
use naga::front::glsl::{Frontend, Options};
use naga::valid::{Capabilities, ModuleInfo, ValidationFlags, Validator};

use super::glsl::{self, layout};
use super::library;
use super::preset::Preset;

#[derive(Debug, Clone, Copy)]
enum Backend {
    Metal,
    Hlsl,
    Spirv,
    Gl,
}

const BACKENDS: [Backend; 4] = [Backend::Metal, Backend::Hlsl, Backend::Spirv, Backend::Gl];

fn front(source: &str) -> Result<(naga::Module, ModuleInfo), String> {
    let module = Frontend::default()
        .parse(&Options::from(ShaderStage::Fragment), source)
        .map_err(|e| e.emit_to_string(source))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map_err(|e| e.emit_to_string(source))?;
    Ok((module, info))
}

fn back(module: &naga::Module, info: &ModuleInfo, backend: Backend) -> Result<(), String> {
    let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match backend {
        Backend::Metal => {
            let options = naga::back::msl::Options { lang_version: (2, 4), ..Default::default() };
            naga::back::msl::write_string(module, info, &options, &Default::default())
                .map(drop)
                .map_err(|e| e.to_string())
        }
        Backend::Hlsl => {
            let mut out = String::new();
            let options = naga::back::hlsl::Options::default();
            let pipeline = naga::back::hlsl::PipelineOptions::default();
            naga::back::hlsl::Writer::new(&mut out, &options, &pipeline)
                .write(module, info, None)
                .map(drop)
                .map_err(|e| e.to_string())
        }
        Backend::Spirv => naga::back::spv::write_vec(module, info, &Default::default(), None)
            .map(drop)
            .map_err(|e| e.to_string()),
        Backend::Gl => {
            let mut out = String::new();
            let options = naga::back::glsl::Options {
                version: naga::back::glsl::Version::Desktop(330),
                ..Default::default()
            };
            let pipeline = naga::back::glsl::PipelineOptions {
                shader_stage: ShaderStage::Fragment,
                entry_point: "main".into(),
                multiview: None,
            };
            naga::back::glsl::Writer::new(&mut out, module, info, &options, &pipeline, Default::default())
                .and_then(|mut writer| writer.write())
                .map(drop)
                .map_err(|e| e.to_string())
        }
    }));
    match written {
        Ok(result) => result,
        Err(_) => Err("the writer panicked".into()),
    }
}

#[test]
fn every_pass_of_every_preset_compiles_for_every_backend() {
    let mut failures = Vec::new();
    let mut passes = 0;
    for (file, source) in library::vendored() {
        let preset = Preset::parse(&source).unwrap_or_else(|e| panic!("{file}: {e}"));
        for pass in &preset.passes {
            passes += 1;
            let at = format!("{file} [{}]", pass.name.as_str());
            let translated = match glsl::translate(&preset, pass) {
                Ok(translated) => translated,
                Err(e) => {
                    failures.push(format!("{at}: translation: {e}"));
                    continue;
                }
            };
            match front(&translated) {
                Err(e) => failures.push(format!("{at}: {e}")),
                Ok((module, info)) => {
                    for backend in BACKENDS {
                        if let Err(e) = back(&module, &info, backend) {
                            failures.push(format!("{at}: {backend:?}: {e}"));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of the passes failed:\n{}", failures.len(), failures.join("\n"));
    // Nine presets, fifteen passes: a count that shrank would mean files
    // were lost, not that everything passed.
    assert!(passes >= 15, "only {passes} passes were compiled");
}

#[test]
fn the_uniforms_and_bindings_are_where_the_renderer_puts_them() {
    let preset = Preset::parse(
        "void mainImage(out vec4 c, in vec2 p) {\n\
         c = texture(iChannel0, p / iResolution.xy) + texture(iChannel1, p) + texture(iChannel2, p)\n\
           + texture(iChannel3, p) + vec4(iTime + iTimeDelta + float(iFrame) + iSampleRate + iParams[7]\n\
           + iChannelTime[3] + iChannelResolution[3].x + iMouse.x + iDate.x);\n\
         }\n",
    )
    .unwrap();
    let (module, _) = front(&glsl::translate(&preset, &preset.passes[0]).unwrap()).unwrap();

    let mut textures = Vec::new();
    let mut samplers = Vec::new();
    for (_, var) in module.global_variables.iter() {
        let binding = var.binding.as_ref().map(|b| (b.group, b.binding));
        match (&var.space, &module.types[var.ty].inner) {
            (naga::AddressSpace::Uniform, naga::TypeInner::Struct { members, span }) => {
                assert_eq!(binding, Some((layout::UNIFORM_GROUP, layout::UNIFORM_BINDING)));
                assert_eq!(*span as usize, layout::SIZE);
                let offset = |name: &str| {
                    members.iter().find(|m| m.name.as_deref() == Some(name)).expect(name).offset as usize
                };
                assert_eq!(offset("iResolution"), layout::RESOLUTION);
                assert_eq!(offset("iTime"), layout::TIME);
                assert_eq!(offset("iMouse"), layout::MOUSE);
                assert_eq!(offset("iDate"), layout::DATE);
                assert_eq!(offset("iTimeDelta"), layout::TIME_DELTA);
                assert_eq!(offset("iFrame"), layout::FRAME);
                assert_eq!(offset("iSampleRate"), layout::SAMPLE_RATE);
                assert_eq!(offset("mstream_params"), layout::PARAMS);
                assert_eq!(offset("mstream_channel_time"), layout::CHANNEL_TIME);
                assert_eq!(offset("mstream_channel_resolution"), layout::CHANNEL_RESOLUTION);
            }
            (_, naga::TypeInner::Image { .. }) => textures.push(binding),
            (_, naga::TypeInner::Sampler { .. }) => samplers.push(binding),
            _ => {}
        }
    }
    textures.sort();
    let group = layout::CHANNEL_GROUP;
    assert_eq!(textures, [Some((group, 0)), Some((group, 1)), Some((group, 2)), Some((group, 3))]);
    assert_eq!(samplers, [Some((group, layout::SAMPLER_BINDING))]);
}

// ── Canaries ────────────────────────────────────────────────────────────────

const HEADER: &str = "\
#version 450
layout(set = 1, binding = 0) uniform texture2D t0;
layout(set = 1, binding = 4) uniform sampler s0;
layout(location = 0) out vec4 color;
";

#[test]
fn naga_still_refuses_combined_sampler_uniforms() {
    // Why the preamble declares textures and a sampler apart and rebuilds
    // `iChannelN` with a macro.
    let source = "#version 450\nlayout(set = 1, binding = 0) uniform sampler2D c;\n\
                  layout(location = 0) out vec4 color;\nvoid main() { color = texture(c, vec2(0.0)); }\n";
    assert!(front(source).is_err(), "naga accepts combined samplers now: the preamble can say sampler2D");
}

#[test]
fn naga_still_refuses_sampler2d_parameters() {
    // Why `glsl::split` exists.
    let source = format!(
        "{HEADER}float hf(sampler2D s, vec2 p) {{ return texture(s, p).r; }}\n\
         void main() {{ color = vec4(hf(sampler2D(t0, s0), gl_FragCoord.xy)); }}\n"
    );
    assert!(front(&source).is_err(), "naga accepts sampler2D parameters now: glsl::split can go");
}

#[test]
fn naga_still_panics_writing_spirv_for_an_inout_swizzle() {
    // Why `glsl::hoist` exists. The panic's message lands in the test
    // output; it is the expected one.
    let source = format!(
        "{HEADER}float mod1(inout float p, float s) {{ p = mod(p, s); return p; }}\n\
         void main() {{ vec2 v = gl_FragCoord.xy; float c = mod1(v.x, 2.0); color = vec4(v, c, 1.0); }}\n"
    );
    let (module, info) = front(&source).expect("it validates; it is the writer that fails");
    assert!(back(&module, &info, Backend::Metal).is_ok());
    assert!(
        back(&module, &info, Backend::Spirv).is_err(),
        "naga writes SPIR-V for an inout swizzle now: glsl::hoist can go"
    );
}

#[test]
fn naga_still_builds_an_invalid_mat2_from_one_vec4() {
    // Why 06-4d-beats.glsl carries a one-line local edit. The fix belongs
    // in the mobile app's copy regardless; this says when naga needs it.
    let source = format!("{HEADER}void main() {{ mat2 r = mat2(vec4(1.0, 0.0, 0.0, 1.0)); color = vec4(r[0], r[1]); }}\n");
    assert!(front(&source).is_err(), "naga builds mat2(vec4) now: 06's edit is no longer needed here");
}
