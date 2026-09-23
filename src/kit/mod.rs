//! The mStream Terminal UI Kit — the shipped counterpart of
//! `docs/ui-kit.md` (and the design canvas it links). The setup wizard
//! is the first consumer; any new ratatui surface in this project builds
//! its screens from these parts.
//!
//! The heart is [`Surface`]: a per-frame interaction registry (click
//! rects, tooltip rects, scrollbar geometry) plus the cross-frame input
//! state (pointer, tooltip dwell, scrollbar capture and hold-repeat),
//! generic over the screen's own action enum. Widgets are free functions
//! that draw AND register: a screen's render pass calls
//! [`Surface::begin_frame`], draws its widgets, and its event loop asks
//! the surface what a press hit, what a drag means, and when a held
//! arrow fires again.
//!
//! Colors come from [`theme`] — the kit's FIXED palette (a truecolor →
//! 256 → named-ANSI ladder plus the OSC 11 ground lease). The player's
//! adaptive `ui::Theme` is deliberately not part of the kit.

pub mod theme;

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use theme::th;

/// How long the pointer rests on a tip target before the tooltip shows.
pub const TIP_DELAY: Duration = Duration::from_millis(500);
/// Tooltip text wraps at this many cells.
pub const TIP_WRAP: usize = 40;
/// Hold-to-repeat on scrollbar arrows: the pause before repeating, then
/// the step cadence (clamped by the consumer's event-loop tick).
pub const ARROW_DELAY: Duration = Duration::from_millis(400);
pub const ARROW_REPEAT: Duration = Duration::from_millis(60);
/// A release this soon after a bar press is a PHANTOM: Apple Terminal
/// reports every press as an instant click (press+release in the same
/// millisecond), sends motion-while-held as plain Moved, and re-clicks
/// at the physical release — holds are invisible to it. A phantom
/// release downgrades the capture to a SOFT one instead of ending it.
pub const PHANTOM_RELEASE: Duration = Duration::from_millis(150);
/// The caret's blink: half a second on, half a second off (the desktop
/// editors' rate), counted from the last key or click so a caret that just
/// moved is always seen.
pub const CARET_BLINK: Duration = Duration::from_millis(500);
/// The soft capture holds while motion stays within this many cells of
/// the press; travelling beyond it resumes normal hover.
pub const SOFT_RADIUS: u16 = 2;

// ── Styles ───────────────────────────────────────────────────────────────────

pub fn accent() -> Style {
    Style::default().fg(th().accent)
}
pub fn dim() -> Style {
    Style::default().fg(th().dim)
}
pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

// ── The interaction surface ──────────────────────────────────────────────────

/// A registered scrollbar: geometry plus the actions its parts emit.
struct BarReg<A> {
    rect: Rect,
    max_scroll: usize,
    step_back: A,
    step_fwd: A,
    jump: Box<dyn Fn(usize) -> A>,
}

/// The per-screen interaction state. Rebuild the registries every frame
/// ([`Surface::begin_frame`], then widget calls); the input state
/// (pointer, dwell, capture) lives across frames.
pub struct Surface<A> {
    /// Everything clickable this frame, in draw order — the LAST drawn
    /// rect wins a hit, which is what puts overlays above screens.
    pub clicks: Vec<(Rect, A)>,
    /// Tooltip targets, rebuilt each frame like `clicks`. A rect here is
    /// NOT necessarily clickable (disabled controls register a tip — the
    /// reason they're disabled — without registering a click).
    pub tips: Vec<(Rect, String)>,
    /// Where the mouse last was, for hover styling. None until it moves.
    /// Screens may stash-and-clear it to make a layer inert (the modal
    /// pattern), restoring afterwards.
    pub pointer: Option<Position>,
    /// The tip target the pointer is resting on, and since when.
    dwell: Option<(Rect, String, Instant)>,
    bars: Vec<BarReg<A>>,
    /// An active thumb drag: index into this frame's `bars`.
    drag: Option<usize>,
    /// A held ▲/▼ endcap: (bar index, direction, when the next step fires).
    arrow_hold: Option<(usize, i8, Instant)>,
    /// Where and when the current bar interaction was armed.
    armed: Option<(Position, Instant)>,
    /// A soft capture left behind by a phantom release: hover stays
    /// suppressed near this press until the pointer genuinely leaves.
    soft_origin: Option<Position>,
    /// The footprints of everything drawn OVER the base layer this frame —
    /// modal frames, the header dropdown, the tooltip — and last frame's.
    /// A pixel surface beneath (a cover) consults last frame's, because
    /// the overlays draw after it: the terminal writer skips a picture's
    /// cells, so a cover an overlay touched draws as text until the frame
    /// after the overlay leaves, which repaints every cell it wrote. One
    /// frame behind on the way in is harmless — the overlay's `Clear`
    /// resets what it covers. Covers an overlay never touches stay pixels.
    overlays: Vec<Rect>,
    covered: Vec<Rect>,
    /// What a right click means where — a row's context verb (the
    /// track-actions contract's sheet). Rebuilt each frame like `clicks`.
    contexts: Vec<(Rect, A)>,
    /// Whether tooltips name the key that does the same (see
    /// [`Surface::tip_keyed`]). On by default — the wizard and the admin
    /// rooms always name theirs; the GUI player has a setting.
    pub key_hints: bool,
    /// The caret's blink clock: when a field last took input (the caret
    /// shows solid from then, the way every editor's does), and whether a
    /// field drew a caret this frame — the shell times its next frame to
    /// the flip. Blinking here rather than through the terminal's cursor
    /// because a terminal profile can veto a DECSCUSR blink, and a caret
    /// that may or may not blink is worse than one that always does.
    caret_since: Option<Instant>,
    caret_drawn: bool,
}

impl<A> Default for Surface<A> {
    fn default() -> Self {
        Surface {
            clicks: Vec::new(),
            tips: Vec::new(),
            pointer: None,
            dwell: None,
            bars: Vec::new(),
            drag: None,
            arrow_hold: None,
            armed: None,
            soft_origin: None,
            overlays: Vec::new(),
            covered: Vec::new(),
            contexts: Vec::new(),
            key_hints: true,
            caret_since: None,
            caret_drawn: false,
        }
    }
}

