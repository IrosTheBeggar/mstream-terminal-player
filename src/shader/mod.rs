//! The mobile app's visualizers, on the desktop (PLAN.md, Phase 10).
//!
//! mstream_music runs ShaderToy-convention fragment shaders over an audio
//! texture, and so does this: the same preset files, vendored verbatim in
//! `assets/visualizer/`, read by the same rules. Everything here is pure —
//! text in, text or bytes out — so it is all testable without a GPU, and
//! the renderer that eventually draws it only has to trust three things:
//!
//! - [`preset`] reads the file format: the passes, how their channels are
//!   wired, the buffer sizes, the `// param:` knobs, the header metadata.
//! - [`glsl`] turns one pass into GLSL that naga — the shader compiler
//!   inside wgpu — accepts, which the presets as written are not quite.
//! - [`audio`] builds the 512×2 texture the shaders sample as `iChannel0`,
//!   by the curve the presets were tuned against.
//!
//! [`library`] is the set this binary carries, and [`render`] draws them —
//! the one part that needs a GPU, and so the one part the tests cannot reach.

pub mod audio;
pub mod glsl;
pub mod library;
pub mod preset;
pub mod render;

#[cfg(test)]
mod matrix;
