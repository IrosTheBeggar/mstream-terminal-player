# ratatui-wgpu, vendored

Upstream: [ratatui-wgpu 0.6.0](https://crates.io/crates/ratatui-wgpu/0.6.0) from crates.io
(repository <https://github.com/Jesterhearts/ratatui-wgpu>, commit
`254e49c4f60e8841258fb2e5c8a89a1d51afe830` per the crate's `.cargo_vcs_info.json`). The root
`Cargo.toml` swaps it in with `[patch.crates-io]`, so the dependency line there still reads
`ratatui-wgpu = "0.6"`.

## Why it is vendored

The GUI's own window (`src/gui/window/`) draws through this backend, and it needs fixes that
0.6.0 does not have: the surface format and the blit's sRGB decode, the face index of a font
collection, a font id that does not read the whole font, failed presents (and how loudly they
are logged), fallback glyphs that fit their cells, a wide glyph narrowed without shifting the
row, a public offscreen path for tests, the build's stage timings and a device made before
the window. They land here, one change
at a time, each recorded below, until an upstream release carries them and the patch entry can
go.

## What differs from 0.6.0

### Packaging

- Dropped: `splash.gif` (5.7 MB, the README's picture), the crate's `Cargo.lock`,
  `Cargo.toml.orig`, `.gitignore`, `.cargo_vcs_info.json`, `.cargo-ok`, and its
  `.gitattributes`, which routes `*.ttf` through Git LFS.
- Dropped: `src/backend/fonts/` (Fairfax.ttf, CascadiaMono-Regular.ttf, 4.8 MB) and
  `src/backend/goldens/`, which only the crate's own tests read (see "Tests" below).
- `Cargo.toml`: the `[dev-dependencies]` removed (the published manifest has no `[[example]]`
  sections and its `examples/` directory was never shipped), so resolving this tree does not
  pull winit, image, chrono and the rest for tests nobody here runs.
- Kept as published: the rest of `src/`, the shaders, `README.md`, `LICENSE-MIT`,
  `LICENSE-Apache` and `rustfmt.toml`. The doc example in `lib.rs` still names the dropped
  Cascadia font; doc tests are not run here.

### Code

1. **Headless rendering is public** (`backend/mod.rs`, `backend/builder.rs`, `lib.rs`).
   Upstream's `HeadlessSurface`, `Builder::build_headless` and
   `Builder::build_headless_with_format` were `#[cfg(test)]` and `pub(crate)`; they are now
   public, `HeadlessSurface` is exported, and `WgpuBackend<_, _, _, HeadlessSurface>` gains
   `read_pixels() -> Option<Vec<u8>>`, the presented frame as RGBA rows (a `Bgra8` surface is
   swizzled; an sRGB one reads back its encoded bytes, as a screen shows them). Why: so the
   player can render a `Terminal` offscreen in its own tests (`src/gui/window/render_tests.rs`)
   and judge the window by its pixels without opening one. The readback buffer's rows are now
   padded to `COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes); upstream used the bare row, which a
   texture-to-buffer copy rejects unless the width is a multiple of 64 pixels.
2. **The surface format is linear where it can be** (`backend/builder.rs`). wgpu lists a
   surface's sRGB formats first, so `get_default_config` picks one (`Bgra8UnormSrgb` on
   macOS), while the colours a ratatui app gives are sRGB-encoded bytes already. After
   `get_default_config` the builder now takes the format's `remove_srgb_suffix()` twin when
   the surface's capabilities list it (only then: configuring an unlisted format panics), so
   the blit passes bytes through untouched. `WgpuBackend::surface_format()` reports the
   choice; a post processor already sees it in `surface_config` (`compile`, `resize`,
   `process`). Measured in the player's window on this Mac: the ground #12131c drew as
   #0a0b16 and gold #e5c07b as #e6c17c; with the linear surface (`Bgra8Unorm`, confirmed)
   both are exact.
3. **Where only sRGB is offered, the blit decodes exactly** (`shaders/blit.wgsl`). The blit's
   decode for an sRGB target was `pow(c, 2.2)`, which is not the inverse of the store's sRGB
   encode and darkened the darks (the #0a0b16 above). It is now the piecewise sRGB decode
   (IEC 61966-2-1), so a byte round-trips to itself within one level; alpha is left alone.
   Headless surfaces are never switched, so `build_headless_with_format(Rgba8UnormSrgb)` keeps
   this branch testable. `shaders/crt.wgsl` (the CRT post processor, unused here) keeps its
   `pow(2.2)`.
4. **A collection's faces open by index** (`fonts.rs`). `Font::new_at(data, index)` maps to
   `Face::from_slice(data, index)`; `Font::new(data)` is `new_at(data, 0)`. Upstream opened
   face 0 only, so Noto CJK's Simplified Chinese (face 2 of `NotoSansCJK-Regular.ttc`) was
   unreachable. The index is hashed into the font's id with the bytes, so two faces of one
   collection do not share cached glyphs.
5. **A failed present is owed, not lost** (`backend/wgpu_backend.rs`). When the surface gave
   no texture, `render` returned with its text pass unsubmitted and nothing presented, and the
   rows it drew were no longer dirty; `flush` only renders when a cell is dirty (or the post
   processor asks), so the window kept the stale frame until some cell changed. Now the text
   pass is submitted anyway (the composite holds the frame), `present_owed` is set, and
   `flush` composites-and-presents while it is set, dirty cells or not; a present clears it.
   While a surface keeps failing (occluded), every flush retries the acquire.
6. **No `unused_mut` warnings** (`backend/wgpu_backend.rs`, `extract_color_image`). Only the
   PNG arm re-reads `src_width`/`src_height`, and this tree builds without the `png` feature;
   the two `let mut`s carry `#[cfg_attr(not(feature = "png"), allow(unused_mut))]`. Path
   dependencies are not lint-capped the way registry crates are, so these showed in every
   build.
7. **A font's id hashes a bounded prefix, not every byte** (`fonts.rs`, `Font::new_at`).
   Upstream fed the whole of the font's data to the id's hasher. The player hands the
   backend system collections it memory-maps (`src/gui/window/mod.rs`, `map_font`: Hiragino
   Sans, Hiragino Sans GB and Apple SD Gothic Neo on macOS, 86 MB together), and hashing
   them read every page, faulting all of it into the resident set at startup: measured on
   an idle window, 223 MB RSS with the whole-data hash against 137 MB with this one. The
   id now hashes the first 64 KiB, the length and the face index. Nothing is lost:
   `RandomState::new` advances its keys on every call, so two constructions never shared an
   id however their bytes compared, and the id only keys the glyph caches of the backend
   that holds the font.
8. **A glyph narrower than its box is centred, not enlarged** (`backend/wgpu_backend.rs`,
   `rasterize_glyph`). Each face is scaled so its line (ascender to descender) is the cell's
   height. Upstream then scaled every glyph again by box width / advance, both ways, so a
   face whose wide glyphs advance less than two cells grew until the advance filled the box;
   the line grew past the cell with it, and the raster is clipped to the box. Apple SD Gothic
   Neo's hangul advance 865 units on a 900/-300 line, so they were drawn 1.39 times over, and
   upstream's x offset (`-(width * (1 - scale))`, positive when enlarging) pushed them right
   as well. Measured in the player's window (32 px cell, 32x32 box): 한, 국 and 어 inked from
   row 0 and up to column 31, the box's edges, with their tops cut; 한's ㅏ lost its short
   stroke and read as 힌, and 어's ㅓ was a 1 px sliver. Now the factor only shrinks
   (`min(1.0)`). The x offset centres the scaled advance in the box, which is no offset when
   the glyph was shrunk, since its advance then fills the box. (Upstream's offset moved a
   shrunk glyph left by `width * (1 - scale) / 2` and cut its left edge; that follows from the
   arithmetic and was not seen here, since no face this player uses is shrunk.) The y offset
   still centres a shrunk line. The same hangul now ink within columns 5..26 and rows 2..25. The rest
   of that window is pixel-identical to before: Hack, Menlo's stars, Hiragino Sans' kana and
   kanji, whose advance already equals the box. The cost is size: hangul are drawn at their
   face's own line, about 23 px tall against the kana's 27 to 30. Filling the box from the
   face's ideographic em box (its `BASE` table: `ideo` -200 for Apple SD Gothic Neo, -120
   for Hiragino) would match the kana, but the hangul ink reaches 804 units, above that box's
   top at 800, so they would touch the cell's top row again. Not done. Noto Sans CJK, the
   Linux faces, have a taller line than their advance (1448 units against 1000), so upstream
   enlarged their kana and hanzi too; they now draw at their line's size as well. That is
   arithmetic, not measured: Linux was not run.

9. **A failed acquire is logged once per outage** (`backend/mod.rs`, `backend/wgpu_backend.rs`).
   Upstream's `RenderSurface::get_current_texture` logged `error!("Failed to acquire surface
   texture")` itself, on every call; with change 5 every flush retries while a present is owed,
   so an occluded window wrote about ten error lines a second into the player's log (at the
   window's 100 ms poll), for as long as it stayed hidden. The trait method now returns
   `Result<Target, String>`, the reason being wgpu's `CurrentSurfaceTexture` variant (the
   headless surface's is that it is not configured), and `render` logs it: at `warn!` when it
   starts an outage (no present was owed yet), at `debug!` for the retries while one is, and a
   `debug!` line when a present succeeds again and pays the debt. The trait is sealed by its
   private token, so nothing outside the crate implemented it. One line is new rather than
   quieter: upstream's headless surface returned `None` without a word when it had not been
   configured, and `render` gave up as silently; it now goes through the same arm, so an
   unconfigured headless surface logs `Failed to acquire surface texture: the headless surface
   is not configured; ...` at `warn!` once, where upstream logged nothing. The player's render
   tests configure theirs before drawing, so it does not show there.
10. **A narrower cell over a wide one erases the whole wide glyph** (`backend/wgpu_backend.rs`,
    `draw`). Drawing a wide cell fills the cells it covers with an empty continuation
    (`NULL_CELL`, symbol `""`), which shapes to nothing. Upstream overwrote only the cell it was
    given, so a narrow cell landing on a wide glyph's first cell left the continuation behind:
    ratatui's diff does not send that cell when it is a blank in both frames (ratatui-core 0.1's
    `BufferDiff` counts on a terminal erasing all of a wide character when any of it is
    overwritten, and only forces the trailing cells for a styled blank or a VS16 emoji), so the
    row's string lost a cell and every glyph after it on the row drew one cell to the left of
    its own. Seen as a field's text leaving residue and its border shifting after CJK in it was
    edited; reproduced headless (`src/gui/window/render_tests.rs`,
    `a_wide_glyph_narrowed_leaves_no_residue`: `│日本│` redrawn as `│ab  │` drew the last `│`
    in cell 4). Now, when the cell written is narrower than the one it replaces, the
    continuation cells it no longer covers become blanks (`Cell::EMPTY`), as a terminal's are.
    Only continuations are touched, so a cell ratatui did send is never overwritten.
11. **The build says what each stage took** (`backend/builder.rs`,
    `backend/wgpu_backend.rs`). `build_with_render_surface` laps a clock after the adapter, the
    device, the surface's configuration and the rest (the textures, the shaders and the
    pipelines, the post processor's `compile` included, which moved out of the struct literal
    into a `let` so it can be timed), and the backend keeps the list:
    `WgpuBackend::build_timings() -> &[(&'static str, Duration)]`. Why: the player's window on
    Windows 10 stayed white for two to three seconds before its first present (0.15 s on a
    Mac), and the stats lever (`MSTREAM_WINDOW_STATS`) now names the stage. Four clock reads per
    build; nothing else changes.
