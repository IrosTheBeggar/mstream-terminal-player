//! The wizard in character cells: what `mstream-player setup` and `qr`
//! draw in a terminal, and what the player's window draws on its grid
//! (face.rs). The wizard itself (its state, its worker, its keys and the
//! loop's two halves) is the parent module, mod.rs; this is its view in
//! cells, and holds what is measured in cells: the column, the four
//! screens, the bottom bar, the Done page's two layouts, the four modals
//! and the tooltip, on the kit's surface. A control is one of the model's
//! [`Act`]s, registered as it is drawn; the loop's input half does what
//! it says.

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use rust_i18n::t;

use super::*;
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim, theme};

/// The widest the content column grows, in cells.
const COLUMN: u16 = 74;

// ── Drawing ──────────────────────────────────────────────────────────────────

pub(crate) fn render(frame: &mut Frame, wizard: &mut Wizard) {
    render_page(frame, wizard);
    // The overlays drew after the pictures: a kitty picture's pixels,
    // riding its first cell, may not have survived the frame.
    crate::tui::graphics::verify_sent(frame.buffer_mut());
}

fn render_page(frame: &mut Frame, wizard: &mut Wizard) {
    wizard.ui.begin_frame();
    let area = frame.area();
    // The fixed scheme paints its own ground — but only when the terminal
    // granted OSC 11 ownership of the whole window, margins included
    // (all or nothing: cell fills alone would sit inside a border of the
    // user's background). Body text drawn with no explicit fg inherits
    // `text` from this fill; without the fill it stays the terminal's
    // default, which is the readable choice on an unknown ground.
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            Block::default().style(Style::default().bg(ground).fg(th().text)),
            area,
        );
    }
    if area.width < 58 || area.height < 20 {
        frame.render_widget(
            Paragraph::new(t!("resize").to_string()).style(dim()),
            area,
        );
        return;
    }

    // The centered column everything lives in — except the Done page,
    // which owns the whole width (its columns anchor LEFT) and, having no
    // rule or bottom bar, most of the height.
    let column = if wizard.screen == Screen::Done {
        Rect { x: 2, y: 2, width: area.width.saturating_sub(4), height: area.height.saturating_sub(6) }
    } else {
        let width = COLUMN.min(area.width.saturating_sub(4));
        Rect { x: (area.width - width) / 2, y: 2, width, height: area.height.saturating_sub(8) }
    };

    // A modal makes the screen beneath INERT: the base draw sees no
    // pointer (so nothing under the modal hovers), and every rect it
    // registered is dropped before the modal draws — only the modal's
    // own controls exist while it is up.
    let modal_open = !matches!(wizard.modal, Modal::None);
    let live_pointer = wizard.ui.pointer;
    if modal_open {
        wizard.ui.pointer = None;
    }

    match wizard.screen {
        Screen::Folders => draw_folders(frame, wizard, column),
        Screen::Login => draw_login(frame, wizard, column),
        Screen::Extras => draw_extras(frame, wizard, column),
        Screen::Done => draw_done(frame, wizard, column),
    }

    // The language selector, top-left on the folders screen — the mirror
    // of the step counter. Everything else re-renders through t!() the
    // moment it changes. The chip is as wide as its label's cells: counted
    // in characters, "日本語" kept one of its three.
    if wizard.screen == Screen::Folders {
        let label = format!("{} {}", g("▾", "▼"), LANGS[wizard.lang].1);
        let chip = Rect { x: 2, y: 0, width: kit::width(&label) as u16, height: 1 };
        let hovered = wizard.ui.pointer.is_some_and(|p| chip.contains(p));
        let style = if hovered { Style::default().fg(th().bright) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(label, style)), chip);
        wizard.ui.click(chip, Act::OpenLanguage);
        wizard.ui.tip(chip, t!("lang.chip_tip"));
    }

    // Step counter, top-right — the chrome's only top element. The
    // standalone QR page is not a step of anything.
    if !wizard.standalone {
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} / 4", wizard.screen.step()), dim()))
                .alignment(Alignment::Right),
            Rect { x: 2, y: 0, width: area.width.saturating_sub(4), height: 1 },
        );
    }

    // Status note (errors, busy) above the keyboard tips. The Done page
    // has no rule or bar, so its two lines sit at the bottom edge.
    let (note_y, tips_y) = if wizard.screen == Screen::Done {
        (area.height.saturating_sub(3), area.height.saturating_sub(2))
    } else {
        (area.height.saturating_sub(6), area.height.saturating_sub(5))
    };
    if let Some((text, is_err)) = wizard.note.clone() {
        let style = if is_err { Style::default().fg(th().gold) } else { dim() };
        let note_area = Rect { x: 2, y: note_y, width: area.width - 4, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(text, style)), note_area);
    }
    if let Some(busy) = wizard.busy.clone() {
        let busy_area = Rect { x: 2, y: note_y, width: area.width - 4, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(busy, accent())), busy_area);
    }

    // Keyboard tips, left, directly above the gold rule.
    let tips = Rect { x: 2, y: tips_y, width: area.width - 4, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(footer_hint(wizard), dim())), tips);

    // The Done page ends here: no horizontal rule, no bottom bar — its
    // vertical rule and in-column scan status carry the structure.
    if wizard.screen == Screen::Done {
        return;
    }

    // The one gold rule, with the 3-row bottom bar under it: scan widget on
    // the left (empty until a scan is actually running — folders commit on
    // Continue, so that is the NEXT screen at the earliest), the screen's
    // forward action on the right as the kit's tall primary block.
    let rule = Rect { x: 2, y: area.height.saturating_sub(4), width: area.width - 4, height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(rule.width as usize), Style::default().fg(th().gold))),
        rule,
    );
    let bar = Rect { x: 2, y: area.height.saturating_sub(3), width: area.width - 4, height: 3 };
    if let Some(scan) = &wizard.scan {
        frame.render_widget(
            Paragraph::new(scan_lines(scan)),
            Rect { x: bar.x, y: bar.y, width: bar.width.saturating_sub(20), height: 3 },
        );
    }
    if wizard.screen == Screen::Folders {
        // Disabled until there is something to continue with; the tooltip
        // carries the why (the kit's one exception to disabled inertness).
        let enabled = !wizard.folders.is_empty();
        let label = if enabled {
            t!("folders.continue_enabled").to_string()
        } else {
            t!("folders.continue_disabled").to_string()
        };
        let at = (label.as_str(), enabled, Act::ContinueFolders);
        let (rect, _) = bar_buttons(frame, &mut wizard.ui, bar, at, None);
        if !enabled {
            wizard.ui.tip(rect, t!("folders.tip_continue_disabled"));
        }
    }
    if wizard.screen == Screen::Extras {
        let (label, back_label) = (t!("extras.continue"), t!("extras.back"));
        let at = (&*label, true, Act::ContinueExtras);
        let back = Some((&*back_label, Act::BackToFolders));
        let (_, back) = bar_buttons(frame, &mut wizard.ui, bar, at, back);
        if let Some(back) = back {
            wizard.ui.tip(back, t!("extras.tip_back"));
        }
    }
    if wizard.screen == Screen::Login {
        let (label, back_label) = (t!("login.create"), t!("login.back"));
        let at = (&*label, true, Act::CreateAdmin);
        let back = Some((&*back_label, Act::BackToExtras));
        let (_, back) = bar_buttons(frame, &mut wizard.ui, bar, at, back);
        if let Some(back) = back {
            wizard.ui.tip(back, t!("login.tip_back"));
        }
    }

    if modal_open {
        wizard.ui.pointer = live_pointer;
        wizard.ui.clear_registries();
    }
    // Each modal registers its footprint as it draws (`kit::modal_frame_on`),
    // and a window that hears it stands down any picture placed beneath:
    // the directory browser's top border crosses the folders wordmark's
    // band at the window's grid, and any modal does in a short window.
    // The folder browser and the path modal each hold a server listing —
    // thousands of names at a music root — so they are drawn from where
    // they live, not from a copy made every frame (performance audit #110).
    match &mut wizard.modal {
        Modal::None => {}
        Modal::SkipWarning => draw_skip_warning(frame, wizard, area),
        Modal::Browser(_) => {
            // Lent to its draw, which never touches the modal, and put back.
            let modal = std::mem::replace(&mut wizard.modal, Modal::None);
            if let Modal::Browser(browse) = &modal {
                draw_browser(frame, wizard, area, browse);
            }
            wizard.modal = modal;
        }
        Modal::PathEntry(draft) => {
            // The draft stays in place — its draw keeps the list's scroll
            // there — and lends the frame only its listing.
            let entries = std::mem::take(&mut draft.entries);
            let lent = PathDraft { entries, ..draft.clone() };
            draw_path_entry(frame, wizard, area, &lent);
            if let Modal::PathEntry(draft) = &mut wizard.modal {
                draft.entries = lent.entries;
            }
        }
        Modal::Language(sel) => {
            let sel = *sel;
            draw_language(frame, wizard, area, sel);
        }
    }

    // The tooltip draws last — over everything, once the dwell matures.
    // It registers no overlay, unlike the GUI's. The language chip's tip
    // touches the wordmark's band, which is the column's whole width, and a
    // window stands a picture down by the band it was placed in, so every
    // hover of the chip would swap the wordmark for the figlet. At the
    // window's grid the picture sits centred in its band, clear of the tip;
    // only a window narrowed below about 74 columns brings the two together.
    if let Some((target, text)) = wizard.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// The scan widget's three lines: the STEP (what is happening), the