impl<A: Clone> Surface<A> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a render pass: the registries empty, the input state stays,
    /// and last frame's overlay footprints become the ones to draw under.
    pub fn begin_frame(&mut self) {
        self.covered = std::mem::take(&mut self.overlays);
        self.clear_registries();
    }

    /// Register something drawn over the base layer — its whole footprint,
    /// border included. Not cleared by [`Self::clear_registries`]: an
    /// overlay registers after the base layer's controls are dropped.
    pub fn overlay(&mut self, rect: Rect) {
        self.overlays.push(rect);
    }

    /// Whether an overlay stood over any part of `rect` LAST frame — the
    /// question a pixel surface asks before drawing pixels there.
    pub fn covered_last_frame(&self, rect: Rect) -> bool {
        self.covered.iter().any(|over| over.intersects(rect))
    }

    /// Whether an overlay OTHER than `own` stood over any part of `rect`
    /// last frame — the question a pixel surface INSIDE a modal asks: its
    /// own frame always covers it, and only something drawn over the modal
    /// (a tooltip, a second modal) makes its picture stand down.
    pub fn covered_last_frame_by_another(&self, rect: Rect, own: Rect) -> bool {
        self.covered.iter().any(|over| *over != own && over.intersects(rect))
    }

    /// Drop every registered rect — the modal-inertness move: a screen
    /// clears the base layer's registries before drawing the layer on
    /// top, so only the top layer's controls exist.
    pub fn clear_registries(&mut self) {
        self.clicks.clear();
        self.tips.clear();
        self.bars.clear();
        self.contexts.clear();
        self.caret_drawn = false;
    }

    /// A field took a key or a click: the caret shows solid from now.
    pub fn caret_touch(&mut self) {
        self.caret_since = Some(Instant::now());
    }

    /// Whether the caret is ON at this instant — a field asks as it draws
    /// (see [`input_display_blink`]). A caret never touched is on. Marks
    /// the frame as one with a caret, for [`Self::caret_next_flip`].
    pub fn caret(&mut self) -> bool {
        self.caret_drawn = true;
        self.caret_since.is_none_or(|since| (since.elapsed().as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2))
    }

    /// How long until the drawn caret flips, if one drew this frame — the
    /// shell shortens its poll to land the next frame ON the flip, so the
    /// blink is crisp instead of a poll tick late.
    pub fn caret_next_flip(&self) -> Option<Duration> {
        if !self.caret_drawn {
            return None;
        }
        let elapsed = self.caret_since.map_or(0, |since| since.elapsed().as_millis());
        let into = elapsed % CARET_BLINK.as_millis();
        Some(Duration::from_millis((CARET_BLINK.as_millis() - into) as u64))
    }

    /// Age the blink clock, so a test can see the other phase.
    #[cfg(test)]
    pub fn caret_backdate(&mut self, by: Duration) {
        self.caret_since = Instant::now().checked_sub(by);
    }

    /// Register what a right click on `rect` does. The last registered
    /// wins a hit, as with clicks.
    pub fn context(&mut self, rect: Rect, act: A) {
        self.contexts.push((rect, act));
    }

    pub fn hit_context(&self, at: Position) -> Option<A> {
        self.contexts.iter().rev().find(|(rect, _)| rect.contains(at)).map(|(_, act)| act.clone())
    }

    /// Register a clickable rect.
    pub fn click(&mut self, rect: Rect, act: A) {
        self.clicks.push((rect, act));
    }

    /// Register a tooltip target.
    pub fn tip(&mut self, rect: Rect, text: impl Into<String>) {
        self.tips.push((rect, text.into()));
    }

    /// A tooltip whose text ends in ` — key`, the key that does the same:
    /// registered whole while the surface names keys, cut at the dash when
    /// it does not. The dash is the one this family writes its key tails
    /// with, so a tip with a sentence after a dash registers through
    /// [`Surface::tip`] instead.
    pub fn tip_keyed(&mut self, rect: Rect, text: impl Into<String>) {
        let text = text.into();
        let text = if self.key_hints {
            text
        } else {
            text.rsplit_once(" — ").map_or(text.clone(), |(label, _)| label.to_string())
        };
        self.tips.push((rect, text));
    }

    /// What a press at `at` hits — the last-drawn matching rect.
    pub fn hit(&self, at: Position) -> Option<A> {
        self.clicks.iter().rev().find(|(rect, _)| rect.contains(at)).map(|(_, act)| act.clone())
    }

    /// Whether the pointer rests in `rect` — the test every control asks
    /// before choosing its color.
    pub fn hovers(&self, rect: Rect) -> bool {
        self.pointer.is_some_and(|p| rect.contains(p))
    }

    /// True while the pointer is over anything clickable — drives the
    /// OSC 22 hand cursor.
    pub fn hovering_clickable(&self) -> bool {
        self.pointer.is_some_and(|p| self.clicks.iter().any(|(rect, _)| rect.contains(p)))
    }

    /// A pointer move (`Moved` or `Drag` — terminals differ on which
    /// they send mid-press). A scrollbar interaction CAPTURES the mouse:
    /// while an arrow is held or the thumb dragged, sub-cell hand tremor
    /// must not retarget hover onto whatever sits beside the 1-cell bar.
    /// A SOFT capture (after a phantom release) suppresses hover only
    /// near the press, until the pointer genuinely travels away.
    pub fn motion(&mut self, at: Position) {
        if self.drag.is_some() || self.arrow_hold.is_some() {
            return;
        }
        if let Some(origin) = self.soft_origin {
            let near = at.x.abs_diff(origin.x) <= SOFT_RADIUS
                && at.y.abs_diff(origin.y) <= SOFT_RADIUS;
            if near {
                return;
            }
            self.soft_origin = None;
        }
        self.pointer = Some(at);
    }

    /// A press begins. Returns `false` for the phantom RE-CLICK Apple
    /// Terminal emits at the physical release of a hold: it lands near
    /// the original press (inside the soft radius) but off every bar —
    /// the screen must swallow it entirely. A press on a bar, or beyond
    /// the radius, is a real interaction and ends the soft capture.
    pub fn begin_press(&mut self, at: Position) -> bool {
        if let Some(origin) = self.soft_origin {
            let near = at.x.abs_diff(origin.x) <= SOFT_RADIUS
                && at.y.abs_diff(origin.y) <= SOFT_RADIUS;
            let on_bar = self.bars.iter().any(|b| b.rect.contains(at));
            if near && !on_bar {
                return false;
            }
            self.soft_origin = None;
        }
        self.pointer = Some(at);
        true
    }

    /// A press on a scrollbar arms its interaction: endcap rows arm
    /// hold-to-repeat (the press itself already stepped via the cell's
    /// registered act), track rows arm a thumb drag. Call after the hit
    /// was dispatched.
    pub fn arm_bars(&mut self, at: Position) {
        let Some(i) = self.bars.iter().position(|b| b.rect.contains(at)) else { return };
        self.armed = Some((at, Instant::now()));
        let rect = self.bars[i].rect;
        if at.y == rect.y {
            self.arrow_hold = Some((i, -1, Instant::now() + ARROW_DELAY));
        } else if at.y == rect.y + rect.height - 1 {
            self.arrow_hold = Some((i, 1, Instant::now() + ARROW_DELAY));
        } else {
            self.drag = Some(i);
        }
    }

    /// The button lifted: the hard capture ends. A release arriving
    /// within [`PHANTOM_RELEASE`] of arming is Apple Terminal's instant
    /// click — the physical hold is still going, so a SOFT capture keeps
    /// hover pinned near the press (repeat and drag stay off: with holds
    /// invisible, a repeat could never be stopped).
    pub fn release(&mut self) {
        if let Some((origin, when)) = self.armed.take() {
            if when.elapsed() < PHANTOM_RELEASE {
                self.soft_origin = Some(origin);
            }
        }
        self.drag = None;
        self.arrow_hold = None;
    }

    /// The action a drag at `at` means (the thumb following the hand),
    /// if a drag is active.
    pub fn drag_action(&mut self, at: Position) -> Option<A> {
        let bar = self.bars.get(self.drag?)?;
        Some((bar.jump)(bar_jump(bar.rect, bar.max_scroll, at.y)))
    }

    /// The next step of a held arrow, once its clock says so. Call every
    /// event-loop pass; reschedules itself at [`ARROW_REPEAT`].
    pub fn hold_action(&mut self) -> Option<A> {
        let (i, delta, next) = self.arrow_hold?;
        if Instant::now() < next {
            return None;
        }
        let bar = self.bars.get(i)?;
        let act = if delta < 0 { bar.step_back.clone() } else { bar.step_fwd.clone() };
        self.arrow_hold = Some((i, delta, Instant::now() + ARROW_REPEAT));
        Some(act)
    }

    /// Advance the tooltip dwell: the timer survives while the pointer
    /// stays on the same tip rect, restarts on a new one, and dies the
    /// moment the pointer leaves. Call once per event-loop pass, after
    /// the frame was drawn.
    pub fn dwell_tick(&mut self) {
        let tip = self
            .pointer
            .and_then(|p| self.tips.iter().find(|(rect, _)| rect.contains(p)).cloned());
        self.dwell = match (tip, self.dwell.take()) {
            (Some((rect, text)), Some((prev, _, since))) if prev == rect => {
                Some((rect, text, since))
            }
            (Some((rect, text)), _) => Some((rect, text, Instant::now())),
            (None, _) => None,
        };
    }

    /// The tooltip to draw this frame, if the dwell has matured.
    pub fn ripe_tooltip(&self) -> Option<(Rect, &str)> {
        let (rect, text, since) = self.dwell.as_ref()?;
        (since.elapsed() >= TIP_DELAY).then_some((*rect, text.as_str()))
    }

    /// Typing dismisses a tooltip (the dwell re-arms if the pointer just
    /// sits there, like native tooltips).
    pub fn dismiss_tooltip(&mut self) {
        self.dwell = None;
    }

    fn register_bar(
        &mut self,
        rect: Rect,
        max_scroll: usize,
        step_back: A,
        step_fwd: A,
        jump: Box<dyn Fn(usize) -> A>,
    ) {
        self.bars.push(BarReg { rect, max_scroll, step_back, step_fwd, jump });
    }
}

