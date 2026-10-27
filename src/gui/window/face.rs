//! What the window shows: a face. The host (mod.rs) owns everything a
//! native window needs and no terminal program knows of: the event loop,
//! the renderer and its build off the loop, the held input and its replay,
//! the input method's placement, the levers, the stats and the instance
//! lock. What it asks of the thing it shows is this trait, a frame half
//! and an input half like the terminal loop's and the few reads and
//! hand-offs around them, so a second program can stand in a window of its
//! own without a second host. The GUI is the first face (`gui::GuiFace`),
//! wrapping the calls the host made into it before there was a trait.
//!
//! The trait is object-safe, and the host holds a `Box<dyn Face>`: one
//! window shows one face for its life, and a call through a pointer a
//! frame costs nothing beside the frame. What every face must answer has
//! no default; what only a face with that surface answers (a copy chord,
//! the script's probes, effects to send before the first frame, a saver to
//! flush) is defaulted to nothing, and a face without it leaves it out.
//! A face here is a program's face; the type faces the host loads for its
//! text (`Early::faces`, `App::faces`) are another thing.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::Event as TermEvent;
use ratatui::layout::{Position, Rect};

use super::{GRID, WindowTerminal};
use crate::gui::{Flow, Host};
use crate::tui::graphics::PictureHost;

pub(crate) trait Face {
    /// The window's title, given as `App::open` makes the window.
    fn title(&self) -> &'static str;

    /// The grid the window opens at when `MSTREAM_WINDOW_SIZE` names none
    /// (`opening_grid`). The GUI's is the installer's own window; a face
    /// laid out for another size says so here.
    fn grid(&self) -> (u16, u16) {
        GRID
    }

    /// The frame half, run on each redraw the backend can draw
    /// (`App::redraw_inner`): the face's work for the turn, one frame drawn
    /// into `terminal`, and the wait until it wants the next, which is when
    /// the host asks for the next redraw. An error ends the window with
    /// exit code 1, as a frame that fails in a terminal does. `host` is
    /// the window's cursor, the pointer's hand over what is clickable.
    fn frame(&mut self, terminal: &mut WindowTerminal, host: &mut dyn Host) -> io::Result<Duration>;

    /// The input half, for one event the window translated into what a
    /// terminal would have sent (`App::feed`). A [`Flow::Quit`] ends the
    /// loop; the face has flushed whatever a quit owes by then, and the
    /// teardown does not ask it to flush again.
    fn input(&mut self, event: TermEvent) -> Flow;

    /// The cell of the field that has the keyboard, as the last frame
    /// noted it, or none. The host turns the input method on while one
    /// does and floats its candidates there (`App::sync_ime`), and a paste
    /// chord types the clipboard only into such a field (`App::feed_now`).
    fn caret_at(&self) -> Option<Position>;

    /// What the input method is composing, not yet committed, for the
    /// field with the keyboard to draw (`App::set_preedit`); empty when
    /// nothing is.
    fn set_composition(&mut self, text: &str);

