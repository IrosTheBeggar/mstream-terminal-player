//! The window's backend drawn offscreen and judged by its pixels.
//!
//! ratatui-wgpu (vendored, `vendor/ratatui-wgpu`) can render into a texture
//! and read it back (`Builder::build_headless`, `WgpuBackend::read_pixels`),
//! so what the window would show is checked here without opening one. These
//! need a wgpu adapter but no window or display: Metal on macOS, the
//! platform's GPU or a software adapter elsewhere.

use std::num::NonZeroU32;

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_wgpu::shaders::DefaultPostProcessor;
use ratatui_wgpu::wgpu::TextureFormat;
use ratatui_wgpu::{Builder, Dimensions, Font};

use super::{emoji_face, hack, script_fallbacks, symbol_fallback, symbols};
use crate::runtime::block_on;
use crate::viz_window::overlay::cjk_faces;

/// The type size the tests draw at: a desktop window's at scale 2.
const PX: u32 = 24;

/// The theme's ground, the colour most of the window is.
const GROUND: [u8; 3] = [0x12, 0x13, 0x1c];
/// The theme's gold, a light colour beside the dark ground.
const GOLD: [u8; 3] = [0xe5, 0xc0, 0x7b];

/// One rendered frame: RGBA bytes and the grid's geometry in pixels.
struct Frame {
    pixels: Vec<u8>,
    width: u32,
    cell_w: u32,
    cell_h: u32,
}

impl Frame {
    fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * self.width + x) * 4) as usize;
        [self.pixels[at], self.pixels[at + 1], self.pixels[at + 2]]
    }

    /// Every pixel of one cell, row by row.
    fn cell(&self, col: u32, row: u32) -> Vec<[u8; 3]> {
        let (x0, y0) = (col * self.cell_w, row * self.cell_h);
        (y0..y0 + self.cell_h)
            .flat_map(|y| (x0..x0 + self.cell_w).map(move |x| (x, y)))
            .map(|(x, y)| self.pixel(x, y))
            .collect()
    }

    /// Whether a cell holds anything but ground: a glyph's ink.
    fn inked(&self, col: u32, row: u32) -> bool {
        self.cell(col, row)
            .iter()
            .any(|px| px.iter().zip(GROUND).any(|(&got, ground)| got.abs_diff(ground) > 40))
    }
}

/// Draws each of `frames` in turn on a `cols`-wide grid of as many rows as
/// the first has, in `faces` (the first is also the last resort, as in the
/// window), onto a headless surface of `format`, white on the ground, and
/// reads the last frame back. Later frames reach the backend as ratatui's
/// diff of the one before, the way a running window's do.
fn render(
    faces: Vec<Font<'_>>,
    cols: u32,
    frames: Vec<Vec<Line<'static>>>,
    format: TextureFormat,
) -> Result<Frame, String> {
    render_at(PX, faces, cols, frames, format)
}

/// [`render`] at a type size of `px`.
fn render_at(
    px: u32,
    faces: Vec<Font<'_>>,
    cols: u32,
    frames: Vec<Vec<Line<'static>>>,
    format: TextureFormat,
) -> Result<Frame, String> {
    let rows = frames[0].len() as u32;
    let last_resort = faces[0].clone();
    // The cell's width is the narrowest face's, which the backend keeps to
    // itself (as the window finds): built wide first, the width read back
    // from the grid it made, then trimmed to the grid asked for.
    let wide = 4096;
    let builder = Builder::<DefaultPostProcessor>::from_font(last_resort)
        .with_regular_fonts(faces)
        .with_font_size_px(px)
        .with_width_and_height(Dimensions {
            width: NonZeroU32::new(wide).unwrap(),
            height: NonZeroU32::new(rows * px).unwrap(),
        })
        .with_bg_color(Color::Rgb(GROUND[0], GROUND[1], GROUND[2]))
        .with_fg_color(Color::White);
    let mut backend = block_on(builder.build_headless_with_format(format))?
        .map_err(|e| format!("no headless wgpu backend: {e}"))?;
    let reported = backend.window_size().map_err(|e| e.to_string())?;
    let cell_w = wide / u32::from(reported.columns_rows.width);
    backend.resize(cols * cell_w, rows * px);

    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    for lines in frames {
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new(lines), frame.area()))
            .map_err(|e| e.to_string())?;
    }
    let backend = terminal.backend();
    let size = backend.size().map_err(|e| e.to_string())?;
    assert_eq!((u32::from(size.width), u32::from(size.height)), (cols, rows));
    let pixels = backend.read_pixels().ok_or("the frame could not be read back")?;
    Ok(Frame { pixels, width: cols * cell_w, cell_w, cell_h: px })
}