// ── Buttons ──────────────────────────────────────────────────────────────────

/// The kit's primary button: a 3-row Rounded frame, NO fill — the frame
/// color is the emphasis (the kit's chosen answer to the terminal's
/// button limits: a filled block cannot have rounded corners, so the
/// standard is the frame and the fills are documented alternatives).
/// Border and label share the color; hover brightens both to Cyan.
/// Disabled: everything DIM, no `▸` in the caller's label, no click rect,
/// no hover, no hand — a tip rect (pushed by the caller) says why.
/// `at.y` is the TOP row of the three. Returns the rect it drew into.
pub fn tall_button<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    at: Rect,
    label: &str,
    enabled: bool,
    act: A,
) -> Rect {
    let tone = |hovered: bool| match (enabled, hovered) {
        (false, _) => (th().dim, false),
        (true, true) => (th().bright, true),
        (true, false) => (th().accent, true),
    };
    tall_frame(frame, s, at, label, 2, tone, enabled.then_some(act))
}

/// The tall SECONDARY: the backward/neutral action beside a primary —
/// the same 3-row Rounded frame, everything DIM until hover brightens
/// border and label to Cyan (label BOLD on hover, like text buttons).
/// Never two primaries in a row group; a secondary is how the second
/// tall control stays honest.
pub fn tall_secondary<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    at: Rect,
    label: &str,
    act: A,
) -> Rect {
    let tone = |hovered: bool| if hovered { (th().bright, true) } else { (th().dim, false) };
    tall_frame(frame, s, at, label, 2, tone, Some(act))
}

/// The 3-row frame every tall control shares: the label with `pad` spaces
/// a side, a Rounded border and the label in one color, the label BOLD or
/// not — `tone` picks both from whether the pointer is in the frame — and
/// the click when `act` is given (none: disabled, no hover, no hand).
/// Returns the rect drawn into.
pub fn tall_frame<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    at: Rect,
    label: &str,
    pad: usize,
    tone: impl Fn(bool) -> (Color, bool),
    act: Option<A>,
) -> Rect {
    tall_frame_bordered(frame, s, at, label, pad, BorderType::Rounded, tone, act)
}