/// kit's ▰▱ bar with its percentage when the step carries an estimate
/// (all-dim cells when it does not — activity reads from the other two
/// lines), and the DIM detail counts.
pub(super) fn scan_lines(scan: &ScanWidget) -> Vec<Line<'static>> {
    let bar = match scan.pct {
        Some(pct) => {
            let filled = ((pct.min(100) as usize) + 5) / 10;
            Line::from(vec![
                Span::styled(g("▰", "■").repeat(filled), Style::default().fg(th().ok)),
                Span::styled(g("▱", "·").repeat(10 - filled), dim()),
                Span::styled(format!(" {pct}%"), dim()),
            ])
        }
        None => Line::from(Span::styled(g("▱", "·").repeat(10), dim())),
    };
    vec![
        Line::from(Span::raw(scan.step.clone())),
        bar,
        Line::from(Span::styled(scan.detail.clone(), dim())),
    ]
}

pub(super) fn footer_hint(wizard: &Wizard) -> String {
    match (&wizard.modal, wizard.screen) {
        (Modal::Browser(_), _) => t!("hint.browser"),
        (Modal::PathEntry(_), _) => t!("hint.path"),
        (Modal::SkipWarning, _) => t!("hint.skip"),
        (Modal::Language(_), _) => t!("lang.hint"),
        (_, Screen::Folders) => match (wizard.folders.is_empty(), wizard.sel) {
            // No rows: only the ways to add one (and the language chip).
            (true, _) => t!("hint.folders_empty"),
            // Rows, cursor stowed: how to pick one up, and continue.
            (false, None) => t!("hint.folders_rows"),
            // A row under the cursor: the full set.
            (false, Some(_)) => t!("hint.folders_selected"),
        },
        (_, Screen::Login) => t!("hint.login"),
        (_, Screen::Extras) => match wizard.extras_sel {
            None => t!("hint.extras"),
            Some(_) => t!("hint.extras_selected"),
        },
        (_, Screen::Done) if wizard.standalone => t!("hint.done_qr"),
        (_, Screen::Done) => t!("hint.done_wizard"),
    }
    .to_string()
}

