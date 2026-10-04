# ratatui-wgpu, vendored

> This copy ships in the desktop product's releases, by the owner's decision of 2026-10-02; this
> file is the record of what differs. Filing the changes upstream is a separate task.

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
row, a public offscreen path for tests, the build's stage timings, a device made before
the window and the adapter in use, a wgpu that keeps to the player's feature set, fallback
faces drawn at the main face's size, colour emoji drawn whole in their cells, and a cell's
face chosen by its first character. They
land here, one change at a time, each recorded below, until an upstream release carries them
and the patch entry can go.

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
   build. (Superseded by change 16, which replaced `extract_color_image` with
   `colour_pixels` and `colour_bitmap`; the window builds now turn `png` on, and the new code
   has no binding that only one arm uses.)
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
13. **The backend says which adapter it draws with** (`backend/builder.rs`,
    `backend/wgpu_backend.rs`). The build keeps the adapter's `AdapterInfo`, and
    `WgpuBackend::adapter_info() -> &wgpu::AdapterInfo` returns it. Why: with change 12 the
    caller cannot tell which adapter is in use: the one it handed over is dropped when it
    cannot present to the surface, and the build then requests another. The player prints
    the name and backend once the backend is built (`src/gui/window/mod.rs`), the one line a
    report from another machine needs. One `get_info` per build.
14. **wgpu without its default features** (`Cargo.toml`). The published manifest takes
    `wgpu = "30.0.0"` with its defaults, which turn on `webgpu` (the browser backend) and,
    through it, naga's `wgsl-out`: exactly what the player's own `default-features = false`
    on wgpu keeps out, and features are additive, so the window brought them back for the
    whole build. It now asks for `std` and `wgsl` only (`include_wgsl!` needs `wgsl`); the
    backends (Metal, DX12, Vulkan, GLES) come from the player's line, as before. Why: the
    desktop build's wgpu is then the terminal build's, feature for feature (`cargo tree -e
    features -i wgpu`), and no browser backend is compiled into a native binary.

