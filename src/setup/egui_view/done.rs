//! The Done page drawn by egui, the wizard's last step and the Quick
//! Connect page alike (`qr --window`: the same page with the wizard's
//! standalone switch, no step and no scan), after the canvas's Done,
//! DoneRest and QuickConnect boards. It owns its whole page, as the cells'
//! Done owns its frame: the top strip with the step, then the two columns
//! the cells draw wherever pictures can be drawn, which a window always
//! can, and under them the status line and the tips line, with no rule and
//! no bar.
//!
//! The left column is the wordmark and, under it, the Quick Connect code as
//! a picture; the right column is the message (where the server listens,
//! and what became of Quick Connect), the scan's three lines on the
//! wizard's page, and the three links as cards. There is no heading: the
//! wordmark heads the page, as it does in the cells' two columns.
//!
//! A click on a card is the cells' click on it: the act that opens its link
//! through the model, which says on the status line that the browser has
//! it, or gives the link to copy where none could be launched. Nothing on
//! the page ends the wizard: there is no Finish to click, as in the cells,
//! and f, q and Esc end it from the keys. The keyboard's selection over the
//! cards is the model's own (↑ ↓ move it, Enter opens it), kept apart from
//! the pointer's hover: a click never moves it.

use egui::epaint::StrokeKind;
use egui::{Align2, Id, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};
use rust_i18n::t;

use super::super::cells::done_buttons;
use super::super::{DONE_ACTIONS, Wizard, done_action};
use super::{Drawn, LINE_H, Page, ROOMY, SIDE, logo_size, logo_texture, parts, scan_widget, status_line, strip, tip, tips_line};
use crate::gui::window::studio::{self, HIT, Pal, Para, Run, body, mono, strong};
use crate::tui::art::Art;

/// The two columns' block at its widest, the boards' 920 points, centred on
/// the page; its left column at its widest, 300, and never more than half
/// the block, as the cells' left column is never more than half the page;
/// and the gutter between the columns, 32, with the rule 16 points into it.
const BLOCK_W: f32 = 920.0;
const LEFT_W: f32 = 300.0;
const GUTTER: f32 = 32.0;
const RULE_IN: f32 = 16.0;
/// The most the block stands under the top of the room the page has for it:
/// the boards' 150, 106 under the strip.
const BLOCK_DROP: f32 = 86.0;
/// The share of the room the block leaves that goes over it, up to
/// [`BLOCK_DROP`].
const DROP_SHARE: f32 = 0.3;
/// The wordmark's band at its tallest (the boards' 288 by 72 in the left
/// column's 300), the gap under it, and the code's square band at its
/// largest (the boards' 280).
const MARK_H: f32 = 72.0;
const MARK_GAP: f32 = 24.0;
const CODE_SIDE: f32 = 280.0;
/// The message's size and leading, and the fewest lines it keeps room for
/// (the boards' 87 points), so nothing under it moves when Quick Connect's
/// sentence lands or a translation runs long.
const SAY: f32 = 15.0;
const SAY_LINE: f32 = 1.45;
const SAY_LINES: usize = 4;
/// The gap between the right column's parts, the boards' 20.
const PART_GAP: f32 = 20.0;
/// The scan's slot on the wizard's page: its three lines, kept from the
/// page's first frame so nothing moves when the first report lands.
const SCAN_H: f32 = 3.0 * LINE_H;
/// A link card, the boards' 420 by 56 and 8 apart, its label 18 points in;
/// and the short card the cells' one-row list becomes where the tall cards
/// have no room, a control's [`HIT`] tall and 4 apart.
const CARD_W: f32 = 420.0;
const CARD_H: f32 = 56.0;
const CARD_GAP: f32 = 8.0;
const CARD_PAD: f32 = 18.0;
const SHORT_GAP: f32 = 4.0;
/// The tips line's foot over the page's bottom, the boards' 18.
const FOOT: f32 = 18.0;

