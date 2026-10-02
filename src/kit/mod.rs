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
//!
//! Frames reach the terminal through [`frames`]: whole, in one write, and
//! shown at once. Every full-screen page starts with its `init`.
//!
//! A press-drag that means something other than moving a thumb (marking a
//! run of log lines) goes through a drag region ([`Surface::drag_region`]):
//! it is told the press, every move while the button is held and the
//! release, wherever the pointer lands, and holds the pointer the way a
//! thumb drag does, so hover never wanders onto what the hand passes over.
//! Text leaves through [`clipboard`], whose routes follow where the player
//! runs; [`os`] names that place as data a test can hand in.

pub mod clipboard;
pub mod frames;
pub mod os;
pub mod theme;

use std::rc::Rc;
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

/// What a drag region is told: the press that took it, each move while
/// the button is held (wherever the pointer is by then), and the release
/// (wherever that lands, inside the region or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    Press,
    Drag,
    Release,
}

/// A registered drag region. `above` is how many clicks were registered
/// before it, so a press can tell the clicks drawn under it (the focus
/// click it shares the rows with) from those drawn over it (a menu's
/// catcher, a modal), which take the press instead.
struct RegionReg<A> {
    rect: Rect,
    above: usize,
    act: Rc<dyn Fn(Grip, Position) -> A>,
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
    /// This frame's drag regions, rebuilt each frame like `bars`.
    regions: Vec<RegionReg<A>>,
    /// The region a press took, held until the release. It keeps its own
    /// handle on the region's act rather than an index, because the drag
    /// outlives the frame it began in and the next frame may not draw the
    /// region at all.
    gripped: Option<Rc<dyn Fn(Grip, Position) -> A>>,
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
    /// Told of each overlay the moment it registers, while the frame is
    /// still drawing: the GUI window's cover board, which paints its
    /// pictures after the cells and so can stand one down on the very frame
    /// a modal opens over it, where the rule above leaves it painted over
    /// the modal for that frame. The order is the point: a cover placed
    /// before an overlay is under it, one placed after (the actions sheet's
    /// own header) is in it. `None` everywhere else, the terminal included,
    /// where nothing changes.
    overlay_watch: Option<Box<dyn Fn(Rect)>>,
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
    /// The cell the focused field drew its caret in this frame (blinked
    /// off or not), cleared with the registries like the frame's mark and
    /// again by every modal laid over the page (see [`Self::modal_over`]):
    /// it marks the field that has the keyboard, so a shell turns its
    /// input method and its paste on for it and floats the method's
    /// candidate list here. A field under a modal may still draw its caret
    /// (and keep the blink's clock), but the modal owns the keys.
    caret_at: Option<Position>,
    /// What an input method is composing for the focused field and has
    /// not committed. Only a shell that receives composition outside the
    /// key stream sets it — the GUI's own window; a terminal composes in
    /// its own UI and sends the commit as keys — so it stays empty, and
    /// changes nothing, everywhere else (see [`input_display_composing`]).
    composition: String,
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
            regions: Vec::new(),
            gripped: None,
            arrow_hold: None,
            armed: None,
            soft_origin: None,
            overlays: Vec::new(),
            covered: Vec::new(),
            overlay_watch: None,
            contexts: Vec::new(),
            key_hints: true,
            caret_since: None,
            caret_drawn: false,
            caret_at: None,
            composition: String::new(),
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
        if let Some(watch) = &self.overlay_watch {
            watch(rect);
        }
    }

    /// Have `watch` told of every overlay as it registers (see
    /// `overlay_watch`). Only the GUI's window watches.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub fn watch_overlays(&mut self, watch: impl Fn(Rect) + 'static) {
        self.overlay_watch = Some(Box::new(watch));
    }

    /// Whether an overlay stood over any part of `rect` LAST frame — the
    /// question a pixel surface asks before drawing pixels there.
    pub fn covered_last_frame(&self, rect: Rect) -> bool {
        self.covered.iter().any(|over| over.intersects(rect))
    }

    /// Whether this frame's overlays stand anywhere other than last
    /// frame's did: one opened, closed or moved. A pixel surface drew this
    /// frame by last frame's footprints, so a cover under a modal that just
    /// opened is still a picture painted over it, until the next frame
    /// draws it as text; a shell that sees this asks for that frame soon
    /// rather than at its idle poll.
    pub fn overlays_moved(&self) -> bool {
        self.overlays != self.covered
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
        self.regions.clear();
        self.contexts.clear();
        self.caret_drawn = false;
        self.caret_at = None;
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

    /// The focused field says where it drew its caret this frame — or
    /// where it would, for a field that shows a placeholder until the
    /// first key yet takes the keys all the same.
    pub fn note_caret(&mut self, at: Position) {
        self.caret_at = Some(at);
    }

    /// The cell of the field with the keyboard this frame, if one has it:
    /// a field drawn in the topmost layer. None while a modal with no
    /// field of its own is up over a page's focused field.
    // The GUI window places its IME box here, and the GUI's Admin tab
    // lifts a hosted room's onto its own surface in every build.
    pub fn caret_at(&self) -> Option<Position> {
        self.caret_at
    }

    /// A modal is laid over what drew so far: the keys are the modal's
    /// from here, so a field beneath stops counting as having them; one
    /// drawn inside the modal (after its frame) notes its caret again.
    /// The blink's flag stays, so a caret still showing beside the modal
    /// keeps its rhythm.
    pub fn modal_over(&mut self) {
        self.caret_at = None;
    }

    /// The input method's uncommitted text, or none (empty).
    // Only the GUI window has an input method to report (and the tests);
    // the GUI's Admin tab hands the GUI's on to a hosted room in every
    // build, where it is always empty but in the window.
    pub fn set_composition(&mut self, text: &str) {
        if self.composition != text {
            self.composition.clear();
            self.composition.push_str(text);
        }
    }

    pub fn composition(&self) -> &str {
        &self.composition
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
    /// A drag region's grip captures too: the hand sweeping across the
    /// page must not light up every control it passes over.
    pub fn motion(&mut self, at: Position) {
        if self.drag.is_some() || self.arrow_hold.is_some() || self.gripped.is_some() {
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
    /// invisible, a repeat could never be stopped). A drag region's grip
    /// ends here too, untold, so a host that only ever calls this never
    /// leaves one behind; [`Self::release_at`] is the call that tells it.
    pub fn release(&mut self) {
        if let Some((origin, when)) = self.armed.take() {
            if when.elapsed() < PHANTOM_RELEASE {
                self.soft_origin = Some(origin);
            }
        }
        self.drag = None;
        self.arrow_hold = None;
        self.gripped = None;
    }

    /// Register a drag region: a rect where a press-drag means something
    /// of the screen's own, told through `act` (see [`Grip`]). Registered
    /// every frame, like a scrollbar. It is transparent to clicks: [`Self::hit`]
    /// still returns the click under it, so the rows it covers can keep a
    /// click of their own (the one that focuses them).
    pub fn drag_region(&mut self, rect: Rect, act: impl Fn(Grip, Position) -> A + 'static) {
        self.regions.push(RegionReg { rect, above: self.clicks.len(), act: Rc::new(act) });
    }

    /// A press on a drag region takes it: the region is told the press and
    /// holds the pointer until the release. Call after the hit was
    /// dispatched, like [`Self::arm_bars`]. A click registered after the
    /// region over the same point wins instead (an open menu's catcher, a
    /// modal), and the answer is `None`. A region never arms the phantom
    /// soft capture: its drag is a gesture of the hand, not a hold to
    /// repeat, so a quick click on it leaves hover free.
    pub fn arm_region(&mut self, at: Position) -> Option<A> {
        let region = self.regions.iter().rev().find(|r| r.rect.contains(at))?;
        let over = self.clicks.get(region.above..).unwrap_or_default();
        if over.iter().any(|(rect, _)| rect.contains(at)) {
            return None;
        }
        let act = Rc::clone(&region.act);
        let press = act(Grip::Press, at);
        self.gripped = Some(act);
        Some(press)
    }

    /// Whether a drag region holds the pointer — the host's cue to keep
    /// every other pointer owner out until the release.
    pub fn gripping(&self) -> bool {
        self.gripped.is_some()
    }

    /// The button lifted at `at`: the gripped region is told the release,
    /// wherever it landed, and every capture ends as with [`Self::release`].
    /// `None` when no region was gripped.
    pub fn release_at(&mut self, at: Position) -> Option<A> {
        let act = self.gripped.take().map(|f| f(Grip::Release, at));
        self.release();
        act
    }

    /// The action a drag at `at` means: a gripped region told the move, or
    /// the thumb following the hand, if either is active.
    pub fn drag_action(&mut self, at: Position) -> Option<A> {
        if let Some(f) = &self.gripped {
            return Some(f(Grip::Drag, at));
        }
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

    /// Age the tooltip dwell, so a test can see a ripe tooltip without
    /// waiting out [`TIP_DELAY`].
    #[cfg(test)]
    pub fn dwell_backdate(&mut self, by: Duration) {
        if let Some((_, _, since)) = &mut self.dwell
            && let Some(earlier) = since.checked_sub(by)
        {
            *since = earlier;
        }
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
    s.modal_over();
    modal_frame_anchored(frame, area, width, height, max_height, title_color)
}

/// Where a modal of this size sits: centred in `area` — wherever the area
/// starts, so a room hosted under another shell's bar keeps its modals
/// inside its own rect — and vertically as if `max_height` tall, so one
/// that grows keeps its top edge.
pub fn modal_rect(area: Rect, width: u16, height: u16, max_height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    let max_height = max_height.max(height).min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - max_height) / 2,
        width,
        height,
    }
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
    frame_at(frame, modal_rect(area, width, height, max_height), title_color)
}

/// Wipe `rect` back to bare ground: `Clear`, then the fixed scheme's
/// ground repainted where the window's ground is owned — the first two
/// steps of [`frame_at`], without the border. A shell hosting another
/// page's drawing calls it over whatever that page must not have touched,
/// so a row the page wrote past its area never reaches the screen.
pub fn blank(frame: &mut Frame, rect: Rect) {
    frame.render_widget(Clear, rect);
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            Block::default().style(Style::default().bg(ground).fg(th().text)),
            rect,
        );
    }
}

/// A frame where the caller puts it — a dropdown under its control, a
/// modal at its computed spot: `Clear`, the ground repainted, a Rounded
/// border in `color`. Returns the inner rect.
pub fn frame_at(frame: &mut Frame, rect: Rect, color: Color) -> Rect {
    frame.render_widget(Clear, rect);
    // Clear resets cells to the terminal default — repaint the ground so
    // the interior matches the fixed scheme (when it is owned).
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            Block::default().style(Style::default().bg(ground).fg(th().text)),
            rect,
        );
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// The modal's close control: `[X]` on the title row, right edge. Dim
/// until hovered, then BRIGHT — dismissal is neutral, unlike a row
/// remove's destructive red. Esc remains the keyboard path (the tip
/// says so — the same words on every modal in the family).
pub fn modal_close<A: Clone>(frame: &mut Frame, s: &mut Surface<A>, inner: Rect, act: A) {
    let rect = modal_close_plain(frame, s, inner, act);
    s.tip_keyed(rect, rust_i18n::t!("path_modal.tip_close").to_string());
}

/// The same `[X]` close control without a tooltip — the admin rooms' choice:
/// their controls carry no tips, the tips line names the keys.
pub fn modal_close_plain<A: Clone>(frame: &mut Frame, s: &mut Surface<A>, inner: Rect, act: A) -> Rect {
    let rect = Rect { x: inner.right().saturating_sub(3), y: inner.y, width: 3, height: 1 };
    let hovered = s.hovers(rect);
    let style = if hovered {
        Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
    } else {
        dim()
    };
    frame.render_widget(Paragraph::new(Span::styled("[X]", style)), rect);
    s.click(rect, act);
    rect
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

/// Display width in cells — exactly what ratatui spends drawing `text`, so
/// budgets and cut points agree with the drawing: a CJK character is two
/// cells, ❤️ and 1️⃣ are two (their VS16 widens them), and a ZWJ family or
/// a skin-toned thumb is two, not the six or four its characters add up to.
///
/// The rule is ratatui's own (`Buffer::set_stringn`, ratatui-core 0.1):
/// the text is cut into extended graphemes, each takes its
/// [`grapheme_cells`], and the sum is the cells the buffer fills. Summing
/// per character instead counted ❤️ one cell narrower than it is drawn,
/// so a field or a clipped label ran a cell past its edge, and a family
/// four cells wider, so it was left out where it fit. One rule for both
/// flavours: the terminal paints by graphemes as the window does.
pub fn width(text: &str) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    if text.is_ascii() {
        // Every ASCII character is a grapheme of one cell, but for the
        // controls ratatui drops ("\r\n" is one grapheme, and dropped too).
        return text.bytes().filter(|b| !b.is_ascii_control()).count();
    }
    text.graphemes(true).map(grapheme_cells).sum()
}

/// One grapheme's cells, as ratatui's `set_stringn` takes them: a grapheme
/// holding a control character is dropped (no cells), and any other is its
/// `CellWidth` — `UnicodeWidthStr::width` (unicode-width 0.2) plus a cell
/// for each halfwidth (han)dakuten. A grapheme of no cells (a stray
/// combining mark at the start of the text) ratatui drops too.
pub fn grapheme_cells(grapheme: &str) -> usize {
    use ratatui::buffer::CellWidth;
    if grapheme.contains(char::is_control) {
        return 0;
    }
    usize::from(grapheme.cell_width())
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
/// offset, whether the next frame reveals the cursor (a keyboard move, a
/// fresh add; the wheel scrolls freely in between), and whether the
/// keyboard holds the cursor at all — the kit's list-cursor law. Stowed
/// (`held` false) is the resting state: no row is lit, and only a walking
/// key picks the cursor up; a row click or Esc puts it down again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListView {
    pub scroll: usize,
    pub reveal: bool,
    pub held: bool,
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

    /// A walking key: the keyboard takes the cursor, and the next frame
    /// brings it into view.
    pub fn pick_up(&mut self) {
        self.held = true;
        self.reveal = true;
    }

    /// Esc, a row click, a room opening: the keyboard's hand is down.
    pub fn stow(&mut self) {
        self.held = false;
    }

    /// The row to paint as the cursor: the list's own while the keyboard
    /// holds it, nothing otherwise. The viewport's reveal is not gated —
    /// it takes the real cursor, so a click's drill still shows its top.
    pub fn shown(&self, cursor: Option<usize>) -> Option<usize> {
        self.held.then_some(cursor).flatten()
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
        // Measured in cells, as the budget is: counting characters let a
        // Japanese line run to twice the width it was given.
        let need = if line.is_empty() { self::width(word) } else { self::width(&line) + 1 + self::width(word) };
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
    let (caret, clip) = input_marks();
    input_display_with(value, cursor, width, if on { caret } else { ' ' }, clip)
}

/// The caret and clip marks this terminal can draw (see [`input_display`]).
fn input_marks() -> (char, char) {
    if crate::kit::theme::legacy_conhost() {
        ('│', '»')
    } else {
        ('▏', '…')
    }
}

/// [`input_display_blink`] for the focused field, with an input method's
/// uncommitted text spliced in at the cursor and the caret after it — the
/// way every editor shows a composition in place, so にほん sits in the
/// field while it is typed and becomes the value only on commit. While
/// composing the caret holds solid: the keys are landing, as after any
/// key. Also the caret's offset from the line's start, in cells, for a
/// shell that places a candidate list by it. With no composition the
/// line is exactly [`input_display_blink`]'s.
///
/// A masked field (a password) hands its value in masked already, and
/// its mark as `mask`: the composition is drawn as that mark, one per
/// character, like the value around it — never in clear beside a row of
/// bullets.
pub fn input_display_composing(
    value: &str,
    cursor: usize,
    width: u16,
    on: bool,
    composition: &str,
    mask: Option<char>,
) -> (String, u16) {
    let (caret, clip) = input_marks();
    let (line, at) = if composition.is_empty() {
        input_window(value, cursor, width, if on { caret } else { ' ' }, clip)
    } else {
        let cursor = cursor.min(value.chars().count());
        let byte = value.char_indices().nth(cursor).map_or(value.len(), |(i, _)| i);
        let composition: String = match mask {
            Some(mark) => composition.chars().map(|_| mark).collect(),
            None => composition.to_string(),
        };
        let spliced = format!("{}{composition}{}", &value[..byte], &value[byte..]);
        input_window(&spliced, cursor + composition.chars().count(), width, caret, clip)
    };
    // The cells before the caret, by the rule the line is drawn by.
    let before: String = line.chars().take(at).collect();
    (line, u16::try_from(self::width(&before)).unwrap_or(u16::MAX))
}

/// The focused field of a page that draws on a surface of its own (an
/// admin room, hosted in the GUI's Admin tab or not): the line
/// [`input_display_composing`] draws, with the surface's composition and
/// the caret held steady, and the caret's cell noted on the surface
/// ([`Surface::note_caret`]), `x` and `y` being the cell the line starts
/// in. A host lifts the note onto its own surface, which is how the GUI's
/// window knows the room's field has the keyboard and turns its paste and
/// its input method on for it, the candidate list at the caret, as it
/// does for the GUI's own fields (`gui::text_field`). Everywhere else the
/// note goes unread and the composition stays empty, so the line is the
/// one [`input_display`] draws. A masked field hands its value in masked
/// already and its mark as `mask`. The caret does not blink here: a host
/// times its frames by its own surface's blink clock, not the page's.
pub fn field_display<A: Clone>(
    ui: &mut Surface<A>,
    x: u16,
    y: u16,
    value: &str,
    cursor: usize,
    width: u16,
    mask: Option<char>,
) -> String {
    let (line, caret) = field_line(ui, value, cursor, width, mask);
    ui.note_caret(Position { x: x.saturating_add(caret), y });
    line
}

/// [`field_display`]'s line and the caret's offset in it, in cells,
/// noting nothing: for a field whose line is placed only once it has been
/// measured (a filter right-aligned with what follows it), which notes
/// its caret itself when it knows where the line starts.
pub fn field_line<A: Clone>(
    ui: &Surface<A>,
    value: &str,
    cursor: usize,
    width: u16,
    mask: Option<char>,
) -> (String, u16) {
    input_display_composing(value, cursor, width, true, ui.composition(), mask)
}

/// Pure core - unit-tested with explicit marks so the assertions hold on
/// every OS and terminal the tests run under.
#[cfg(test)]
fn input_display_with_fancy(value: &str, cursor: usize, width: u16) -> String {
    input_display_with(value, cursor, width, '▏', '…')
}

pub fn input_display_with(value: &str, cursor: usize, width: u16, caret: char, clip: char) -> String {
    input_window(value, cursor, width, caret, clip).0
}

/// The windowed line, and the caret's index in it, in characters. The
/// clip marks never land on the caret: a window that starts past the
/// value's start keeps the caret at least one in, and one that stops short
/// of its end keeps it at least one short.
///
/// The window is measured in cells, not characters: `width` is the cells
/// the field has, and a wide character (kana, hanzi, hangul) takes two of
/// them. Counting characters let a line of CJK run to twice the field's
/// width, past its border, and put the caret's cell outside the field.
/// That was wrong in a terminal as much as in the GUI's window, so this is
/// a correctness fix both share; for text of narrow characters only, the
/// window is what it always was. A wide character that does not fit
/// beside a clip mark is left out whole, so a line may come up a cell
/// short of `width`, never over it.
///
/// And it walks graphemes, not characters, each at its [`grapheme_cells`]:
/// the clusters ratatui fills its cells by, so a cluster shows whole or not
/// at all and the line's cells are the cells drawn. Walking characters, the
/// characters that extend a grapheme (a flag's tags, a combining mark,
/// VS16, a ZWJ) took no cells: the walk back took them for free and stopped
/// on their base when it did not fit, so the line began with the tail of a
/// cluster whose base was cut (ratatui hangs it on the clip mark's cell,
/// which the window drew as a box), ❤️ counted a cell short and ran past
/// the field, and a ZWJ family counted six cells for its two. The caret is
/// a character of its own in the line it is cut from; a caret inside a
/// cluster (the cursor counts characters, so Left steps into a flag's tags)
/// splits it there, and the window keeps the caret's own cluster.
fn input_window(
    value: &str,
    cursor: usize,
    width: u16,
    caret: char,
    clip: char,
) -> (String, usize) {
    use unicode_segmentation::UnicodeSegmentation;
    let w = width as usize;
    if w < 3 {
        return (clip.to_string(), 0);
    }
    let mut chars: Vec<char> = value.chars().collect();
    let cursor = cursor.min(chars.len());
    chars.insert(cursor, caret);
    let line: String = chars.iter().collect();
    // Each cluster's first character, and the cells before it (a prefix
    // sum, so a run's cells are one subtraction): `starts[i]..starts[i + 1]`
    // are cluster i's characters, `before[j] - before[i]` the cells of
    // clusters i..j.
    let mut starts = vec![0];
    let mut before = vec![0];
    for grapheme in line.graphemes(true) {
        starts.push(starts[starts.len() - 1] + grapheme.chars().count());
        before.push(before[before.len() - 1] + grapheme_cells(grapheme));
    }
    let n = starts.len() - 1;
    let cells = |from: usize, to: usize| before[to] - before[from];
    if cells(0, n) <= w {
        return (line, cursor);
    }
    // The caret's cluster: the caret itself, or a cluster it begins (a
    // combining mark after it hangs on it).
    let at = starts.iter().rposition(|&s| s <= cursor).unwrap_or(0).min(n - 1);
    // The window is clusters start..end between the clip marks it needs: one
    // before when it starts past the value's start, one after when it
    // stops short of its end.
    let fits = |start: usize, end: usize| {
        cells(start, end) + usize::from(start > 0) + usize::from(end < n) <= w
    };
    // The caret is the window's last cluster before the trailing clip —
    // unless what follows it would take no more than the clip's own cell,
    // when it shows instead — and the window reaches back from there as
    // far as fits. One that reaches the value's start has room left after
    // the caret, which the text after it takes.
    let mut end = at + 1;
    if cells(end, n) <= 1 {
        end = n;
    }
    let mut start = end;
    while start > 0 && fits(start - 1, end) {
        start -= 1;
    }
    while start == 0 && end < n && fits(0, end + 1) {
        end += 1;
    }
    let (from, to) = (starts[start], starts[end]);
    let mut out = String::new();
    if start > 0 {
        out.push(clip);
    }
    out.extend(&chars[from..to]);
    if end < n {
        out.push(clip);
    }
    (out, cursor - from + usize::from(start > 0))
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

    /// The footprints moving is what a shell watches to bring the next
    /// frame forward: the frame an overlay opens on, the frame it closes on
    /// and the frame it moves on, never a frame that repeats the last.
    #[test]
    fn the_overlays_moving_is_told_on_the_frame_they_move() {
        let mut s: Surface<u8> = Surface::new();
        let modal = Rect { x: 0, y: 0, width: 10, height: 10 };
        s.begin_frame();
        assert!(!s.overlays_moved(), "nothing then nothing");
        s.overlay(modal);
        assert!(s.overlays_moved(), "the frame a modal opens on");
        s.begin_frame();
        s.overlay(modal);
        assert!(!s.overlays_moved(), "the same modal again is still");
        s.begin_frame();
        s.overlay(Rect { x: 1, ..modal });
        assert!(s.overlays_moved(), "a moved one");
        s.begin_frame();
        assert!(s.overlays_moved(), "the frame it closes on");
        s.begin_frame();
        assert!(!s.overlays_moved());
    }

    /// A watcher hears every overlay as it registers, in order, the modal
    /// frame's among them, and changes nothing the surface itself answers.
    #[test]
    fn an_overlay_watcher_is_told_each_footprint_as_it_registers() {
        use std::sync::{Arc, Mutex};
        let heard = Arc::new(Mutex::new(Vec::new()));
        let mut s: Surface<u8> = Surface::new();
        let ear = heard.clone();
        s.watch_overlays(move |rect| ear.lock().unwrap().push(rect));
        let tip = Rect { x: 1, y: 1, width: 4, height: 1 };
        s.begin_frame();
        let backend = ratatui::backend::TestBackend::new(40, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                modal_frame_on(frame, &mut s, frame.area(), 20, 6, Color::White);
            })
            .unwrap();
        s.overlay(tip);
        let modal = modal_rect(Rect::new(0, 0, 40, 12), 20, 6, 6);
        assert_eq!(*heard.lock().unwrap(), [modal, tip]);
        s.begin_frame();
        assert!(s.covered_last_frame(modal) && s.covered_last_frame(tip));
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
    fn sentences_wrap_by_cells_so_a_wide_script_keeps_inside_its_budget() {
        let text = "最初のユーザーがログインを有効にします — ウェブアプリ、アプリ、そしてこのプレイヤーで。";
        let lines = wrap_words(text, 79);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| width(l) <= 79), "{lines:?}");
        assert_eq!(lines.join(" "), text);
        assert_eq!(wrap_words("one two three", 7), vec!["one two", "three"], "a Latin line is as it was");
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
    fn the_list_view_shows_its_cursor_only_while_the_keyboard_holds_it() {
        // The list-cursor law: stowed at rest, so the row is painted only
        // after a walking key; the reveal comes with the pick-up and is
        // spent by the next frame; a stow leaves the cursor unpainted but
        // does not move it.
        let mut view = ListView::default();
        assert_eq!(view.shown(Some(3)), None, "nothing lit at rest");
        view.pick_up();
        assert_eq!(view.shown(Some(3)), Some(3), "picked up, the row paints");
        assert!(view.reveal, "and the frame brings it into view");
        assert_eq!(view.window(20, Some(15), 8), (8, 8), "the reveal yanks the window");
        assert!(!view.reveal, "once");
        view.stow();
        assert_eq!(view.shown(Some(3)), None, "stowed, nothing lit");
        assert_eq!(view.window(20, Some(3), 8), (8, 8), "and the window stays where it was");
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

    /// The window counts cells: wide text fits the field it is drawn in,
    /// in a terminal and in the window alike, and the caret's cell the
    /// composing line reports stays inside it. Counting characters, ten
    /// kana in a ten-cell field drew twenty cells and put the caret at 20.
    #[test]
    fn wide_text_windows_by_cells_and_keeps_the_caret_in_the_field() {
        let w = |line: &str| width(line);
        let kana = "あいうえおかきくけこ";
        // At the end: the clip, as many kana as fit, the caret.
        assert_eq!(input_display_with_fancy(kana, 10, 10), "…きくけこ▏");
        // At the start: the caret, the kana that fit, the clip.
        assert_eq!(input_display_with_fancy(kana, 0, 10), "▏あいうえ…");
        // Mid-value: the caret one short of the trailing clip. A fourth
        // kana before it would make eleven cells, so the line is nine.
        assert_eq!(input_display_with_fancy(kana, 6, 10), "…えおか▏…");
        // Narrow and wide together, cursor anywhere: never past the field.
        let mixed = "ab日本cd語ef한국gh";
        for width in 3..20u16 {
            for cursor in 0..=mixed.chars().count() {
                let shown = input_display_with_fancy(mixed, cursor, width);
                assert!(w(&shown) <= width as usize, "{shown:?} at {cursor} in {width}");
                assert!(shown.contains('▏'), "the caret always shows: {shown:?}");
            }
        }
        // A composition of kana: the caret's reported cell is in the field.
        for width in [6u16, 10, 15] {
            let (line, at) = input_display_composing("東京", 2, width, true, "にほんご", None);
            assert!(w(&line) <= width as usize, "{line:?} in {width}");
            assert!(at < width, "the caret's cell {at} is past a {width}-cell field: {line:?}");
            assert!(line.ends_with('▏'), "the caret follows the composition: {line:?}");
        }
    }

    /// A field never shows part of a cluster at its clipped ends. England's flag is 🏴 and six
    /// tag characters that take no cells; with the caret at the end of a value that scrolls,
    /// the line's left clip fell between 🏴 (two cells, which did not fit) and its tags (free),
    /// so the line began `…` and the tags, which ratatui hangs on the clip's cell and the
    /// window drew as a box. Every line, at every caret between clusters and every width, is
    /// whole clusters of the value between its marks.
    #[test]
    fn a_clipped_field_shows_a_cluster_whole_or_not_at_all() {
        use unicode_segmentation::UnicodeSegmentation;
        let england = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";
        // The reported case: the base does not fit beside the clip, the tags would.
        let value = format!("aaaa{england}bbbbbbbbbb");
        let end = value.chars().count();
        assert_eq!(input_display_with_fancy(&value, end, 13), "…bbbbbbbbbb▏");
        assert_eq!(input_display_with_fancy(&value, end, 14), format!("…{england}bbbbbbbbbb▏"));
        // And at the other end, the window reaching right from the value's start: a ZWJ
        // family whose man fits and whose woman does not was cut after the joiner. It is
        // one cluster of two cells, ratatui's width for it: left out whole where those two
        // cells and the clip do not fit, shown whole where they do (counted per character,
        // its six cells kept it out of a line with room for it).
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(input_display_with_fancy(&format!("aa{family}bb"), 0, 5), "▏aa…");
        let shown = input_display_with_fancy(&format!("aa{family}bb"), 0, 6);
        assert_eq!(shown, format!("▏aa{family}…"));
        let w = |line: &str| width(line);
        let clusters = [
            england,
            "e\u{301}",
            "\u{2764}\u{FE0F}",
            "\u{1F1FA}\u{1F1F8}",
            family,
            "\u{1F44D}\u{1F3FD}",
        ];
        for cluster in clusters {
            let value = format!("ab{cluster}cd{cluster}ef");
            let mut bounds = vec![0];
            for grapheme in value.graphemes(true) {
                bounds.push(bounds[bounds.len() - 1] + grapheme.chars().count());
            }
            let chars: Vec<char> = value.chars().collect();
            for &cursor in &bounds {
                for width in 3..24u16 {
                    let shown = input_display_with_fancy(&value, cursor, width);
                    assert!(w(&shown) <= width as usize, "{shown:?} at {cursor} in {width}");
                    let text: String = shown.chars().filter(|&c| c != '▏' && c != '…').collect();
                    let shown_chars = text.chars().count();
                    let found = (0..=chars.len() - shown_chars).find(|&from| {
                        chars[from..from + shown_chars].iter().copied().eq(text.chars())
                            && bounds.contains(&from)
                            && bounds.contains(&(from + shown_chars))
                    });
                    assert!(found.is_some(), "{shown:?} at {cursor} in {width} cuts a cluster");
                }
            }
        }
    }

    /// The clusters the width rule is held to ratatui on: an emoji with VS16, a keycap, a flag of
    /// regional indicators, a subdivision flag of tags, a ZWJ family, a skin tone, hangul, kana,
    /// a hanzi, a decomposed é, a plain word, and a mixed string of them.
    const PARITY: [&str; 12] = [
        "\u{2764}\u{FE0F}",
        "1\u{FE0F}\u{20E3}",
        "\u{1F1FA}\u{1F1F8}",
        "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        "\u{1F44D}\u{1F3FD}",
        "\u{D55C}",
        "\u{304B}",
        "\u{65E5}",
        "e\u{301}",
        "music",
        concat!(
            "a\u{2764}\u{FE0F}b1\u{FE0F}\u{20E3}\u{1F1FA}\u{1F1F8}c",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{1F44D}\u{1F3FD}",
            "\u{D55C}\u{304B}\u{65E5}e\u{301}",
        ),
    ];

    /// The cells ratatui fills setting `text` into a buffer wide enough for it: the cursor's
    /// advance, which counts each grapheme's cell and the cells its width hides.
    fn ratatui_cells(text: &str) -> usize {
        let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 200, 1));
        let (x, _) = buffer.set_stringn(0, 0, text, usize::MAX, Style::default());
        x as usize
    }

    /// The row `line` leaves in a 40-cell `TestBackend` filled with `#`, set from column 0
    /// the way the GUI's `put` sets it: with the buffer's edge as its only budget.
    fn drawn_over_sentinels(line: &str) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
        terminal
            .draw(|frame| {
                let buffer = frame.buffer_mut();
                for x in 0..40 {
                    buffer[(x, 0)].set_symbol("#");
                }
                buffer.set_stringn(0, 0, line, usize::MAX, Style::default());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..40).map(|x| buffer[(x, 0)].symbol().to_string()).collect()
    }

    /// One width rule, ratatui's: the kit measures every cluster at the cells ratatui fills
    /// with it. Summing per character, ❤️ and 1️⃣ were a cell short (their VS16 widens them),
    /// a ZWJ family four cells over and a skin tone two.
    #[test]
    fn the_width_rule_is_the_cells_ratatui_draws() {
        use unicode_width::UnicodeWidthChar;
        let per_char = |text: &str| text.chars().filter_map(UnicodeWidthChar::width).sum::<usize>();
        for text in PARITY {
            assert_eq!(width(text), ratatui_cells(text), "{text:?}: the kit's against ratatui's");
            let graphemes: usize = unicode_segmentation::UnicodeSegmentation::graphemes(text, true)
                .map(grapheme_cells)
                .sum();
            assert_eq!(graphemes, width(text), "{text:?}: grapheme by grapheme");
            eprintln!("{text:?}: {} cells, {} summed per character", width(text), per_char(text));
        }
        for cluster in &PARITY[..10] {
            assert!(width(cluster) <= 2, "{cluster:?} is one cluster of at most two cells");
        }
        // The controls ratatui drops, and a mark with no base, take no cells.
        for text in ["a\tb", "a\r\nb", "\u{301}a", "a\u{7}"] {
            assert_eq!(width(text), ratatui_cells(text), "{text:?}");
        }
        // Halfwidth katakana's sound mark: its own cell, as ratatui counts it.
        assert_eq!(width("\u{FF76}\u{FF9E}"), ratatui_cells("\u{FF76}\u{FF9E}"));
    }

    /// A field N cells wide never writes past its Nth cell, whatever clusters it holds and
    /// wherever its caret: the cell after it keeps the `#` it had. And the caret's reported
    /// cell is where ratatui draws the caret.
    #[test]
    fn a_field_of_any_cluster_stays_inside_its_cells() {
        use unicode_segmentation::UnicodeSegmentation;
        for cluster in PARITY {
            let values = [cluster.repeat(9), format!("ab{cluster}cd{cluster}ef{cluster}gh")];
            for value in values {
                let mut bounds = vec![0];
                for grapheme in value.graphemes(true) {
                    bounds.push(bounds[bounds.len() - 1] + grapheme.chars().count());
                }
                for &cursor in &bounds {
                    for n in 3..30u16 {
                        let (line, at) = input_display_composing(&value, cursor, n, true, "", None);
                        let row = drawn_over_sentinels(&line);
                        assert_eq!(width(&line), ratatui_cells(&line), "{line:?}");
                        assert!(width(&line) <= n as usize, "{line:?} at {cursor} over {n} cells");
                        for (x, cell) in row.iter().enumerate().skip(n as usize) {
                            assert_eq!(cell, "#", "{line:?} at {cursor} in {n} wrote cell {x}");
                        }
                        let caret = (0..40).find(|&x| row[x].contains(input_marks().0));
                        assert_eq!(caret, Some(at as usize), "{line:?}: the caret's cell");
                    }
                }
            }
        }
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
    fn a_composition_splices_in_at_the_cursor_with_the_caret_after_it() {
        // No composition: the very line the blinking caret draws, either
        // phase, and the caret's cell after the text before it.
        for on in [true, false] {
            let (line, at) = input_display_composing("abcd", 2, 20, on, "", None);
            assert_eq!(line, input_display_blink("abcd", 2, 20, on));
            assert_eq!(at, 2);
        }
        // Composing at the end, mid-value and at the start: the text goes
        // in at the cursor and the caret follows it, solid even in the
        // blink's off phase. Kana are two cells each.
        let caret = input_display_blink("", 0, 5, true);
        let shown = input_display_composing("", 0, 20, false, "にほん", None);
        assert_eq!(shown, (format!("にほん{caret}"), 6));
        let shown = input_display_composing("ab", 1, 20, true, "x", None);
        assert_eq!(shown, (format!("ax{caret}b"), 2));
        let shown = input_display_composing("ab", 0, 20, false, "日", None);
        assert_eq!(shown, (format!("日{caret}ab"), 2));
        // The value itself is not touched; a cursor past the end is the
        // end; a long composition windows around the caret like any text.
        assert_eq!(input_display_composing("ab", 9, 20, true, "c", None).0, format!("abc{caret}"));
        let (line, at) = input_display_composing("0123456789", 10, 8, true, "abcdef", None);
        assert_eq!(line.chars().count(), 8);
        assert!(line.ends_with(&format!("cdef{caret}")), "{line}");
        assert_eq!(at, 7);
    }

    /// A masked field's composition is drawn as its mark, one per
    /// character: nothing an input method composes for a password shows in
    /// clear. The value comes in masked already, as the field draws it.
    #[test]
    fn a_masked_fields_composition_is_drawn_masked() {
        let caret = input_display_blink("", 0, 5, true);
        let (line, at) = input_display_composing("••", 2, 20, true, "にほ", Some('•'));
        assert_eq!((line.as_str(), at), (format!("••••{caret}").as_str(), 4));
        assert!(!line.contains('に') && !line.contains('ほ'), "{line}");
        // Mid-value too, and an unmasked field is untouched.
        let (line, _) = input_display_composing("•••", 1, 20, false, "pw", Some('•'));
        assert_eq!(line, format!("•••{caret}••"));
        let plain = input_display_composing("ab", 2, 20, true, "pw", None).0;
        assert_eq!(plain, format!("abpw{caret}"));
    }

    #[test]
    fn the_surface_keeps_the_caret_cell_for_a_frame_and_the_composition_until_told() {
        let mut s: Surface<i32> = Surface::new();
        assert_eq!((s.caret_at(), s.composition()), (None, ""));
        s.note_caret(Position { x: 7, y: 3 });
        s.set_composition("にほ");
        assert_eq!(s.caret_at(), Some(Position { x: 7, y: 3 }));
        s.begin_frame();
        assert_eq!(s.caret_at(), None, "a frame that draws no field has no caret");
        assert_eq!(s.composition(), "にほ", "the input method's text outlives frames");
        s.set_composition("");
        assert_eq!(s.composition(), "");
    }

    /// A hosted page's focused field ([`field_display`]): while nothing is
    /// composing the line is the one [`input_display`] draws, and the cell
    /// noted holds the caret wherever the window over a long value stands;
    /// a composition goes in before the caret, a masked field's as marks.
    #[test]
    fn a_hosted_field_notes_the_cell_its_caret_is_drawn_in_however_the_value_is_windowed() {
        let caret = input_display_blink("", 0, 5, true).chars().next().unwrap();
        let mut s: Surface<i32> = Surface::new();
        let value = "0123456789abcdefghij";
        // The cursor at the end, mid-value and at the start of a value
        // twice the field's width.
        for cursor in [20, 10, 0] {
            let line = field_display(&mut s, 30, 4, value, cursor, 10, None);
            assert_eq!(line, input_display(value, cursor, 10));
            let at = s.caret_at().expect("the field noted its caret");
            assert_eq!(at.y, 4);
            assert_eq!(line.chars().nth(usize::from(at.x - 30)), Some(caret), "cursor {cursor}: {line:?}");
        }
        // Kana are two cells each: the caret's cell counts them so.
        s.set_composition("にほ");
        let line = field_display(&mut s, 30, 4, value, 20, 10, None);
        let before: String = line.chars().take_while(|c| *c != caret).collect();
        assert!(before.ends_with("にほ"), "the composition before the caret: {line:?}");
        assert_eq!(s.caret_at(), Some(Position { x: 30 + width(&before) as u16, y: 4 }));
        let line = field_display(&mut s, 30, 4, "••", 2, 10, Some('•'));
        assert_eq!(line, format!("••••{caret}"), "a masked field's composition is marks");
        // The bare line notes nothing, for a field placed once measured.
        s.begin_frame();
        assert_eq!(field_line(&s, "ab", 2, 10, None), (format!("abにほ{caret}"), 6));
        assert_eq!(s.caret_at(), None);
    }

    #[test]
    fn a_modal_takes_the_keyboard_from_the_field_beneath_it() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let mut s: Surface<i32> = Surface::new();
        terminal
            .draw(|frame| {
                // The page's field, then a modal with none: the field's
                // caret still blinks beside it, but it has no keyboard.
                s.caret();
                s.note_caret(Position { x: 3, y: 1 });
                modal_frame_on(frame, &mut s, frame.area(), 20, 5, Color::Reset);
                assert_eq!(s.caret_at(), None);
                assert!(s.caret_next_flip().is_some(), "the blink keeps its clock");
                // A field drawn inside the modal has it again.
                s.note_caret(Position { x: 12, y: 5 });
            })
            .unwrap();
        assert_eq!(s.caret_at(), Some(Position { x: 12, y: 5 }));
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

    /// The acts a test region emits: what it was told, and where.
    fn told(g: Grip, at: Position) -> (Grip, u16, u16) {
        (g, at.x, at.y)
    }

    #[test]
    fn a_drag_region_arms_on_a_press_follows_the_hand_and_ends_on_the_release_anywhere() {
        let mut s: Surface<(Grip, u16, u16)> = Surface::new();
        let rows = Rect { x: 2, y: 3, width: 20, height: 5 };
        s.drag_region(rows, told);
        assert_eq!(s.arm_region(Position { x: 40, y: 3 }), None, "a press beside the region takes nothing");
        assert!(!s.gripping());
        assert_eq!(s.arm_region(Position { x: 5, y: 4 }), Some((Grip::Press, 5, 4)));
        assert!(s.gripping());
        assert_eq!(s.drag_action(Position { x: 6, y: 6 }), Some((Grip::Drag, 6, 6)));
        assert_eq!(s.drag_action(Position { x: 70, y: 0 }), Some((Grip::Drag, 70, 0)), "the hand may leave the region");
        assert_eq!(s.release_at(Position { x: 70, y: 30 }), Some((Grip::Release, 70, 30)), "and let go anywhere");
        assert!(!s.gripping());
        assert_eq!(s.drag_action(Position { x: 6, y: 6 }), None, "nothing follows the hand after the release");
        assert_eq!(s.release_at(Position { x: 6, y: 6 }), None, "a second release tells nobody");
    }

    #[test]
    fn a_grip_holds_the_pointer_like_a_thumb_drag() {
        let mut s: Surface<(Grip, u16, u16)> = Surface::new();
        s.drag_region(Rect { x: 0, y: 0, width: 10, height: 4 }, told);
        let press = Position { x: 3, y: 1 };
        assert!(s.begin_press(press));
        s.arm_region(press);
        s.motion(Position { x: 30, y: 9 });
        assert_eq!(s.pointer, Some(press), "hover stays at the press while gripping");
        s.release_at(Position { x: 30, y: 9 });
        s.motion(Position { x: 30, y: 9 });
        assert_eq!(s.pointer, Some(Position { x: 30, y: 9 }), "free after the release");
    }

    #[test]
    fn a_click_registered_over_a_region_after_it_takes_the_press() {
        let mut s: Surface<(Grip, u16, u16)> = Surface::new();
        let rows = Rect { x: 0, y: 0, width: 20, height: 6 };
        let focus = (Grip::Release, 99, 99);
        s.click(rows, focus);
        s.drag_region(rows, told);
        let catcher = (Grip::Release, 77, 77);
        s.click(Rect { x: 10, y: 2, width: 10, height: 4 }, catcher);
        let under_menu = Position { x: 12, y: 3 };
        assert_eq!(s.hit(under_menu), Some(catcher));
        assert_eq!(s.arm_region(under_menu), None, "the catcher drawn after the region wins");
        assert!(!s.gripping());
        let on_a_line = Position { x: 4, y: 3 };
        assert_eq!(s.hit(on_a_line), Some(focus), "the region is transparent to the click drawn before it");
        assert_eq!(s.arm_region(on_a_line), Some((Grip::Press, 4, 3)));
    }

    #[test]
    fn regions_clear_with_the_registries_and_a_grip_outlives_its_frame() {
        let mut s: Surface<(Grip, u16, u16)> = Surface::new();
        let rows = Rect { x: 0, y: 0, width: 20, height: 6 };
        s.begin_frame();
        s.drag_region(rows, told);
        s.arm_region(Position { x: 1, y: 1 });
        s.begin_frame();
        assert_eq!(s.drag_action(Position { x: 2, y: 2 }), Some((Grip::Drag, 2, 2)), "the grip keeps its own act");
        assert_eq!(s.arm_region(Position { x: 1, y: 1 }), None, "the next frame drew no region");
        s.release();
        assert!(!s.gripping(), "the plain release drops the grip");
        assert_eq!(s.drag_action(Position { x: 2, y: 2 }), None);
    }

    #[test]
    fn a_quick_click_on_a_region_leaves_no_soft_capture() {
        let mut s: Surface<(Grip, u16, u16)> = Surface::new();
        s.drag_region(Rect { x: 0, y: 0, width: 20, height: 6 }, told);
        let press = Position { x: 5, y: 2 };
        assert!(s.begin_press(press));
        s.arm_region(press);
        s.release_at(press);
        assert!(s.begin_press(Position { x: 6, y: 2 }), "a press one cell away is a press, not a phantom");
        s.motion(Position { x: 6, y: 3 });
        assert_eq!(s.pointer, Some(Position { x: 6, y: 3 }), "hover is free at once");
    }

    #[test]
    fn a_modal_centres_in_its_area_wherever_the_area_starts() {
        let origin = Rect { x: 0, y: 0, width: 100, height: 30 };
        assert_eq!(
            modal_rect(origin, 60, 10, 10),
            Rect { x: 20, y: 10, width: 60, height: 10 },
            "an area at the origin puts the modal exactly where it always sat"
        );
        assert_eq!(modal_rect(origin, 60, 6, 10), Rect { x: 20, y: 10, width: 60, height: 6 }, "a growing modal keeps its top edge");
        let hosted = Rect { x: 17, y: 1, width: 83, height: 23 };
        let r = modal_rect(hosted, 60, 10, 10);
        assert_eq!(r, Rect { x: 17 + 11, y: 1 + 6, width: 60, height: 10 });
        assert!(hosted.contains(Position { x: r.x, y: r.y }) && r.right() <= hosted.right() && r.bottom() <= hosted.bottom());
        let short = Rect { x: 17, y: 1, width: 83, height: 17 };
        let r = modal_rect(short, 84, 22, 22);
        assert_eq!(r, Rect { x: 19, y: 2, width: 79, height: 15 }, "a form taller and wider than the area clamps inside it");
    }

    #[test]
    fn blank_clears_the_rect_and_nothing_else() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        let rect = Rect { x: 4, y: 2, width: 6, height: 3 };
        terminal
            .draw(|frame| {
                let all = frame.area();
                for y in all.top()..all.bottom() {
                    for x in all.left()..all.right() {
                        frame.buffer_mut()[(x, y)].set_symbol("x");
                    }
                }
                blank(frame, rect);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        for y in 0..8 {
            for x in 0..20 {
                let want = if rect.contains(Position { x, y }) { " " } else { "x" };
                assert_eq!(buffer[(x, y)].symbol(), want, "cell ({x}, {y})");
            }
        }
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