/// The language picker: one row a language, autonyms, the kit's modal
/// conventions.
fn draw_language(frame: &mut Frame, wizard: &mut Wizard, area: Rect, sel: usize) {
    let inner =
        kit::modal_frame_on(frame, &mut wizard.ui, area, 30, LANGS.len() as u16 + 4, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("lang.modal_title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut wizard.ui, inner, Act::PathCancel);
    for (i, (code, autonym)) in LANGS.iter().enumerate() {
        let y = inner.y + 2 + i as u16;
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
        let current = wizard.lang == i;
        let style = if sel == i {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else if current {
            accent()
        } else {
            Style::default()
        };
        let marker = if current { "• " } else { "  " };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{marker}{autonym}"), style)),
            rect,
        );
        wizard.ui.click(rect, Act::SetLanguage(i));
        let _ = code;
    }
}

fn card(frame: &mut Frame, at: Rect, focused: bool, hovered: bool) -> Rect {
    let style = if hovered {
        Style::default().fg(th().bright)
    } else if focused {
        accent()
    } else {
        dim()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style);
    let inner = block.inner(at);
    frame.render_widget(block, at);
    inner
}

// Figlet "mStream" — kept byte-identical to the server's boot banner.
pub(super) const LOGO: [&str; 5] = [
    r"           ____  _",
    r" _ __ ___ / ___|| |_ _ __ ___  __ _ _ __ ___",
    r"| '_ ` _ \\___ \| __| '__/ _ \/ _` | '_ ` _ \",
    r"| | | | | |___) | |_| | |  __/ (_| | | | | | |",
    r"|_| |_| |_|____/ \__|_|  \___|\__,_|_| |_| |_|",
];
/// The logo's widest line. Every line draws at one fixed x computed from
/// this, so ragged line widths can't skew per-line centering.
const LOGO_W: u16 = 46;

fn draw_folders(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    // The wordmark as pixels where the terminal can draw them and the
    // ground is ours (the picture is flattened onto that ground, so on an
    // unowned background the figlet is the honest rendering). Both paths
    // occupy the same band, so the screen never reflows on capability.
    // A window's ground is always ours: it paints the theme's ground
    // behind every cell, the color the picture was flattened onto. In a
    // window the figlet also stands in wherever a modal stood over the band
    // last frame, the rule the GUI's covers follow: the window does not
    // paint a picture placed before a modal that touches it (the modal
    // registers as it draws), so the band would otherwise be bare while
    // the modal is up. A terminal's picture draws as it always has.
    let band = Rect { x: column.x, y, width: column.width, height: LOGO.len() as u16 };
    let under_a_modal = wizard.graphics.is_hosted() && wizard.ui.covered_last_frame(band);
    let drew = (theme::ground_owned() || wizard.graphics.is_hosted())
        && !under_a_modal
        && match &wizard.logo_art {
            Some(art) => wizard.logo_gfx.draw(frame, band, art),
            None => false,
        };
    if !drew {
        let logo_x = column.x + column.width.saturating_sub(LOGO_W) / 2;
        for (i, line) in LOGO.iter().enumerate() {
            frame.render_widget(
                Paragraph::new(Span::styled(*line, accent())),
                Rect { x: logo_x, y: y + i as u16, width: LOGO_W.min(column.width), height: 1 },
            );
        }
    }
    y += band.height + 2;

    // The picker card — the screen's one add affordance (typing a path is
    // the `t` shortcut, in the tips line) — sits ABOVE the table so it
    // holds one spot as folders come and go. Green: the affirmative add.
    let add_rect = Rect { x: column.x, y, width: column.width, height: 3 };
    let add_hover = wizard.ui.pointer.is_some_and(|p| add_rect.contains(p));
    let add_color = if add_hover { th().bright } else { th().ok };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(add_color));
    let inner = block.inner(add_rect);
    frame.render_widget(block, add_rect);
    frame.render_widget(
        Paragraph::new(Span::styled(
            t!("folders.add_card").to_string(),
            Style::default().fg(add_color).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        inner,
    );
    wizard.ui.click(add_rect, Act::BrowseNative);
    y += 4;

    // The chosen folders as a table: NAME first — the vpath is the point.
    // The [X] remove control sits to the RIGHT of the selection area, not
    // inside it: selecting a row never highlights its remove button.
    const NAME_W: u16 = 16;
    const REMOVE_W: u16 = 4; // ' [X]'
    let sel_width = column.width.saturating_sub(REMOVE_W);
    let header = Rect { x: column.x, y, width: sel_width, height: 1 };
    // The name heading is padded to NAME_W in cells, where the rows start
    // their paths: `{:<16}` pads characters, which set the folder heading
    // two cells right of its column under a Japanese "名前".
    let name_col = t!("folders.col_name").to_string();
    let name_pad = " ".repeat(usize::from(NAME_W).saturating_sub(kit::width(&name_col)));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{name_col}{name_pad}"), dim()),
            Span::styled(t!("folders.col_folder").to_string(), dim()),
        ])),
        header,
    );
    y += 1;
    // The rule spans the FULL row — selection area and the [X] column.
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(column.width as usize), dim())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;

    // The table frame is ALWAYS on screen; an empty list says so where
    // the first row would be.
    if wizard.folders.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("folders.nothing_yet").to_string(), dim())),
            Rect { x: column.x, y, width: sel_width, height: 1 },
        );
        return;
    }
    // Rows live in whatever height remains; overflow scrolls. The view
    // yanks to a fresh add or a moved keyboard cursor, else the wheel
    // offset stands.
    let avail = (column.y + column.height).saturating_sub(y) as usize;
    let sel_moved = wizard.sel != wizard.sel_anchor;
    wizard.sel_anchor = wizard.sel;
    let reveal = wizard.reveal.take().or(if sel_moved { wizard.sel } else { None });
    let (first, visible) = kit::table_view(wizard.folders.len(), reveal, wizard.tscroll, avail);
    wizard.tscroll = first;
    if visible == 0 {
        return;
    }
    let rows_y = y;
    for i in first..first + visible {
        let selected = wizard.sel == Some(i);
        let rect = Rect { x: column.x, y, width: sel_width, height: 1 };

        let folder = &wizard.folders[i];
        let editing = wizard.editing.as_ref().is_some_and(|(row, _)| *row == i);
        // The chip's caret, in cells from the line's start inside its
        // bracket, noted once the chip's cell is known.
        let mut chip_caret = None;
        let name = match (&wizard.editing, editing) {
            (Some((_, draft)), true) => {
                let (line, caret) = kit::field_line(
                    &wizard.ui,
                    draft.value(),
                    draft.cursor(),
                    NAME_W.saturating_sub(2),
                    None,
                );
                chip_caret = Some(caret);
                format!("[{line}]")
            }
            _ => folder.name.clone(),
        };
        let row_bg = if selected && !editing {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else {
            Style::default()
        };
        let name_rect = Rect { x: column.x, y, width: NAME_W.min(sel_width), height: 1 };
        // The rename affordance lights up under the pointer — the same
        // brightening every other clickable gets (a selected row keeps
        // its bg; only the name's fg brightens).
        let name_hover = !editing && wizard.ui.pointer.is_some_and(|p| name_rect.contains(p));
        let name_style = if editing {
            Style::default().fg(th().bright)
        } else if name_hover && selected {
            Style::default().fg(th().bright).bg(th().accent).add_modifier(Modifier::BOLD)
        } else if name_hover {
            Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
        } else {
            accent()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(" ".repeat(sel_width as usize), row_bg)),
            rect,
        );
        frame.render_widget(Paragraph::new(Span::styled(name, name_style)), name_rect);
        if let Some(caret) = chip_caret {
            wizard.ui.note_caret(Position { x: name_rect.x + 1 + caret, y });
        }
        wizard.ui.click(name_rect, Act::RenameFolder(i));
        if !editing {
            // The tip tells the truth per row: a committed name cannot
            // be renamed here (the server has no rename API — only the
            // admin panel's remove-and-re-add), and promising "click to
            // rename" on it reads as a broken click.
            if folder.committed {
                wizard.ui.tip(name_rect, t!("note.committed_rename"));
            } else {
                wizard.ui.tip(name_rect, t!("folders.tip_name"));
            }
        }
        let path_x = column.x + NAME_W;
        let path_rect =
            Rect { x: path_x, y, width: sel_width.saturating_sub(NAME_W), height: 1 };
        // The LOCAL filesystem's verdict, worn by the row: problems paint
        // the path gold, the tooltip says why. Advisory only — Continue
        // still works, the server has final say at commit.
        let problem: Option<String> = folder.problem();
        let path_style = match problem {
            Some(_) if !selected => Style::default().fg(th().gold),
            _ => row_bg,
        };
        // Long paths keep their LEAF visible: a leading … and the tail,
        // fitted to the cell (a path's informative end is its end — the
        // table rule's trailing … would show ten identical prefixes).
        // The full path rides the tooltip whenever the cell clipped;
        // a validation problem still outranks it.
        let (shown, clipped) = ellipsize_path_start(&folder.path, path_rect.width as usize);
        frame.render_widget(Paragraph::new(Span::styled(shown, path_style)), path_rect);
        match problem {
            Some(tip) => wizard.ui.tip(path_rect, tip),
            None if clipped => {
                // The tooltip carries at most THREE lines of path;
                // anything longer keeps its tail behind the same leading
                // …. The cap is THIS caller's policy, not the kit's —
                // wrap_tip itself has no line limit, so future tips may
                // run as long as they need.
                let (tip, _) =
                    ellipsize_path_start(&folder.path, crate::kit::TIP_WRAP * 3);
                wizard.ui.tip(path_rect, tip);
            }
            None => {}
        }
        let x_rect = Rect { x: column.x + sel_width + 1, y, width: 3, height: 1 };
        let x_hover = wizard.ui.pointer.is_some_and(|p| x_rect.contains(p));
        let x_style = if x_hover {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled("[X]", x_style)), x_rect);
        wizard.ui.click(x_rect, Act::RemoveAt(i));
        wizard.ui.tip(x_rect, REMOVE_TIP);
        y += 1;
    }

    // Overflow → the kit's scrollbar, just right of the [X] column (the
    // column is centered, so the margin cell is always there). Endcaps
    // are click rects; the wheel scrolls too.
    let bar = Rect { x: column.x + column.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(
        frame,
        &mut wizard.ui,
        bar,
        wizard.folders.len(),
        visible,
        first,
        Act::TableScroll(-1),
        Act::TableScroll(1),
        Act::TableScrollTo,
    );
}

/// One login field under its label. `caret` is the focused field's
/// caret, in cells from the start of `value` (the caller draws the line
/// with it), and None for the others.
fn field_row(
    frame: &mut Frame,
    wizard: &mut Wizard,
    x: u16,
    y: u16,
    width: u16,
    label: &str,
    value: String,
    field: LoginField,
    caret: Option<u16>,
) -> u16 {
    frame.render_widget(
        Paragraph::new(Span::styled(label, dim())),
        Rect { x, y, width, height: 1 },
    );
    let rect = Rect { x, y: y + 1, width, height: 3 };
    let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
    let inner = card(frame, rect, caret.is_some(), hovered);
    frame.render_widget(Paragraph::new(Span::raw(value)), inner);
    // The focused field notes its caret's cell, which is how a window
    // knows a field has the keyboard (see `kit::field_display`).
    if let Some(off) = caret {
        wizard.ui.note_caret(Position { x: inner.x + off, y: inner.y });
    }
    wizard.ui.click(rect, Act::Focus(field));
    y + 4
}

fn draw_login(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("login.title").to_string(), bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Span::styled(
            t!("login.subtitle").to_string(),
            dim(),
        )),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let width = column.width.min(44);
    let x = column.x + (column.width - width) / 2;
    let focus = wizard.field;
    // A secret is handed in masked, with its mark, so an input method's
    // composition draws as marks too and never in clear.
    let show = |input: &Input, secret: bool, focused: bool| -> (String, Option<u16>) {
        let value =
            if secret { mask(input.value()) } else { input.value().to_string() };
        if focused {
            let mark = secret.then_some(MASK);
            let (line, caret) =
                kit::field_line(&wizard.ui, &value, input.cursor(), width.saturating_sub(2), mark);
            (line, Some(caret))
        } else {
            (value, None)
        }
    };
    let (username, password, confirm) = (
        show(&wizard.username, false, focus == LoginField::Username),
        show(&wizard.password, true, focus == LoginField::Password),
        show(&wizard.confirm, true, focus == LoginField::Confirm),
    );
    y = field_row(frame, wizard, x, y, width, &t!("login.field_username"), username.0, LoginField::Username, username.1);
    y = field_row(frame, wizard, x, y, width, &t!("login.field_password"), password.0, LoginField::Password, password.1);
    y = field_row(frame, wizard, x, y, width, &t!("login.field_confirm"), confirm.0, LoginField::Confirm, confirm.1);
    y += 1;

    let skip = kit::button(
        frame,
        &mut wizard.ui,
        Rect { x, y, width, height: 1 },
        &t!("login.skip"),
        false,
        Act::SkipLogin,
    );
    wizard.ui.tip(skip, t!("login.tip_skip"));
}

/// The mark a secret is drawn in, one per character.
const MASK: char = '•';

fn mask(secret: &str) -> String {
    MASK.to_string().repeat(secret.chars().count())
}