/// The whole page over the whole of `ui`: the top strip (the step on the
/// wizard's page, nothing on the Quick Connect page), the wordmark and the
/// code in the left column, the rule, the message, the scan and the cards
/// in the right column, then the status line and the tips line. What its
/// clicks asked for: a card's link opened, in the cells' act for it. No
/// field, so no caret.
pub(super) fn show(ui: &mut Ui, wizard: &mut Wizard, page: &Page) -> Drawn {
    let pal = &page.pal;
    let mut drawn = Drawn::default();
    let full = ui.max_rect();
    let (words, figure) = message(wizard);
    let says = studio::with_figures(&words, &[&figure], body(SAY), mono(SAY - 1.0), pal.text);
    let mark = logo_size(ui.ctx());
    let at = places(full, wizard.standalone, |w| said_height(ui, &says, w), mark);
    strip(ui, at.strip, wizard, page, &mut drawn.acts);

    // The left column: the wordmark over the ground with its own alpha, and
    // the code under it, whole, its modules square. Bare ground for the
    // code while there is none to draw, as the message says why.
    if let Some(band) = at.mark
        && let Some(texture) = logo_texture(ui.ctx(), band)
    {
        studio::picture(ui, band, &texture);
    }
    if let Some(art) = &wizard.qr_art
        && at.code.width() >= 1.0
        && let Some(texture) = code_texture(ui.ctx(), art)
    {
        studio::picture(ui, at.code, &texture);
    }
    // The one rule, a hairline down the block: in the kit's look gold is a
    // warning, as the frame's foot rule says.
    ui.painter().line_segment([at.rule.center_top(), at.rule.center_bottom()], Stroke::new(1.0, pal.edge));

    // The right column: the message, cut by whole lines only where the page
    // is too short for it, then the scan as it stands, then the cards.
    Para::runs(&says).line(SAY_LINE).floor(at.message.max.y).paint(ui, at.message.min, at.message.width());
    if let (Some(slot), Some(scan)) = (at.scan, &wizard.scan) {
        scan_widget(ui, slot, scan, pal);
    }
    for (i, ((label, why), rect)) in done_buttons().into_iter().zip(at.cards).enumerate() {
        let response = link_card(ui, Id::new("setup-link").with(i), rect, &label, wizard.done_sel == Some(i), pal);
        tip(page, &response, &why);
        if response.clicked() {
            drawn.acts.push(done_action(i));
        }
    }

    status_line(ui, at.status, wizard, pal);
    tips_line(ui, at.tips, wizard, pal);
    drawn
}

/// The message and its figure, in the cells' words: where the server
/// listens (its port, or the whole address where none can be read), then
/// what became of Quick Connect (that the code is on the left, or the
/// model's note for it: off, starting, a ticket that would not draw;
/// nothing while it loads or where the read failed, which the status line
/// says). The figure is what the monospace sets: the port, or the address.
fn message(wizard: &Wizard) -> (String, String) {
    let server = wizard.client.server();
    let (opening, figure) = match server_port(&server) {
        Some(port) => (t!("done.running_port", port = port).to_string(), port.to_string()),
        None => (t!("done.running_at", server = server).to_string(), server),
    };
    let second = if wizard.qr.is_some() { t!("done.scan_left").to_string() } else { wizard.qr_note.clone() };
    // The cells join the two with a space and trim it where there is no
    // second; here there is none to trim.
    let words = if second.is_empty() { opening } else { format!("{opening} {second}") };
    (words, figure)
}

/// The port a server URL listens on, as the cells read it for the same
/// sentence (cells.rs `server_port`): the figures after the last colon,
/// else the scheme's own, else none.
fn server_port(server: &str) -> Option<u16> {
    if let Some((_, tail)) = server.rsplit_once(':')
        && let Ok(port) = tail.trim_end_matches('/').parse()
    {
        return Some(port);
    }
    match server.split_once("://").map(|(scheme, _)| scheme) {
        Some("http") => Some(80),
        Some("https") => Some(443),
        _ => None,
    }
}

/// How tall the message stands at `w`, exactly as [`Para`] lays it out
/// (egui rounds each line to a whole point: four lines are 88 points, not
/// 87), before the page decides what goes under it: a paint with its floor
/// at its own top, which leaves every line out whole, so nothing is drawn
/// and only its height is had.
fn said_height(ui: &Ui, says: &[Run], w: f32) -> f32 {
    let at = ui.max_rect().min;
    Para::runs(says).line(SAY_LINE).floor(at.y).paint(ui, at, w).height()
}

/// Where the page's parts stand on it.
struct Places {
    strip: Rect,
    /// The wordmark's band, where there is a wordmark to draw.
    mark: Option<Rect>,
    /// The code's square band, centred in the left column.
    code: Rect,
    /// The rule, a point wide, the block's height.
    rule: Rect,
    /// The message's slot: as tall as its lines want, never fewer than
    /// [`SAY_LINES`], and cut only where the page is too short.
    message: Rect,
    /// The scan's slot, on the wizard's page alone.
    scan: Option<Rect>,
    cards: [Rect; DONE_ACTIONS],
    status: Rect,
    tips: Rect,
}

