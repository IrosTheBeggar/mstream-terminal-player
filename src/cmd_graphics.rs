//! `graphics-probe`: what this terminal can draw, demonstrated.
//!
//! The player decides at startup whether covers render as real pixels or
//! as the half-block mosaic, and decides it from the terminal's own
//! answers — which makes "why does the cover look like that?" unanswerable
//! without a way to see the answers. This prints them, then draws the same
//! test card both ways, so one screenshot says what a terminal does. The
//! sibling of `quickconnect-probe`: a diagnostic for the layer that only
//! misbehaves on somebody else's machine.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::tui::art::{self, Art};
use crate::tui::canvas::Canvas;
use crate::tui::graphics::Graphics;

/// Cell rows the demonstration gets. Enough that the card reads as a
/// picture in both renderings; small enough to sit in scrollback like any
/// other command output.
const DEMO_ROWS: u16 = 8;

/// Cell columns per rendering, pixel and mosaic side by side.
const DEMO_COLS: u16 = 16;

pub fn run() -> i32 {
    // Before the probe, not after: the query writes escape sequences to
    // stdout unconditionally and waits on stdin for the answers, so with
    // either end redirected it lands the queries as binary soup in the
    // capture, stalls out its whole timeout against a terminal that was
    // never asked — and, timing out, leaves the crate's echo-off termios
    // for the shell to trip over. A diagnostic must not need diagnosing.
    let stdout_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if !stdout_tty || !stdin_tty {
        let which = match (stdout_tty, stdin_tty) {
            (false, true) => "stdout is not a terminal",
            (true, false) => "stdin is not a terminal",
            _ => "neither stdout nor stdin is a terminal",
        };
        println!("({which} — the probe asks the terminal through both, so there is nothing to ask; run this from an interactive session)");
        return 0;
    }

    // A tty that answers nothing leaves the crate's private raw mode
    // behind: its restore rides a reader thread that stays parked in
    // read() past the query timeout. Bracketing the probe with
    // crossterm's raw mode snapshots the real state first and re-asserts
    // it after, so the shell gets its echo back whatever the terminal
    // did. (The player's own probe sits inside ratatui::init/restore,
    // which is the same bracket at session scale.)
    let _ = ratatui::crossterm::terminal::enable_raw_mode();
    let start = std::time::Instant::now();
    let mut graphics = Graphics::probe();
    let elapsed = start.elapsed();
    let _ = ratatui::crossterm::terminal::disable_raw_mode();

    println!("probe answered in {elapsed:?}");
    println!("{}", graphics.diagnostics());
    if std::env::var("MSTREAM_NO_GRAPHICS").is_ok_and(|v| !v.is_empty() && v != "0") {
        println!("(MSTREAM_NO_GRAPHICS is set — unset it to let the probe ask)");
    }

    let Some(card) = test_card() else {
        // Only reachable if the image crate cannot round-trip its own PNG.
        eprintln!("could not build the test card");
        return 1;
    };

    match graphics.protocol() {
        Some(name) => println!("\nthe same card as {name} pixels, then as the mosaic:\n"),
        None => println!("\nthis terminal gets the half-block mosaic:\n"),
    }

    // An inline viewport rather than the alternate screen: the point of the
    // output is to sit in scrollback next to the printed facts, where a
    // screenshot catches both. The extra height is the margin the drawing
    // leaves: a sixel image touching a terminal's very last row makes some
    // of them scroll (upstream #57), so the card keeps a blank row under it
    // — the player's own cover never risks this, sitting above the
    // transport rows.
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = match Terminal::with_options(
        backend,
        TerminalOptions { viewport: Viewport::Inline(DEMO_ROWS + 1) },
    ) {
        Ok(terminal) => terminal,
        // An inline viewport asks the terminal where the cursor is; a
        // terminal that answered none of the probe's questions will not
        // answer this one either. The facts above are the diagnosis —
        // a demonstration that cannot be positioned is a skip, not a
        // failure.
        Err(e) => {
            println!("(no answer to the cursor query ({e}) — skipping the demonstration)");
            return 0;
        }
    };

    let drew = terminal.draw(|frame| {
        let [row, _margin] =
            Layout::vertical([Constraint::Length(DEMO_ROWS), Constraint::Length(1)])
                .areas(frame.area());
        let [pixels, _, blocks] = Layout::horizontal([
            Constraint::Length(DEMO_COLS),
            Constraint::Length(2),
            Constraint::Length(DEMO_COLS),
        ])
        .areas(row);

        if !graphics.draw(frame, pixels, &card) {
            // No protocol, or one that refused: say so where the picture
            // would have been, so the two panes never silently show the
            // same rendering twice.
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(""),
                    Line::from(Span::styled("(no pixels)", Style::new())),
                ]),
                pixels,
            );
        }
        render_mosaic(frame, blocks, &card);
    });

    if let Err(e) = drew {
        eprintln!("could not draw the demonstration: {e}");
        return 1;
    }
    // Leave the cursor under the viewport rather than in it, so the shell
    // prompt lands below the card instead of over it.
    println!();
    0
}

/// The demonstration picture: a hue wheel around a bright centre, built
/// here and round-tripped through PNG so it exercises the exact decode
/// path a server's cover takes. Chosen over any real cover because its
/// point is legibility of *rendering* differences — smooth gradients show
/// sixel's palette stepping, the disc's edge shows resolution, and it
/// needs no file shipped alongside the binary.
fn test_card() -> Option<Art> {
    const SIDE: u32 = 256;
    let mut pixels = image::RgbImage::new(SIDE, SIDE);
    let centre = (SIDE / 2) as f32;
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let (dx, dy) = (x as f32 - centre, y as f32 - centre);
        let radius = (dx * dx + dy * dy).sqrt() / centre;
        let angle = dy.atan2(dx);
        let hue = |offset: f32| {
            let v = ((angle + offset).cos() * 0.5 + 0.5) * (1.0 - radius * 0.6);
            (v.clamp(0.0, 1.0) * 255.0) as u8
        };
        let ring = if (0.55..0.65).contains(&radius) { 255 } else { 0 };
        *pixel = image::Rgb([
            hue(0.0).max(ring),
            hue(2.0944).max(ring),
            hue(4.1888).max(ring),
        ]);
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    pixels.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
    art::decode(&bytes.into_inner())
}

/// The card the way every unsupported terminal will see covers: sampled
/// through the same box-averaging the player's own mosaic uses.
fn render_mosaic(frame: &mut ratatui::Frame, area: Rect, card: &Art) {
    let mut canvas = Canvas::new(area);
    if canvas.is_empty() {
        return;
    }
    let (w, h) = (u32::from(canvas.width()), u32::from(canvas.height()));
    for y in 0..h {
        for x in 0..w {
            canvas.set(x as i32, y as i32, art::cover_sample(card, w, h, x, y));
        }
    }
    frame.render_widget(Paragraph::new(canvas.into_lines()), area);
}