/// [`tall_frame`] with the border of the caller's choosing: the thick
/// border is how a control stands out from the rounded frames beside it
/// (the GUI player's transport).
#[allow(clippy::too_many_arguments)]
pub fn tall_frame_bordered<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    at: Rect,
    label: &str,
    pad: usize,
    border: BorderType,
    tone: impl Fn(bool) -> (Color, bool),
    act: Option<A>,
) -> Rect {
    let text = format!("{:pad$}{label}{:pad$}", "", "");
    let width = (text.chars().count() as u16 + 2).min(at.width);
    let rect = Rect { x: at.x, y: at.y, width, height: 3.min(at.height.max(1)) };
    let hovered = act.is_some() && s.hovers(rect);
    let (color, bold) = tone(hovered);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border)
        .border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let label_style = if bold {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    frame.render_widget(Paragraph::new(Span::styled(text, label_style)), inner);
    if let Some(act) = act {
        s.click(rect, act);
    }
    rect
}

/// The keyboard cursor on a framed control the pointer isn't in: the ring
/// redrawn in `style`, so the mark never steals the hover contract.
pub fn cursor_ring(frame: &mut Frame, rect: Rect, style: Style) {
    frame.render_widget(
        Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(style),
        rect,
    );
}

/// A one-line clickable button: draws itself, registers its click, and
/// lights up when the pointer is over it. Returns the rect it drew into.
pub fn button<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    at: Rect,
    label: &str,
    primary: bool,
    act: A,
) -> Rect {
    let text = format!("  {label}  ");
    let width = (text.chars().count() as u16).min(at.width);
    let rect = Rect { x: at.x, y: at.y, width, height: 1 };
    let hovered = s.hovers(rect);
    let style = match (primary, hovered) {
        (true, true) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
        (true, false) => Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        (false, true) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
        (false, false) => dim(),
    };
    frame.render_widget(Paragraph::new(Span::styled(text, style)), rect);
    s.click(rect, act);
    rect
}

// ── Modals ───────────────────────────────────────────────────────────────────

pub fn modal_frame(
    frame: &mut Frame,
    area: Rect,
    width: u16,
    height: u16,
    title_color: Color,
) -> Rect {
    modal_frame_anchored(frame, area, width, height, height, title_color)
}

/// [`modal_frame`] on a screen with pixel surfaces beneath: the frame's
/// footprint is registered with the surface, so a cover it touches draws
/// as text (see [`Surface::overlay`]) and every other cover stays pixels.
pub fn modal_frame_on<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    area: Rect,
    width: u16,
    height: u16,
    title_color: Color,
) -> Rect {
    modal_frame_anchored_on(frame, s, area, width, height, height, title_color)
}

/// [`modal_frame_anchored`], registering its footprint like [`modal_frame_on`].
pub fn modal_frame_anchored_on<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    area: Rect,
    width: u16,
    height: u16,
    max_height: u16,
    title_color: Color,
) -> Rect {
    s.overlay(modal_rect(area, width, height, max_height));
    modal_frame_anchored(frame, area, width, height, max_height, title_color)
}

/// Where a modal of this size sits: centred, and vertically as if
/// `max_height` tall, so one that grows keeps its top edge.
pub fn modal_rect(area: Rect, width: u16, height: u16, max_height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    let max_height = max_height.max(height).min(area.height.saturating_sub(2));
    Rect { x: (area.width - width) / 2, y: (area.height - max_height) / 2, width, height }
}

/// Like [`modal_frame`], but vertically positioned as if the modal were
/// `max_height` tall: a modal whose height varies (a suggestion list)
/// keeps a FIXED top edge and grows downward — its input line never
/// jumps as content comes and goes.
pub fn modal_frame_anchored(
    frame: &mut Frame,
    area: Rect,
    width: u16,
    height: u16,
    max_height: u16,
    title_color: Color,
) -> Rect {
    let rect = modal_rect(area, width, height, max_height);
    frame.render_widget(Clear, rect);
    // Clear resets cells to the terminal default — repaint the ground so
    // the modal interior matches the fixed scheme (when it is owned).
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            Block::default().style(Style::default().bg(ground).fg(th().text)),
            rect,
        );
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(title_color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// The modal's close control: `[X]` on the title row, right edge. Dim
/// until hovered, then BRIGHT — dismissal is neutral, unlike a row
/// remove's destructive red. Esc remains the keyboard path (the tip
/// says so — the same words on every modal in the family).
pub fn modal_close<A: Clone>(frame: &mut Frame, s: &mut Surface<A>, inner: Rect, act: A) {
    let rect = Rect { x: inner.right().saturating_sub(3), y: inner.y, width: 3, height: 1 };
    let hovered = s.hovers(rect);
    let style = if hovered {
        Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
    } else {
        dim()
    };
    frame.render_widget(Paragraph::new(Span::styled("[X]", style)), rect);
    s.click(rect, act);
    s.tip_keyed(rect, rust_i18n::t!("path_modal.tip_close").to_string());
}

// ── Scrolling ────────────────────────────────────────────────────────────────

/// Map a pointer row on a scrollbar to a scroll position: the track is
/// proportional, endcap rows clamp to the ends, and a single-cell track
/// lands midway.
pub fn bar_jump(bar: Rect, max_scroll: usize, y: u16) -> usize {
    let track_top = bar.y + 1;
    let span = bar.height.saturating_sub(2).max(1) as usize;
    if span == 1 {
        return max_scroll / 2;
    }
    let rel = y.saturating_sub(track_top).min(span as u16 - 1) as usize;
    (rel * max_scroll + (span - 1) / 2) / (span - 1)
}

/// A list viewport: given the row count, a row to reveal (a moved
/// keyboard cursor, or a fresh add), the wheel offset and the available
/// height → (first visible index, visible count). The wheel scrolls
/// freely; a reveal yanks the view to that row.
pub fn table_view(len: usize, reveal: Option<usize>, scroll: usize, avail: usize) -> (usize, usize) {
    if len == 0 || avail == 0 {
        return (0, 0);
    }
    let visible = avail.min(len);
    let mut first = scroll.min(len - visible);
    if let Some(row) = reveal {
        if row < first {
            first = row;
        } else if row >= first + visible {
            first = row + 1 - visible;
        }
    }
    (first, visible)
}

/// A list's viewport state, the table contract in one place: the wheel
/// offset, and whether the next frame reveals the cursor (a keyboard move,
/// a fresh add; the wheel scrolls freely in between).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListView {
    pub scroll: usize,
    pub reveal: bool,
}

impl ListView {
    /// The viewport for this frame — [`table_view`] over `len` rows in
    /// `avail` cells, revealing `selected` (a drawn-row position) when a
    /// reveal was asked since the last frame — with the offset written back
    /// clamped.
    pub fn window(&mut self, len: usize, selected: Option<usize>, avail: usize) -> (usize, usize) {
        let reveal = std::mem::take(&mut self.reveal).then_some(selected).flatten();
        let (first, visible) = table_view(len, reveal, self.scroll, avail);
        self.scroll = first;
        (first, visible)
    }