/// Where each part stands on a page `full`, the Quick Connect page or the
/// wizard's (`standalone`), with a message `say(w)` points tall wrapped at
/// `w` and a wordmark `mark` pixels (none where it cannot be drawn). The
/// boards' measures on 1280 by 800: the strip y 16 to 44; the block 920
/// wide and centred, from y 150; the left column 300 wide, the wordmark
/// 288 by 72 in it and the code's 280 square 24 under that; the rule 16
/// into the 32 gutter; the right column the rest, the message's slot 87
/// tall, the scan's 54 a gap of 20 under it and the cards 56 tall and 8
/// apart a gap of 20 under that (directly under the message on the Quick
/// Connect page); the status line y 740 and the tips line y 764, 18 over
/// the bottom. On a smaller page the block narrows and rises, the code
/// shrinks whole to the room left under the wordmark, the cards turn short
/// where the tall ones have no room, as the cells' turn into their one-row
/// list, and only then does the message give up its last lines; the foot
/// stays on the bottom.
fn places(full: Rect, standalone: bool, say: impl Fn(f32) -> f32, mark: Option<Vec2>) -> Places {
    let roomy = full.height() >= ROOMY;
    let k = |at_ease: f32, tight: f32| if roomy { at_ease } else { tight };
    let strip = parts(full).strip;
    let (left_x, right_x) = (full.min.x + SIDE, full.max.x - SIDE);
    let foot = full.max.y - k(FOOT, 10.0);
    let tips = Rect::from_min_max(pos2(left_x, foot - LINE_H), pos2(right_x, foot));
    let status_y = tips.min.y - k(6.0, 4.0) - LINE_H;
    let status = Rect::from_min_max(pos2(left_x, status_y), pos2(right_x, status_y + LINE_H));
    // The room the block has: the frame's column's, down to over the foot.
    let top = strip.max.y + k(20.0, 12.0);
    let bottom = (status.min.y - 8.0).max(top);
    let room = bottom - top;

    let w = BLOCK_W.min(full.width() - 2.0 * SIDE).max(0.0);
    let x0 = full.center().x - w / 2.0;
    let left_w = LEFT_W.min(w / 2.0);
    let right_w = (w - left_w - GUTTER).max(0.0);

    // The left column: the wordmark fitted to its width, the gap, and the
    // code's square in what is left, at most the column's width.
    let mark_h = mark.filter(|size| size.x > 0.0).map_or(0.0, |size| MARK_H.min(left_w * size.y / size.x));
    let mark_room = if mark_h > 0.0 { mark_h + MARK_GAP } else { 0.0 };
    let side = CODE_SIDE.min(left_w).min(room - mark_room).max(0.0);
    let left_h = mark_room + side;

    // The right column: the message, the scan's slot, the cards, tall where
    // all of them fit the room.
    let say_h = say(right_w).max(SAY_LINES as f32 * SAY * SAY_LINE);
    let scan_h = if standalone { 0.0 } else { SCAN_H + PART_GAP };
    let n = DONE_ACTIONS as f32;
    let tall = n * CARD_H + (n - 1.0) * CARD_GAP;
    let (card_h, gap) = if say_h + PART_GAP + scan_h + tall <= room { (CARD_H, CARD_GAP) } else { (HIT, SHORT_GAP) };
    let under = PART_GAP + scan_h + n * card_h + (n - 1.0) * gap;
    let block_h = left_h.max(say_h + under);
    let block_top = top + BLOCK_DROP.min((room - block_h).max(0.0) * DROP_SHARE);
    let say_h = say_h.min(bottom - block_top - under).max(0.0);
    let block_h = left_h.max(say_h + under);

    let mark = (mark_h > 0.0).then(|| Rect::from_min_size(pos2(x0, block_top), vec2(left_w, mark_h)));
    let code = Rect::from_min_size(pos2(x0 + (left_w - side) / 2.0, block_top + mark_room), vec2(side, side));
    let rule = Rect::from_min_size(pos2(x0 + left_w + RULE_IN, block_top), vec2(1.0, block_h));
    let right = x0 + left_w + GUTTER;
    let message = Rect::from_min_size(pos2(right, block_top), vec2(right_w, say_h));
    let scan = (!standalone).then(|| Rect::from_min_size(pos2(right, message.max.y + PART_GAP), vec2(right_w, SCAN_H)));
    let first = message.max.y + PART_GAP + scan_h;
    let cards = std::array::from_fn(|i| Rect::from_min_size(pos2(right, first + i as f32 * (card_h + gap)), vec2(CARD_W.min(right_w), card_h)));
    Places { strip, mark, code, rule, message, scan, cards, status, tips }
}

