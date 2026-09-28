//! How a full-screen page's frames reach the terminal: whole, in one
//! write, and shown at once.
//!
//! ratatui's own terminal writes through `Stdout`, whose buffer is a
//! kilobyte, and its crossterm backend flushes on its own at every cursor
//! move and clear besides: a screen of styled cells went out in fifty
//! writes, a wall of covers in hundreds. A terminal that paints between two
//! reads could show any frame half done — most visibly after a resize, when
//! the frame opens by clearing the screen and the clear could be painted on
//! its own: a flash of empty window before the redraw.
//!
//! So a frame is encoded into memory and written once, when ratatui
//! flushes it, and a frame that changes anything is fenced in DEC mode
//! 2026, synchronized output: a terminal that knows the mode (kitty,
//! iTerm2, WezTerm, Ghostty and others) holds its screen from the first
//! byte to the last and shows the result whole; one that does not ignores
//! it, as it ignores any private mode it does not know. A frame that
//! changes nothing writes nothing — no fence, no colour resets, no cursor
//! hidden again that was hidden already.
//!
//! Holding assumes every command is bytes. On Windows, crossterm drives a
//! console that cannot read escape sequences through the console API
//! instead, one call per command at once, which would overtake whatever is
//! held; but consoles have read them since Windows 10's first update,
//! crossterm switches them on, and the player already writes escapes of
//! its own straight to the console (the window title, the pointer shape).

use std::cell::RefCell;
use std::io::{self, Stdout, Write};
use std::rc::Rc;

use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::queue;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::layout::{Position, Size};

/// A page's terminal: ratatui's, over [`Synced`] frames.
pub type PageTerminal = Terminal<Synced<Stdout>>;

/// `ratatui::init` — raw mode, the alternate screen, the panic hook that
/// undoes them — with the frames going out whole.
pub fn init() -> PageTerminal {
    // What init does to the terminal is what is wanted, not the terminal
    // it hands back.
    drop(ratatui::init());
    Terminal::new(Synced::new(std::io::stdout())).expect("a terminal to draw in")
}

/// A frame's bytes as crossterm encodes them, shared with the flush that
/// sends them — the encoder's own writer is not reachable from outside it.
#[derive(Clone, Default)]
struct Held(Rc<RefCell<Vec<u8>>>);

impl Write for Held {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A frame's bytes, held until the frame is done and then written in one
/// go, fenced when the frame changes anything.
pub struct Synced<W: Write> {
    /// The frame so far, encoded by crossterm into memory.
    frame: CrosstermBackend<Held>,
    held: Held,
    out: W,
    /// The fence is open: this frame has changed something.
    open: bool,
    /// Whether the cursor is hidden, as far as anything written here says:
    /// ratatui hides it again on every frame, and saying so again is a
    /// write for nothing.
    hidden: Option<bool>,
}

impl<W: Write> Synced<W> {
    pub fn new(out: W) -> Self {
        let held = Held::default();
        Synced { frame: CrosstermBackend::new(held.clone()), held, out, open: false, hidden: None }
    }

    fn open(&mut self) -> io::Result<()> {
        if !self.open {
            queue!(self.frame, BeginSynchronizedUpdate)?;
            self.open = true;
        }
        Ok(())
    }
}

impl<W: Write> Backend for Synced<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut content = content.peekable();
        if content.peek().is_none() {
            // Nothing changed: not even the colour resets crossterm writes
            // after a run of cells.
            return Ok(());
        }
        self.open()?;
        self.frame.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.open()?;
        self.frame.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        if self.hidden == Some(true) {
            return Ok(());
        }
        self.hidden = Some(true);
        self.frame.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        if self.hidden == Some(false) {
            return Ok(());
        }
        self.hidden = Some(false);
        self.frame.show_cursor()
    }

    /// Asks the terminal itself: crossterm writes the query and reads the
    /// reply on its own. ratatui asks only between frames, when nothing is
    /// held here.
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.frame.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.frame.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.open()?;
        self.frame.clear()
    }

    /// Inside the fence: a resize clears the screen first, and the clear
    /// is shown with the frame that follows it, never on its own.
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.open()?;
        self.frame.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.frame.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.frame.window_size()
    }

    /// The frame is done: its bytes go out in one write, fence and all.
    fn flush(&mut self) -> io::Result<()> {
        if self.open {
            queue!(self.frame, EndSynchronizedUpdate)?;
            self.open = false;
        }
        let mut bytes = self.held.0.borrow_mut();
        if !bytes.is_empty() {
            let written = self.out.write_all(&bytes);
            bytes.clear();
            written?;
        }
        self.out.flush()
    }
}