    /// A wheel or page step; the next frame clamps it.
    pub fn step(&mut self, delta: i32) {
        self.scroll = self.scroll.saturating_add_signed(delta as isize);
    }
}

/// The kit scrollbar, fully live: endcaps step (and hold-repeat), track
/// cells jump proportionally, a track press arms a thumb drag, and the
/// bar brightens under the pointer. Draws only on overflow; registers
/// every cell and the bar geometry with the surface.
#[allow(clippy::too_many_arguments)]
pub fn scroll_list<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    bar: Rect,
    len: usize,
    visible: usize,
    first: usize,
    step_back: A,
    step_fwd: A,
    jump: impl Fn(usize) -> A + 'static,
) {
    if len <= visible || visible == 0 {
        return;
    }
    let max_scroll = len - visible;
    let mut state = ScrollbarState::new(max_scroll + 1).position(first);
    let bar_hover = s.hovers(bar);
    let ends = if bar_hover { Style::default().fg(th().bright) } else { dim() };
    let thumb = if bar_hover {
        Style::default().fg(th().bright)
    } else {
        Style::default().fg(th().accent)
    };
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_style(dim())
            .thumb_style(thumb)
            .begin_style(ends)
            .end_style(ends),
        bar,
        &mut state,
    );
    // The whole bar is live: track cells jump proportionally (rects
    // first, so the endcaps win their own cells), endcaps step.
    for ty in (bar.y + 1)..(bar.y + bar.height).saturating_sub(1) {
        s.click(Rect { x: bar.x, y: ty, width: 1, height: 1 }, jump(bar_jump(bar, max_scroll, ty)));
    }
    s.click(Rect { x: bar.x, y: bar.y, width: 1, height: 1 }, step_back.clone());
    s.click(
        Rect { x: bar.x, y: bar.y + bar.height - 1, width: 1, height: 1 },
        step_fwd.clone(),
    );
    s.register_bar(bar, max_scroll, step_back, step_fwd, Box::new(jump));
}

// ── Letter strip ─────────────────────────────────────────────────────────────

/// The strip's buckets: `#` then A–Z (library-rooms contract, clause 10).
pub const STRIP_BUCKETS: usize = 27;
/// Rows an alphabetical list needs before the strip shows — the record's
/// default threshold.
pub const STRIP_MIN_ROWS: usize = 25;

/// Which bucket a label files under: its first character uppercased when
/// that is A–Z, else `#` — digits, punctuation and any non-Latin initial,
/// the record's rule.
pub fn letter_bucket(label: &str) -> usize {
    match label.trim_start().chars().next().map(|c| c.to_ascii_uppercase()) {
        Some(c @ 'A'..='Z') => (c as u8 - b'A') as usize + 1,
        _ => 0,
    }
}

pub fn bucket_glyph(bucket: usize) -> char {
    if bucket == 0 { '#' } else { (b'A' + (bucket - 1) as u8) as char }
}

/// The nearest present bucket to `wanted` — itself when present — so a
/// click on a dim letter still lands somewhere; `None` when nothing is.
pub fn snap_bucket(present: &[bool; STRIP_BUCKETS], wanted: usize) -> Option<usize> {
    if present.get(wanted).copied().unwrap_or(false) {
        return Some(wanted);
    }
    (1..STRIP_BUCKETS as i32).find_map(|d| {
        let below = wanted as i32 - d;
        let above = wanted as i32 + d;
        if below >= 0 && present[below as usize] {
            Some(below as usize)
        } else if (above as usize) < STRIP_BUCKETS && present[above as usize] {
            Some(above as usize)
        } else {
            None
        }
    })
}

/// The strip's index over a list's labels, in list order: which buckets
/// are present, and the position of the first row in each.
pub fn letter_index<'a>(labels: impl IntoIterator<Item = &'a str>) -> ([bool; STRIP_BUCKETS], [usize; STRIP_BUCKETS]) {
    let mut present = [false; STRIP_BUCKETS];
    let mut first_of = [0usize; STRIP_BUCKETS];
    for (pos, label) in labels.into_iter().enumerate() {
        let bucket = letter_bucket(label);
        if !present[bucket] {
            present[bucket] = true;
            first_of[bucket] = pos;
        }
    }
    (present, first_of)
}

/// A row of `# A B … Z`: present letters live, absent ones dim, each a click
/// target whose jump snaps to the nearest present letter. Spaced when the
/// row has the width, packed otherwise.
pub fn letter_strip<A: Clone>(
    frame: &mut Frame,
    s: &mut Surface<A>,
    at: Rect,
    present: &[bool; STRIP_BUCKETS],
    jump: impl Fn(usize) -> A,
) {
    let step: u16 = if at.width >= (STRIP_BUCKETS * 2 - 1) as u16 { 2 } else { 1 };
    for bucket in 0..STRIP_BUCKETS {
        let x = at.x + bucket as u16 * step;
        if x >= at.right() {
            break;
        }
        let cell = Rect { x, y: at.y, width: 1, height: 1 };
        let hover = s.hovers(cell);
        let style = match (present[bucket], hover) {
            (_, true) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
            (true, false) => Style::default().fg(th().text),
            (false, false) => dim(),
        };
        frame.render_widget(Paragraph::new(Span::styled(bucket_glyph(bucket).to_string(), style)), cell);
        if let Some(target) = snap_bucket(present, bucket) {
            s.click(cell, jump(target));
            // The dwell reads one rect; 27 formatted tips a frame said
            // nothing the one under the pointer does not.
            if hover {
                s.tip(cell, rust_i18n::t!("gui.lib.jump_tip", letter = bucket_glyph(target)).to_string());
            }
        }
    }
}

// ── Tooltips ─────────────────────────────────────────────────────────────────

/// Greedy word wrap at `width` cells — the rooms' sentences, one algorithm.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    wrap_at(text, width, false)
}

/// Greedy word wrap for tooltip copy, at [`TIP_WRAP`] cells. A word wider
/// than the box (a file path) hard-breaks at the character level — the
/// greedy wrap would emit it as one line wider than the box, which clips.
pub fn wrap_tip(text: &str) -> Vec<String> {
    wrap_at(text, TIP_WRAP, true)
}

