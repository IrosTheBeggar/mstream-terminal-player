//! The compile matrix: every vendored preset, every pass, every backend.
//!
//! A pass that naga cannot compile is a preset that shows nothing, on some
//! machine nobody here is sitting at. This finds that out on every run of
//! the tests, for all four targets wgpu translates to, without a GPU:
//! naga's frontend and validator, then the Metal, HLSL, SPIR-V and GLSL
//! writers, each behind `catch_unwind` because a writer can panic where it
//! should have returned an error — one does, below. On Windows the HLSL
//! then goes through FXC, the compiler after naga on the DX12 path, which
//! has refused a pass that every writer accepted (09's image pass did).
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
            let written = naga::back::hlsl::Writer::new(&mut out, &options, &pipeline)
                .write(module, info, None)
                .map_err(|e| e.to_string())?;
            #[cfg(windows)]
            for entry in written.entry_point_names {
                fxc::compile(&out, &entry.map_err(|e| e.to_string())?)?;
            }
            #[cfg(not(windows))]
            drop(written);
            Ok(())
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

// ── FXC ─────────────────────────────────────────────────────────────────────

/// FXC, which wgpu's DX12 backend compiles naga's HLSL with unless it finds
/// a DXC (`dxcompiler.dll`, which we do not ship) on the DLL search path,
/// and DX12 is what Windows draws with first (`gpu_pick`). It has rules
/// naga's writer does not know: a texture sampled with an implicit LOD
/// inside a loop it cannot unroll is refused, which is why
/// 09-mountainbytes.glsl carries a local edit. It is part of Windows itself
/// (`d3dcompiler_47.dll`, in System32 since 8.1), so the test needs nothing
/// installed.
#[cfg(windows)]
mod fxc {
    use std::ffi::{CString, c_char, c_void};

    #[link(name = "d3dcompiler_47", kind = "raw-dylib")]
    unsafe extern "system" {
        fn D3DCompile(
            source: *const c_void,
            source_len: usize,
            source_name: *const c_char,
            defines: *const c_void,
            include: *mut c_void,
            entry_point: *const c_char,
            target: *const c_char,
            flags1: u32,
            flags2: u32,
            code: *mut *mut c_void,
            errors: *mut *mut c_void,
        ) -> i32;
    }

    /// The flag wgpu compiles with, when its debug layer is off.
    const D3DCOMPILE_ENABLE_STRICTNESS: u32 = 1 << 11;

    /// An `ID3DBlob`'s vtable: IUnknown's three entries, then its own two.
    #[repr(C)]
    struct BlobVtbl {
        _query_interface: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        buffer_pointer: unsafe extern "system" fn(*mut c_void) -> *mut c_void,
        buffer_size: unsafe extern "system" fn(*mut c_void) -> usize,
    }

    /// A blob's bytes as text, and the blob released; empty for no blob.
    ///
    /// # Safety
    /// `blob` is null or an `ID3DBlob` this code owns a reference to.
    unsafe fn take(blob: *mut c_void) -> String {
        if blob.is_null() {
            return String::new();
        }
        unsafe {
            let vtbl = &**blob.cast::<*const BlobVtbl>();
            let bytes = std::slice::from_raw_parts(
                (vtbl.buffer_pointer)(blob).cast::<u8>(),
                (vtbl.buffer_size)(blob),
            );
            let text = String::from_utf8_lossy(bytes).trim_end_matches('\0').trim().to_string();
            (vtbl.release)(blob);
            text
        }
    }

    /// Compile `hlsl`'s `entry` as a pixel shader for the shader model wgpu
    /// asks FXC for (5.1, its last), with wgpu's flags.
    pub fn compile(hlsl: &str, entry: &str) -> Result<(), String> {
        let entry = CString::new(entry).map_err(|e| e.to_string())?;
        let mut code = std::ptr::null_mut();
        let mut errors = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call, the lengths are the
        // buffers', and the two blobs handed back are ours to release.
        let (result, errors) = unsafe {
            let result = D3DCompile(
                hlsl.as_ptr().cast(),
                hlsl.len(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                entry.as_ptr(),
                c"ps_5_1".as_ptr(),
                D3DCOMPILE_ENABLE_STRICTNESS,
                0,
                &mut code,
                &mut errors,
            );
            take(code);
            (result, take(errors))
        };
        if result < 0 {
            // The warnings come too; the errors are what matter, when FXC
            // names any.
            let named: Vec<&str> = errors.lines().filter(|l| l.contains(": error ")).collect();
            let reason = if named.is_empty() { errors.clone() } else { named.join("\n") };
            return Err(format!("FXC (0x{result:08x}): {reason}"));
        }
        Ok(())
    }
}