impl<W: Write> Drop for Synced<W> {
    /// What is still held goes out — ratatui shows the cursor again as its
    /// terminal is dropped. Unwinding from a panic, a frame half made is
    /// thrown away instead, and only the cursor is given back.
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.held.0.borrow_mut().clear();
            let _ = queue!(self.out, Show);
            let _ = self.out.flush();
        } else {
            let _ = Backend::flush(self);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::rc::Rc;

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// Every write the terminal would have received, one entry each.
    #[derive(Clone, Default)]
    struct Writes(Rc<RefCell<Vec<Vec<u8>>>>);

    impl Write for Writes {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().push(buf.to_vec());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    const BEGIN: &[u8] = b"\x1b[?2026h";
    const END: &[u8] = b"\x1b[?2026l";

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// A screen of changed cells, as ratatui's diff would hand them over.
    fn cells(buffer: &Buffer) -> impl Iterator<Item = (u16, u16, &Cell)> {
        let area = buffer.area;
        (area.top()..area.bottom())
            .flat_map(move |y| (area.left()..area.right()).map(move |x| (x, y)))
            .map(move |(x, y)| (x, y, &buffer[(x, y)]))
    }

    /// ratatui's terminal over a fixed viewport, so no size is asked of a
    /// terminal the tests do not have.
    fn fixed(writes: Writes, area: Rect) -> Terminal<Synced<Writes>> {
        let options = ratatui::TerminalOptions { viewport: ratatui::Viewport::Fixed(area) };
        Terminal::with_options(Synced::new(writes), options).unwrap()
    }

    #[test]
    fn a_frame_goes_out_in_one_write_fenced_from_the_clear_to_the_cursor() {
        let writes = Writes::default();
        let mut backend = Synced::new(writes.clone());
        let mut screen = Buffer::empty(Rect::new(0, 0, 40, 10));
        let yellow = ratatui::style::Style::default().fg(ratatui::style::Color::Yellow);
        screen.set_string(0, 0, "Cassini IV", yellow);

        // What a resize's frame does: clear, draw, hide the cursor, flush.
        backend.clear_region(ClearType::All).unwrap();
        backend.draw(cells(&screen)).unwrap();
        backend.hide_cursor().unwrap();
        assert!(writes.0.borrow().is_empty(), "nothing leaves before the frame is done");
        backend.flush().unwrap();

        let sent = writes.0.borrow();
        assert_eq!(sent.len(), 1, "one write");
        let bytes = &sent[0];
        assert!(bytes.starts_with(BEGIN), "the fence opens before the clear");
        assert!(bytes.ends_with(END), "and closes after the cursor");
        assert!(contains(bytes, b"\x1b[2J") && contains(bytes, b"Cassini IV"));
    }

    #[test]
    fn a_frame_that_changes_nothing_writes_nothing() {
        let writes = Writes::default();
        let mut backend = Synced::new(writes.clone());
        backend.hide_cursor().unwrap();
        backend.flush().unwrap();
        assert_eq!(writes.0.borrow().len(), 1, "the first hide is said");

        // An idle frame: an empty diff, the cursor hidden again.
        backend.draw(std::iter::empty()).unwrap();
        backend.hide_cursor().unwrap();
        backend.flush().unwrap();
        assert_eq!(writes.0.borrow().len(), 1, "and nothing after it");

        // Shown and hidden again, each is said once.
        backend.show_cursor().unwrap();
        backend.flush().unwrap();
        backend.hide_cursor().unwrap();
        backend.flush().unwrap();
        let sent = writes.0.borrow();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|w| !contains(w, BEGIN)), "a cursor alone needs no fence");
    }

    #[test]
    fn a_terminal_draws_through_it() {
        // ratatui's own path, end to end.
        let writes = Writes::default();
        let mut terminal = fixed(writes.clone(), Rect::new(0, 0, 20, 4));
        terminal.draw(|frame| frame.render_widget("mStream", frame.area())).unwrap();
        terminal.draw(|frame| frame.render_widget("mStream", frame.area())).unwrap();
        let sent = writes.0.borrow();
        assert_eq!(sent.len(), 1, "the unchanged second frame wrote nothing: {sent:?}");
        let frame = &sent[0];
        assert!(contains(frame, b"mStream") && frame.starts_with(BEGIN) && frame.ends_with(END));
    }

    #[test]
    fn a_dropped_terminal_gives_the_cursor_back() {
        let writes = Writes::default();
        {
            let mut terminal = fixed(writes.clone(), Rect::new(0, 0, 10, 2));
            terminal.draw(|frame| frame.render_widget("x", frame.area())).unwrap();
        }
        let sent = writes.0.borrow();
        assert!(contains(sent.last().unwrap(), b"\x1b[?25h"), "{sent:?}");
    }
}