/// A link card at `rect`, as the boards draw the three (the kit has no
/// card of the kind: its tick card has a box and a hint, its call to act a
/// centred glyph): the panel's fill in an edge hairline, `label` in the
/// semibold text colour 18 points in, cut with an ellipsis, never changing
/// colour. Under the pointer the fill rises and the edge strengthens, with
/// the hand; the keyboard's selection (`selected`) is the kit's accent ring
/// on the panel's fill. The hover wins the look, as the cells' bright
/// border wins over their selected one, and the selection's ring comes back
/// when the pointer leaves: the selection itself never moves for the
/// pointer. The whole card takes the click and the tooltip.
fn link_card(ui: &Ui, id: Id, rect: Rect, label: &str, selected: bool, pal: &Pal) -> Response {
    let response = studio::hand(ui.interact(rect, id, Sense::click()));
    if response.hovered() {
        ui.painter().rect_filled(rect, studio::square(), pal.raised);
        ui.painter().rect_stroke(rect, studio::square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
    } else if selected {
        ui.painter().rect_filled(rect, studio::square(), pal.panel);
        studio::ring(ui, rect, pal);
    } else {
        studio::panel(ui, rect, pal);
    }
    let room = (rect.width() - 2.0 * CARD_PAD).max(1.0);
    studio::text(ui, pos2(rect.min.x + CARD_PAD, rect.center().y), label, strong(14.0), pal.text, room, Align2::LEFT_CENTER);
    response
}

/// The Quick Connect code as egui holds it: the model's picture of the
/// ticket (8 pixels a module on its white quiet zone) made into a texture
/// once and kept until the ticket changes, sampled nearest-neighbour so its
/// modules stay square at any size.
fn code_texture(ctx: &egui::Context, art: &Art) -> Option<egui::TextureHandle> {
    studio::texture(ctx, Id::new("setup-code"), art.source_id(), true, || code_image(art))
}

/// The code's pixels: its source decoded whole, or, for a picture that
/// kept no source, its own pixels, which are then all there is.
fn code_image(art: &Art) -> Option<egui::ColorImage> {
    if art.source().is_empty() {
        let size = [art.width() as usize, art.height() as usize];
        return Some(egui::ColorImage::from_rgb(size, art.rgb()));
    }
    let image = image::load_from_memory(art.source()).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{Color32, Event, FullOutput, Id, Pos2, Rect, TextureFilter, pos2, vec2};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rust_i18n::t;

    use super::super::super::cells::{done_buttons, footer_hint};
    use super::super::tests::{BOARD, Probe, color_of, drawn, has, tipped, wizard};
    use super::{Places, places};
    use crate::api::ApiError;
    use crate::api::types::IrohStatus;
    use crate::gui::window::studio::Pal;
    use crate::setup::tests::LOCALE_LOCK;
    use crate::setup::{APP_STORE_URL, Done, PLAY_STORE_URL, ProgressReport, Screen, Wizard, handle_key};

    /// Card `i`, as the view numbers them.
    fn card(i: usize) -> Id {
        Id::new("setup-link").with(i)
    }

    /// The wizard on its last step, the server's answers yet to come.
    fn done() -> Wizard {
        let mut wizard = wizard();
        wizard.screen = Screen::Done;
        wizard
    }

    /// The Quick Connect page as `qr` opens it.
    fn quick_connect() -> Wizard {
        let mut wizard = done();
        wizard.standalone = true;
        wizard
    }

    /// The server's Quick Connect answer folded in as the frame half folds
    /// the worker's: on with `ticket` as its code, on with none yet, or off.
    fn answered(wizard: &mut Wizard, enabled: bool, ticket: Option<&str>) {
        let status = IrohStatus { enabled, qr: ticket.map(str::to_string), ..Default::default() };
        wizard.apply(Done::Iroh(Ok(status)));
    }

    fn opening() -> String {
        t!("done.running_port", port = 9).to_string()
    }

    fn key(wizard: &mut Wizard, code: KeyCode) {
        handle_key(wizard, KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// The pictures a pass drew, where, and whether each is sampled
    /// nearest-neighbour (the code) or smoothed (the wordmark), with its
    /// texture's size.
    fn pictures(probe: &Probe, out: &FullOutput) -> Vec<(Rect, bool, [usize; 2])> {
        let manager = probe.ctx.tex_manager();
        let manager = manager.read();
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    let meta = manager.meta(mesh.texture_id)?;
                    Some((mesh.calc_bounds(), meta.options.magnification == TextureFilter::Nearest, meta.size))
                }
                _ => None,
            })
            .collect()
    }

    /// The edge a pass drew round `rect`, and its fill.
    fn look(out: &FullOutput, rect: Rect) -> (Option<Color32>, Option<Color32>) {
        let at = |r: &Rect| (r.min - rect.min).length() < 0.01 && (r.max - rect.max).length() < 0.01;
        let shapes = || out.shapes.iter().filter_map(|c| if let Shape::Rect(r) = &c.shape { at(&r.rect).then_some(r) } else { None });
        let edge = shapes().filter(|r| r.stroke.width > 0.0).last().map(|r| r.stroke.color);
        let fill = shapes().filter(|r| r.fill != Color32::TRANSPARENT).last().map(|r| r.fill);
        (edge, fill)
    }

    /// The meter's lit stripes in `color`: three to a lit segment.
    fn stripes(out: &FullOutput, color: Color32) -> usize {
        out.shapes
            .iter()
            .filter(|c| match &c.shape {
                Shape::Rect(r) => r.fill == color && (r.rect.width() - 5.0).abs() < 0.01 && (r.rect.height() - 2.0).abs() < 0.01,
                _ => false,
            })
            .count()
    }

    fn near(a: Rect, b: Rect) -> bool {
        (a.min - b.min).length() < 0.5 && (a.max - b.max).length() < 0.5
    }

    /// A line of the message as egui lays it out: the boards' 21.75 points
    /// rounded to a whole one.
    const LINE: f32 = 22.0;

    /// The boards' measures, from the layout alone: the wizard's page and
    /// the Quick Connect page, whose cards stand where the scan would.
    #[test]
    fn the_page_stands_where_the_boards_put_it() {
        let board = Rect::from_min_size(Pos2::ZERO, BOARD);
        let mark = Some(vec2(1224.0, 306.0));
        let at = places(board, false, |_| 3.0 * LINE, mark);
        assert_eq!(at.strip, Rect::from_min_max(pos2(24.0, 16.0), pos2(1256.0, 44.0)));
        assert_eq!(at.mark, Some(Rect::from_min_size(pos2(180.0, 150.0), vec2(300.0, 72.0))), "the wordmark's band");
        assert_eq!(at.code, Rect::from_min_size(pos2(190.0, 246.0), vec2(280.0, 280.0)), "the code's band");
        assert_eq!(at.rule, Rect::from_min_size(pos2(496.0, 150.0), vec2(1.0, 376.0)), "the rule down the block");
        assert_eq!(at.message, Rect::from_min_size(pos2(512.0, 150.0), vec2(588.0, 87.0)), "four lines kept");
        assert_eq!(at.scan, Some(Rect::from_min_size(pos2(512.0, 257.0), vec2(588.0, 54.0))));
        let ys = at.cards.map(|c| (c.min.y, c.size()));
        assert_eq!(ys, [331.0, 395.0, 459.0].map(|y| (y, vec2(420.0, 56.0))), "the cards");
        assert_eq!((at.status.min.y, at.tips.min.y, at.tips.max.y), (740.0, 764.0, 782.0), "the foot");

        let at = places(board, true, |_| 3.0 * LINE, mark);
        assert_eq!(at.scan, None, "no scan on the Quick Connect page");
        assert_eq!(at.cards.map(|c| c.min.y), [257.0, 321.0, 385.0], "the cards under the message");
        assert_eq!(at.code.min.y, 246.0, "the left column as the wizard's");

        // A message longer than four lines takes its lines, and what is
        // under it moves down; with no wordmark the code heads the column.
        let at = places(board, false, |_| 6.0 * LINE, None);
        assert_eq!(at.message.height(), 6.0 * LINE);
        assert!(at.mark.is_none() && at.code.min.y == at.rule.min.y, "{:?}", at.code);
    }

    /// On a page the boards' block does not fit, down to the cells' floor,
    /// nothing overlaps: the code shrinks whole and square, the cards turn
    /// short, the message gives up whole lines last, and the foot stays on
    /// the bottom.
    #[test]
    fn on_a_small_page_the_code_shrinks_whole_and_the_cards_turn_short() {
        let mark = Some(vec2(1224.0, 306.0));
        for (size, standalone) in [(super::super::FLOOR, false), (super::super::FLOOR, true), (vec2(1280.0, 560.0), false), (vec2(700.0, 800.0), false)] {
            let full = Rect::from_min_size(Pos2::ZERO, size);
            let at: Places = places(full, standalone, |w| if w < 300.0 { 9.0 * LINE } else { 3.0 * LINE }, mark);
            let label = format!("{size:?} {standalone}");
            assert_eq!(at.tips.max.y, full.max.y - if size.y >= 640.0 { 18.0 } else { 10.0 }, "{label}");
            assert!(at.status.max.y <= at.tips.min.y, "{label}");
            let column = at.rule.min.x - super::RULE_IN;
            assert!(at.code.width() == at.code.height() && at.code.max.x <= column + 0.01, "{label}: the code square, in its column");
            let last = at.cards[2];
            assert!(last.max.y <= at.status.min.y, "{label}: the cards over the foot: {last:?}");
            assert!(at.code.max.y <= at.status.min.y && at.code.min.y >= at.strip.max.y, "{label}");
            assert!(at.message.min.x > at.rule.max.x && at.cards[0].min.y >= at.message.max.y, "{label}");
            assert!(at.message.height() >= 15.0 * 1.45 - 0.01 || standalone, "{label}: a line at least");
        }
        let floor = places(Rect::from_min_size(Pos2::ZERO, super::super::FLOOR), false, |_| 9.0 * LINE, mark);
        assert_eq!(floor.cards[0].height(), crate::gui::window::studio::HIT, "short cards at the floor");
        let floor = places(Rect::from_min_size(Pos2::ZERO, super::super::FLOOR), true, |_| 2.0 * LINE, mark);
        assert_eq!(floor.message.height(), 4.0 * 15.0 * 1.45, "the Quick Connect page keeps its four lines there");
    }

    /// The wizard's page with its code come: the two columns in the cells'
    /// words, no heading (the wordmark heads it), the wordmark smoothed and
    /// the code nearest-neighbour in their bands, whole, the rule, the
    /// three cards and the frame's step, status and tips lines.
    #[test]
    fn the_wizards_done_page_draws_its_two_columns_in_the_cells_words() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = done();
        answered(&mut wizard, true, Some("iroh-ticket-abc123"));
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let message = format!("{} {}", opening(), t!("done.scan_left"));
        let hint = footer_hint(&wizard);
        assert_eq!(hint, t!("hint.done_wizard"));
        for w in ["4 / 4", message.as_str(), hint.as_str()] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        for (label, _) in done_buttons() {
            assert!(has(&words, &label), "{label} drawn: {words:?}");
            assert_eq!(color_of(&out, &label), Some(pal.text));
        }
        for heading in [t!("done.heading_wizard"), t!("done.heading_qr"), t!("qc.scan_app")] {
            assert!(!has(&words, &heading), "{heading} is the stacked page's, or never drawn: {words:?}");
        }
        assert!(!has(&words, "English"), "the chip is Folders' alone, as the l key is");
        // The port in the monospace, a point under the words.
        let job = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == message => Some(t.galley.job.clone()),
            _ => None,
        });
        let job = job.expect("the message");
        let port = job.sections.iter().find(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0] == "9").expect("the port");
        assert_eq!(port.format.font_id, crate::gui::window::studio::mono(14.0));

        let pictures = pictures(&probe, &out);
        // The wordmark whole at the boards' 288 by 72, centred in the left
        // column, from its quarter (306 by 76, the odd half pixel of 76.5
        // left out as the frame's halving leaves it, so a point wider).
        let mark = pictures.iter().find(|(_, crisp, _)| !crisp).expect("the wordmark");
        assert_eq!(mark.2, [306, 76], "{mark:?}");
        let (at, size) = (mark.0.center(), mark.0.size());
        assert!((at.x - 330.0).abs() < 0.5 && (mark.0.min.y - 150.0).abs() < 0.5 && (size.y - 72.0).abs() < 0.5, "{mark:?}");
        assert!((size.x - 288.0).abs() < 2.0, "{mark:?}");
        let code = pictures.iter().find(|(_, crisp, _)| *crisp).expect("the code, nearest-neighbour");
        assert!(near(code.0, Rect::from_min_size(pos2(190.0, 246.0), vec2(280.0, 280.0))), "whole in its band: {code:?}");
        let art = wizard.qr_art.as_ref().expect("the model's picture");
        let png = image::load_from_memory(art.source()).expect("its source");
        assert_eq!(code.2, [png.width() as usize, png.height() as usize], "the whole picture, 8 pixels a module");

        let rule = out.shapes.iter().any(|c| match &c.shape {
            Shape::LineSegment { points, stroke } => {
                stroke.color == pal.edge && points[0].x == 496.5 && points[1].x == 496.5 && points[0].y == 150.0 && points[1].y == 526.0
            }
            _ => false,
        });
        assert!(rule, "the hairline rule, not the cells' gold");
        for (i, y) in [331.0, 395.0, 459.0].into_iter().enumerate() {
            assert_eq!(probe.rect(card(i)), Rect::from_min_size(pos2(512.0, y), vec2(420.0, 56.0)), "card {i}");
            assert_eq!(look(&out, probe.rect(card(i))), (Some(pal.edge), Some(pal.panel)), "card {i} at rest");
        }
        assert!(shown.caret.is_none() && shown.outcome.is_none() && !shown.hand, "no field, nothing ended, no hand");
    }

    /// A message that wraps past four lines in a narrow right column takes
    /// every line it wraps to, none clipped, and the cards move under it.
    #[test]
    fn a_long_message_takes_its_lines_and_the_cards_move_under_it() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(vec2(560.0, 800.0));
        let mut wizard = quick_connect();
        answered(&mut wizard, false, None);
        let (out, _) = probe.settle(&mut wizard);
        let message = format!("{} {}", opening(), t!("qc.off_panel"));
        let shape = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(text) if text.galley.text() == message => Some((c.clip_rect, text.pos, text.galley.clone())),
            _ => None,
        });
        let (clip, pos, galley) = shape.expect("the message");
        assert!(galley.rows.len() > 4, "a narrow column: {} lines", galley.rows.len());
        let whole = Rect::from_min_size(pos, galley.size());
        assert!(clip.contains_rect(whole), "every line shown: {clip:?} {whole:?}");
        assert!(probe.rect(card(0)).min.y >= whole.max.y + 20.0 - 0.01, "the cards under it");
    }

    /// The code's texture is made once and kept: passes after the first add
    /// none, and the same texture is drawn.
    #[test]
    fn the_codes_texture_is_made_once() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = done();
        answered(&mut wizard, true, Some("iroh-ticket-abc123"));
        let (first, _) = probe.settle(&mut wizard);
        let textures = probe.ctx.tex_manager().read().num_allocated();
        let code = |out: &FullOutput| {
            out.shapes.iter().find_map(|c| match &c.shape {
                Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    let manager = probe.ctx.tex_manager();
                    let crisp = manager.read().meta(mesh.texture_id).is_some_and(|m| m.options.magnification == TextureFilter::Nearest);
                    crisp.then_some(mesh.texture_id)
                }
                _ => None,
            })
        };
        let id = code(&first).expect("the code drawn");
        for _ in 0..5 {
            let (out, _) = probe.pass(&mut wizard, Vec::new());
            assert_eq!(code(&out), Some(id), "the same texture");
        }
        assert_eq!(probe.ctx.tex_manager().read().num_allocated(), textures, "none made again");
    }

    /// Each answer the Quick Connect read can bring, in the message's second
    /// sentence and on the status line, as the cells say them; the band bare
    /// in all but the code's.
    #[test]
    fn each_quick_connect_answer_draws_its_words_and_only_a_code_fills_the_band() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let code_drawn = |out: &FullOutput| pictures(&probe, out).iter().any(|(_, crisp, _)| *crisp);

        // Loading: the opening alone, the busy line in the accent.
        let mut wizard = quick_connect();
        wizard.queue(crate::setup::Op::LoadDone, t!("busy.fetching_qc"));
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(has(&words, &opening()), "the opening alone: {words:?}");
        assert_eq!(color_of(&out, &t!("busy.fetching_qc")), Some(pal.accent));
        assert!(!code_drawn(&out));

        // Off, starting, a ticket that will not draw: the model's note second.
        for (enabled, ticket, note) in [(false, None, "qc.off_panel"), (true, None, "qc.starting")] {
            let mut wizard = quick_connect();
            answered(&mut wizard, enabled, ticket);
            let (out, _) = probe.settle(&mut wizard);
            let words = drawn(&out);
            assert!(has(&words, &format!("{} {}", opening(), t!(note))), "{note}: {words:?}");
            assert!(!code_drawn(&out), "{note}: bare ground");
        }
        let mut wizard = quick_connect();
        wizard.qr_note = t!("qc.on_panel").to_string();
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), &format!("{} {}", opening(), t!("qc.on_panel"))));

        // Failed: the opening alone, and the cells' gold line.
        let mut wizard = quick_connect();
        wizard.apply(Done::Iroh(Err(ApiError::Unauthorized)));
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let (note, is_err) = wizard.note.clone().expect("the failed read's note");
        assert!(is_err && note.starts_with(&t!("note.qc_state_failed").to_string()), "{note}");
        assert!(has(&words, &opening()) && has(&words, &note), "{words:?}");
        assert_eq!(color_of(&out, &note), Some(pal.gold));
    }

    /// The Quick Connect page is the same view with its standalone switches:
    /// no step, no scan, the cards under the message, its own tips line.
    #[test]
    fn the_quick_connect_page_has_no_step_and_no_scan() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = quick_connect();
        answered(&mut wizard, true, Some("iroh-ticket-abc123"));
        // A scan the page would never poll for: none is drawn all the same.
        wizard.scan = Some(crate::setup::ScanWidget { step: "Scanning music".into(), pct: Some(44), detail: String::new() });
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(!has(&words, "4 / 4") && !has(&words, "Scanning music"), "{words:?}");
        assert!(has(&words, &t!("hint.done_qr")), "{words:?}");
        assert_eq!(probe.rect(card(0)).min.y, 257.0);
    }

    /// The scan's slot is the wizard's from the page's first frame, so the
    /// cards stand still when the first report lands; a running pass named
    /// as the cells name it, its meter in the accent by whole tenths, and
    /// the scan complete all lit in green.
    #[test]
    fn the_scan_keeps_its_slot_amber_while_it_runs_and_green_once_complete() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = done();
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(stripes(&out, pal.accent) + stripes(&out, pal.ok), 0, "no news, no widget");
        assert_eq!(probe.rect(card(0)).min.y, 331.0, "the slot kept");

        let report = ProgressReport::Enrichment { pass: "waveform".into(), attempted: 1681, total: Some(2402), more: true };
        wizard.apply(Done::Progress(Ok(report)));
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let detail = t!("scan.detail_of", attempted = 1681, total = 2402, more = t!("scan.more_queued")).to_string();
        for w in [t!("scan.pass_waveform").to_string(), "69%".to_string(), detail] {
            assert!(has(&words, &w), "{w} drawn: {words:?}");
        }
        assert_eq!(stripes(&out, pal.accent), 6 * 3, "six whole tenths lit in the accent");
        assert_eq!(probe.rect(card(0)).min.y, 331.0, "nothing moved");

        wizard.apply(Done::Progress(Ok(ProgressReport::Idle)));
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), &t!("scan.complete")));
        assert_eq!((stripes(&out, pal.ok), stripes(&out, pal.accent)), (10 * 3, 0), "complete: all ten in green");
    }

    /// A click on each card opens its link through the model, as the cells'
    /// click does: with no browser to launch (the test seam), the status
    /// line gives the link to copy, dim. A click never moves the keyboard's
    /// selection and never ends the wizard.
    #[test]
    fn a_click_on_each_card_opens_its_link_and_leaves_the_selection() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the setup tests' own seam, set the same way by
        // `the_standalone_qr_page_drops_the_wizard_chrome`: never a browser.
        unsafe { std::env::set_var("MSTREAM_NO_OPEN", "1") };
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = done();
        let _ = probe.settle(&mut wizard);
        let admin = format!("{}/admin", wizard.client.server());
        let links = [("done.label_play", PLAY_STORE_URL), ("done.label_appstore", APP_STORE_URL), ("done.label_admin", admin.as_str())];
        for (i, (label, url)) in links.into_iter().enumerate() {
            let shown = probe.click(&mut wizard, card(i));
            assert!(shown.outcome.is_none(), "card {i} ends nothing");
            let note = t!("note.link_headless", label = t!(label), url = url).to_string();
            assert_eq!(wizard.note, Some((note.clone(), false)), "card {i}");
            assert_eq!(wizard.done_sel, None, "a click selects nothing");
            let (out, _) = probe.settle(&mut wizard);
            assert_eq!(color_of(&out, &note), Some(pal.dim), "the link to copy, dim");
        }
        key(&mut wizard, KeyCode::Up);
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, card(0));
        assert_eq!(wizard.done_sel, Some(2), "the selection stays where the keys put it");
        assert!(wizard.note.as_ref().is_some_and(|(note, _)| note.contains("play.google.com")));
        // The hand over a card.
        let at = probe.rect(card(1)).center();
        let _ = probe.pass(&mut wizard, vec![Event::PointerMoved(at)]);
        assert!(probe.pass(&mut wizard, Vec::new()).1.hand);
    }

    /// The keyboard's selection is the model's, drawn as the kit's accent
    /// ring on its card; the pointer raises the card under it, and the hover
    /// wins the look on the selected card until the pointer leaves it.
    #[test]
    fn the_keyboard_selection_is_a_ring_and_the_hover_wins_the_look() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = done();
        key(&mut wizard, KeyCode::Up);
        assert_eq!(wizard.done_sel, Some(2), "up from nothing takes the last");
        let (out, _) = probe.settle(&mut wizard);
        let rects = [0, 1, 2].map(|i| probe.rect(card(i)));
        assert_eq!(look(&out, rects[2]), (Some(pal.accent), Some(pal.panel)), "the selection's ring on the panel's fill");
        assert_eq!(look(&out, rects[0]), (Some(pal.edge), Some(pal.panel)));
        // The pointer on another card raises that one; the ring stays.
        let out = probe.rest(&mut wizard, rects[0].center(), 0.0);
        assert_eq!(look(&out, rects[0]), (Some(pal.edge_strong), Some(pal.raised)), "raised under the pointer");
        assert_eq!(look(&out, rects[2]).0, Some(pal.accent));
        // On the selected card the hover wins; off it, the ring is back.
        let out = probe.rest(&mut wizard, rects[2].center(), 0.0);
        assert_eq!(look(&out, rects[2]), (Some(pal.edge_strong), Some(pal.raised)), "the hover wins");
        let out = probe.rest(&mut wizard, pos2(4.0, 4.0), 0.0);
        assert_eq!(look(&out, rects[2]), (Some(pal.accent), Some(pal.panel)));
        assert_eq!(wizard.done_sel, Some(2), "the pointer never moved it");
    }

    /// Each card's tooltip is the cells' words, after the cells' dwell.
    #[test]
    fn each_cards_tip_is_the_cells_after_the_dwell() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = done();
        let _ = probe.settle(&mut wizard);
        for (i, (_, why)) in done_buttons().into_iter().enumerate() {
            let at = probe.rect(card(i)).center();
            let _ = probe.pass_after(&mut wizard, vec![Event::PointerMoved(pos2(4.0, 4.0))], 0.2);
            assert!(!has(&drawn(&probe.rest(&mut wizard, at, 0.3)), &tipped(&why)), "card {i}: not before the dwell");
            assert!(has(&drawn(&probe.rest(&mut wizard, at, 0.6)), &tipped(&why)), "card {i}: {why}");
        }
    }

    /// The page has no field: no caret is handed to the host, whatever the
    /// input method holds, and the host's input method stays off.
    #[test]
    fn the_page_has_no_field_and_so_no_caret() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let mut probe = Probe::new(BOARD);
        probe.composition = "にほ".into();
        for mut wizard in [done(), quick_connect()] {
            let (out, shown) = probe.settle(&mut wizard);
            assert!(shown.caret.is_none(), "{:?}", drawn(&out));
            assert!(!drawn(&out).iter().any(|w| w.contains("にほ")), "no composition drawn");
            let shown = probe.click(&mut wizard, card(1));
            assert!(shown.caret.is_none());
        }
    }
}