12. **A device made before the window** (`backend/builder.rs`). `Builder::with_device(adapter,
    device, queue)` hands the build an adapter, and a device and queue requested from it, made
    with the same instance as `with_instance`'s. The build uses them when the adapter supports
    the surface (`Adapter::is_surface_supported`; a headless surface takes them as they are),
    with the device's own limits in place of `with_limits`; otherwise it requests its own as
    before, and the timings say `adapter (given)`/`device (given)` or `adapter`/`device`. Why:
    the adapter and the device need no window, and the player requests them on a thread of its
    own while the event loop starts and the window opens (`src/gui/window/mod.rs`, `Gpu`), so
    the window's `resumed`, where it sits blank, no longer waits for them (on Vulkan with two
    GPUs, the slowest steps of the startup).

### Tests

The `#[cfg(test)] mod tests` of `backend/wgpu_backend.rs` (golden-image tests) and of
`utils/text_atlas.rs` (`reuse`) are deleted: they `include_bytes!` the dropped fonts and
goldens and use the dropped dev-dependencies (image, serial_test, futures-lite, oneshot), so
they could not compile here. Nothing else of upstream's tests was touched;
`cargo check -p ratatui-wgpu --tests` builds clean. The player's own render tests cover the
headless path instead.

The reported wide-glyph shift (cells after CJK drawn about two cells left) does not reproduce
in this code: `render_tests.rs` draws `AB日本CD` with Hack and Hiragino, on a first frame and
as partial updates in and out of CJK, and C and D land in cells 6 and 7 pixel for pixel. The
dump's `get_text` spells the cell after a wide glyph as an empty string, so its text puts C
two characters early; that is likely all the report saw. No shaping change was made. A shift of
one cell did reproduce later, in another case — a wide glyph replaced by narrow cells, with
something after it on the row — and is change 10.