fn wrap_at(text: &str, width: usize, hard_break: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if hard_break && word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(width) {
                if chunk.len() == width {
                    lines.push(chunk.iter().collect());
                } else {
                    line = chunk.iter().collect();
                }
            }
            continue;
        }
        let need = if line.is_empty() { word.chars().count() } else { line.chars().count() + 1 + word.chars().count() };
        if need > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Where a w×h tooltip goes for a tip TARGET: anchored to the target's
/// rect — centered under it, above it when below would leave `area`,
/// pulled inside at the edges — so the box holds ONE spot however the
/// pointer moves within the target (and never redraws while it rests).
pub fn tooltip_rect(area: Rect, target: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let mut x = (target.x + target.width / 2).saturating_sub(w / 2);
    let mut y = target.bottom();
    if y + h > area.bottom() {
        y = target.y.saturating_sub(h);
    }
    if x + w > area.right() {
        x = area.right().saturating_sub(w);
    }
    Rect { x: x.max(area.x), y: y.max(area.y), width: w, height: h }
}

/// The caret cell that points the tooltip at its target: a box-drawing
/// stem merged INTO the border — `┴` on the top border when the box
/// hangs below the target, `┬` on the bottom border when it floats
/// above — at the target's center, clamped off the corners. None when
/// the box neither sits below nor above (degenerate clamps) or is too
/// narrow to keep its corners.
pub fn caret_cell(rect: Rect, target: Rect) -> Option<(u16, u16, &'static str)> {
    if rect.width < 3 {
        return None;
    }
    let x = (target.x + target.width / 2).clamp(rect.x + 1, rect.right().saturating_sub(2));
    if rect.y >= target.bottom() {
        Some((x, rect.y, "┴"))
    } else if rect.bottom() <= target.y {
        Some((x, rect.bottom().saturating_sub(1), "┬"))
    } else {
        None
    }
}

/// A miniature of the neutral modal, anchored to its target: Clear +
/// ground repaint beneath, Rounded DIM border with a caret stem pointing
/// at the target, wrapped default-fg text. Draw LAST — over everything.
/// Returns its footprint, for a screen with pixel surfaces to register
/// (see [`Surface::overlay`]).
pub fn draw_tooltip(frame: &mut Frame, area: Rect, target: Rect, text: &str) -> Rect {
    let lines = wrap_tip(text);
    if lines.is_empty() {
        return Rect::default();
    }
    let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16 + 4;
    let h = lines.len() as u16 + 2;
    let rect = tooltip_rect(area, target, w, h);
    frame.render_widget(Clear, rect);
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            Block::default().style(Style::default().bg(ground).fg(th().text)),
            rect,
        );
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(dim());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if let Some((x, y, glyph)) = caret_cell(rect, target) {
        frame.render_widget(
            Paragraph::new(Span::styled(glyph, dim())),
            Rect { x, y, width: 1, height: 1 },
        );
    }
    let body: Vec<Line> = lines.into_iter().map(|l| Line::from(format!(" {l}"))).collect();
    frame.render_widget(Paragraph::new(body), inner);
    rect
}

// ── Text input display ───────────────────────────────────────────────────────

/// The input line with the caret at the CURSOR (mid-line edits), windowed
/// so the caret stays visible: clipped edges render the clip mark. On bare
/// conhost the fancy caret (U+258F) and ellipsis are not in the legacy
/// fonts and draw as '?' - CP437's own `│` and `»` stand in, same single
/// cell, so the windowing math is identical (found live: the rename and
/// directory-modal carets rendered as question marks).
pub fn input_display(value: &str, cursor: usize, width: u16) -> String {
    input_display_blink(value, cursor, width, true)
}

/// [`input_display`] with the caret drawn or withheld — its cell stays
/// reserved either way, so the line never shifts as it blinks. A shell
/// asks [`Surface::caret`] for the phase.
pub fn input_display_blink(value: &str, cursor: usize, width: u16, on: bool) -> String {
    let (caret, clip) = if crate::kit::theme::legacy_conhost() {
        ('│', '»')
    } else {
        ('▏', '…')
    };
    input_display_with(value, cursor, width, if on { caret } else { ' ' }, clip)
}

/// Pure core - unit-tested with explicit marks so the assertions hold on
/// every OS and terminal the tests run under.
#[cfg(test)]
fn input_display_with_fancy(value: &str, cursor: usize, width: u16) -> String {
    input_display_with(value, cursor, width, '▏', '…')
}

pub fn input_display_with(value: &str, cursor: usize, width: u16, caret: char, clip: char) -> String {
    let w = width as usize;
    if w < 3 {
        return clip.to_string();
    }
    let mut chars: Vec<char> = value.chars().collect();
    let cursor = cursor.min(chars.len());
    chars.insert(cursor, caret);
    let total = chars.len();
    if total <= w {
        return chars.into_iter().collect();
    }
    let start = cursor.saturating_sub(w.saturating_sub(2)).min(total - w);
    let mut out: Vec<char> = chars[start..start + w].to_vec();
    if start > 0 {
        out[0] = clip;
    }
    if start + w < total {
        out[w - 1] = clip;
    }
    out.into_iter().collect()
}

// ── The pointer contract (OSC 22) ────────────────────────────────────────────

/// The OSC 22 payload for a pointer state — both name families, X cursor
/// names first and CSS names last, so every dialect lands on the same
/// shape: xterm (where OSC 22 originates) resolves the X/theme names,
/// while kitty, Ghostty and foot speak the kitty spec's CSS names.
/// Unknown names are ignored, so the pair is harmless everywhere else.
/// Probed 2026-08: NEITHER macOS terminal implements OSC 22 — Apple
/// Terminal (470.2) and iTerm2 (3.6.11) both keep their I-beam; their
/// pointers cannot be changed by any escape.
pub fn pointer_shape_seq(hand: bool) -> &'static str {
    if hand {
        "\x1b]22;hand2\x1b\\\x1b]22;pointer\x1b\\"
    } else {
        "\x1b]22;left_ptr\x1b\\\x1b]22;default\x1b\\"
    }
}

/// Empty name = hand the pointer back to the terminal's own behavior —
/// the shell underneath wants its text beam again, not our arrow.
pub const POINTER_RESET: &str = "\x1b]22;\x1b\\";

