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
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_wgpu::shaders::DefaultPostProcessor;
use ratatui_wgpu::wgpu::TextureFormat;
use ratatui_wgpu::{Builder, Dimensions, Font};

use super::{hack, script_fallbacks, symbol_fallback};
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
fn frame_or_skip(rendered: Result<Frame, String>) -> Option<Frame> {
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
