# ratatui-wgpu, vendored

Upstream: [ratatui-wgpu 0.6.0](https://crates.io/crates/ratatui-wgpu/0.6.0) from crates.io
(repository <https://github.com/Jesterhearts/ratatui-wgpu>, commit
`254e49c4f60e8841258fb2e5c8a89a1d51afe830` per the crate's `.cargo_vcs_info.json`). The root
`Cargo.toml` swaps it in with `[patch.crates-io]`, so the dependency line there still reads
`ratatui-wgpu = "0.6"`.

## Why it is vendored

The GUI's own window (`src/gui/window/`) draws through this backend, and it needs fixes that
0.6.0 does not have: the surface format, the face index of a font collection, wide-glyph
shaping and failed presents. They land here, one change at a time, each recorded below, until
an upstream release carries them and the patch entry can go.

## What differs from 0.6.0

Nothing in the code yet. Only the packaging was trimmed:

- Dropped: `splash.gif` (5.7 MB, the README's picture), the crate's `Cargo.lock`,
  `Cargo.toml.orig`, `.gitignore`, `.cargo_vcs_info.json`, `.cargo-ok`, and its
  `.gitattributes`, which routes `*.ttf` through Git LFS and would make the test fonts below
  LFS pointers in this repository.
- `Cargo.toml`: the `[dev-dependencies]` removed (the published manifest has no `[[example]]`
  sections and its `examples/` directory was never shipped), so resolving this tree does not
  pull winit, image, chrono and the rest for tests nobody here runs.
- Kept as published: `src/` whole — including `src/backend/fonts/` and
  `src/backend/goldens/`, which only the crate's own `#[cfg(test)]` modules read — the
  shaders, `README.md`, `LICENSE-MIT`, `LICENSE-Apache` and `rustfmt.toml`.

Its two `unused_mut` warnings in `wgpu_backend.rs` are upstream's; path dependencies are not
lint-capped the way registry crates are, so they show in every build of this tree.

## Trimmed

`src/backend/fonts/` (Fairfax.ttf, CascadiaMono-Regular.ttf, 4.8 MB) and `src/backend/goldens/` are gone: only the crate's own `#[cfg(test)]` modules read them, and this tree never runs those tests (`cargo test` for the binary compiles no dependency tests).