/// Set the pointer over the surface: the default arrow everywhere, a hand
/// over clickables. Announce once at startup (terminals keep their text
/// beam until an app says otherwise), then emit only on state CHANGES
/// so the stream is not littered with it.
pub fn set_pointer_shape(hand: bool, mouse_on: bool) {
    if !mouse_on {
        return;
    }
    let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(pointer_shape_seq(hand)));
}

/// Restores the terminal's original default background (the exact value
/// the OSC 11 query captured) on drop — including the unwind path, where
/// ratatui's panic hook restores everything except the background claim.
pub struct GroundGuard;
impl Drop for GroundGuard {
    fn drop(&mut self) {
        if let Some(seq) = theme::release_ground() {
            let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(seq));
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_footprints_are_the_next_frames_cover_to_draw_under() {
        let mut s: Surface<u8> = Surface::new();
        s.begin_frame();
        s.overlay(Rect { x: 0, y: 0, width: 10, height: 10 });
        let inside = Rect { x: 2, y: 2, width: 3, height: 3 };
        let outside = Rect { x: 20, y: 20, width: 3, height: 3 };
        assert!(!s.covered_last_frame(inside), "this frame's overlays draw after the base layer");
        s.begin_frame();
        assert!(s.covered_last_frame(inside), "last frame's footprint stands for one frame");
        assert!(!s.covered_last_frame(outside));
        s.clear_registries();
        assert!(s.covered_last_frame(inside), "the modal-inertness clear keeps the footprints");
        s.begin_frame();
        assert!(!s.covered_last_frame(inside), "and the frame after it leaves is clear");
    }

    #[test]
    fn pointer_shapes_speak_both_name_families_and_reset_is_empty() {
        assert_eq!(pointer_shape_seq(true), "\x1b]22;hand2\x1b\\\x1b]22;pointer\x1b\\");
        assert_eq!(pointer_shape_seq(false), "\x1b]22;left_ptr\x1b\\\x1b]22;default\x1b\\");
        assert_eq!(POINTER_RESET, "\x1b]22;\x1b\\");
    }

    #[test]
    fn tooltips_wrap_at_the_cap_and_never_split_words() {
        assert_eq!(wrap_tip("Remove this folder"), vec!["Remove this folder"]);
        let two = wrap_tip("This folder's name in mStream — click to rename");
        assert_eq!(two.len(), 2);
        assert!(two.iter().all(|l| l.chars().count() <= TIP_WRAP));
        assert_eq!(two.join(" "), "This folder's name in mStream — click to rename");
        assert!(wrap_tip("   ").is_empty());
    }

    #[test]
    fn tooltips_anchor_to_the_target_and_flip_inside_the_frame() {
        let area = Rect { x: 0, y: 0, width: 100, height: 40 };
        let mid = Rect { x: 40, y: 10, width: 14, height: 3 };
        let r = tooltip_rect(area, mid, 24, 3);
        assert_eq!((r.x, r.y), (35, 13));
        let bar = Rect { x: 84, y: 37, width: 14, height: 3 };
        let r = tooltip_rect(area, bar, 22, 3);
        assert_eq!(r.y, 34);
        assert!(r.right() <= 100);
        let x_ctl = Rect { x: 88, y: 12, width: 3, height: 1 };
        let r = tooltip_rect(area, x_ctl, 22, 3);
        assert!(r.right() <= 100 && r.bottom() <= 40);
        assert_eq!(r.y, 13);
        let r = tooltip_rect(area, mid, 200, 3);
        assert_eq!(r.width, 100);
    }

    #[test]
    fn the_caret_stem_points_at_the_target_center_from_the_connecting_edge() {
        let area = Rect { x: 0, y: 0, width: 100, height: 40 };
        let mid = Rect { x: 40, y: 10, width: 14, height: 3 };
        let r = tooltip_rect(area, mid, 24, 3);
        assert_eq!(caret_cell(r, mid), Some((47, r.y, "┴")));
        let bar = Rect { x: 84, y: 37, width: 14, height: 3 };
        let r = tooltip_rect(area, bar, 22, 3);
        assert_eq!(caret_cell(r, bar), Some((91, r.bottom() - 1, "┬")));
        let edge = Rect { x: 97, y: 10, width: 3, height: 1 };
        let r = tooltip_rect(area, edge, 22, 3);
        let (x, _, _) = caret_cell(r, edge).unwrap();
        assert!(x > r.x && x < r.right() - 1);
        assert_eq!(caret_cell(Rect { x: 0, y: 5, width: 2, height: 3 }, mid), None);
    }

    #[test]
    fn bar_jump_maps_the_track_proportionally_and_clamps_the_ends() {
        let bar = Rect { x: 79, y: 10, width: 1, height: 6 };
        assert_eq!(bar_jump(bar, 9, 11), 0, "top of the track");
        assert_eq!(bar_jump(bar, 9, 14), 9, "bottom of the track");
        assert_eq!(bar_jump(bar, 9, 12), 3, "proportional in between");
        assert_eq!(bar_jump(bar, 9, 10), 0, "endcap rows clamp to the ends");
        assert_eq!(bar_jump(bar, 9, 40), 9, "past the bar clamps too");
        let tiny = Rect { x: 79, y: 10, width: 1, height: 3 };
        assert_eq!(bar_jump(tiny, 4, 11), 2);
    }

    #[test]
    fn the_table_view_scrolls_freely_but_follows_a_reveal() {
        assert_eq!(table_view(3, None, 9, 10), (0, 3));
        assert_eq!(table_view(20, None, 5, 8), (5, 8));
        assert_eq!(table_view(20, None, 99, 8), (12, 8));
        assert_eq!(table_view(20, Some(15), 0, 8), (8, 8));
        assert_eq!(table_view(20, Some(2), 10, 8), (2, 8));
        assert_eq!(table_view(20, Some(6), 5, 8), (5, 8));
        assert_eq!(table_view(0, None, 0, 8), (0, 0));
        assert_eq!(table_view(5, None, 0, 0), (0, 0));
    }

    #[test]
    fn a_long_input_windows_around_the_cursor() {
        assert_eq!(input_display_with_fancy("short", 5, 20), "short▏");
        let long = "/very/long/path/that/does/not/fit/anywhere/music";
        let shown = input_display_with_fancy(long, long.chars().count(), 20);
        assert_eq!(shown.chars().count(), 20);
        assert!(shown.starts_with('…') && shown.ends_with("music▏"));
        let shown = input_display_with_fancy(long, 0, 20);
        assert!(shown.starts_with("▏/very") && shown.ends_with('…'));
        let shown = input_display_with_fancy(long, 24, 20);
        assert_eq!(shown.chars().count(), 20);
        assert!(shown.starts_with('…') && shown.ends_with('…') && shown.contains('▏'));
        assert_eq!(input_display_with_fancy("123456789", 4, 10), "1234▏56789");
    }

    #[test]
    fn the_caret_withheld_leaves_its_cell_so_the_line_never_shifts() {
        assert_eq!(input_display_blink("1234", 2, 10, false), "12 34");
        assert_eq!(input_display_blink("1234", 2, 10, true).chars().count(), 5);
        let long = "/very/long/path/that/does/not/fit/anywhere/music";
        let on = input_display_blink(long, 24, 20, true);
        let off = input_display_blink(long, 24, 20, false);
        assert_eq!((on.chars().count(), off.chars().count()), (20, 20));
        let differ: Vec<usize> =
            on.chars().zip(off.chars()).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect();
        assert_eq!(differ.len(), 1, "one cell blinks, the rest stand: {on} / {off}");
        assert_eq!(off.chars().nth(differ[0]), Some(' '));
    }

    #[test]
    fn the_surface_blinks_the_caret_half_a_second_at_a_time() {
        let mut s: Surface<i32> = Surface::new();
        assert!(s.caret_next_flip().is_none(), "no caret drawn, no flip to time");
        s.caret_touch();
        assert!(s.caret(), "solid right after a touch");
        let flip = s.caret_next_flip().expect("a caret drew this frame");
        assert!(flip <= CARET_BLINK && flip > Duration::from_millis(400), "{flip:?}");
        s.caret_backdate(CARET_BLINK + Duration::from_millis(50));
        assert!(!s.caret(), "off in the second half");
        s.caret_backdate(2 * CARET_BLINK + Duration::from_millis(50));
        assert!(s.caret(), "on again in the third");
        s.clear_registries();
        assert!(s.caret_next_flip().is_none(), "the frame's mark clears with the registries");
    }

    #[test]
    fn the_surface_arms_holds_on_endcaps_and_drags_on_the_track() {
        let mut s: Surface<i32> = Surface::new();
        let bar = Rect { x: 10, y: 5, width: 1, height: 6 };
        s.register_bar(bar, 9, -1, 1, Box::new(|p| p as i32 + 100));
        // Track press: drag arms; drag positions map through the bar.
        s.arm_bars(Position { x: 10, y: 7 });
        assert_eq!(s.drag_action(Position { x: 10, y: 9 }), Some(109));
        s.release();
        // Endcap press: hold arms; the first repeat waits out the delay.
        s.arm_bars(Position { x: 10, y: 10 });
        assert!(s.hold_action().is_none(), "the initial delay gates the first repeat");
        // Capture: motion no longer retargets the pointer.
        s.pointer = Some(Position { x: 10, y: 10 });
        s.motion(Position { x: 9, y: 10 });
        assert_eq!(s.pointer, Some(Position { x: 10, y: 10 }));
        s.release();
        // The instant release is a PHANTOM (Apple Terminal's dialect):
        // near-motion stays pinned, travel resumes hover.
        s.motion(Position { x: 9, y: 10 });
        assert_eq!(s.pointer, Some(Position { x: 10, y: 10 }), "soft capture pins tremor");
        s.motion(Position { x: 20, y: 10 });
        assert_eq!(s.pointer, Some(Position { x: 20, y: 10 }), "hover resumes on travel");
    }

    #[test]
    fn a_phantom_release_leaves_a_soft_capture_and_an_honest_one_does_not() {
        let mut s: Surface<i32> = Surface::new();
        let bar = Rect { x: 10, y: 5, width: 1, height: 6 };
        s.register_bar(bar, 9, -1, 1, Box::new(|p| p as i32));
        // Apple Terminal's dialect: press + instant release.
        s.begin_press(Position { x: 10, y: 5 });
        s.arm_bars(Position { x: 10, y: 5 });
        s.release();
        // Tremor near the press: hover stays pinned.
        s.motion(Position { x: 9, y: 6 });
        assert_eq!(s.pointer, Some(Position { x: 10, y: 5 }), "soft capture pins hover");
        // Travelling away resumes normal hover.
        s.motion(Position { x: 20, y: 5 });
        assert_eq!(s.pointer, Some(Position { x: 20, y: 5 }));
        s.motion(Position { x: 11, y: 5 });
        assert_eq!(s.pointer, Some(Position { x: 11, y: 5 }), "soft capture ended for good");

        // The phantom RE-CLICK at the physical release: near the press,
        // off the bar — swallowed whole.
        s.arm_bars(Position { x: 10, y: 5 });
        s.release();
        assert!(!s.begin_press(Position { x: 9, y: 6 }), "the release re-click is swallowed");
        // A press ON the bar inside the radius is a real step.
        assert!(s.begin_press(Position { x: 10, y: 6 }));
        // A press beyond the radius is a fresh interaction.
        s.arm_bars(Position { x: 10, y: 5 });
        s.release();
        assert!(s.begin_press(Position { x: 20, y: 5 }));
        s.motion(Position { x: 20, y: 6 });
        assert_eq!(s.pointer, Some(Position { x: 20, y: 6 }));

        // An HONEST hold (the release comes late) ends cleanly: no soft
        // capture, hover free right away.
        s.arm_bars(Position { x: 10, y: 5 });
        std::thread::sleep(PHANTOM_RELEASE + Duration::from_millis(20));
        s.release();
        s.motion(Position { x: 9, y: 6 });
        assert_eq!(s.pointer, Some(Position { x: 9, y: 6 }));
    }

    #[test]
    fn hits_prefer_the_last_drawn_rect() {
        let mut s: Surface<i32> = Surface::new();
        s.click(Rect { x: 0, y: 0, width: 10, height: 1 }, 1);
        s.click(Rect { x: 4, y: 0, width: 2, height: 1 }, 2);
        assert_eq!(s.hit(Position { x: 5, y: 0 }), Some(2), "the overlay wins");
        assert_eq!(s.hit(Position { x: 1, y: 0 }), Some(1));
        assert_eq!(s.hit(Position { x: 50, y: 0 }), None);
    }
}