/// The frame, or a skip: these tests draw on this machine's GPU through a
/// headless surface, and a runner without one (CI's ubuntu and windows
/// boxes) must not go red for a renderer it cannot run — the repo's GPU
/// tests have always stood aside there. Any other failure is a failure.
fn frame_or_skip<T>(rendered: Result<T, String>) -> Option<T> {
    match rendered {
        Ok(frame) => Some(frame),
        Err(e) if e.starts_with("no headless wgpu backend") => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// One GPU at a time: the test harness runs tests on several threads, and
/// on a Windows box with an NVIDIA driver one test's Vulkan device going
/// down inside the driver while another's instance brings a WGL context up
/// in the same DLL never came back (the Windows run of 2026-10-01: four
/// hangs in four runs, every test passing on its own). The lock is poisoned
/// by a panicking test, which is no reason for the next to fail.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

/// A system face with Japanese in it, as the window would borrow for `ja`,
/// with where it came from; `None` on a system without one.
fn japanese_face() -> Option<(String, &'static [u8], u32)> {
    cjk_faces("ja").into_iter().find_map(|(path, index)| {
        let bytes = std::fs::read(&path).ok()?;
        // Leaked as the window leaks its faces: a face borrows its bytes.
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        Font::new_at(bytes, index)?;
        Some((path.display().to_string(), bytes, index))
    })
}

/// The reported wide-glyph shift: after CJK in a row, the cells that follow
/// were seen drawn about two cells left. `AB日本CD` puts 日 in cells 2–3
/// and 本 in 4–5, so C belongs in cell 6. A row with blanks where the CJK
/// is draws C and D where they belong by construction; the CJK row's C and
/// D cells must be those cells pixel for pixel, and nothing may follow D.
/// Checked on a first frame, and on rows that became and stopped being CJK
/// since the frame before, which reach the backend as partial updates.
#[test]
fn a_wide_glyph_leaves_the_cells_after_it_in_place() {
    let _gpu = one_at_a_time();
    let japanese = japanese_face();
    match &japanese {
        Some((path, _, index)) => eprintln!("CJK face: {path} (face {index})"),
        // Without a CJK face 日本 draw as Hack's empty-glyph boxes, still
        // two cells each, so the layout half of the check still holds.
        None => eprintln!("no system CJK face: 日本 draw as boxes, the ink check is skipped"),
    }
    let faces = || {
        let mut faces = vec![hack().unwrap()];
        faces.extend(japanese.as_ref().and_then(|(_, bytes, index)| Font::new_at(bytes, *index)));
        faces
    };
    let reference = || Line::from("AB    CD");
    let cases = [
        ("a first frame", vec![vec![Line::from("AB日本CD"), reference()]]),
        (
            "a row turned CJK",
            vec![
                vec![Line::from("ABEFGHCD"), reference()],
                vec![Line::from("AB日本CD"), reference()],
            ],
        ),
        (
            "a row turned CJK after another CJK",
            vec![
                vec![Line::from("A日本語CD"), reference()],
                vec![Line::from("AB日本CD"), reference()],
            ],
        ),
    ];
    for (case, frames) in cases {
        let Some(frame) = frame_or_skip(render(faces(), 20, frames, TextureFormat::Rgba8Unorm)) else { return };
        for col in [0, 1, 6, 7] {
            assert!(frame.inked(col, 1), "{case}: the reference row has no glyph in cell {col}");
            assert!(
                frame.cell(col, 0) == frame.cell(col, 1),
                "{case}: cell {col} of `AB日本CD` is not the glyph the reference row has there"
            );
        }
        for col in 8..20 {
            assert!(!frame.inked(col, 0), "{case}: ink in cell {col}, after D: the row is shifted");
        }
        if japanese.is_some() {
            for col in 2..6 {
                assert!(frame.inked(col, 0), "{case}: 日本 left cell {col} empty");
            }
        }
    }

    // And back: a CJK row turned ASCII must lose every trace of the CJK.
    let Some(frame) = frame_or_skip(render(
        faces(),
        20,
        vec![
            vec![Line::from("AB日本CD"), reference()],
            vec![Line::from("ABEFGHCD"), Line::from("ABEFGHCD")],
        ],
        TextureFormat::Rgba8Unorm,
    )) else { return };
    for col in 0..20 {
        assert!(
            frame.cell(col, 0) == frame.cell(col, 1),
            "a row turned ASCII: cell {col} still differs from a row drawn ASCII from the start"
        );
    }
}

/// The colours the GUI asks for are the bytes the surface holds, whether
/// the surface is linear (bytes pass through the blit) or sRGB (the blit
/// decodes and the store re-encodes, which must round-trip): the ground and
/// gold to within one level.
#[test]
fn colours_reach_the_surface_unchanged() {
    let _gpu = one_at_a_time();
    for format in [
        TextureFormat::Rgba8Unorm,
        TextureFormat::Rgba8UnormSrgb,
        TextureFormat::Bgra8Unorm,
        TextureFormat::Bgra8UnormSrgb,
    ] {
        let gold = Style::new().bg(Color::Rgb(GOLD[0], GOLD[1], GOLD[2]));
        let line = Line::from(vec![Span::raw("  "), Span::styled("  ", gold)]);
        let Some(frame) = frame_or_skip(render(vec![hack().unwrap()], 4, vec![vec![line]], format)) else { return };
        for (col, want) in [(0, GROUND), (2, GOLD)] {
            let got = frame.pixel(col * frame.cell_w + 1, 1);
            let off = got.iter().zip(want).map(|(&g, w)| g.abs_diff(w)).max().unwrap();
            assert!(
                off <= 1,
                "{format:?}: cell {col} reads back {got:02x?}, wanted {want:02x?}"
            );
        }
    }
}

/// A collection's later faces open by index, and are faces of their own.
#[test]
fn a_collection_opens_at_any_face() {
    let _gpu = one_at_a_time();
    let Some((_, bytes)) = cjk_faces("zh")
        .into_iter()
        .chain(cjk_faces("ja"))
        .filter_map(|(path, _)| Some((path.clone(), std::fs::read(path).ok()?)))
        .find(|(_, bytes)| bytes.starts_with(b"ttcf"))
    else {
        eprintln!("no font collection on this system: nothing to open");
        return;
    };
    let count = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
    assert!(Font::new(&bytes).is_some());
    assert!(Font::new_at(&bytes, count - 1).is_some(), "the last of {count} faces would not open");
    assert!(Font::new_at(&bytes, count).is_none(), "a face past the end opened");
}

/// Kana, hanzi and hangul draw in an English window, through the faces the
/// window borrows for every language: each glyph's cells are inked and
/// differ from what Hack alone draws there (its box for a glyph it lacks).
/// The borrowed faces must not narrow the cell either: the backend's cell
/// is its narrowest face's, and the grid is Hack's.
#[test]
fn every_script_draws_in_an_english_window() {
    let _gpu = one_at_a_time();
    let text = "日本語テスト한국어简体中文";
    let row = || vec![vec![Line::from(text)]];
    let Some(alone) = frame_or_skip(render(vec![hack().unwrap()], 30, row(), TextureFormat::Rgba8Unorm)) else { return };
    let scripts = script_fallbacks("en");
    if scripts.is_empty() {
        eprintln!("no system CJK face: the scripts draw as boxes, nothing to check");
        return;
    }
    let mut faces = vec![hack().unwrap()];
    faces.extend(symbol_fallback());
    faces.extend(scripts);
    let Some(frame) = frame_or_skip(render(faces, 30, row(), TextureFormat::Rgba8Unorm)) else { return };
    assert_eq!(frame.cell_w, alone.cell_w, "a borrowed face narrowed the cell");
    for (i, ch) in text.chars().enumerate() {
        // Every one of them is wide: two cells each.
        let (left, right) = (i as u32 * 2, i as u32 * 2 + 1);
        assert!(frame.inked(left, 0) || frame.inked(right, 0), "{ch} left its cells empty");
        let differs = |col| frame.cell(col, 0) != alone.cell(col, 0);
        assert!(differs(left) || differs(right), "{ch} draws as Hack's box");
    }
}

/// Where one glyph's ink lies in its box of `cols` cells starting at `col`
/// on row 0: the inked pixels' least and greatest x and y, relative to the
/// box; `None` if nothing is inked.
fn ink_extent(frame: &Frame, col: u32, cols: u32) -> Option<([u32; 2], [u32; 2])> {
    let (x0, w) = (col * frame.cell_w, cols * frame.cell_w);
    let mut extent: Option<([u32; 2], [u32; 2])> = None;
    for y in 0..frame.cell_h {
        for x in 0..w {
            let px = frame.pixel(x0 + x, y);
            if px.iter().zip(GROUND).all(|(&got, ground)| got.abs_diff(ground) <= 40) {
                continue;
            }
            let ([x_lo, x_hi], [y_lo, y_hi]) = extent.get_or_insert(([x, x], [y, y]));
            (*x_lo, *x_hi) = ((*x_lo).min(x), (*x_hi).max(x));
            (*y_lo, *y_hi) = ((*y_lo).min(y), (*y_hi).max(y));
        }
    }
    extent
}

/// A borrowed face's glyph draws whole inside its two cells. The backend
/// fits each face's line to the cell's height; it used to enlarge a glyph
/// whose advance was narrower than its box until the advance filled it,
/// which grew the line past the cell, and the raster is clipped to the box:
/// Apple SD Gothic Neo's hangul (865 units wide on a 1200-unit line) lost
/// their tops and their right-hand strokes, so 한 read as 힌. Every glyph
/// must keep the box's first and last rows clear, and a hangul syllable,
/// narrower than its box, sits in the middle of it with both edge columns
/// clear. Checked at the tests' size and at the window's (32 px, scale 2).
#[test]
fn a_borrowed_glyph_fits_its_box() {
    let _gpu = one_at_a_time();
    let text = "日本語テスト한국어简体中文";
    if script_fallbacks("en").is_empty() {
        eprintln!("no system CJK face: the scripts draw as boxes, nothing to check");
        return;
    }
    for px in [PX, 32] {
        let mut faces = vec![hack().unwrap()];
        faces.extend(script_fallbacks("en"));
        let row = vec![vec![Line::from(text)]];
        let Some(frame) = frame_or_skip(render_at(px, faces, 30, row, TextureFormat::Rgba8Unorm)) else { return };
        let (box_w, box_h) = (2 * frame.cell_w, frame.cell_h);
        for (i, ch) in text.chars().enumerate() {
            let Some(([left, right], [top, bottom])) = ink_extent(&frame, i as u32 * 2, 2) else {
                panic!("{px} px: {ch} left its cells empty");
            };
            eprintln!("{px} px: {ch} inked x {left}..={right}, y {top}..={bottom}");
            assert!(top > 0, "{px} px: {ch} reaches the box's top row: it is cut off there");
            assert!(bottom < box_h - 1, "{px} px: {ch} reaches the box's bottom row");
            if ('\u{ac00}'..='\u{d7a3}').contains(&ch) {
                assert!(right < box_w - 1, "{px} px: {ch} reaches the box's last column");
                let (before, after) = (left, box_w - 1 - right);
                assert!(
                    before.abs_diff(after) <= 2,
                    "{px} px: {ch} is off centre: {before} px clear before it, {after} after"
                );
            }
        }
    }
}

/// A wide glyph replaced by narrow ones leaves no residue and moves
/// nothing: the reported trace at a field's right edge, and its border
/// shifted, after CJK text in it was edited. `日本` fills cells 0..3; the
/// next frame puts `ab` in cells 0 and 1 and blanks after, and every cell
/// must be what a fresh frame of `ab` draws. ratatui's diff sends the
/// blank of cell 2 (it differs from 本) but not of cell 3, which was the
/// wide glyph's continuation and is a blank in both buffers: a terminal
/// erased all of 本 when cell 2 was written. The backend kept cell 3 as
/// the empty continuation, which shapes to nothing, so whatever followed
/// on the row drew one cell left: `│日本│` redrawn as `│ab  │` put its
/// last `│` in cell 4 (VENDORED.md, change 10). Also a wide glyph moved
/// by one cell, and one replaced by narrow text and a blank.
#[test]
fn a_wide_glyph_narrowed_leaves_no_residue() {
    let _gpu = one_at_a_time();
    let japanese = japanese_face();
    let faces = || {
        let mut faces = vec![hack().unwrap()];
        faces.extend(japanese.as_ref().and_then(|(_, bytes, index)| Font::new_at(bytes, *index)));
        faces
    };
    let cases = [
        ("日本", "ab"),
        ("│日本│", "│ab  │"),
        ("日本│", "ab  │"),
        ("日本", "a日"),
        ("日本語│", "a b   │"),
    ];
    for (before, after) in cases {
        let frames = vec![vec![Line::from(before)], vec![Line::from(after)]];
        let Some(frame) = frame_or_skip(render(faces(), 20, frames, TextureFormat::Rgba8Unorm))
        else {
            return;
        };
        let fresh = vec![vec![Line::from(after)]];
        let Some(fresh) = frame_or_skip(render(faces(), 20, fresh, TextureFormat::Rgba8Unorm))
        else {
            return;
        };
        for col in 0..20 {
            assert!(
                frame.cell(col, 0) == fresh.cell(col, 0),
                "{before:?} then {after:?}: cell {col} is not what a fresh {after:?} draws there"
            );
        }
    }
    // The case as reported, by ink: nothing of 本 after `ab`.
    let frames = vec![vec![Line::from("日本")], vec![Line::from("ab")]];
    let Some(frame) = frame_or_skip(render(faces(), 20, frames, TextureFormat::Rgba8Unorm)) else {
        return;
    };
    for col in 2..20 {
        assert!(!frame.inked(col, 0), "residue of 本 in cell {col}");
    }
}

/// An emoji that grows wide in place draws as a fresh one does, and keeps the rest of its row in
/// place. Typing ❤ then its VS16 into a field turns `❤▏` into `❤️▏`: the heart's cell becomes
/// two wide, and ratatui's diff sends the cell it now covers as a blank (its clear for terminals
/// that leave a VS16 emoji's second half behind). The backend wrote that blank over the
/// continuation, which then shaped as a cell of its own, so everything after the heart on the
/// row drew one cell right: the live window's search box put its right border a cell past the
/// box's corner. And the heart itself drew nothing, its second cell keeping the caret: the wide
/// heart and the narrow one were one entry in the backend's placements, which the narrow one's
/// removal took away (VENDORED.md, change 18). Also a VS16 emoji over two narrow cells, a
/// keycap grown in place, and a heart narrowed back (its VS16 deleted).
#[test]
fn an_emoji_widened_in_place_keeps_its_row_in_place() {
    let _gpu = one_at_a_time();
    let Some(faces) = emoji_faces() else { return };
    let cases = [
        ("\u{2764}\u{258F}|", "\u{2764}\u{FE0F}\u{258F}|"),
        ("\u{2764}x|", "\u{2764}\u{FE0F}|"),
        ("ab|", "\u{2764}\u{FE0F}|"),
        ("1x2|", "1\u{FE0F}\u{20E3}2|"),
        ("\u{2764}\u{FE0F}\u{258F}|", "\u{2764}\u{258F}|"),
    ];
    for (before, after) in cases {
        let frames = vec![vec![Line::from(before)], vec![Line::from(after)]];
        let Some(frame) = frame_or_skip(render(faces.clone(), 8, frames, TextureFormat::Rgba8Unorm))
        else {
            return;
        };
        let Some(fresh) = row(&faces, 8, after) else { return };
        for col in 0..8 {
            assert!(
                frame.cell(col, 0) == fresh.cell(col, 0),
                "{before:?} then {after:?}: cell {col} is not what a fresh {after:?} draws there"
            );
        }
    }
}

/// Whether a pixel is coloured rather than grey: its channels apart by more
/// than a grey's (white, the ground, and everything blended between them,
/// are within a few levels of each other).
fn coloured(px: [u8; 3]) -> bool {
    px.iter().max().unwrap() - px.iter().min().unwrap() > 40
}

/// One row of `line` drawn `cols` wide in `faces` at the tests' size, or a
/// skip ([`frame_or_skip`]).
fn row(faces: &[Font<'_>], cols: u32, line: impl Into<Line<'static>>) -> Option<Frame> {
    let frames = vec![vec![line.into()]];
    frame_or_skip(render(faces.to_vec(), cols, frames, TextureFormat::Rgba8Unorm))
}

/// The GUI's own symbols draw from the bundled face with no system face at
/// all, each inside its one cell, and not as Hack's box for a glyph it
/// lacks. The window used to borrow them from the system (Menlo here,
/// DejaVu on Linux, nothing on a bare Windows), so this is what every
/// platform now draws.
#[test]
fn the_symbols_draw_from_the_bundled_face_alone() {
    let _gpu = one_at_a_time();
    let text = super::BEYOND_HACK;
    let Some(alone) = row(&[hack().unwrap()], 8, text) else { return };
    let Some(frame) = row(&[hack().unwrap(), symbols().unwrap()], 8, text) else { return };
    assert_eq!(frame.cell_w, alone.cell_w, "the bundled face changed the cell");
    for (col, ch) in text.chars().enumerate() {
        let col = col as u32;
        let Some(([left, right], [top, bottom])) = ink_extent(&frame, col, 1) else {
            panic!("{ch} left its cell empty");
        };
        let (w, h) = (frame.cell_w, frame.cell_h);
        eprintln!("{ch}: inked x {left}..={right}, y {top}..={bottom} of {w}x{h}");
        assert!(frame.cell(col, 0) != alone.cell(col, 0), "{ch} draws as Hack's box");
        assert!(top > 0 && bottom < h - 1, "{ch} touches the cell's top or bottom");
    }
    for col in text.chars().count() as u32..8 {
        assert!(!frame.inked(col, 0), "ink in cell {col}, after the symbols");
    }
}

/// A hangul syllable, a kana and a hanzi side by side are one size: every
/// borrowed face is drawn at Hack's pixels per em (VENDORED.md, change 15).
/// Each face used to be fitted by its own line, so Apple SD Gothic Neo's
/// hangul (a 1200-unit line) drew at 23 px beside Hiragino's kana (a
/// 1000-unit line) at 27 to 30, in a 32 px cell. The ink heights must be
/// within 20% of each other, and each glyph inside its box.
#[test]
fn hangul_kana_and_hanzi_draw_at_one_size() {
    let _gpu = one_at_a_time();
    let text = "한か日";
    if script_fallbacks("en").len() < 3 {
        eprintln!("skipped: this system lacks a face for kana, hanzi or hangul");
        return;
    }
    for px in [PX, 32] {
        let mut faces = vec![hack().unwrap()];
        faces.extend(script_fallbacks("en"));
        let lines = vec![vec![Line::from(text)]];
        let drawn = render_at(px, faces, 8, lines, TextureFormat::Rgba8Unorm);
        let Some(frame) = frame_or_skip(drawn) else { return };
        let heights: Vec<u32> = text
            .chars()
            .enumerate()
            .map(|(i, ch)| {
                let Some(([left, right], [top, bottom])) = ink_extent(&frame, i as u32 * 2, 2)
                else {
                    panic!("{px} px: {ch} left its cells empty");
                };
                let tall = bottom - top + 1;
                eprintln!("{px} px: {ch} inked x {left}..={right}, y {top}..={bottom}: {tall}");
                let inside = top > 0 && bottom < frame.cell_h - 1;
                assert!(inside, "{px} px: {ch} touches its box's edge");
                tall
            })
            .collect();
        let (low, high) = (*heights.iter().min().unwrap(), *heights.iter().max().unwrap());
        // 20%, not tighter: the faces' designs differ though their em is
        // one. On Windows Malgun Gothic's hangul inks 20 px beside Yu
        // Gothic's kana at 17 px in 24 px type (CI, run 37028415435); the
        // per-line fit this guards against was 23 against 30, past it.
        assert!(
            f64::from(high) <= f64::from(low) * 1.20,
            "{px} px: {text} ink heights {heights:?} differ by more than 20% (one em for \
             every face holds them within it; past that is the per-line fit come back)"
        );
    }
}

/// The faces an emoji test draws with: Hack, the bundled symbols, and the
/// system's colour emoji face; `None`, with a line saying so, on a system
/// without one (CI's ubuntu has none). `MSTREAM_TEST_EMOJI_FACE=<file>`
/// draws them with that face instead, so a Mac can check a Noto Color
/// Emoji (CBDT) or a Segoe UI Emoji (COLR) copied from elsewhere.
fn emoji_faces() -> Option<Vec<Font<'static>>> {
    let chosen = std::env::var_os("MSTREAM_TEST_EMOJI_FACE").map(std::path::PathBuf::from);
    let found = match chosen {
        Some(path) => {
            let bytes = std::fs::read(&path).expect("MSTREAM_TEST_EMOJI_FACE is not a file");
            Some((path, &*Box::leak(bytes.into_boxed_slice())))
        }
        None => emoji_face(),
    };
    let Some((path, bytes)) = found else {
        eprintln!("skipped: no colour emoji face on this system");
        return None;
    };
    eprintln!("emoji face: {}", path.display());
    let Some(emoji) = Font::new(bytes) else {
        eprintln!("skipped: emoji face failed to parse: {}", path.display());
        return None;
    };
    Some(vec![hack().unwrap(), symbols().unwrap(), emoji])
}

/// England's flag: the black flag and the tags for "gbeng", closed by
/// CANCEL TAG.
const ENGLAND: &str = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";

/// The rainbow flag: a white flag with VS16, ZWJ a rainbow.
const RAINBOW: &str = "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}";

/// Every kind of emoji sequence draws as one picture over the two cells
/// its width claims, in colour, with nothing in the cell after it: an
/// emoji with VS16, a flag of regional indicators, a subdivision flag of
/// tags, a ZWJ flag (the rainbow flag), a ZWJ family and a skin tone. The
/// family is not asked for colour: Noto Color Emoji's Emoji 15.1 redesign
/// draws it as grey silhouettes. Before: the VS16 heart and the flag
/// were squeezed into the first of their two cells (their box was measured
/// by the first character), and a sequence the face could not join drew
/// its second emoji in the next cell. Each is also a picture of its own,
/// not its base emoji's: the face joined the sequence.
///
/// Faces differ in which sequences they join: Segoe UI Emoji (CI's
/// windows-latest) has no country flags, so 🇺🇸 there is its first
/// regional indicator alone. A case the face does not join
/// ([`Font::joins`]) is skipped with a line, keeping only what holds for
/// any face: ink in its own cells and nothing in the cells after. England's
/// flag alone may also be joined yet drawn as its base: Segoe UI Emoji has
/// a glyph for it and no picture in it (CI run 37028415435), and the base 🏴
/// is the degradation the window specifies — never a box, never empty
/// cells. Any other joined case drawn as its base is the renderer drawing a
/// cluster's first character, and fails; and at least one sequence of
/// several emoji must be drawn joined, so a skip cannot hide that on every
/// case at once.
#[test]
fn emoji_sequences_draw_as_one_picture_in_their_cells() {
    let _gpu = one_at_a_time();
    let Some(faces) = emoji_faces() else { return };
    let emoji = faces.last().unwrap();
    // The fourth field: whether the picture is in colour in every colour
    // face (the family is grey in Noto Color Emoji since Emoji 15.1). The
    // fifth: whether a face may join it and still draw only its base
    // (England's flag in Segoe UI Emoji).
    let cases = [
        ("VS16 heart", "\u{2764}\u{FE0F}", "\u{2764}\u{FE0F}", true, false),
        ("flag", "\u{1F1FA}\u{1F1F8}", "\u{1F1FA}", true, false),
        ("subdivision flag", ENGLAND, "\u{1F3F4}", true, true),
        ("ZWJ rainbow flag", RAINBOW, "\u{1F3F3}\u{FE0F}", true, false),
        ("ZWJ family", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", "\u{1F468}", false, false),
        ("skin tone", "\u{1F44D}\u{1F3FD}", "\u{1F44D}", true, false),
    ];
    // Every colour face joins some of them (the heart's VS16 at least); none
    // joined would be the shaping or the face lookup broken, not coverage.
    let joined = cases.iter().filter(|(_, text, ..)| emoji.joins(text)).count();
    assert!(joined > 0, "the emoji face joined none of the sequences");
    // The sequences of several emoji drawn as one picture, not as their
    // base: `joins` is the shaper's word, this is the pixels'.
    let mut drawn_joined = 0;
    let Some(bar) = row(&faces, 6, "  |") else { return };
    for (case, text, base, in_colour, may_draw_base) in cases {
        assert_eq!(crate::kit::width(text), 2, "{case}: the GUI's width rule");
        let Some(frame) = row(&faces, 6, format!("{text}|")) else { return };
        let Some(([left, right], [top, bottom])) = ink_extent(&frame, 0, 2) else {
            panic!("{case}: its two cells are empty");
        };
        let (w, h) = (2 * frame.cell_w, frame.cell_h);
        eprintln!("{case}: inked x {left}..={right}, y {top}..={bottom} of {w}x{h}");
        // The `|` after it is in cell 2, where a fresh `|` draws it, and
        // nothing is past it: true of a sequence degraded to its base too.
        assert!(frame.cell(2, 0) == bar.cell(2, 0), "{case}: cell 2 is not the `|` alone");
        for col in 3..6 {
            assert!(!frame.inked(col, 0), "{case}: ink in cell {col}");
        }
        if !emoji.joins(text) {
            eprintln!("skipped {case}: the emoji face has no one picture for it");
            continue;
        }
        // Both cells first: a face that joined the sequence and drew a picture into
        // one cell only is a defect, whatever the base comparison below says.
        assert!(frame.inked(0, 0) && frame.inked(1, 0), "{case}: not over both of its cells");
        if base != text {
            let Some(alone) = row(&faces, 6, format!("{base}|")) else { return };
            if (0..2).all(|col| frame.cell(col, 0) == alone.cell(col, 0)) {
                // Joined, yet drawn as its base alone. For England's flag
                // that is a face whose glyph has no picture (Segoe UI
                // Emoji), and the base is the specified degradation; the
                // empty cells it must never be are the panic above.
                assert!(may_draw_base, "{case}: drew only its base: the face did not join it");
                eprintln!(
                    "skipped {case}: the face joined it but has no picture for it (drew the base)"
                );
                continue;
            }
            drawn_joined += 1;
        }
        if in_colour {
            let pixels = frame.cell(0, 0).into_iter().chain(frame.cell(1, 0));
            let colour = pixels.filter(|&px| coloured(px)).count();
            assert!(colour > 0, "{case}: no colour in its cells");
        }
    }
    // Every colour face CI meets (Apple, Noto, Segoe) has the family and the
    // skin tones; drawing none of them joined is the renderer, not the face.
    assert!(drawn_joined > 0, "no sequence of several emoji drew as one picture");
}

/// A sequence no face joins degrades to its base emoji in its own two
/// cells: a man ZWJ a dinosaur is one grapheme two cells wide, which no
/// emoji face has a picture for, so it shapes to two emoji, and only the
/// first is drawn. Before, the dinosaur drew in the next two cells, over
/// what was there.
///
/// Also a pair of regional indicators that is no country (A A): Apple
/// Color Emoji draws it as Segoe UI Emoji draws every flag, having none (its
/// first letter alone), so this is that path on a Mac; Noto Color Emoji has
/// one picture for any unknown pair. Either way: ink in its own two cells,
/// none after.
#[test]
fn an_unjoined_sequence_draws_its_base_and_nothing_after() {
    let _gpu = one_at_a_time();
    let Some(faces) = emoji_faces() else { return };
    let emoji = faces.last().unwrap();
    let no_country = "\u{1F1E6}\u{1F1E6}";
    assert_eq!(crate::kit::width(no_country), 2);
    if emoji.joins(no_country) {
        eprintln!("the emoji face has one picture for an unknown flag");
    } else {
        eprintln!("the emoji face has no picture for an unknown flag: its first letter alone");
    }
    let Some(frame) = row(&faces, 6, format!("{no_country}|")) else { return };
    let Some(bar) = row(&faces, 6, "  |") else { return };
    assert!(ink_extent(&frame, 0, 2).is_some(), "the unknown flag's cells are empty");
    assert!(frame.cell(2, 0) == bar.cell(2, 0), "the unknown flag drew into cell 2");
    for col in 3..6 {
        assert!(!frame.inked(col, 0), "the unknown flag drew into cell {col}");
    }
    let text = "\u{1F468}\u{200D}\u{1F996}";
    assert_eq!(crate::kit::width(text), 2);
    assert!(!emoji.joins(text), "the emoji face has a picture for a man ZWJ a dinosaur");
    let Some(frame) = row(&faces, 6, text) else { return };
    let Some(alone) = row(&faces, 6, "\u{1F468}") else { return };
    for col in 0..6 {
        assert!(frame.cell(col, 0) == alone.cell(col, 0), "cell {col} is not the man alone");
    }
    for col in 2..6 {
        assert!(!frame.inked(col, 0), "ink in cell {col}: the dinosaur overdrew the next cells");
    }
}

/// A symbol an emoji face also has keeps its text form from the bundled
/// face, in the cell's colour: the heavy tick (U+2714) is an emoji
/// character, but with no VS16 it is the bundled face's monochrome tick,
/// drawn gold in a gold cell, not a colour picture and not white.
#[test]
fn a_text_symbol_takes_the_cells_colour_not_the_emoji_faces() {
    let _gpu = one_at_a_time();
    let Some(faces) = emoji_faces() else { return };
    let gold = Style::new().fg(Color::Rgb(GOLD[0], GOLD[1], GOLD[2]));
    let Some(frame) = row(&faces, 4, Span::styled("\u{2714}", gold)) else { return };
    let ink: Vec<[u8; 3]> = frame
        .cell(0, 0)
        .into_iter()
        .filter(|px| px.iter().zip(GROUND).any(|(&got, ground)| got.abs_diff(ground) > 40))
        .collect();
    assert!(!ink.is_empty(), "the heavy tick left its cell empty");
    let fullest = ink.iter().map(|px| px[0]).max().unwrap();
    let gold_red = GOLD[0];
    assert!(fullest.abs_diff(gold_red) <= 2, "the tick's ink is {fullest:02x}, not {gold_red:02x}");
    assert!(ink.iter().all(|px| px[2] < 0xd0), "the tick has white ink: drawn as colour");
}

/// A pasted England flag shows in a field as one picture: the kit's field
/// line (the caret after the value, as the search box draws it) keeps 🏴
/// and its tags together, measures them as the two cells ratatui gives the
/// grapheme, and the window draws one flag over those cells with the caret
/// in the cell after them. The kit counts a tag as no cells (unicode-width
/// gives each zero), so its caret offset agrees with the drawing.
#[test]
fn a_pasted_subdivision_flag_shows_in_a_field_as_one_picture() {
    let _gpu = one_at_a_time();
    let Some(faces) = emoji_faces() else { return };
    let value = format!("ab{ENGLAND}");
    let cursor = value.chars().count();
    let (line, caret) = crate::kit::input_display_composing(&value, cursor, 20, true, "", None);
    assert!(line.contains(ENGLAND), "the field's line split the flag: {line:?}");
    assert_eq!(caret, 4, "the caret is not in the cell after the flag's two");
    let Some(frame) = row(&faces, 8, line) else { return };
    let Some(flag) = row(&faces, 8, value) else { return };
    for col in 0..4 {
        assert!(frame.cell(col, 0) == flag.cell(col, 0), "cell {col} is not the flag drawn alone");
    }
    // Only a face with the flag's picture is sure to cover both cells; one
    // without (Segoe UI Emoji has no subdivision flags) draws the black
    // flag alone, whose ink is the face's to place.
    if faces.last().unwrap().joins(ENGLAND) {
        assert!(frame.inked(2, 0) && frame.inked(3, 0), "the flag is not over both its cells");
    } else {
        eprintln!("the emoji face has no England flag: drawn as its black flag");
        assert!(frame.inked(2, 0) || frame.inked(3, 0), "the flag's cells are empty");
    }
    assert!(frame.inked(4, 0), "no caret in the cell after the flag");
    for col in 5..8 {
        assert!(!frame.inked(col, 0), "ink in cell {col}, past the caret");
    }
}

/// Tags with no 🏴 before them draw nothing of their own: the cell is its first character
/// alone. A field that scrolled used to clip a pasted England flag between its 🏴 and its tags,
/// which hung on the clip mark's cell, and a caret moved in between the 🏴 and its tags has
/// them hang on the caret's: the renderer gave such a cell to the emoji face, the only face
/// with the tags, which has no `…`, `▏` or `b` and drew a box over the cell. The base's face
/// now draws it (VENDORED.md, change 17), and the field no longer cuts the flag at its clip
/// (`kit::input_window`), so its line begins with the clip and the text.
#[test]
fn a_stray_tag_run_draws_as_its_base_alone() {
    let _gpu = one_at_a_time();
    let Some(faces) = emoji_faces() else { return };
    let tags = &ENGLAND['\u{1F3F4}'.len_utf8()..];
    for base in ["\u{2026}", "\u{258F}", "b"] {
        let Some(frame) = row(&faces, 4, format!("{base}{tags}b")) else { return };
        let Some(alone) = row(&faces, 4, format!("{base}b")) else { return };
        assert!(frame.inked(0, 0), "{base} with tags left its cell empty");
        for col in 0..4 {
            assert!(frame.cell(col, 0) == alone.cell(col, 0), "{base} with tags: cell {col}");
        }
    }
    // The reported field: 🏴 does not fit beside the clip, and the tags are not shown either.
    let value = format!("aaaa{ENGLAND}bbbbbbbbbb");
    let cursor = value.chars().count();
    let (line, _) = crate::kit::input_display_composing(&value, cursor, 13, true, "", None);
    assert!(!line.contains(tags) && !line.contains('\u{E007F}'), "the field split the flag");
    let clip = line.chars().next().unwrap();
    let Some(frame) = row(&faces, 14, line) else { return };
    let Some(alone) = row(&faces, 14, clip.to_string()) else { return };
    assert!(frame.cell(0, 0) == alone.cell(0, 0), "the clip's cell is not the clip alone");
}

/// One frame through the window's own post processor (covers.rs): two
/// solid magenta covers, `under` and `beside`, placed through the hosted
/// `Graphics` as the GUI's draw sites place theirs, and then — later in the
/// same frame, as the GUI draws its overlays last — a 20×8 modal opened
/// through the kit on a surface the board watches (`watched`) or not.
/// Returns the frame and the two covers' cell rects.
fn covers_and_a_modal(watched: bool) -> Result<(Frame, Rect, Rect), String> {
    use std::sync::Arc;

    use super::covers::{Board, CoverPost};
    use crate::kit::Surface;
    use crate::tui::art::Art;
    use crate::tui::graphics::Graphics;

    let (cols, rows) = (40u32, 12u32);
    let board = Arc::new(Board::default());
    let mut surface: Surface<()> = Surface::new();
    if watched {
        let watching = board.clone();
        surface.watch_overlays(move |rect| watching.overlay(rect));
    }
    let wide = 4096;
    let builder = Builder::<CoverPost>::from_font_and_user_data(hack()?, board.clone())
        .with_font_size_px(PX)
        .with_width_and_height(Dimensions {
            width: NonZeroU32::new(wide).unwrap(),
            height: NonZeroU32::new(rows * PX).unwrap(),
        })
        .with_bg_color(Color::Rgb(GROUND[0], GROUND[1], GROUND[2]))
        .with_fg_color(Color::White);
    let mut backend = block_on(builder.build_headless())?
        .map_err(|e| format!("no headless wgpu backend: {e}"))?;
    let reported = backend.window_size().map_err(|e| e.to_string())?;
    let cell_w = wide / u32::from(reported.columns_rows.width);
    backend.resize(cols * cell_w, rows * PX);

    let grid = Rect::new(0, 0, cols as u16, rows as u16);
    let modal = crate::kit::modal_rect(grid, 20, 8, 8);
    let under = Rect { x: modal.x + 2, y: modal.y + 2, width: 6, height: 3 };
    let beside = Rect { x: 1, y: modal.y + 2, width: 6, height: 3 };
    assert!(!beside.intersects(modal) && modal.contains(under.as_position()));
    let art = Art::from_rgb(4, 4, [255, 0, 255].repeat(16)).ok_or("no art")?;
    let mut graphics = Graphics::hosted(board.clone());

    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    board.begin_frame();
    surface.begin_frame();
    terminal
        .draw(|frame| {
            graphics.draw(frame, under, &art);
            graphics.draw(frame, beside, &art);
            crate::kit::modal_frame_on(frame, &mut surface, grid, 20, 8, Color::White);
        })
        .map_err(|e| e.to_string())?;
    let pixels = terminal.backend().read_pixels().ok_or("the frame could not be read back")?;
    Ok((Frame { pixels, width: cols * cell_w, cell_w, cell_h: PX }, under, beside))
}

/// The cover flash. A draw site asks whether an overlay stood over its
/// cover LAST frame, so on the frame a modal opens it still places the
/// picture, and the post processor painted it over the modal for that
/// frame (10 ms, until the hot frame drew the mosaic). The board now hears
/// the modal register and leaves out the covers placed before it that it
/// touches: no cover pixel under the modal, the one beside it painted. The
/// unwatched surface is the control: the same frame without the fix
/// paints the cover over the modal.
#[test]
fn a_cover_under_a_modal_that_opens_this_frame_is_not_painted_over_it() {
    let _gpu = one_at_a_time();
    let magenta = |frame: &Frame, rect: Rect| {
        let cells = rect.positions().flat_map(|at| frame.cell(at.x.into(), at.y.into()));
        cells.filter(|px| *px == [255, 0, 255]).count()
    };
    let Some((frame, under, beside)) = frame_or_skip(covers_and_a_modal(true)) else { return };
    assert_eq!(magenta(&frame, under), 0, "the cover was painted over the modal");
    // The cover beside is a square fitted in its box: most of the box.
    let (w, h) = (u32::from(beside.width) * frame.cell_w, u32::from(beside.height) * frame.cell_h);
    let side = w.min(h) as usize;
    assert!(magenta(&frame, beside) >= side * side * 9 / 10, "the cover beside was not painted");
    let Some((control, under, _)) = frame_or_skip(covers_and_a_modal(false)) else { return };
    assert!(magenta(&control, under) > 0, "without the watch the flash does not show");
}

/// One frame's drawing, handed the frame and a hosted `Graphics`.
type Draw = Box<dyn FnMut(&mut ratatui::Frame, &mut crate::tui::graphics::Graphics)>;

/// The band's waveform as the window paints it (waves.rs): a headless
/// backend with the window's post processor, a grid of `cols`×`rows` at
/// [`PX`], and `draw` given the frame and a hosted `Graphics` on the board.
fn waved(
    cols: u32,
    rows: u32,
    format: TextureFormat,
    mut draws: Vec<Draw>,
) -> Result<(Frame, Vec<bool>), String> {
    use std::sync::Arc;

    use ratatui_wgpu::PostProcessor;

    use super::covers::{Board, CoverPost};
    use crate::tui::graphics::Graphics;

    let board = Arc::new(Board::default());
    let wide = 4096;
    let builder = Builder::<CoverPost>::from_font_and_user_data(hack()?, board.clone())
        .with_font_size_px(PX)
        .with_width_and_height(Dimensions {
            width: NonZeroU32::new(wide).unwrap(),
            height: NonZeroU32::new(rows * PX).unwrap(),
        })
        .with_bg_color(Color::Rgb(GROUND[0], GROUND[1], GROUND[2]))
        .with_fg_color(Color::White);
    let mut backend = block_on(builder.build_headless_with_format(format))?
        .map_err(|e| format!("no headless wgpu backend: {e}"))?;
    let reported = backend.window_size().map_err(|e| e.to_string())?;
    let cell_w = wide / u32::from(reported.columns_rows.width);
    backend.resize(cols * cell_w, rows * PX);
    let mut graphics = Graphics::hosted(board.clone());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    // After each frame: would the next, placing the same, present?
    let mut wants = Vec::new();
    for draw in &mut draws {
        board.begin_frame();
        terminal.draw(|frame| draw(frame, &mut graphics)).map_err(|e| e.to_string())?;
        wants.push(terminal.backend().post_processor().needs_update());
    }
    let pixels = terminal.backend().read_pixels().ok_or("the frame could not be read back")?;
    Ok((Frame { pixels, width: cols * cell_w, cell_w, cell_h: PX }, wants))
}

/// Peaks in blocks of a hundred bars, loud then quiet: the first eighth of
/// the track loud, the second quiet, and so on.
fn blocky_peaks() -> Vec<u8> {
    (0..800).map(|bar| if (bar / 100) % 2 == 0 { 255 } else { 32 }).collect()
}

/// The blocky peaks as the band's shape for a 30-column bar.
fn blocky_shape() -> crate::tui::graphics::WaveShape {
    crate::tui::ui::wave_shape(&blocky_peaks(), 30)
}

/// The pixel test: the shape follows the peaks, the playhead splits the
/// colours, and nothing leaves the bar's rect — the cell after it keeps its
/// own colour and the rows above and below keep the ground. Glyphs drawn in
/// the rect first are blanked, as the band's are.
fn a_wave_paints_its_peaks_in_its_rect_on(format: TextureFormat) {
    use crate::tui::graphics::WaveLook;

    let _gpu = one_at_a_time();
    const PLAYED: [u8; 3] = [230, 40, 40];
    const UNPLAYED: [u8; 3] = [40, 80, 230];
    const MARK: [u8; 3] = [40, 220, 60];
    const AFTER: [u8; 3] = [200, 200, 0];
    let rect = Rect::new(4, 4, 30, 2);
    let after = Rect::new(34, 4, 1, 2);
    fn rgb([r, g, b]: [u8; 3]) -> Color {
        Color::Rgb(r, g, b)
    }
    let look = move |hover| WaveLook {
        progress: 0.5,
        hover,
        played: rgb(PLAYED),
        unplayed: rgb(UNPLAYED),
        marker: rgb(MARK),
    };
    let frame_with = move |hover: Option<u16>| -> Draw {
        Box::new(move |frame: &mut ratatui::Frame, graphics: &mut crate::tui::graphics::Graphics| {
            let glyphs = Paragraph::new(vec![Line::raw("X".repeat(30)), Line::raw("X".repeat(30))]);
            frame.render_widget(glyphs, rect);
            frame.render_widget(
                Paragraph::new(vec![Line::raw(" "), Line::raw(" ")])
                    .style(Style::new().bg(rgb(AFTER))),
                after,
            );
            assert!(graphics.draw_wave(frame, rect, "t", blocky_shape, look(hover)));
        })
    };
    let draws = vec![frame_with(None), frame_with(None), frame_with(Some(20))];
    let Some((frame, wants)) = frame_or_skip(waved(40, 12, format, draws)) else { return };
    // Each frame presented what it placed: nothing more is owed after it.
    assert_eq!(wants, [false, false, false], "after each present nothing more is owed");

    let near = |px: [u8; 3], want: [u8; 3]| px.iter().zip(want).all(|(&a, b)| a.abs_diff(b) <= 3);
    let (x0, y0) = (u32::from(rect.x) * frame.cell_w, u32::from(rect.y) * frame.cell_h);
    let width = u32::from(rect.width) * frame.cell_w;
    let height = u32::from(rect.height) * frame.cell_h;
    // The ink in one pixel column of the rect, top to bottom.
    let column = |u: f32| -> Vec<[u8; 3]> {
        let x = x0 + (u * width as f32) as u32;
        (y0..y0 + height).map(|y| frame.pixel(x, y)).collect()
    };
    let inked = |u: f32| column(u).iter().filter(|px| !near(**px, GROUND)).count();
    let (loud, quiet) = (inked(0.06), inked(0.19));
    let tall = height as usize * 9 / 10;
    assert!(loud >= tall, "a loud bar reaches the band's edges: {loud} of {height}");
    assert!(quiet > 0 && quiet * 3 < loud, "a quiet bar is a thin line: {quiet} against {loud}");
    // The centre row of each: played left of the playhead, unplayed right.
    let centre = (height / 2) as usize;
    assert!(near(column(0.06)[centre], PLAYED), "{:?}", column(0.06)[centre]);
    assert!(near(column(0.56)[centre], UNPLAYED), "{:?}", column(0.56)[centre]);
    // The glyphs under it were blanked: the quiet bar's top is ground.
    assert!(near(column(0.19)[2], GROUND), "the X under the wave shows: {:?}", column(0.19)[2]);
    // The hover's marker at column 20's left edge, the whole band tall.
    let marker = column(20.0 / 30.0 + 0.5 / width as f32);
    assert!(marker.iter().all(|px| near(*px, MARK)), "the marker: {:?}", &marker[..4]);
    // Nothing outside the rect: the cell after it is its own colour, and
    // the rows above and below are ground.
    let after_px = [4, 5].into_iter().flat_map(|row| frame.cell(u32::from(after.x), row));
    assert!(after_px.into_iter().all(|px| px == AFTER), "the cell after the rect was touched");
    for row in [3, 6] {
        for col in 0..40 {
            assert!(frame.cell(col, row).iter().all(|px| near(*px, GROUND)), "ink at {col},{row}");
        }
    }
}

#[test]
fn a_wave_paints_its_peaks_in_its_rect() {
    a_wave_paints_its_peaks_in_its_rect_on(TextureFormat::Rgba8Unorm);
}

/// The same on a surface that stores sRGB (VENDORED.md change 3's branch):
/// the colours decoded for the store to encode, so they land as the theme's
/// own bytes.
#[test]
fn a_wave_keeps_its_colours_on_an_srgb_surface() {
    a_wave_paints_its_peaks_in_its_rect_on(TextureFormat::Rgba8UnormSrgb);
}

/// A hover move or a seek with no cell changing owes a present; the same
/// wave placed again does not; a playhead that moved less than a pixel
/// does not either. (The bar's cells are blank whatever the wave looks
/// like, so the backend's dirty cells cannot say.)
#[test]
fn a_wave_that_changed_owes_a_present_and_one_that_did_not_does_not() {
    use std::sync::Arc;

    use ratatui_wgpu::PostProcessor;

    use super::covers::{Board, CoverPost};
    use crate::tui::graphics::{Graphics, PictureHost, WaveLook, WaveSpec};

    let _gpu = one_at_a_time();
    let board = Arc::new(Board::default());
    let builder = Builder::<CoverPost>::from_font_and_user_data(hack().unwrap(), board.clone())
        .with_font_size_px(PX)
        .with_width_and_height(Dimensions {
            width: NonZeroU32::new(640).unwrap(),
            height: NonZeroU32::new(10 * PX).unwrap(),
        });
    let built = block_on(builder.build_headless())
        .and_then(|built| built.map_err(|e| format!("no headless wgpu backend: {e}")));
    let Some(backend) = frame_or_skip(built) else { return };
    let mut terminal = Terminal::new(backend).unwrap();
    let mut graphics = Graphics::hosted(board.clone());
    let rect = Rect::new(2, 4, 30, 2);
    let look = |progress: f32, hover| WaveLook {
        progress,
        hover,
        played: Color::Cyan,
        unplayed: Color::DarkGray,
        marker: Color::White,
    };
    let mut key = 0;
    let mut grid = Rect::default();
    board.begin_frame();
    terminal
        .draw(|frame| {
            grid = frame.area();
            assert!(graphics.draw_wave(frame, rect, "t", blocky_shape, look(0.25, None)));
        })
        .unwrap();
    if let Some(wave) = board_waves(&board).first() {
        key = wave.key;
    }
    let wants = |terminal: &Terminal<_>| -> bool {
        let terminal: &Terminal<ratatui_wgpu::WgpuBackend<'_, '_, CoverPost, _>> = terminal;
        terminal.backend().post_processor().needs_update()
    };
    // What the next frame's drawing path would hand the board, without
    // drawing it: the backend asks needs_update before it flushes.
    let place = |look: WaveLook| {
        board.begin_frame();
        board.place_wave(rect, grid, &WaveSpec { key, shape: blocky_shape(), look });
    };
    assert!(!wants(&terminal), "presented: nothing owed");
    place(look(0.25, None));
    assert!(!wants(&terminal), "the same wave again owes nothing");
    place(look(0.2501, None));
    assert!(!wants(&terminal), "a playhead short of a pixel owes nothing");
    place(look(0.25, Some(7)));
    assert!(wants(&terminal), "a hover owes a present");
    place(look(0.75, None));
    assert!(wants(&terminal), "a seek owes a present");
    board.begin_frame();
    assert!(wants(&terminal), "a wave gone owes a present");
}

/// This frame's waves on the board, painted or not.
fn board_waves(board: &super::covers::Board) -> Vec<super::waves::PlacedWave> {
    board.waves_for_tests()
}

/// The window's own type size at a display scale of 2.
const SHOT_PX: u32 = 32;

/// The window's backend, drawing offscreen.
type WindowBackend = ratatui_wgpu::WgpuBackend<
    'static,
    'static,
    super::covers::CoverPost,
    ratatui_wgpu::HeadlessSurface,
>;

/// A headless window renderer, `cols`×`rows` cells at [`SHOT_PX`], built
/// the way the window builds its own (the theme's ground, text and named
/// colours, the post processor on `board`); `None` with no GPU here.
fn window_terminal(
    board: &std::sync::Arc<super::covers::Board>,
    cols: u32,
    rows: u32,
) -> Option<Terminal<WindowBackend>> {
    use super::covers::CoverPost;
    use crate::kit::theme::th;

    let theme = th();
    let mut faces = vec![hack().unwrap(), symbols().unwrap()];
    faces.extend(symbol_fallback());
    let wide = 8192;
    let builder = Builder::<CoverPost>::from_font_and_user_data(hack().unwrap(), board.clone())
        .with_regular_fonts(faces)
        .with_font_size_px(SHOT_PX)
        .with_width_and_height(Dimensions {
            width: NonZeroU32::new(wide).unwrap(),
            height: NonZeroU32::new(rows * SHOT_PX).unwrap(),
        })
        .with_bg_color(theme.ground.unwrap_or(Color::Black))
        .with_fg_color(theme.text)
        .with_color_table(super::named_colours(theme));
    let built = block_on(builder.build_headless())
        .and_then(|built| built.map_err(|e| format!("no headless wgpu backend: {e}")));
    let mut backend = frame_or_skip(built)?;
    let reported = backend.window_size().unwrap();
    let cell_w = wide / u32::from(reported.columns_rows.width);
    backend.resize(cols * cell_w, rows * SHOT_PX);
    Some(Terminal::new(backend).unwrap())
}

/// A Gui on Now Playing with the demo's 6AM playing `progress` of the way
/// through, its server shape `bars`; its covers and waves go to `board`
/// when `hosted`, as the window's do.
fn now_playing_gui(
    board: &std::sync::Arc<super::covers::Board>,
    hosted: bool,
    bars: &[u8],
    progress: f64,
) -> crate::gui::Gui {
    use crate::api::types::{Track, TrackMetadata};
    use crate::gui::{Act, Gui, Screen};
    use crate::tui::app::App;
    use crate::tui::graphics::Graphics;

    let mut gui = Gui::new(crate::config::Config::default(), false, App::new(None, None, None));
    if hosted {
        gui.app.graphics = Graphics::hosted(board.clone());
        let watching = board.clone();
        gui.ui.watch_overlays(move |rect| watching.overlay(rect));
    }
    gui.app.connected = true;
    let duration = 123.0;
    gui.app.now_playing = Some(Track {
        filepath: "library/Boukmanflow/6AM.mp3".into(),
        metadata: TrackMetadata {
            title: Some("6AM".into()),
            artist: Some("Boukmanflow".into()),
            duration: Some(duration),
            ..TrackMetadata::default()
        },
    });
    gui.app.status = crate::player::PlayerStatus {
        playing: true,
        position: progress * duration,
        duration,
        source: "x".into(),
        ..Default::default()
    };
    gui.app.waveforms.insert("library/Boukmanflow/6AM.mp3".into(), Some(bars.to_vec()));
    gui.act(Act::Screen(Screen::NowPlaying));
    gui
}

/// The Auto-DJ tab's sources picker is the TUI's overlay, lent to the
/// GUI's Now Playing screen; it registered no footprint, so a picture under
/// it was painted over it. A long list of libraries reaches the band's top
/// row: on the frame it opens the wave stands down (the board heard the
/// overlay), and from the next the band draws as glyphs.
#[test]
fn the_dj_sources_picker_stands_the_band_down() {
    use std::sync::Arc;

    use super::covers::Board;
    use crate::gui::render;

    let _gpu = one_at_a_time();
    let board = Arc::new(Board::default());
    let Some(mut terminal) = window_terminal(&board, 100, 30) else { return };
    let mut gui = now_playing_gui(&board, true, &blocky_peaks(), 0.35);
    let mut frame = |gui: &mut crate::gui::Gui| {
        board.begin_frame();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        board.placed_waves()
    };
    frame(&mut gui);
    let (painted, under) = frame(&mut gui);
    assert_eq!((painted.len(), under), (1, 0), "the band's wave is painted at rest");
    let libraries = (0..40).map(|i| format!("Library {i}")).collect();
    let picker = crate::tui::app::GenrePicker { all: libraries, ..Default::default() };
    gui.app.dj_panel.sources = Some(picker);
    let (painted, under) = frame(&mut gui);
    assert_eq!((painted.len(), under), (0, 1), "the frame the picker opens: the wave under it");
    let (painted, under) = frame(&mut gui);
    assert_eq!((painted.len(), under), (0, 0), "the next: the band is glyphs");
    gui.app.dj_panel.sources = None;
    frame(&mut gui);
    let (painted, _) = frame(&mut gui);
    assert_eq!(painted.len(), 1, "the picker gone, the wave is back");
}

/// A playing track with no input, the window's most common idle state:
/// the wave's playhead steps with the time label (`ui::label_progress`),
/// on frames whose cells changed anyway, so the painted band presents no
/// more often than the glyph band did. Before, its pixel playhead moved
/// about eight times a second at a hundred columns and every move was a
/// present of its own — five times the glyphs'.
#[test]
fn a_playing_wave_presents_no_more_often_than_the_glyphs() {
    use std::sync::Arc;

    use super::covers::Board;
    use crate::gui::render;
    use crate::kit::theme::th;

    let _gpu = one_at_a_time();
    let mut presents = Vec::new();
    for hosted in [false, true] {
        let board = Arc::new(Board::with_palette(super::named_colours(th())));
        let Some(mut terminal) = window_terminal(&board, 100, 30) else { return };
        let mut gui = now_playing_gui(&board, hosted, &blocky_peaks(), 0.35);
        for _ in 0..3 {
            board.begin_frame();
            terminal.draw(|frame| render(frame, &mut gui)).unwrap();
        }
        let before = processed(&terminal);
        // Six seconds of the GUI's idle pace, the clock 100 ms a frame.
        for _ in 0..60 {
            gui.app.status.position += 0.1;
            board.begin_frame();
            terminal.draw(|frame| render(frame, &mut gui)).unwrap();
            if hosted {
                let (painted, _) = board.placed_waves();
                assert_eq!(painted.len(), 1, "the wave is painted while it plays");
            }
        }
        presents.push(processed(&terminal) - before);
    }
    let (glyphs, wave) = (presents[0], presents[1]);
    assert!(glyphs > 0, "the time label ticked over six seconds");
    assert!(wave <= glyphs, "the wave presented {wave} times, the glyphs {glyphs}");
}

/// The waveform spike's pictures: the Now Playing screen as the window
/// draws it, offscreen, with the band painted (`hosted`) and as glyphs
/// (what the window drew before the spike: the glyph path is untouched),
/// at a few widths, playheads and hovers. Writes `<name>.png` (the frame)
/// and `<name>-band.png` (the band's rows at twice the size) under
/// `MSTREAM_WAVE_SHOTS`; `MSTREAM_WAVE_BARS` is a file of the server's 800
/// bytes for the track, the blocky test peaks without it.
#[test]
#[ignore = "pictures for the waveform spike: MSTREAM_WAVE_SHOTS=<dir>"]
fn wave_shots() {
    use std::sync::Arc;

    use super::covers::Board;
    use crate::gui::render;
    use crate::kit::theme::th;

    let Some(dir) = std::env::var_os("MSTREAM_WAVE_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let bars = std::env::var_os("MSTREAM_WAVE_BARS")
        .and_then(|path| std::fs::read(path).ok())
        .unwrap_or_else(blocky_peaks);
    let _gpu = one_at_a_time();
    let rows = 30u32;
    let shots: &[(&str, u32, bool, f64, Option<u16>)] = &[
        ("glyphs-100", 100, false, 0.35, None),
        ("wave-100", 100, true, 0.35, None),
        ("glyphs-100-hover", 100, false, 0.35, Some(60)),
        ("wave-100-hover", 100, true, 0.35, Some(60)),
        ("wave-100-seek25", 100, true, 0.25, None),
        ("wave-100-seek75", 100, true, 0.75, None),
        ("glyphs-140", 140, false, 0.35, None),
        ("wave-140", 140, true, 0.35, None),
    ];
    for &(name, cols, hosted, progress, hover) in shots {
        let board = Arc::new(Board::with_palette(super::named_colours(th())));
        let Some(mut terminal) = window_terminal(&board, cols, rows) else { return };
        let mut gui = now_playing_gui(&board, hosted, &bars, progress);
        // Two frames: the second is the screen at rest, laid out by the
        // first (the band's rect, the hover's column on it).
        for _ in 0..2 {
            board.begin_frame();
            terminal.draw(|frame| render(frame, &mut gui)).unwrap();
            if let Some(column) = hover {
                let band = gui.now.band;
                let at = ratatui::layout::Position { x: band.x + column, y: band.y + 1 };
                gui.app.pointer = Some(at);
            }
        }
        let pixels = terminal.backend().read_pixels().unwrap();
        let width = (pixels.len() / 4) as u32 / (rows * SHOT_PX);
        let image = image::RgbaImage::from_raw(width, rows * SHOT_PX, pixels).unwrap();
        image.save(dir.join(format!("{name}.png"))).unwrap();
        let band = gui.now.band;
        let y = u32::from(band.y) * SHOT_PX;
        let height = u32::from(band.height) * SHOT_PX;
        let crop = image::imageops::crop_imm(&image, 0, y, width, height).to_image();
        image::imageops::resize(&crop, width * 2, height * 2, image::imageops::FilterType::Nearest)
            .save(dir.join(format!("{name}-band.png")))
            .unwrap();
        let (waves, under) = board.placed_waves();
        eprintln!("{name}: band {band:?}, waves {waves:?} ({under} under), {width} px wide");
    }
}

/// The waveform spike's cost, offscreen: the Now Playing screen drawn 200
/// frames at a time through the window's renderer, the band as glyphs and
/// as the painted wave, at rest, with the pointer moving along the band a
/// column a frame, and with the track playing (the clock 100 ms a frame,
/// the GUI's idle pace). Prints each case's draw time (the GUI's frame,
/// the diff, the backend's flush with its post processor, the submit) and
/// how many frames the post processor ran, which is how many the window
/// would present. `sync` also reads every frame back, so the GPU's work is
/// inside the time. `MSTREAM_WAVE_BENCH=1`.
#[test]
#[ignore = "the waveform spike's offscreen bench: MSTREAM_WAVE_BENCH=1"]
fn wave_bench() {
    use std::sync::Arc;
    use std::time::Instant;

    use super::covers::Board;
    use crate::gui::render;
    use crate::kit::theme::th;

    if std::env::var_os("MSTREAM_WAVE_BENCH").is_none() {
        return;
    }
    let bars = std::env::var_os("MSTREAM_WAVE_BARS")
        .and_then(|path| std::fs::read(path).ok())
        .unwrap_or_else(blocky_peaks);
    let _gpu = one_at_a_time();
    for sync in [false, true] {
        let cases = [("rest", false, false), ("hover", true, false), ("playing", false, true)];
        for (case, moving, playing) in cases {
            for hosted in [false, true] {
                let board = Arc::new(Board::with_palette(super::named_colours(th())));
                let Some(mut terminal) = window_terminal(&board, 100, 30) else { return };
                let mut gui = now_playing_gui(&board, hosted, &bars, 0.35);
                for _ in 0..5 {
                    board.begin_frame();
                    terminal.draw(|frame| render(frame, &mut gui)).unwrap();
                }
                let before = processed(&terminal);
                let mut took = Vec::new();
                for i in 0..200u16 {
                    let band = gui.now.band;
                    if moving {
                        let x = band.x + i % 84;
                        gui.app.pointer = Some(ratatui::layout::Position { x, y: band.y + 1 });
                    }
                    if playing {
                        gui.app.status.position += 0.1;
                    }
                    let started = Instant::now();
                    board.begin_frame();
                    terminal.draw(|frame| render(frame, &mut gui)).unwrap();
                    if sync {
                        let _ = terminal.backend().read_pixels();
                    }
                    took.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                let presents = processed(&terminal) - before;
                took.sort_by(f64::total_cmp);
                let at = |q: f64| took[((took.len() - 1) as f64 * q) as usize];
                let band = if hosted { "wave" } else { "glyphs" };
                let (p50, p95) = (at(0.5), at(0.95));
                eprintln!(
                    "bench sync={sync} {case:8} {band:6}: draw p50 {p50:.3} ms p95 {p95:.3} ms, \
                     {presents} presents of 200"
                );
            }
        }
    }
}

/// How many times the window's post processor has run: its presents.
fn processed<S: ratatui_wgpu::RenderSurface<'static>>(
    terminal: &Terminal<ratatui_wgpu::WgpuBackend<'static, 'static, super::covers::CoverPost, S>>,
) -> u64 {
    terminal.backend().post_processor().report()["processed"].as_u64().unwrap_or(0)
}