    /// Pictures are the window's to draw, as textures over the grid: the
    /// face hands each one to `host` rather than encoding it for a
    /// terminal, and tells `overlays` of every rect it draws over the base
    /// layer, so a picture a modal opens over is not painted over the modal
    /// on the frame it opens (`run`, before the loop starts).
    fn host_pictures(&mut self, host: Arc<dyn PictureHost>, overlays: Box<dyn Fn(Rect) + 'static>);

    /// Work to start before the first frame, while the window and its
    /// renderer are still being built (`run`, before the loop starts): the
    /// GUI sends its first effects, the connect among them, so input held
    /// in a blank window finds the answer in. A face with nothing on the
    /// wire leaves it out.
    fn send_early(&mut self) {}

    /// The window's copy chord (Cmd+C on a Mac, Ctrl+Shift+C or
    /// Ctrl+Insert elsewhere), which never reaches [`Face::input`]: true
    /// when it changed anything, so the window draws again
    /// (`App::feed_now`). A face with nothing to copy swallows the chord.
    fn copy(&mut self) -> bool {
        false
    }

    /// What a quit through [`Face::input`] would have saved, saved now: the
    /// close button and the platform's quit end the window without passing
    /// through the input half (`App::teardown`, before [`Face::finish`]).
    fn flush(&mut self) {}

    /// The face's own way out, after the window and its renderer are gone
    /// (`App::teardown`), and the whole of the way out when no window could
    /// be opened at all (`run`, with no display), where nothing was drawn
    /// and [`Face::flush`] is not called.
    fn finish(&mut self);

    /// The exit code of a window the face closed cleanly, asked once the
    /// loop has ended and [`Face::finish`] has run (`run`). A frame that
    /// failed, or a window that never opened, keeps the host's own code.
    fn exit_code(&self) -> i32 {
        0
    }

    /// What a press at `at` would hit, as the script's dumps report it
    /// (`App::feed_now`, with `MSTREAM_WINDOW_SCRIPT` set). None says
    /// nothing is registered there, which is all a face without hit
    /// testing can say.
    fn hit_debug(&self, _at: Position) -> Option<String> {
        None
    }

    /// Whether the press just fed began a drag, for the same dumps.
    fn drag_began(&self) -> bool {
        false
    }

    /// The face drawn into any frame, with none of the frame half's work
    /// around it: the fidelity dump (`MSTREAM_WINDOW_DUMP`, `dump`) draws
    /// it into a `TestBackend` of the window's size and compares the two.
    fn render_test(&mut self, frame: &mut Frame<'_>);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::widgets::Paragraph;

    use super::*;
    use crate::gui::window::{compare, face_rows, shown_rows};

    /// The least face there is: one line of text, Esc to leave. What it
    /// leaves out is the trait's optional surface.
    struct StubFace {
        line: &'static str,
    }

    impl StubFace {
        fn draw(&self, frame: &mut Frame<'_>) {
            frame.render_widget(Paragraph::new(self.line), frame.area());
        }
    }

    impl Face for StubFace {
        fn title(&self) -> &'static str {
            "stub"
        }

        fn frame(
            &mut self,
            terminal: &mut WindowTerminal,
            _host: &mut dyn Host,
        ) -> io::Result<Duration> {
            terminal.draw(|frame| self.draw(frame))?;
            Ok(crate::gui::POLL)
        }

        fn input(&mut self, event: TermEvent) -> Flow {
            match event {
                TermEvent::Key(key) if key.code == KeyCode::Esc => Flow::Quit,
                _ => Flow::Continue,
            }
        }

        fn caret_at(&self) -> Option<Position> {
            None
        }

        fn set_composition(&mut self, _text: &str) {}

        fn host_pictures(
            &mut self,
            _host: Arc<dyn PictureHost>,
            _overlays: Box<dyn Fn(Rect) + 'static>,
        ) {
        }

        fn finish(&mut self) {}

        fn render_test(&mut self, frame: &mut Frame<'_>) {
            self.draw(frame);
        }
    }

    fn key(code: KeyCode) -> TermEvent {
        TermEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// A face that answers only what it must gets the host's defaults for
    /// the rest: the window's own grid, a copy chord that changes nothing,
    /// no hit or drag to report, a clean exit, and an early send and a
    /// flush that leave it as it was.
    #[test]
    fn the_faces_defaults_are_the_optional_surface() {
        let mut stub = StubFace { line: "stub" };
        let face: &mut dyn Face = &mut stub;
        face.send_early();
        face.flush();
        assert_eq!(face.title(), "stub");
        assert_eq!(face.grid(), GRID);
        assert!(!face.copy());
        assert!(!face.drag_began());
        assert_eq!(face.hit_debug(Position { x: 1, y: 1 }), None);
        assert_eq!(face.exit_code(), 0);
        assert_eq!(face.input(key(KeyCode::Char('j'))), Flow::Continue);
        assert_eq!(face.input(key(KeyCode::Esc)), Flow::Quit);
    }

    /// The fidelity dump asks only the trait: a face's own drawing, read
    /// back as the window's rows would be, is EQUAL to what the dump draws
    /// of it into a `TestBackend` of the same size, and a window that
    /// shows something else is not.
    #[test]
    fn dump_is_generic_over_faces() {
        let mut shown = Terminal::new(TestBackend::new(20, 3)).unwrap();
        let stub = StubFace { line: "a stub face" };
        shown.draw(|frame| stub.draw(frame)).unwrap();
        let window_rows = shown_rows(shown.backend().buffer());
        let window: Vec<&str> = window_rows.iter().map(String::as_str).collect();

        let mut face: Box<dyn Face> = Box::new(stub);
        let test_rows = face_rows(&mut *face, 20, 3).unwrap();
        assert_eq!(compare(&window, &test_rows), "EQUAL");

        let mut other: Box<dyn Face> = Box::new(StubFace { line: "another" });
        let other_rows = face_rows(&mut *other, 20, 3).unwrap();
        assert!(compare(&window, &other_rows).starts_with("UNEQUAL from row 0"));
    }
}