fn draw_extras(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("extras.title").to_string(), bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("extras.subtitle").to_string(), dim())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let rows: [(String, String); 4] = [
        (t!("extras.updates_label").to_string(), t!("extras.updates_desc").to_string()),
        (t!("extras.discovery_label").to_string(), t!("extras.discovery_desc").to_string()),
        (t!("extras.federation_label").to_string(), t!("extras.federation_desc").to_string()),
        (t!("extras.audio_label").to_string(), t!("extras.audio_desc").to_string()),
    ];
    for (i, (label, desc)) in rows.into_iter().enumerate() {
        let selected = wizard.extras_sel == Some(i);
        let rect = Rect { x: column.x, y, width: column.width, height: 4 };
        let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
        // The border wears the TOGGLE: green when the extra is on, the
        // accent blue when it is off. The keyboard cursor goes GREY —
        // the one grey card in a colored column reads instantly, where
        // cyan-on-blue barely registered. Hover keeps the kit brighten.
        let border = if hovered {
            th().bright
        } else if selected {
            th().dim
        } else if wizard.extras[i] {
            th().ok
        } else {
            th().accent
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        wizard.ui.click(rect, Act::Toggle(i));
        let box_span = if wizard.extras[i] {
            Span::styled(if crate::kit::theme::legacy_conhost() { "[x] " } else { "[✓] " }, Style::default().fg(th().ok))
        } else {
            Span::styled("[ ] ", dim())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![box_span, Span::styled(label, bold())])),
            Rect { x: inner.x + 1, y: inner.y, width: inner.width, height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(desc, dim())),
            Rect { x: inner.x + 5, y: inner.y + 1, width: inner.width.saturating_sub(5), height: 1 },
        );
        y += 4;
    }
}

/// The Done page's button list, top to bottom: (label, tip).
pub(super) fn done_buttons() -> [(String, String); DONE_ACTIONS] {
    [
        (t!("done.btn_android").to_string(), t!("done.tip_android").to_string()),
        (t!("done.btn_ios").to_string(), t!("done.tip_ios").to_string()),
        (t!("done.btn_admin").to_string(), t!("done.tip_admin").to_string()),
    ]
}

/// The port a server URL listens on, for the Done page's opening line.
/// None for URLs that don't carry one (a Quick Connect tunnel id) and
/// don't imply one by scheme.
fn server_port(server: &str) -> Option<u16> {
    if let Some((_, tail)) = server.rsplit_once(':') {
        if let Ok(port) = tail.trim_end_matches('/').parse() {
            return Some(port);
        }
    }
    match server.split_once("://").map(|(scheme, _)| scheme) {
        Some("http") => Some(80),
        Some("https") => Some(443),
        _ => None,
    }
}

fn draw_done(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    let server = wizard.client.server();
    let opening = match server_port(&server) {
        Some(port) => {
            t!("done.running_port", port = port).to_string()
        }
        None => t!("done.running_at", server = server).to_string(),
    };

    // Two columns when the code draws as pixels (it scales to its box, so
    // it fits half the width at any window). The fixed-size half-block
    // fallback keeps the stacked layout below — it simply cannot fit a
    // half column.
    if wizard.done_two_column() {
        // Left-anchored: the wordmark over the code at a FIXED width (the
        // pictures scale to it — capped so the code never balloons on a
        // wide window), a full-height vertical gold rule, and the right
        // column takes the rest. No heading — the wordmark is the header.
        let height = column.height.saturating_sub(y - column.y);
        let left_w = 30u16.min(column.width / 2);
        let left = Rect { x: column.x, y, width: left_w, height };
        let rule_x = left.right() + 2;
        let right_x = rule_x + 3;
        let right =
            Rect { x: right_x, y, width: column.right().saturating_sub(right_x), height };
        for row in column.y..column.bottom() {
            frame.render_widget(
                Paragraph::new(Span::styled("│", Style::default().fg(th().gold))),
                Rect { x: rule_x, y: row, width: 1, height: 1 },
            );
        }

        // LEFT: the wordmark, then the code.
        let mut ly = left.y;
        if let Some(art) = &wizard.logo_art {
            let logo_band = Rect { x: left.x, y: ly, width: left.width, height: 4 };
            if wizard.logo_gfx.draw(frame, logo_band, art) {
                ly = logo_band.bottom() + 1;
            }
        }
        let band = Rect {
            x: left.x,
            y: ly,
            width: left.width,
            height: left.height.saturating_sub(ly - left.y).min(15),
        };
        if let Some(art) = &wizard.qr_art {
            wizard.graphics.draw(frame, band, art);
        }

        // RIGHT: the opening line, the scan's state, and the button list.
        let mut ry = right.y + 1;
        let scan_line = if wizard.qr.is_some() {
            t!("done.scan_left").to_string()
        } else {
            wizard.qr_note.clone()
        };
        let message = Paragraph::new(format!("{opening} {scan_line}")).wrap(Wrap { trim: true });
        // The message takes the rows its wrapped text needs, never fewer
        // than the four the page was drawn around (so the cards hold still
        // across languages at the default size), and the cards move down
        // under a longer one: a fixed four cut "automatically." off the
        // English in a window narrowed to seventy columns. A column too
        // short for the tall cards lists the actions as the stacked page's
        // one-row buttons instead, and only then does the message give up
        // its tail, to keep the list inside the column and off the tips.
        // The scan's rows are kept only where a scan can be: the wizard's
        // page, from its first frame on (so nothing moves when the first
        // poll lands), never the standalone Quick Connect page, which has no
        // scan and would have cut its message and its cards for blank rows.
        // The tall cards may end one row past the column, on the blank row
        // above the note, as they always did: counting only the column's
        // rows gave every 24-row window the one-row list, at the most common
        // terminal size. The one-row list, and the message's cut, keep to
        // the column.
        let need = u16::try_from(message.line_count(right.width)).unwrap_or(u16::MAX).max(4);
        let rest = right.bottom().saturating_sub(ry);
        let scan_rows = done_scan_rows(wizard);
        let (tall, short) = (3 * DONE_ACTIONS as u16, DONE_ACTIONS as u16);
        let cards = need + 1 + scan_rows + tall <= rest + DONE_CARDS_SPARE_ROW;
        let room = if cards { rest + DONE_CARDS_SPARE_ROW } else { rest };
        let list = if cards { tall } else { short };
        let message_h = need.min(room.saturating_sub(1 + scan_rows + list));
        frame.render_widget(
            message,
            Rect { x: right.x, y: ry, width: right.width, height: message_h },
        );
        ry += message_h + 1;
        if let Some(scan) = &wizard.scan {
            frame.render_widget(
                Paragraph::new(scan_lines(scan)),
                Rect { x: right.x, y: ry, width: right.width, height: 3 },
            );
        }
        ry += scan_rows;
        if !cards {
            let width = done_list_width().min(right.width);
            done_list(frame, wizard, right.x, ry, width);
            return;
        }
        for (i, (label, tip)) in done_buttons().into_iter().enumerate() {
            let rect = Rect { x: right.x, y: ry, width: right.width.min(44), height: 3 };
            let selected = wizard.done_sel == Some(i);
            let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
            let inner = card(frame, rect, selected, hovered);
            // One cell in from the card's left border, and so one short of
            // the inner width, or a long label paints over the right one.
            frame.render_widget(
                Paragraph::new(Span::styled(label, bold())),
                Rect {
                    x: inner.x + 1,
                    y: inner.y,
                    width: inner.width.saturating_sub(1),
                    height: 1,
                },
            );
            wizard.ui.click(rect, done_action(i));
            wizard.ui.tip(rect, tip);
            ry += 3;
        }
        return;
    }

    let heading =
        if wizard.standalone {
            t!("done.heading_qr").to_string()
        } else {
            t!("done.heading_wizard").to_string()
        };
    frame.render_widget(
        Paragraph::new(Span::styled(heading, bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let scan_line = if wizard.qr.is_some() {
        t!("done.scan_above").to_string()
    } else {
        wizard.qr_note.clone()
    };
    let message = Paragraph::new(format!("{opening} {scan_line}"))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    // As in the two columns: the rows the wrapped message needs, never
    // fewer than the two this page was drawn around. What goes under it
    // (a blank, the scan's rows, the button list) is counted before the
    // code is let in, so the list never runs past the column onto the note
    // and the tips. The scan's rows are the wizard's from its first frame
    // (not from the first poll, which would swap a code drawn a moment ago
    // for the apology line) and never the standalone page's.
    let need = u16::try_from(message.line_count(column.width)).unwrap_or(u16::MAX).max(2);
    let mut scan_h = done_scan_rows(wizard);

    if let Some(qr) = wizard.qr.clone() {
        let rows = qr.len() as u16;
        // The code gets whatever height the message, the blank under it and
        // the list do not need, less the blank under the code itself.
        let fixed = need + 1 + DONE_ACTIONS as u16 + 1;
        let avail = column.bottom().saturating_sub(y).saturating_sub(fixed);
        // The code is what the page is for, so the scan's status gives way
        // to it first: its three lines, else its step on one, else nothing,
        // whichever lets the whole code in.
        let fits = [scan_h, scan_h.min(DONE_SCAN_SHORT_ROWS), 0]
            .into_iter()
            .find(|&scan| rows + scan <= avail);
        // The half-block code is fixed-size — drawn only when it fits
        // WHOLE (a cropped QR scans as nothing), and otherwise replaced
        // by the one honest line.
        if let Some(scan) = fits {
            scan_h = scan;
            let qr_width = qr.first().map(|l| kit::width(l)).unwrap_or(0) as u16;
            let x = column.x + (column.width.saturating_sub(qr_width)) / 2;
            for line in &qr {
                frame.render_widget(
                    Paragraph::new(Span::raw(line.clone())),
                    Rect { x, y, width: qr_width.min(column.width), height: 1 },
                );
                y += 1;
            }
            y += 1;
        } else {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    t!("done.too_short").to_string(),
                    dim(),
                ))
                .alignment(Alignment::Center),
                Rect { x: column.x, y, width: column.width, height: 1 },
            );
            y += 2;
        }
    }
    // Only at the floor, with the apology line above it, does the message
    // give up its tail, and then only to keep the list inside the column.
    let below = 1 + scan_h + DONE_ACTIONS as u16;
    let message_h = need.min(column.bottom().saturating_sub(y + below));
    frame.render_widget(message, Rect { x: column.x, y, width: column.width, height: message_h });
    y += message_h + 1;

    // With no bottom bar on this page, the scan status lives here: whole,
    // or its step alone where the code needed the other rows.
    if let Some(scan) = &wizard.scan {
        let lines = if scan_h >= DONE_SCAN_ROWS {
            scan_lines(scan)
        } else {
            let mut step = vec![Span::raw(scan.step.clone())];
            step.extend(scan.pct.map(|pct| Span::styled(format!(" {pct}%"), dim())));
            vec![Line::from(step)]
        };
        frame.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center),
            Rect { x: column.x, y, width: column.width, height: scan_h.saturating_sub(1) },
        );
    }
    y += scan_h;

    // The same button list, one row each, centered.
    let width = done_list_width().min(column.width);
    done_list(frame, wizard, column.x + (column.width - width) / 2, y, width);
}