15. **Fallback faces are drawn at the main face's em** (`fonts.rs`, `Fonts::face_scale`;
    `backend/wgpu_backend.rs`, `flush` and `rasterize_glyph`). Upstream fitted every face's own
    line (ascender to descender) to the cell's height, so a face's glyphs came out as large as
    its line was short: in the player's 32 px cell Apple SD Gothic Neo's hangul (a 1200-unit
    line on a 1000-unit em) drew about 23 px tall beside Hiragino Sans' kana (a 1000-unit
    line) at 27 to 30, and at 24 px 한, か and 日 inked 17, 20 and 21 px. Now the last resort
    (the builder's font, Hack in the player) is sized as before, and every other face at the
    last resort's pixels per em, with its baseline on the last resort's baseline; a face with
    the last resort's em, line and ascender (another copy of Hack, the player's bundled
    symbol face, Menlo) takes the old arithmetic unchanged, so its pixels are the same. Since a
    face drawn that way can reach past the cell where its own line did not, a glyph whose ink
    (its bounding box) would leave the box is moved back inside, shrunk to the box's height
    only when its ink is taller than the box. None of the kana, hanzi or hangul the render
    tests draw on this Mac needed either (Apple Color Emoji's glyph boxes do, but its bitmaps
    are placed by `colour_bitmap`, change 16, which ignores the outline's offsets).
    Underlines and strike-outs take the shared baseline too. Measured
    (`src/gui/window/render_tests.rs`, `hangul_kana_and_hanzi_draw_at_one_size`): 한, か and 日
    ink 18, 18 and 18 px at 24 px, and 24, 23 and 23 px at 32 px, where the test allows 15%
    between them. The kana and hanzi are smaller than before (Hiragino's em drew at the cell's
    height, 32 px; it now draws at Hack's, 27.5 px), the hangul a little larger. Noto Sans
    CJK (Linux) draws at the same em now rather than at its line's 1448 units: arithmetic, not
    measured, as Linux was not run.
16. **Colour emoji, drawn whole in their cells** (`backend/wgpu_backend.rs`, `flush`,
    `rasterize_glyph`, `colour_pixels`, `colour_bitmap`; `utils/text_atlas.rs`, `Key`;
    `fonts.rs`, `Font::fallback_width`). The player hands the window the system's colour emoji
    face (Apple Color Emoji, Segoe UI Emoji, Noto Color Emoji) and builds this crate with its
    `png` feature. Six things upstream did wrong with one:
    - A glyph's box was as many cells as the cluster's *first character* is wide, which is
      narrower than ratatui's cell for an emoji with VS16 (❤ is one cell, ❤️ two) and a flag
      (each regional indicator is one cell, the pair two), so those pictures were squeezed into
      the first of their two cells. The box is now the cell's own width, which ratatui measured
      for the whole grapheme; for every other cell the two agree. The atlas key carries the
      width (`Key::cells`), since one glyph can stand in a narrow cell and a wide one.
    - A grapheme the face has no ligature for (a ZWJ sequence it does not know, a skin tone it
      lacks) shapes to several glyphs that each advance, and each was drawn a cell further on,
      over whatever the next cells held. Only a cell's first advancing glyph is drawn now: the
      sequence degrades to its base emoji in its own cells (never to boxes). Combining marks,
      which do not advance, are drawn as before. The latent limit: a grapheme that shapes to
      several *advancing* glyphs that are not emoji (Thai or Devanagari in a face that splits
      a base and its mark into glyphs that each advance) would lose all but the first, the
      mark among them; no face the window loads today shapes any script that way. (Change 21
      leaves out what follows a glyph left out too, and draws an unjoined flag's two letters.)
    - The colour mask (the atlas's "draw these pixels as they are", against "fill this coverage
      with the cell's colour") was set when the cluster's first character is an emoji, so an
      emoji character drawn from a text face's outline (the player's ✔) came out white
      whatever the cell's colour. It is now set when the raster is in colour: a COLR glyph
      painted, or a colour bitmap decoded. A COLR face whose layers the font reader cannot
      paint falls back to its glyph's outline in the cell's colour (Segoe UI Emoji has
      monochrome outlines beside its layers); a bitmap face whose bitmap cannot be decoded
      draws nothing, as before.
    - The bitmap came from the largest strike (Apple Color Emoji's 160 px) and was stretched
      over the whole box with one bilinear sample per pixel, offset by its bearings read as
      font units (they are the strike's pixels), and a straight-alpha PNG was handed to
      raqote, which takes premultiplied pixels. `colour_bitmap` now takes the smallest strike
      at least twice the box's height (then a larger one: Apple Color Emoji's 40, 48 and 52 px
      strikes have no ZWJ family glyphs), scales the strike's em to the size the face is drawn
      at (change 15's em, at most the box's shorter side), centres it, and averages the bitmap
      pixels each box pixel covers, in premultiplied terms, into straight RGBA, which is what
      the text pipeline's `ALPHA_BLENDING` expects. CBDT's premultiplied BGRA bitmaps are
      un-premultiplied the same way. PNG decoding uses `png`'s `normalize_to_color8` and takes
      RGBA, RGB, grey and grey-alpha, where upstream took RGBA and grey-alpha only.
    - `Font::joins` (public, new) says whether a face shapes a string to one advancing glyph
      that is not `.notdef`: the player's render tests ask it before demanding a joined
      picture, since faces differ in which sequences they join (Segoe UI Emoji has no country
      flags, so 🇺🇸 there is its first regional indicator alone, by the rule above).
    - A face without an `m` (a symbol or emoji face: its width is read from `m`'s advance,
      and without one it is `.notdef`'s) narrowed the grid when that advance was small;
      `Fonts` now leaves such a face out of the cell's width (the last resort always counts).
    Checked offscreen (`render_tests.rs`): ❤️, 🇺🇸, 🏴󠁧󠁢󠁥󠁮󠁧󠁿, 👨‍👩‍👧 and 👍🏽 each ink both of their
    cells, in colour, as a picture that differs from their base emoji's (each case a face
    does not join is skipped with a line, keeping only the cell-after checks), with the cell
    after them untouched, through Apple Color Emoji (sbix) and through Noto Color Emoji (CBDT);
    👨‍🦖, which no face joins, draws exactly as 👨 alone; ✔ from a text face is drawn in the
    cell's gold. COLR (Segoe UI Emoji) goes through upstream's `paint_color_glyph`, with change
    15's scale and baseline, and was not run here (no COLR face on this Mac).
17. **A cell's face is one that has its first character** (`fonts.rs`, `Fonts::select_font`).
    Upstream gave a cell to the face with the most of its characters, whichever they were. A
    cell whose base is followed by characters only an emoji face has went to the emoji face,
    which has no glyph for the base, and its `.notdef` (a box) was drawn in the base's place.
    The player met it in a text field: a pasted England flag (🏴 and six tag characters, which
    take no cells) scrolled until the field's left clip fell between the 🏴 and its tags, and
    ratatui hung the tags on the clip mark's cell, `…` with six tags, which Apple Color Emoji
    won six to Hack's one. A caret moved into the flag, between the 🏴 and its tags, does the
    same to the caret's `▏`. A face with the base now beats any face without it, and among those
    the most characters wins as before, so a sequence an emoji face has whole (❤️, a keycap, a
    flag) still goes to it. With the base's face, rustybuzz hides the default-ignorable rest
    (Hack shapes each tag to a blank glyph that does not advance). The field no longer makes
    that cell (the player cuts its line at grapheme boundaries), but text from anywhere else,
    a title with a stray tag run, takes this path. Checked offscreen (`render_tests.rs`,
    `a_stray_tag_run_draws_as_its_base_alone`): `…`, `▏` and `b` with England's tags after them
    draw exactly as they do alone, where upstream's rule drew a box over the first; and live,
    in the search box on this Mac, `…` with the tags drew as `…` where it was a box. Change 21
    refines the rule for a cluster with emoji presentation.
18. **An emoji grown wide in place** (`backend/wgpu_backend.rs`, `draw`; the `Rendered` key).
    Typing ❤ and then its VS16 into a field turns the cell `❤` (one wide) into `❤️` (two
    wide) where it stands, and two things went wrong, both met live in the search box:
    - ratatui's diff sends the cell a VS16 emoji newly covers as a blank when that cell's
      symbol changed (its clear for terminals that leave such an emoji's second half
      behind). `draw` wrote the blank over the continuation it had just made, and the blank
      shaped as a cell of its own, so everything after the emoji on the row drew one cell
      right (the box's right border a cell past its corner). A blank sent for a cell that is
      still a continuation is now ignored: the glyph covers that cell, and a continuation is
      only ever left in place while its glyph covers it (change 10 turns the ones a narrower
      cell uncovers into blanks first).
    - `Rendered` was keyed by place and glyph, and `Sourced` (by change 16) by place, glyph
      and width: the narrow heart and the wide one, the same glyph at the same place, were one
      entry, which the removal of the narrow one's `Sourced` took away. The wide heart drew
      nothing and its second cell kept the caret drawn there before. `Rendered` now carries
      the width in its key too.
    Checked offscreen (`render_tests.rs`, `an_emoji_widened_in_place_keeps_its_row_in_place`):
    `❤▏|` then `❤️▏|`, `❤x|` then `❤️|`, `ab|` then `❤️|`, `1x2|` then `1️⃣2|` and `❤️▏|`
    then `❤▏|` each draw cell for cell as the second line does fresh. Before, the first four
    drew their rows from the emoji's second cell on one cell right, and with the blank
    ignored but the key unchanged the two hearts grown in place still differed in both their
    cells (no white of their own, the old `▏` or `x` in the second). Upstream had neither:
    its widths were the cluster's first character's, which a VS16 does not change.

19. **The build in two halves, the second off the window's thread** (`backend/builder.rs`,
    `lib.rs`). `build_with_target` was the surface's creation and the rest of the build in
    one call, on the caller's thread. `Builder::create_surface(target)` is now the first half
    on its own — the one part that must run on the thread that owns the window, since on
    macOS a surface is the view's `CAMetalLayer` — and `Builder::build_parts_with_surface`
    the second, which may run on any thread: the adapter's check against the surface, a
    device when none was given, the surface's capabilities and configuration, the atlas,
    the shaders and the pipelines all work from wgpu's handles, which are `Send` and `Sync`.
    The finished backend is not `Send` — the shaping plans' and the glyph atlas's LRU caches
    (`evictor`, over `tether_map`) link their entries by raw pointer — so the second half
    returns `Built`, every field but those two caches, which is `Send` whenever the post
    processor is (checked at compile time for the default one), and `Built::finish` on the
    window's thread makes the caches, empty and cheap, and hands back the backend. The struct
    literal that ended `build_with_render_surface` moved into `finish` unchanged, but for the
    blink clocks, which now start when the backend is finished; `build_with_target`,
    `build_with_surface` and the headless builds are the two halves called in a row and
    behave as before. Why: the configuration and the pipelines are driver work (shader
    compiles; a swapchain on DX12) that took 10 ms here but is the kind of stage that took
    seconds on the Windows machine, and a loop blocked on it answers nothing. The player
    makes the surface on the loop's thread and runs the rest on a thread the loop waits for
    up to 50 ms as the window opens (on macOS the loop's next turn comes only after AppKit
    has shown the window, so a build claimed then drew its first frame 40 ms later than an
    inline one) and polls after that (`src/gui/window/mod.rs`, `Building`, `OPEN_WAIT`);
    with 1.5 s planted in the pipelines step its loop went on answering, and a resize and a
    key made meanwhile took effect.
20. **A present can be owed on purpose** (`backend/wgpu_backend.rs`, `owes_present`,
    `owe_present`). Change 5's `present_owed` was private; the backend now says whether a
    present is owed and lets its caller owe one. The next flush then composites the text
    pass's target — which holds every cell as last drawn — onto a fresh surface texture and
    presents it, though no cell changed. Why: on macOS the first present goes to a window
    not yet on screen, and the player's answer to the window coming into view was a repaint
    of every cell, a full frame of about 40 ms in a debug build here; owing the present
    puts the same screen up for the post processor's pass alone, and the player's stats can
    tell that present from a frame that changed nothing.
21. **An emoji draws from a colour face; an unjoined flag is its two letters** (`fonts.rs`,
    `Fonts::select_font`, `Font::colour_glyph`, `emoji_presentation`, which is public and
    exported from `lib.rs`; `backend/wgpu_backend.rs`, `flush`, `regional_pair`). Change 17's
    rule is refined, and change 16's "only a cell's first advancing glyph" gains an exception
    and a corollary. Met on Windows 10 in the player's window: 🎵 drew as a monochrome
    outline, 👨‍👩‍👧 as one grey silhouette and 🇯🇵 as an unreadable sliver, where Windows
    Terminal on the same machine draws the note and the family in colour; ❤️, the bundled
    face's stars and its tick drew right.
    - The player puts Segoe UI Symbol ahead of Segoe UI Emoji so that ♥ and ✔ keep their text
      form, and Segoe UI Symbol has monochrome outlines for many emoji (🎵; each of the
      family's people, and the ZWJ). The first face with every character of a cell won
      outright, so the emoji face was never asked. On macOS the symbol faces (Menlo, Apple
      Symbols) lack those code points, which is why it was not seen there. A cluster with
      emoji presentation now prefers, among the faces with its base, one whose glyph for the
      base is in colour: COLR layers, or an sbix or CBDT bitmap that is a PNG or premultiplied
      BGRA (`Font::colour_glyph`; a face with none of those tables is never asked). The order
      is: has the base, then in colour (for such a cluster only), then the most of the
      cluster, then the earlier face; a face with all of the cluster ends the search only
      when the cluster is text or the face is in colour. Emoji presentation is UTS #51's
      (`emoji_presentation`): a character with `Emoji_Presentation=Yes` not followed by VS15,
      any character followed by VS16, or an emoji followed by a skin tone, a keycap's U+20E3,
      tags or a ZWJ and another emoji. The property comes from the `unicode-properties` table
      the crate already used (Unicode 17), not from `Emoji`, which ♥, ✔ and `#` also have. A
      cluster with text presentation (♥, ✔, ❤, ★ and ✓ without VS16, a digit, `#`, anything
      the bundled symbol face draws) is chosen exactly as before, and an emoji that no colour
      face has still goes to the first face with the most of it. No face is named. No glyph
      the GUI draws of its own has emoji presentation (its census keeps them to symbols one
      cell wide, and no such character has it).
    - No face the window has on Windows 10 has country flags. Neither Segoe UI Emoji's
      regional indicators nor Segoe UI Symbol's are in colour, so the preference above passes
      them by, and Segoe UI Symbol, the earlier face with both, draws the pair: two monochrome
      letters (narrow, half an em tall), of which only the first was drawn, centred in the
      flag's two cells. A pair that shapes to two advancing glyphs, `.notdef` not counted, is
      now drawn a letter to a cell, `J` then `P`, each in a box one cell wide, as Windows
      Terminal draws it. The second letter's entry stands in the wide cell's continuation, so
      the composite takes that entry's background from the flag's cell, not from the
      continuation's default one, which would have been a gap in a highlighted row. A face
      with the flag (Apple Color Emoji, Noto Color Emoji) shapes the pair to one glyph and
      draws as before, and a face with neither letter (the last resort, on a system with no
      emoji face) keeps change 16's one box over both cells rather than a box to a cell. A
      subdivision flag needs nothing: no face on Windows 10 maps its tags, the shaper hides
      them, and the face with its 🏴 draws that, which is change 16's degradation.
    - A consequence on Windows 10, accepted: 🏴 has emoji presentation, so a subdivision flag
      (England, Scotland, Wales) and a plain 🏴 now draw from Segoe UI Emoji's colour black
      flag, as Windows Terminal draws them, where Segoe UI Symbol's light outline drew before.
      On the player's dark ground the black flag's pole alone stands out. Keeping the outline
      would take a rule for this one character against the presentation Unicode gives it.
    - What follows a glyph left out is left out too. Windows 10's Segoe UI Emoji has no
      family picture: it composes 👨‍👩‍👧 from family-member glyphs, the man and the woman
      advancing and the girl not, placed back over the woman by its positioning. Change 16
      dropped the woman but drew the girl, which does not advance, at the man's place, and
      she reached into the cell before the family's. A glyph that does not advance is now
      drawn only while its cell has drawn no more than its advancing glyphs; combining marks
      before a dropped glyph are drawn as before. Change 22 then draws such a composition
      whole.
    - Latent: a blinking cell's toggle redraws its own index alone, so an unjoined flag's
      second letter would not blink with the first; the player blinks no cell.
    Checked offscreen on Windows 10 (GTX 1060, DX12) with Segoe UI Emoji (COLR) and Segoe UI
    Symbol (`src/gui/window/render_tests.rs`): `only_emoji_presentation_asks_for_a_colour_face`
    holds the property to known code points (♥, ✔, ★, ⚠, ☺ and `#` are text; 🎵, ⌚, ⭐, a
    regional indicator, a VS16 heart, a keycap, England's flag and a ZWJ family are
    pictures); `a_default_presentation_emoji_draws_from_the_colour_face_past_a_text_face`
    draws 🎵 with Segoe UI Symbol before the emoji face exactly as with the two swapped (152
    coloured pixels), and fails with the preference turned off;
    `a_text_presentation_heart_keeps_its_text_face_and_the_cells_colour` draws ♥ from Segoe UI
    Symbol in the cell's gold and ♥️ from the emoji face;
    `an_unjoined_sequence_draws_its_base_and_nothing_after` draws 🇦🇦 and 🇯🇵 as each letter
    exactly as it draws alone, in cells 0 and 1, with nothing after (it fails with the split
    turned off), both with the test faces and in the window's order with Segoe UI Symbol
    ahead of the emoji face, which is where the window's letters come from;
    `emoji_sequences_draw_as_one_picture_in_their_cells` holds 🇺🇸's two cells to each letter
    drawn alone; `a_flag_no_face_has_draws_as_one_box_like_any_missing_emoji` draws 🇯🇵 with
    Hack alone exactly as 🎵, one box over two cells (it failed with `.notdef` counted). Where
    the symbol face lacks 🎵 (macOS's do) the first default
    picture a text face has stands in, and a host with none skips with a line; a face with
    the flag or the family skips those checks with a line, as before. Two tests met Windows
    10's face and were made to tell a picture from its base:
    `emoji_sequences_draw_as_one_picture_in_their_cells` (England's flag drawn as its 🏴 is
    the allowed degradation before the both-cells check, which still holds every picture a
    face has) and `a_pasted_subdivision_flag_shows_in_a_field_as_one_picture` (the flag drawn
    exactly as 🏴 is a face without the picture, whatever `Font::joins` says). Live, in the
    player's window on the same machine: an album of emoji titles drew 🎵 and the family's
    man in colour, ❤️ as before, the bundled stars and tick unchanged, and 🇯🇵 as `J` `P`.
22. **A composed emoji drawn whole; a removal takes only its own cell's entry**
    (`backend/wgpu_backend.rs`, `flush`, `composed_runs`, `rasterize_glyph`;
    `utils/text_atlas.rs`, `Key::run`). Two things change 21 left, found in its review:
    - Windows 10's Segoe UI Emoji has no picture for a family or a couple. It composes them
      from member glyphs by its positioning: 👨‍👩‍👧 shapes to a man (advance 1696 units, offset
      256) and a woman (1552) and a girl who does not advance, placed 1552 units back, in
      front of them; 👩‍❤️‍👨 to a woman, a heart placed over her and a man; 👨‍👦 to a man and a
      boy placed in front of him. Windows Terminal draws the whole composition in the cell,
      and change 21 drew the first person alone. A cell is now composed when the face is a
      COLR face, the cluster has emoji presentation, every glyph of it with ink is a COLR
      picture, and a picture after the first does not advance, which is what tells a
      composition from separate emoji the face cannot join: 👨‍🦖, a man and a dinosaur that
      both advance, still draws as its man by change 16. The cell is drawn once, as its first
      glyph's entry, into one box its cells wide: each picture is painted at its pen position
      plus its offsets, and the run is fitted to its total advance (3248 units for the
      family) as one glyph is to its own, with the ink's height taken over every picture for
      change 15's fit. The atlas key carries a hash of the pictures and their places
      (`Key::run`, 0 for one glyph), as one first glyph can begin different runs. sbix and
      CBDT faces (Apple, Noto) join what they know into one glyph and never take this path.
      `Font::composes` (public, new) says whether a face composes a string, by the same rule,
      as `Font::joins` says whether it joins one: the player's tests ask it.
    - `Rendered` and `Sourced` are keyed by place, glyph and width, and a removal took the
      key out of `Rendered` whichever cell had put it there. Change 21 made one place
      routinely change hands: an unjoined flag's second letter stands in the continuation and
      belongs to its flag's cell, so `x🇵🇪` redrawn as `🇯🇵` puts the same P at the same place,
      cell 1's on the first frame and cell 0's on the second, and cell 1's removal took the
      entry cell 0 had just put there (and the reverse with the frames swapped). The letter was
      left on screen as the frame before drew it, in that frame's colours, and no entry drew
      it again until its row changed. A removal now takes an entry only when the entry's
      `RenderInfo::cell` is the cell whose `Sourced` is being diffed.
    Checked offscreen on Windows 10 (GTX 1060, DX12) with Segoe UI Emoji
    (`src/gui/window/render_tests.rs`): `a_family_the_face_composes_draws_whole_in_its_cells`
    draws `|👨‍👩‍👧|` with both bars as `|  |` draws them and the family's ink 18 px wide and 16
    tall at 24 px, where the man alone was 11 wide and 20 tall (the test failed so), and
    `an_unjoined_sequence_draws_its_base_and_nothing_after` holds that 👨‍🦖 is not composed;
    `a_flag_letter_that_changes_cells_in_place_is_drawn_by_its_new_cell` draws `x🇵🇪|` (gold)
    then `🇯🇵|`, the reverse, and the same with 🇦🇦, each cell for cell as the second line draws
    fresh, and failed at cell 1 before the removal took the owner into account. A row of 👨‍👩‍👧,
    👩‍❤️‍👨, 👨‍👦 and 👨‍👩‍👧‍👦 drawn at 48 px in the window's faces, looked at, showed each group
    whole in its two cells, with 👨‍🦖 still its man.

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