/// The bottom bar's buttons: the screen's forward action as the kit's tall
/// primary block, right-anchored so its frame ends on the bar's edge, and,
/// where the screen has one, the way back as a tall secondary two cells to
/// its left. Both are placed by their frames' widths in cells
/// (`kit::tall_width`), as the kit draws them: a character count put a
/// Japanese button short of the bar's edge, and the kit then cut its label
/// ("続け" of "続ける ▸"). One function for the three screens' bars, so the
/// test that draws it with the Japanese and Chinese labels checks the very
/// arithmetic each bar uses, without switching the process-wide locale.
fn bar_buttons(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    bar: Rect,
    (label, enabled, act): (&str, bool, Act),
    back: Option<(&str, Act)>,
) -> (Rect, Option<Rect>) {
    let x = bar.right().saturating_sub(kit::tall_width(label));
    let at = Rect { x, y: bar.y, width: bar.width, height: 3 };
    let rect = kit::tall_button(frame, ui, at, label, enabled, act);
    let back = back.map(|(label, act)| {
        let x = rect.x.saturating_sub(kit::tall_width(label) + 2);
        let at = Rect { x, y: bar.y, width: bar.width, height: 3 };
        kit::tall_secondary(frame, ui, at, label, act)
    });
    (rect, back)
}

/// The rows the two-column Done page's tall cards may run past the column:
/// the blank one between the column's bottom and the note (`render` puts
/// the column at the page's height less six from row two, so it ends at the
/// height less four, and the note at the height less three).
const DONE_CARDS_SPARE_ROW: u16 = 1;

/// The rows the Done page keeps for the scan's status: its three lines
/// and a blank under them.
const DONE_SCAN_ROWS: u16 = 4;

/// The rows the stacked Done page keeps for the scan's step alone, and a
/// blank under it, when its three lines would keep the code off the page.
const DONE_SCAN_SHORT_ROWS: u16 = 2;

/// The rows the Done page keeps for the scan's status: the wizard's from
/// its first frame, before any poll has answered, so the page does not move
/// (or, stacked, lose its code) when the first answer lands; none on the
/// standalone Quick Connect page, which never scans.
fn done_scan_rows(wizard: &Wizard) -> u16 {
    if wizard.standalone { 0 } else { DONE_SCAN_ROWS }
}

/// The cells the widest Done action takes as a one-row button, its two
/// cells of padding a side counted; measured as `kit::button` measures, so
/// the centred list's x agrees with what is drawn.
fn done_list_width() -> u16 {
    done_buttons().iter().map(|(label, _)| kit::width(label)).max().unwrap_or(0) as u16 + 4
}

/// The Done actions as one-row buttons, one under another from (x, y), in
/// slots `width` cells wide: the stacked page's list, and the two columns'
/// when the column is too short for the tall cards.
fn done_list(frame: &mut Frame, wizard: &mut Wizard, x: u16, y: u16, width: u16) {
    for (i, (label, tip)) in done_buttons().into_iter().enumerate() {
        let rect = Rect { x, y: y + i as u16, width, height: 1 };
        let selected = wizard.done_sel == Some(i);
        kit::button(frame, &mut wizard.ui, rect, &label, selected, done_action(i));
        wizard.ui.tip(rect, tip);
    }
}

fn draw_skip_warning(frame: &mut Frame, wizard: &mut Wizard, area: Rect) {
    let inner = kit::modal_frame_on(frame, &mut wizard.ui, area, 62, 15, th().gold);
    kit::modal_close(frame, &mut wizard.ui, inner, Act::SkipCancel);
    let lines = vec![
        Line::from(Span::styled(t!("skip_modal.title").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(t!("skip_modal.body").to_string()),
        Line::from(""),
        Line::from(Span::styled(t!("skip_modal.plus_access").to_string(), Style::default().fg(th().ok))),
        Line::from(Span::styled(t!("skip_modal.plus_tv").to_string(), Style::default().fg(th().ok))),
        Line::from(Span::styled(t!("skip_modal.minus_control").to_string(), Style::default().fg(th().gold))),
        Line::from(Span::styled(t!("skip_modal.minus_qc").to_string(), Style::default().fg(th().gold))),
        Line::from(""),
        Line::from(Span::styled(t!("skip_modal.later").to_string(), dim())),
    ];
    // The body clips ABOVE the button row: wrapped lines must never
    // share cells with the buttons (a longer translation once pushed
    // "…later from the admin panel." under them, and the 2-cell button
    // gap exposed the "ro" of "from" as a phantom typo). It stops two rows
    // short, so the row above the buttons is free for a confirmation too
    // long to sit beside the way back (skip_buttons).
    let body = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(2),
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    skip_buttons(frame, &mut wizard.ui, inner, &t!("skip_modal.back"), &t!("skip_modal.confirm"));
}

/// The skip warning's two buttons: the way back on the modal's last row,
/// then the confirmation two cells to its right where the whole label fits
/// there, or else on the row above, from the left edge. Given the inner
/// width whatever its x, the confirmation (and its click rect) once ran
/// over the border and past it wherever the two buttons outgrew the row:
/// French at any width (63 cells with the gap, in a 60-cell row), and
/// German, Spanish, Italian, Portuguese and Russian at the wizard's
/// 58-column floor, where the row is 52. Clamping it to the row kept it
/// inside but cut the destructive action's label mid-word ("Passer en
/// public quand mêm", Russian down to "Всё равно пу"), the phantom-typo
/// fault the body's clip above exists to prevent; so it moves up instead.
/// That row is free: the body stops two rows short of the bottom. Each
/// button is still clamped to the room left before the right edge, as the
/// directory browser's row is, so a label wider than the modal on a row of
/// its own (none of the shipped ones) is cut inside the frame and the
/// click stays on what is drawn. Apart from the modal's body, with its
/// labels passed in, so the test draws it in each language without
/// switching the process-wide locale. Returns both buttons' rects.
fn skip_buttons(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    inner: Rect,
    back: &str,
    confirm: &str,
) -> (Rect, Rect) {
    let y = inner.bottom().saturating_sub(1);
    let row = |x: u16, y: u16| Rect { x, y, width: inner.right().saturating_sub(x), height: 1 };
    let back = kit::button(frame, ui, row(inner.x, y), back, true, Act::SkipCancel);
    let beside = back.right() + 2;
    let fits = beside as usize + kit::width(&format!("  {confirm}  ")) <= inner.right() as usize;
    // A modal too short for a second row keeps the confirmation beside,
    // clamped, rather than drawing it on the frame's top edge.
    let at = if fits || y <= inner.y { row(beside, y) } else { row(inner.x, y - 1) };
    let confirm = kit::button(frame, ui, at, confirm, false, Act::SkipConfirm);
    (back, confirm)
}

fn draw_browser(frame: &mut Frame, wizard: &mut Wizard, area: Rect, browse: &Browse) {
    let inner = kit::modal_frame_on(frame, &mut wizard.ui, area, 66, 18, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("browse.title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut wizard.ui, inner, Act::BrowseCancel);
    frame.render_widget(
        Paragraph::new(Span::styled(browse.path.clone(), dim())),
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
    );

    let list_top = inner.y + 3;
    let visible = inner.height.saturating_sub(5) as usize;
    let first = browse.sel.saturating_sub(visible.saturating_sub(1));
    for (row, i) in (first..browse.dirs.len().min(first + visible)).enumerate() {
        let selected = i == browse.sel;
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else {
            Style::default()
        };
        let rect = Rect { x: inner.x, y: list_top + row as u16, width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} {}", g("▸", "►"), browse.dirs[i]), style)),
            rect,
        );
        wizard.ui.click(rect, Act::BrowseRow(i));
    }
    if browse.dirs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("browse.empty").to_string(), dim())),
            Rect { x: inner.x, y: list_top, width: inner.width, height: 1 },
        );
    }

    let y = inner.bottom().saturating_sub(1);
    // Each button gets the room left before the modal's right edge, so the
    // kit cuts the last label inside the frame: the Japanese row is 57 cells,
    // and a narrow window's modal is not, which drew 閉じる (and its click
    // rect) over the border and past it.
    let row = |x: u16| Rect { x, y, width: inner.right().saturating_sub(x), height: 1 };
    let up = kit::button(frame, &mut wizard.ui, row(inner.x), &t!("browse.up"), false, Act::BrowseUp);
    let open = kit::button(frame, &mut wizard.ui, row(up.right() + 1), &t!("browse.open"), false, Act::BrowseEnter);
    let add = kit::button(frame, &mut wizard.ui, row(open.right() + 1), &t!("browse.add"), true, Act::BrowseAdd);
    kit::button(frame, &mut wizard.ui, row(add.right() + 1), &t!("browse.close"), false, Act::BrowseCancel);
}

fn draw_path_entry(frame: &mut Frame, wizard: &mut Wizard, area: Rect, draft: &PathDraft) {
    let count = draft.suggestion_count();
    let shown = count.min(6) as u16;
    // Anchored as if always full: the title and input hold one spot and
    // the suggestion list grows DOWNWARD beneath them.
    let inner =
        kit::modal_frame_anchored_on(frame, &mut wizard.ui, area, 62, 7 + shown, 13, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("path_modal.title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut wizard.ui, inner, Act::PathCancel);
    // Drawn after render cleared the page's registries for the modal, so
    // its caret is the frame's: the modal's field has the keyboard.
    let line = kit::field_display(
        &mut wizard.ui,
        inner.x,
        inner.y + 2,
        draft.text.value(),
        draft.text.cursor(),
        inner.width,
        None,
    );
    frame.render_widget(
        Paragraph::new(Span::raw(line)),
        Rect { x: inner.x, y: inner.y + 2, width: inner.width, height: 1 },
    );
    let sel_moved = draft.sel != draft.sel_anchor;
    let reveal = if sel_moved { draft.sel } else { None };
    let (first, visible) = kit::table_view(count, reveal, draft.scroll, 6);
    if let Modal::PathEntry(d) = &mut wizard.modal {
        d.scroll = first;
        d.sel_anchor = d.sel;
    }
    let overflow = count > visible;
    let row_width = if overflow { inner.width.saturating_sub(1) } else { inner.width };
    for (row, entry) in draft.suggestions().skip(first).take(visible).enumerate() {
        let i = first + row;
        let selected = draft.sel == Some(i);
        let rect =
            Rect { x: inner.x, y: inner.y + 4 + row as u16, width: row_width, height: 1 };
        let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            dim()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} {entry}", g("▸", "►")), style)),
            rect,
        );
        wizard.ui.click(rect, Act::PathSuggest(i));
    }
    let bar = Rect {
        x: inner.x + inner.width.saturating_sub(1),
        y: inner.y + 4,
        width: 1,
        height: visible as u16,
    };
    kit::scroll_list(
        frame,
        &mut wizard.ui,
        bar,
        count,
        visible,
        first,
        Act::PathScroll(-1),
        Act::PathScroll(1),
        Act::PathScrollTo,
    );
    // A listing failure for the current dir-part shows where suggestions
    // would be — kit error style, the server's words.
    if let Some(err) = &draft.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            Rect { x: inner.x, y: inner.y + 4, width: inner.width, height: 1 },
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::tests::{LOCALE_LOCK, cells_text, click_rect, draw_100x30, row_text};
    use super::*;

    #[test]
    fn the_logo_is_the_server_banner_verbatim() {
        assert!(LOGO.iter().all(|l| l.chars().count() <= LOGO_W as usize));
        assert_eq!(LOGO[0].trim(), "____  _");
        // The joins that line breaks once mangled: the S's back, the r's
        // stem, and the two-cell gap before the e.
        assert!(LOGO[2].contains(r"_ \\___ \|"), "m's trailing and S's leading backslash are ADJACENT");
        assert!(LOGO[3].contains(r"| |_| | |  __/"));
        assert!(LOGO[4].contains(r"\__|_|  \___|"));
    }

    /// The language chip and the bars' buttons measure their labels in
    /// cells: in Japanese and Chinese at 100×30 the chip shows the whole
    /// name of the language (it showed "▾ 日"), and each screen's bar frames
    /// the whole label of its forward action (it framed "続け" of "続ける ▸")
    /// with its right edge on the bar's, where a character count left it
    /// short, and the whole way back two cells to its left.
    ///
    /// Nothing here switches the process-global locale, which tests that
    /// assert English without the lock would read mid-draw: the chip's
    /// label is the language's own name from `LANGS`, drawn whatever the
    /// locale once the index is set, and the buttons are drawn by
    /// `bar_buttons`, the one function the Folders, Extras and Login bars
    /// all call, with the labels translated at the call. The English wizard
    /// checks the bar's own path through `render`. The glyph tier is pinned
    /// first: the chip's arrow is read once by the draw and once for the
    /// label expected, and on a bare Windows console another test's pin
    /// landing between the two reads would make them differ.
    #[test]
    fn the_chip_and_the_bar_buttons_show_their_whole_cjk_labels_at_100x30() {
        crate::kit::theme::pin_modern_terminal();
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let bars = [
            ("folders.continue_enabled", Act::ContinueFolders, None),
            ("extras.continue", Act::ContinueExtras, Some(("extras.back", Act::BackToFolders))),
            ("login.create", Act::CreateAdmin, Some(("login.back", Act::BackToExtras))),
        ];
        for code in ["ja", "zh"] {
            let mut wizard = Wizard::new(Client::new("http://127.0.0.1:9").expect("client"));
            wizard.lang = LANGS.iter().position(|(c, _)| *c == code).expect(code);
            wizard.add_folder("/srv/music".to_string());
            let buf = draw_100x30(&mut wizard);

            let chip_label = format!("{} {}", g("▾", "▼"), LANGS[wizard.lang].1);
            let chip = click_rect(&wizard, Act::OpenLanguage);
            assert_eq!(usize::from(chip.width), kit::width(&chip_label), "{code}: its width");
            assert_eq!(cells_text(&buf, 0, chip.x, chip.right()), chip_label, "{code}: the chip");

            for (key, act, back) in bars.clone() {
                let label = t!(key, locale = code).to_string();
                let back_label = back.map(|(key, act)| (t!(key, locale = code).to_string(), act));
                let backend = ratatui::backend::TestBackend::new(100, 3);
                let mut terminal = ratatui::Terminal::new(backend).unwrap();
                let mut ui = kit::Surface::<Act>::new();
                let bar = Rect { x: 2, y: 0, width: 96, height: 3 };
                let mut drawn = (Rect::default(), None);
                let back = back_label.as_ref().map(|(label, act)| (label.as_str(), act.clone()));
                terminal
                    .draw(|frame| {
                        drawn = bar_buttons(frame, &mut ui, bar, (&label, true, act), back);
                    })
                    .unwrap();
                let buf = terminal.backend().buffer();
                let (button, back_rect) = drawn;
                assert_eq!(button.right(), 98, "{code} {key}: on the bar's right edge");
                let inside = cells_text(buf, button.y + 1, button.x + 1, button.right() - 1);
                assert_eq!(inside, format!("  {label}  "), "{code} {key}: its whole label");
                if let Some((back_label, _)) = &back_label {
                    let back = back_rect.expect("the way back is drawn");
                    assert_eq!(back.right() + 2, button.x, "{code} {key}: two cells left of it");
                    assert_eq!(back.width, kit::tall_width(back_label), "{code} {key}: its width");
                    let inside = cells_text(buf, back.y + 1, back.x + 1, back.right() - 1);
                    assert_eq!(inside, format!("  {back_label}  "), "{code} {key}: the way back");
                }
            }
        }
        let mut wizard = Wizard::new(Client::new("http://127.0.0.1:9").expect("client"));
        wizard.add_folder("/srv/music".to_string());
        draw_100x30(&mut wizard);
        let button = click_rect(&wizard, Act::ContinueFolders);
        assert_eq!(button.right(), 98, "the bar's own frame ends on its right edge");
        assert_eq!(button.width, kit::tall_width(&t!("folders.continue_enabled")));
    }

    /// A Done page on a server at port 3000 with a Quick Connect code, in
    /// two columns (pictures) or stacked (none).
    fn done_page(two_column: bool) -> Wizard {
        let mut wizard = Wizard::new(Client::new("http://127.0.0.1:3000").expect("client"));
        wizard.screen = Screen::Done;
        wizard.qr = Some(vec!["▀▀▀▀".to_string()]);
        if two_column {
            wizard.graphics =
                crate::tui::graphics::Graphics::forced(ratatui_image::picker::ProtocolType::Kitty);
        }
        assert_eq!(wizard.done_two_column(), two_column);
        wizard
    }

    fn draw_at(wizard: &mut Wizard, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, wizard)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The Done page's rows of text, each trimmed, with the two columns'
    /// right half only (what follows the vertical gold rule) — the
    /// message reads back whole by joining them.
    fn done_text(buf: &ratatui::buffer::Buffer, two_column: bool) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| row_text(buf, y))
            .map(|row| match row.split_once('│') {
                Some((_, right)) if two_column => right.trim().to_string(),
                _ => row.trim().to_string(),
            })
            .collect()
    }

    /// The Done message takes the rows its wrapped text needs. At 70
    /// columns the English wraps to five rows in the two columns' right
    /// half, where a fixed four lost "automatically.", and to three on the
    /// stacked page, where it had two; either way it shows whole, the
    /// buttons start under it, and the list ends inside the column.
    #[test]
    fn the_done_message_shows_whole_at_seventy_columns_in_english() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        for (two_column, scan) in [(true, t!("done.scan_left")), (false, t!("done.scan_above"))] {
            let mut wizard = done_page(two_column);
            let buf = draw_at(&mut wizard, 70, 30);
            let message = format!("{} {scan}", t!("done.running_port", port = 3000));
            let text = done_text(&buf, two_column);
            assert!(text.join(" ").contains(&message), "two_column {two_column}: {text:#?}");
            let last = text.iter().rposition(|row| row.ends_with("automatically.")).expect("end");
            let first = click_rect(&wizard, done_action(0));
            assert!(first.y > last as u16, "two_column {two_column}: the list under the message");
            let list_end = click_rect(&wizard, done_action(DONE_ACTIONS - 1)).bottom();
            assert!(list_end <= 2 + 30 - 6, "two_column {two_column}: inside the column");
        }
    }

    /// At the wizard's floor, 58×20, with a scan running and a code to
    /// scan, the Done page keeps its button list inside the column, clear
    /// of the note and the tips under it, in both layouts: the two columns
    /// trade the tall cards for one-row buttons when the cards do not fit,
    /// and the message gives up its tail before the list moves past it.
    #[test]
    fn the_done_list_stays_inside_the_column_at_the_floor() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        for two_column in [true, false] {
            let mut wizard = done_page(two_column);
            wizard.scan = Some(ScanWidget {
                step: "Scanning".to_string(),
                pct: Some(40),
                detail: "1,234 files".to_string(),
            });
            let buf = draw_at(&mut wizard, 58, 20);
            let text = done_text(&buf, two_column);
            assert!(text.iter().any(|row| row.starts_with("Your mStream")), "{text:#?}");
            let mut bottom = 0;
            for i in 0..DONE_ACTIONS {
                let rect = click_rect(&wizard, done_action(i));
                assert!(rect.y >= bottom, "two_column {two_column}: {i} under the one before");
                bottom = rect.bottom();
            }
            assert!(bottom <= 2 + 20 - 6, "two_column {two_column}: the list ends at {bottom}");
        }
        // With the room for them, the two columns keep the tall cards.
        let mut wizard = done_page(true);
        draw_100x30(&mut wizard);
        assert_eq!(click_rect(&wizard, done_action(0)).height, 3, "the tall cards at 100×30");
        // The standalone Quick Connect page has no scan, so it keeps no
        // rows for one: at the floor its message shows whole, where four
        // blank rows kept for a scan cut its last three.
        let mut wizard = done_page(true);
        wizard.standalone = true;
        let buf = draw_at(&mut wizard, 58, 20);
        let message = format!("{} {}", t!("done.running_port", port = 3000), t!("done.scan_left"));
        let text = done_text(&buf, true);
        assert!(text.join(" ").contains(&message), "the standalone page at the floor: {text:#?}");
    }

    /// At 80×24, the most common terminal size, the two-column wizard Done
    /// page keeps its tall cards under its whole four-row English message
    /// and the scan's rows: the last card ends on the blank row above the
    /// note, one past the column, as it always did, and the note's row stays
    /// its own. A bound of the column alone gave every 24-row window the
    /// one-row list.
    #[test]
    fn the_two_column_done_page_keeps_its_tall_cards_at_80x24() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let mut wizard = done_page(true);
        wizard.note = Some(("A note".to_string(), false));
        let buf = draw_at(&mut wizard, 80, 24);
        let message = format!("{} {}", t!("done.running_port", port = 3000), t!("done.scan_left"));
        let text = done_text(&buf, true);
        assert!(text.join(" ").contains(&message), "the message whole: {text:#?}");
        let last = text.iter().rposition(|row| row.ends_with("automatically.")).expect("end");
        let first = click_rect(&wizard, done_action(0));
        assert_eq!(first.height, 3, "the tall cards");
        assert!(first.y > last as u16 + DONE_SCAN_ROWS, "the cards under the message and the scan");
        let end = click_rect(&wizard, done_action(DONE_ACTIONS - 1)).bottom();
        assert_eq!(end, 24 - 3, "the last card ends on the row above the note");
        assert_eq!(row_text(&buf, 24 - 3).trim(), "A note", "the note's row is its own");
    }

    /// The stacked Done page lets the half-block code in before the scan's
    /// status: at a height where the code fits only without the scan's
    /// three lines it is drawn, with the scan's step on one row or none,
    /// and the list stays inside the column. Whether the code shows never
    /// depends on whether a poll has answered yet, so the first answer
    /// cannot swap a code just drawn for the apology line.
    #[test]
    fn the_stacked_done_page_keeps_its_code_before_the_scan_status() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let rows: u16 = 29;
        let code_row = "▀".repeat(29);
        let shows_code = |buf: &ratatui::buffer::Buffer| {
            (0..buf.area.height).filter(|&y| row_text(buf, y).contains(&code_row)).count()
        };
        // The code comes in as soon as it fits with the message (two rows
        // here), the list and the blanks between: no rows are held back for
        // the scan's three lines (y 4 for the heading, the column's bottom
        // at the height less four).
        let threshold = 4 + rows + 2 + 1 + DONE_ACTIONS as u16 + 1 + 4;
        let mut drawn_at = None;
        for height in 40..=60 {
            let mut shown = Vec::new();
            for scan in [None, Some(40)] {
                let mut wizard = done_page(false);
                wizard.qr = Some(vec![code_row.clone(); usize::from(rows)]);
                wizard.scan = scan.map(|pct| ScanWidget {
                    step: "Scanning media".to_string(),
                    pct: Some(pct),
                    detail: "1204 tracks so far".to_string(),
                });
                let buf = draw_at(&mut wizard, 100, height);
                let code = shows_code(&buf);
                assert!(code == 0 || code == usize::from(rows), "{height} rows: the code whole or not at all");
                let list_end = click_rect(&wizard, done_action(DONE_ACTIONS - 1)).bottom();
                assert!(list_end <= height - 4, "{height} rows: the list ends at {list_end}");
                // The scan's step from two rows past the threshold, and its
                // whole three lines from four past it (and wherever the
                // code is not drawn at all).
                let text = done_text(&buf, false).join("\n");
                if scan.is_some() && (code == 0 || height >= threshold + DONE_SCAN_SHORT_ROWS) {
                    assert!(text.contains("Scanning media"), "{height} rows: {text}");
                }
                if scan.is_some() && (code == 0 || height >= threshold + DONE_SCAN_ROWS) {
                    assert!(text.contains("1204 tracks so far"), "{height} rows: {text}");
                } else if scan.is_some() {
                    assert!(!text.contains("1204 tracks so far"), "{height} rows: {text}");
                }
                shown.push(code > 0);
            }
            assert_eq!(shown[0], shown[1], "{height} rows: the code alike before and after a poll");
            if shown[0] && drawn_at.is_none() {
                drawn_at = Some(height);
            }
        }
        assert_eq!(drawn_at, Some(threshold));
    }

    /// Draws the skip modal's frame and buttons at `width`×30 with the given
    /// labels; returns the modal's inner rect, both buttons' rects, the
    /// buffer, and the confirmation's click rect.
    fn draw_skip_buttons(
        width: u16,
        back_label: &str,
        confirm_label: &str,
    ) -> (Rect, Rect, Rect, ratatui::buffer::Buffer, Option<Rect>) {
        let backend = ratatui::backend::TestBackend::new(width, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut ui = kit::Surface::<Act>::new();
        let (mut inner, mut drawn) = (Rect::default(), (Rect::default(), Rect::default()));
        terminal
            .draw(|frame| {
                let area = frame.area();
                inner = kit::modal_frame_on(frame, &mut ui, area, 62, 15, th().gold);
                drawn = skip_buttons(frame, &mut ui, inner, back_label, confirm_label);
            })
            .unwrap();
        let clicked = ui.clicks.iter().find(|(_, a)| *a == Act::SkipConfirm).map(|(r, _)| *r);
        (inner, drawn.0, drawn.1, terminal.backend().buffer().clone(), clicked)
    }

    /// The skip warning's buttons stay inside the modal and whole in every
    /// language, at the wizard's floor (58 columns, where the modal is 54
    /// wide) and at 100 (its full 62): the way back on the last row, the
    /// confirmation beside it or, where the pair outgrows the row, on the
    /// row above from the left edge; each ends at the inner right edge at
    /// the latest, the border's cell on its row is still the border, and
    /// the confirmation's click rect is what it drew. Which languages stack
    /// is left to their translations' lengths, so shortening one never
    /// fails this; the two placements themselves are pinned by the
    /// synthetic test below. Drawn through `skip_buttons` with each
    /// language's labels, so no test switches the process-wide locale.
    #[test]
    fn the_skip_modal_buttons_stay_whole_inside_its_border_in_every_language() {
        for (code, _) in LANGS {
            let back_label = t!("skip_modal.back", locale = code).to_string();
            let confirm_label = t!("skip_modal.confirm", locale = code).to_string();
            for width in [58, 100] {
                let (inner, back, confirm, buf, clicked) = draw_skip_buttons(width, &back_label, &confirm_label);
                let at = format!("{code} at {width} columns");
                let row = |rect: Rect| cells_text(&buf, rect.y, rect.x, rect.right());
                assert_eq!(back.y, inner.bottom() - 1, "{at}: the way back on the last row");
                assert_eq!(row(back), format!("  {back_label}  "), "{at}: the way back, whole");
                assert_eq!(row(confirm), format!("  {confirm_label}  "), "{at}: the confirmation, whole");
                assert_eq!(clicked, Some(confirm), "{at}: its click is its rect");
                for rect in [back, confirm] {
                    assert!(rect.right() <= inner.right(), "{at}: {rect:?} past {inner:?}");
                    assert_eq!(buf[(inner.right(), rect.y)].symbol(), "│", "{at}: the border");
                }
                let beside = (confirm.x, confirm.y) == (back.right() + 2, back.y);
                let above = (confirm.x, confirm.y) == (inner.x, back.y - 1);
                assert!(beside || above, "{at}: beside the way back or above it: {back:?} {confirm:?}");
            }
        }
    }

    /// The two placements, with labels whose widths are fixed here rather
    /// than by a translation: a 40-cell confirmation beside an 8-cell way
    /// back takes 54 cells with the gap, so it sits beside at 100 columns
    /// (a 60-cell row) and moves up whole at 58 (a 52-cell row); a 64-cell
    /// CJK one is wider than the modal on a row of its own, so it moves up
    /// and is clamped there, cut cleanly at the inner right edge with the
    /// border intact and its click on what it drew.
    #[test]
    fn the_skip_modal_confirmation_moves_above_the_way_back_when_the_pair_outgrows_the_row() {
        let ascii = "x".repeat(40);
        for (width, stacked) in [(100, false), (58, true)] {
            let (inner, back, confirm, buf, clicked) = draw_skip_buttons(width, "Back", &ascii);
            let at = format!("{width} columns");
            if stacked {
                assert_eq!((confirm.x, confirm.y), (inner.x, back.y - 1), "{at}: above the way back");
            } else {
                assert_eq!((confirm.x, confirm.y), (back.right() + 2, back.y), "{at}: beside the way back");
            }
            assert_eq!(cells_text(&buf, confirm.y, confirm.x, confirm.right()), format!("  {ascii}  "), "{at}: whole");
            assert_eq!(clicked, Some(confirm), "{at}: its click is its rect");
        }
        let cjk = "公開".repeat(15);
        assert_eq!(kit::width(&cjk), 60);
        for width in [58, 100] {
            let (inner, back, confirm, buf, clicked) = draw_skip_buttons(width, "Back", &cjk);
            let at = format!("{width} columns, CJK");
            assert_eq!((confirm.x, confirm.y), (inner.x, back.y - 1), "{at}: above the way back");
            assert_eq!(confirm.right(), inner.right(), "{at}: clamped to the edge");
            assert_eq!(buf[(inner.right(), confirm.y)].symbol(), "│", "{at}: the border");
            let text = cells_text(&buf, confirm.y, confirm.x, confirm.right());
            assert!(format!("  {cjk}  ").starts_with(text.trim_end()) && text.len() > 2, "{at}: cut, not garbled: {text:?}");
            assert_eq!(clicked, Some(confirm), "{at}: its click is its rect");
        }
    }

    #[test]
    fn the_path_modal_is_drawn_from_its_own_draft_and_keeps_its_listing() {
        // The draft lends the frame its listing instead of being copied
        // with it every frame (performance audit #110): the listing is back
        // after a draw, and the scroll the draw keeps lands in the draft.
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        let entries: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        wizard.modal = Modal::PathEntry(PathDraft {
            text: "/srv/".into(),
            listed_for: "/srv/".to_string(),
            listed_path: "/srv".to_string(),
            entries: entries.clone(),
            sel: Some(11),
            ..PathDraft::default()
        });
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        terminal.draw(|frame| render(frame, &mut wizard)).unwrap();
        let Modal::PathEntry(draft) = &wizard.modal else { panic!("the path modal") };
        assert_eq!(draft.entries, entries, "the listing is back after the frame");
        assert_eq!(draft.sel_anchor, Some(11), "the draw kept its anchor");
        assert!((6..=11).contains(&draft.scroll), "scrolled to the cursor: {}", draft.scroll);
        // The browser is lent and put back whole.
        let browse = Browse { path: "/srv".into(), dirs: entries, sel: 3 };
        wizard.modal = Modal::Browser(browse.clone());
        terminal.draw(|frame| render(frame, &mut wizard)).unwrap();
        assert_eq!(wizard.modal, Modal::Browser(browse));
    }
}
