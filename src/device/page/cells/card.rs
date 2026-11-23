//! An SD card's view in character cells (clauses 20a, 25, 25a, 44–53 and
//! 58 of docs/ux-contracts/mp3-player-screen.md): its tab, its rows in the
//! board card's grammar and Details, the board's card line, the gold gates
//! and the Settings sheet, laid out and drawn on the kit's surface. Its
//! model is page/card.rs (the card's state, its keys, its hints and what
//! its gates ask); this reads it as any view would.

use super::*;
use super::super::card::{
    CardGate, CardPrimary, CardSheet, GateKind, SRow, SheetTab, ago, card_name, codec_words, list_entries, next_words,
    now, peer_name, plan_of, primary, server_name, span, waits,
};
use crate::device::card::engine::Removal;
use crate::device::card::plan::{Fit, TRACK_CAP};
use crate::device::card::state::Convert;
use crate::device::card::volume::{Fs, Refusal, Table};
use crate::device::card::worker::{CardView, Ended, Gate as Asked, Next, Pause, Phase, Read, Run, Server};

// ── The card's tab ──────────────────────────────────────────────────────────

/// A card's mark on its tab (clause 25a).
pub(in crate::device::page) fn mark(card: &CardView) -> (String, Color) {
    let g = glyphs();
    if card.out {
        return (t!("dev.card.tab_out").to_string(), th().dim);
    }
    if let Some(pct) = card.percent() {
        return (format!("{pct}%"), th().accent);
    }
    if let Some(Ended::Paused { bytes: (done, all), .. }) = &card.ended {
        let pct = if *all > 0 { done * 100 / all } else { 0 };
        return (format!("{}%", pct.min(100)), th().dim);
    }
    if card.run.is_some() || card.busy.is_some() || matches!(card.read, Read::Reading) {
        return (g.busy.to_string(), th().dim);
    }
    let cant = match &card.read {
        Read::Refused | Read::Failed(_) => true,
        Read::Read(f) => !f.newer.is_empty() || f.check,
        Read::Reading => false,
    };
    if cant {
        return (g.no.to_string(), th().gold);
    }
    match &card.next {
        Next::Asking => return (g.busy.to_string(), th().dim),
        Next::Unreached { .. } => return (g.unknown.to_string(), th().dim),
        _ => {}
    }
    match primary(card) {
        Some(CardPrimary::Eject) | None if !waits(card) => (g.ok.to_string(), th().ok),
        _ => (g.warn.to_string(), th().gold),
    }
}

/// A card's tab's name (clause 25): on Windows its drive, then its name;
/// elsewhere its name alone. `room` cells at most for the name.
pub(super) fn tab_name(card: &CardView, room: usize) -> String {
    let name = card.name().unwrap_or_else(|| t!("dev.card.tab_new").to_string());
    let name = if room < kit::width(&name) { fit(&name, room.max(6)) } else { name };
    if cfg!(windows) { format!("{} {name}", card.drive()) } else { name }
}

// ── The Card view ───────────────────────────────────────────────────────────

/// A card's or a library's size in the locale's unit: `127.9 GB`, `127.9 Go`.
fn size_words(bytes: u64) -> String {
    t!("dev.card_size", size = gb(bytes)).to_string()
}

/// The head's right edge: `E: · FAT32`, `G: · FAT32 on GPT`, `E: · FAT32
/// · read only`, `G: · exFAT · 127.9 GB`, or `not in a reader`.
fn head_right(card: &CardView) -> String {
    if card.out {
        return t!("dev.card.head_out").to_string();
    }
    let v = &card.found.volume;
    let mut fs = v.fs.name().to_string();
    if v.table == Table::Gpt {
        fs = t!("dev.card.head_on", fs = fs, table = "GPT").to_string();
    }
    let mut parts = vec![card.drive(), fs];
    let unusable = matches!(v.fs, Fs::ExFat | Fs::Ntfs | Fs::Fat12 | Fs::Other(_));
    if let Some(space) = v.space.filter(|_| unusable) {
        parts.push(size_words(space.size));
    }
    if v.read_only || card.found.disk.read_only {
        parts.push(t!("dev.card.head_read_only").to_string());
    }
    parts.join(" · ")
}

/// A gold problem row: `! words` bold, then dim lines under it.
fn problem(label: &str, glyph: &str, words: &str, lines: &[String], w: u16) -> Vec<Row> {
    let gold = Style::default().fg(th().gold);
    let pieces = [(format!("{glyph} "), gold), (words.to_string(), gold.add_modifier(Modifier::BOLD))];
    let mut rows = value_rows(Some(label.to_string()), &pieces, w);
    for line in lines {
        rows.extend(dim_rows(None, line, w));
    }
    rows
}

/// The Space row: the bar, and its numbers (clause 44).
fn space_rows(card: &CardView, bar: bool) -> Vec<Row> {
    let label = Some(t!("dev.card.label_space").to_string());
    let Some(space) = card.found.volume.space else { return Vec::new() };
    let used = space.size.saturating_sub(space.free);
    let mut left = t!("dev.card_used", used = gb(used), size = gb(space.size)).to_string();
    if let Some(facts) = card.facts().filter(|f| f.tracks > 0) {
        left.push_str(&format!(" · {}", t!("dev.card_tracks", n = fmt_count(facts.tracks as u64))));
    }
    let free = t!("dev.card_free", free = gb(space.free)).to_string();
    let full = space.size > 0 && (used * 100 / space.size >= 98 || space.free < 1_000_000_000);
    let right = if full {
        Line::from(Span::styled(format!("{} {free}", glyphs().warn), Style::default().fg(th().gold)))
    } else {
        Line::from(Span::raw(free))
    };
    let mut rows = Vec::new();
    if bar {
        let mut permille = (u128::from(used) * 1000 / u128::from(space.size.max(1))) as u32;
        if used > 0 {
            permille = permille.max(1);
        }
        let color = if full { th().gold } else { th().accent };
        rows.push(Row::Bar { label, permille, color });
        rows.push(Row::Value { label: None, left: Line::from(left), right: Some(right) });
    } else {
        rows.push(Row::Value { label, left: Line::from(left), right: Some(right) });
    }
    rows
}

/// The Last sync row (clause 44).
fn last_rows(card: &CardView, w: u16) -> Vec<Row> {
    let label = Some(t!("dev.card.label_last").to_string());
    let Some(facts) = card.facts() else { return Vec::new() };
    let gold = Style::default().fg(th().gold);
    let mut pieces: Vec<(String, Style)> = Vec::new();
    match &card.ended {
        Some(Ended::Done { skipped, plays, .. }) => {
            pieces.push((format!("{} · {}", t!("dev.card.ago_now"), t!("dev.card.verified")), Style::default()));
            if *plays > 0 {
                let words = t!("dev.card.done_plays", n = fmt_count(*plays as u64), server = server_name(card));
                pieces.push((format!(" · {words}"), Style::default()));
            }
            if !skipped.is_empty() {
                let words = t!("dev.card.skipped", n = fmt_count(skipped.len() as u64));
                pieces.push((format!(" · {} {words}", glyphs().warn), gold));
            }
        }
        _ if facts.tracks == 0 => {
            let words = match (facts.set_up(), facts.committed) {
                (true, Some(at)) => t!("dev.card.last_setup", when = ago(at, now())),
                _ => t!("dev.card.last_never"),
            };
            pieces.push((words.to_string(), Style::default()));
        }
        _ => {
            let when = facts.committed.map_or_else(|| "?".to_string(), |at| ago(at, now()));
            pieces.push((when, Style::default()));
        }
    }
    let mut rows = value_rows(label, &pieces, w);
    if let Some(Ended::Refused { peer, .. }) = &card.ended {
        let peer = peer_name(card, *peer);
        let mut words = t!("dev.card.err_refused", peer = peer.clone()).to_string();
        let libs = card
            .facts()
            .and_then(|f| f.state.as_ref())
            .and_then(|s| s.settings.peers.iter().find(|p| p.nickname == peer).map(|p| p.libraries.join(", ")))
            .unwrap_or_default();
        words.push_str(&format!(": {}", t!("dev.card.err_refused_2", libs = libs)));
        rows.extend(value_rows(None, &[(format!("{} {words}", glyphs().warn), gold)], w));
    }
    rows
}

/// The Next sync row (clause 44): what a sync would do now, the server
/// not answering, or the fit's verdict.
fn next_rows(page: &Page, card: &CardView, w: u16) -> Vec<Row> {
    let label = Some(t!("dev.card.label_next").to_string());
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let server = server_name(card);
    if card.run.as_ref().is_some_and(|r| r.phase == Phase::Waiting) {
        let running = page.cards().into_iter().find(|c| c.syncing()).map(|c| card_name(&c)).unwrap_or_default();
        let mut words = Vec::new();
        if let Some(plan) = plan_of(card).filter(|p| p.new > 0) {
            words.push(t!("dev.card.next_new", n = fmt_count(plan.new as u64)).to_string());
        }
        words.push(t!("dev.card.waits_for", card = running).to_string());
        return dim_rows(label, &words.join(" · "), w).into_iter().map(undim).collect();
    }
    match &card.next {
        Next::Unasked => Vec::new(),
        Next::Asking => value_rows(label, &scan(None, &t!("dev.card.next_asking", server = server)), w),
        // The plays that wait for it, as the worker counted them off the
        // card and this computer's outbox: the server answered nothing.
        Next::SignIn { plays } => {
            if *plays == 0 {
                return Vec::new();
            }
            let n = fmt_count(*plays as u64);
            let words = t!("dev.card.next_signin", n = n, server = server).to_string();
            value_rows(label, &[(words, Style::default())], w)
        }
        Next::Unreached { asked, .. } => {
            let words = t!("dev.card.next_unreached", server = server, when = ago(*asked, now()));
            value_rows(label, &[(words.to_string(), Style::default())], w)
        }
        Next::Refused(why) => value_rows(label, &[(why.clone(), dim())], w),
        Next::Plan(plan) => {
            if let Some(fit) = plan.fit.filter(|f| f.over_cap()) {
                let n = fmt_count(fit.tracks as u64);
                let head = t!("dev.card.fit_cap", n = n, max = fmt_count(TRACK_CAP as u64));
                let mut rows = value_rows(label, &[(format!("{} ", g.no), gold), (head.to_string(), gold)], w);
                let cards = fit.tracks.div_ceil(TRACK_CAP).max(2);
                rows.extend(dim_rows(None, &t!("dev.card.fit_cap_2", n = cards), w));
                return rows;
            }
            if let Some(fit) = plan.fit.filter(|f| !f.fits()) {
                let words = t!("dev.card.fit_no", gb = gb(fit.short()));
                return value_rows(label, &[(format!("{} ", g.no), gold), (words.to_string(), gold)], w);
            }
            value_rows(label, &[(next_words(card, plan, false), Style::default())], w)
        }
    }
}

/// A dim row made plain.
fn undim(row: Row) -> Row {
    match row {
        Row::Value { label, left, right } => {
            let spans: Vec<Span> = left.spans.into_iter().map(|s| Span::raw(s.content.to_string())).collect();
            Row::Value { label, left: Line::from(spans), right }
        }
        other => other,
    }
}

/// The Converts row (clause 44): the rule, and under it, dim, the player
/// assumed or a card for someone else.
fn converts_rows(card: &CardView, w: u16) -> Vec<Row> {
    let label = Some(t!("dev.card.label_converts").to_string());
    let Some(facts) = card.facts() else { return Vec::new() };
    let Some(state) = &facts.state else { return Vec::new() };
    let c = state.settings.conversion;
    let codec = codec_words(c.codec, c.bitrate);
    let words = match c.convert {
        Convert::Unplayable => t!("dev.card.conv_needed", codec = codec),
        Convert::Everything => t!("dev.card.conv_all", codec = codec),
    };
    let mut rows = value_rows(label, &[(words.to_string(), Style::default())], w);
    if !facts.device.present {
        rows.extend(dim_rows(None, &t!("dev.card.conv_default"), w));
    }
    if state.settings.for_someone_else {
        rows.extend(dim_rows(None, &t!("dev.card.conv_gift"), w));
    }
    rows
}

/// The scan widget of a sync asking for its waveforms (clause 51a): how far the
/// server's answers have come.
fn waves_scan(run: &Run, server: &str) -> Vec<(String, Style)> {
    let (i, n) = (fmt_count(run.waves.0 as u64), fmt_count(run.waves.1 as u64));
    scan(Some(run.waves_percent()), &t!("dev.card.sync_waves", server = server, i = i, n = n))
}

/// The rows of a sync under way, paused, or ended in an error (clauses
/// 51–53).
fn run_rows(page: &Page, card: &CardView, w: u16) -> Option<Vec<Row>> {
    let label = Some(t!("dev.card.label_syncing").to_string());
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let server = server_name(card);
    if let Some(run) = card.run.as_ref().filter(|r| r.phase != Phase::Waiting) {
        let mut rows = Vec::new();
        match run.phase {
            Phase::Listing => {
                rows.extend(value_rows(label, &scan(None, &t!("dev.card.next_asking", server = server)), w));
            }
            Phase::Planning => rows.extend(value_rows(label, &scan(None, &t!("dev.card.sync_planning")), w)),
            Phase::Index | Phase::Verifying if run.files == 0 || run.phase == Phase::Verifying => {
                let key = if run.phase == Phase::Index { "dev.card.sync_index" } else { "dev.card.sync_verifying" };
                let words = t!(key);
                rows.extend(value_rows(label, &scan(None, &words), w));
            }
            Phase::Waves if run.files == 0 => rows.extend(value_rows(label, &waves_scan(run, &server), w)),
            _ => {
                let permille = card.percent().map_or(0, |p| u32::from(p) * 10);
                rows.push(Row::Bar { label, permille, color: th().accent });
                let count = t!(
                    "dev.card.sync_count",
                    i = fmt_count(run.done_files as u64),
                    n = fmt_count(run.files as u64),
                    done = gb(run.done_bytes),
                    total = gb(run.bytes)
                );
                let left_bytes = run.bytes.saturating_sub(run.done_bytes);
                let right = run.rate.filter(|r| *r > 0.0).map(|rate| {
                    let secs = (left_bytes as f64 / rate) as u64;
                    Line::from(t!("dev.card.sync_left", t = span(secs.max(60))).to_string())
                });
                rows.push(Row::Value { label: None, left: Line::from(count.to_string()), right });
                if let Some(rate) = run.rate.filter(|r| *r > 0.0) {
                    let saved = run.saved.map_or_else(|| "?".to_string(), |at| ago(at, now()));
                    let words = t!("dev.card.sync_rate", rate = format!("{:.1}", rate / 1e6), when = saved);
                    rows.extend(dim_rows(None, &words, w));
                }
                if run.phase == Phase::Index {
                    rows.extend(value_rows(None, &scan(None, &t!("dev.card.sync_index")), w));
                }
                if run.phase == Phase::Waves {
                    rows.extend(value_rows(None, &waves_scan(run, &server), w));
                }
            }
        }
        if let Some(now_path) = &run.now {
            let label = Some(t!("dev.card.label_now").to_string());
            rows.extend(value_rows(label, &[(now_path.clone(), Style::default())], w));
        }
        return Some(rows);
    }
    let ended = card.ended.as_ref()?;
    let bar = |at: usize, of: usize| -> Vec<Row> {
        let permille = if of > 0 { (at * 1000 / of) as u32 } else { 0 };
        vec![Row::Bar { label: label.clone(), permille, color: th().dim }]
    };
    let count = |at: usize, of: usize| -> String {
        t!("dev.card.sync_of", i = fmt_count(at as u64), n = fmt_count(of as u64)).to_string()
    };
    let rows = match ended {
        Ended::Paused { pause, at, of, .. } => {
            let mut rows = bar(*at, *of);
            rows.extend(value_rows(None, &[(count(*at, *of), Style::default())], w));
            let (why, more) = match pause {
                Pause::Budget { until, peer, days, mb } => {
                    let peer = peer.parse().map(|id| peer_name(card, id)).unwrap_or_else(|_| peer.clone());
                    let mb = fmt_count(*mb);
                    let why = t!("dev.card.paused_budget", peer = peer, mb = mb);
                    let left = span(until.saturating_sub(now()));
                    let more = t!("dev.card.paused_budget_2", t = left, days = days);
                    (why.to_string(), Some(more.to_string()))
                }
                Pause::Server { until, tries } => {
                    let left = span(until.saturating_sub(now()));
                    (t!("dev.card.paused_server", server = server, t = left, n = tries).to_string(), None)
                }
                Pause::SignIn => (
                    t!("dev.card.paused_signin", server = server).to_string(),
                    Some(t!("dev.card.paused_signin_2").to_string()),
                ),
            };
            let pieces = [
                (format!("{} ", g.warn), gold),
                (t!("dev.card.paused").to_string(), gold.add_modifier(Modifier::BOLD)),
                (format!(" · {why}"), gold),
            ];
            rows.extend(value_rows(None, &pieces, w));
            if let Some(more) = more {
                rows.extend(dim_rows(None, &more, w));
            }
            rows
        }
        Ended::Full { at, of } => {
            let mut rows = bar(*at, *of);
            let words = t!("dev.card.err_full_2", i = fmt_count(*at as u64), n = fmt_count(*of as u64));
            rows.extend(problem_lines(g.no, &t!("dev.card.err_full"), &words, w));
            rows.extend(dim_rows(None, &t!("dev.card.err_full_why"), w));
            rows
        }
        Ended::Locked { at, of } => {
            let mut rows = bar(*at, *of);
            rows.extend(problem_lines(g.no, &t!("dev.card.err_locked"), &t!("dev.card.err_locked_why"), w));
            rows.extend(dim_rows(None, &t!("dev.card.err_locked_2"), w));
            rows
        }
        Ended::Out { at, of } => {
            let words = t!("dev.card.err_out_2", i = fmt_count(*at as u64), n = fmt_count(*of as u64));
            let pieces = [
                (format!("{} ", g.no), gold),
                (t!("dev.card.err_out").to_string(), gold.add_modifier(Modifier::BOLD)),
                (format!(" · {words}"), gold),
            ];
            let mut rows = value_rows(label, &pieces, w);
            let other = page.cards().into_iter().find(|c| c.drive() == card.drive() && c.key != card.key);
            if let Some(other) = other {
                let words = t!("dev.card.err_other_card", drive = card.drive(), other = card_name(&other));
                rows.extend(dim_rows(None, &words, w));
            }
            return Some(rows);
        }
        Ended::OtherCard => {
            let words = t!("dev.card.err_other_card", drive = card.drive(), other = "?");
            problem(&t!("dev.card.label_syncing"), g.no, &words, &[], w)
        }
        Ended::Failed(why) => problem(&t!("dev.card.label_syncing"), g.no, why, &[], w),
        Ended::Stopped { .. } | Ended::Done { .. } | Ended::Fit(_) | Ended::Refused { .. } => return None,
    };
    Some(rows)
}

/// A gold line in the value column: the glyph, the words bold, the rest
/// after ` · `.
fn problem_lines(glyph: &str, words: &str, rest: &str, w: u16) -> Vec<Row> {
    let gold = Style::default().fg(th().gold);
    let pieces = [
        (format!("{glyph} "), gold),
        (words.to_string(), gold.add_modifier(Modifier::BOLD)),
        (format!(" · {rest}"), gold),
    ];
    value_rows(None, &pieces, w)
}

/// Every row of `card`'s view, top to bottom (clause 44).
pub(in crate::device::page) fn card_layout(page: &Page, card: &CardView, content_w: u16) -> Vec<Row> {
    let w = content_w.saturating_sub(LABEL_W);
    let g = glyphs();
    let mut rows = vec![Row::Blank];
    let name = card_name(card);
    let right = Line::from(Span::styled(head_right(card), dim()));
    rows.push(Row::Wide { left: Line::from(Span::styled(name, bold())), right: Some(right) });
    rows.push(Row::Blank);
    let holds = t!("dev.card.label_holds").to_string();
    let mut links: Vec<(String, Act)> = Vec::new();
    let details = format!("{} {}", if page.details { g.open } else { g.forward }, t!("dev.details"));
    links.push((details, Act::Details));
    let locked = page.lock.is_some();
    match &card.read {
        Read::Reading => rows.extend(value_rows(Some(holds), &scan(None, &t!("dev.card.reading")), w)),
        Read::Failed(why) => rows.extend(problem(&holds, g.no, why, &[], w)),
        Read::Refused => {
            let refusals = &card.found.refusals;
            if let Some(Refusal::Shared(others)) = refusals.iter().find(|r| matches!(r, Refusal::Shared(_))) {
                let fs = others.join(", ");
                let lines = [t!("dev.card.shared", fs = fs).to_string(), t!("dev.card.shared_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.shared_title"), &lines, w));
            } else if let Some(r) = refusals.iter().find(|r| matches!(r, Refusal::Filesystem(_) | Refusal::Table(_))) {
                let (what, why) = match r {
                    Refusal::Table(_) => (t!("dev.card_gpt"), t!("dev.card_mbr_only")),
                    Refusal::Filesystem(Fs::ExFat) => (t!("dev.card_exfat"), t!("dev.card_fat32_only")),
                    Refusal::Filesystem(Fs::Ntfs) => (t!("dev.card_ntfs"), t!("dev.card_fat32_only")),
                    _ => (t!("dev.card.not_fat_card", fs = card.found.volume.fs.name()), t!("dev.card_fat32_only")),
                };
                let route = match page.boards.iter().find(|b| b.ours()) {
                    Some(board) => t!("dev.card.route_board", port = short(board.port())),
                    None => t!("dev.card.route_none"),
                };
                let label = t!("dev.card.label_format").to_string();
                let pieces = [
                    (format!("{} ", g.warn), Style::default().fg(th().gold)),
                    (what.to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                    (format!(" · {why}"), Style::default().fg(th().gold)),
                ];
                rows.extend(value_rows(Some(label), &pieces, w));
                rows.extend(dim_rows(None, &route, w));
            } else if refusals.iter().any(|r| matches!(r, Refusal::ReadOnly)) {
                let lines = [t!("dev.card.err_locked_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.locked"), &lines, w));
                rows.extend(space_rows(card, false));
            } else if refusals.iter().any(|r| matches!(r, Refusal::NotUtf8)) {
                let lines = [t!("dev.card.not_utf8_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.not_utf8"), &lines, w));
            }
            links.push((t!("dev.card.link_hide").to_string(), Act::CardHide));
        }
        Read::Read(facts) => {
            let set_up = facts.set_up();
            if !facts.newer.is_empty() {
                let lines = [t!("dev.card.newer_2").to_string()];
                let words = match facts.newer_major {
                    Some(major) => t!("dev.card.newer", major = major),
                    None => t!("dev.card.newer_any"),
                };
                rows.extend(problem(&holds, g.unknown, &words, &lines, w));
                rows.extend(space_rows(card, false));
            } else if facts.check || card.gate.as_ref().is_some_and(|g| g.contains(&Asked::DiskCheck)) {
                let label = t!("dev.card.label_check").to_string();
                let lines = [
                    t!("dev.card.check_2").to_string(),
                    t!("dev.card.check_win", drive = card.drive()).to_string(),
                    t!("dev.card.check_mac").to_string(),
                ];
                rows.extend(problem(&label, g.no, &t!("dev.card.check"), &lines, w));
            } else if facts.copied {
                let lines = [t!("dev.card.copied_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.copied"), &lines, w));
                rows.extend(space_rows(card, true));
            } else if !set_up {
                if facts.music.unlisted > 0 {
                    let n = fmt_count(u64::from(facts.music.unlisted));
                    rows.extend(problem(&holds, g.warn, &t!("dev.card.foreign", n = n), &[], w));
                } else {
                    let lines = [t!("dev.card.new_body_2").to_string()];
                    rows.extend(problem(&holds, g.warn, &t!("dev.card.new_body"), &lines, w));
                }
                rows.extend(space_rows(card, true));
                rows.extend(last_rows(card, w));
            } else {
                let libs = facts.libraries().join(", ");
                let server = server_name(card);
                let holds_words = if libs.is_empty() {
                    t!("dev.card.holds_none").to_string()
                } else {
                    t!("dev.card.holds_from", libs = libs, server = server).to_string()
                };
                rows.extend(value_rows(Some(holds.clone()), &[(holds_words, Style::default())], w));
                if let Some((i, n)) = facts.set {
                    let words = t!("dev.card.set_of", i = i, n = n);
                    let label = Some(t!("dev.card.label_set").to_string());
                    rows.extend(value_rows(label, &[(words.to_string(), Style::default())], w));
                }
                let label = t!("dev.card.label_server").to_string();
                match &card.server {
                    Server::Unknown(nick) => {
                        let nick = if nick.is_empty() { "?".to_string() } else { nick.clone() };
                        let lines = [t!("dev.card.unknown_server_2").to_string()];
                        rows.extend(problem(&label, g.warn, &t!("dev.card.unknown_server", server = nick), &lines, w));
                    }
                    Server::New(None) => {
                        let lines = [t!("dev.card.no_server_2").to_string()];
                        rows.extend(problem(&label, g.warn, &t!("dev.card.no_server"), &lines, w));
                    }
                    _ if matches!(card.next, Next::SignIn { .. }) || matches!(card.paused(), Some(Pause::SignIn)) => {
                        rows.extend(problem(&label, g.warn, &t!("dev.card.signin", server = server), &[], w));
                    }
                    _ => {}
                }
                match run_rows(page, card, w) {
                    Some(run) => rows.extend(run),
                    None => {
                        rows.extend(space_rows(card, true));
                        rows.extend(last_rows(card, w));
                        // With no server to ask, nothing to say of the next
                        // sync or what it converts (card 03).
                        if card.server.link().is_some() {
                            rows.extend(next_rows(page, card, w));
                            rows.extend(converts_rows(card, w));
                        }
                    }
                }
                let syncing = card.syncing();
                if syncing {
                    let run = card.run.as_ref().expect("a run");
                    if run.stopping {
                        // The stop is under way: no link, the words say so.
                        // The run fetches one file at a time: that one
                        // finishes, then the save, with no count to give.
                        links.push((t!("dev.card.stopping").to_string(), Act::Details));
                    } else if matches!(run.phase, Phase::Listing | Phase::Planning) {
                        links.push((t!("dev.card.link_stop_plain").to_string(), Act::CardStop));
                    } else if run.phase != Phase::Verifying {
                        links.push((t!("dev.card.link_stop").to_string(), Act::CardStop));
                    }
                } else if card.run.is_some() {
                    links.push((t!("dev.card.link_dont_wait").to_string(), Act::CardDontWait));
                } else if card.out {
                    links.push((t!("dev.card.link_give_up").to_string(), Act::CardGiveUp));
                } else if let Some(pause) = card.paused() {
                    if matches!(pause, Pause::Server { .. }) {
                        links.push((t!("dev.card.link_try_now").to_string(), Act::CardTryNow));
                    }
                    links.push((t!("dev.card.link_stop_now").to_string(), Act::CardStopPause));
                } else {
                    if matches!(card.next, Next::Unreached { .. }) {
                        links.push((t!("dev.card.link_try_now").to_string(), Act::CardTryNow));
                    }
                    if matches!(card.server, Server::Unknown(_)) {
                        let others = page.servers();
                        if let Some(other) = others.first() {
                            let words = t!("dev.card.link_other_server", server = other.name.clone());
                            links.push((words.to_string(), Act::CardOtherServer));
                        }
                    } else {
                        links.push((t!("dev.card.link_settings").to_string(), Act::CardSettings));
                    }
                    if plan_of(card).is_some_and(|p| p.plays > 0) {
                        links.push((t!("dev.card.link_plays").to_string(), Act::CardPlays));
                    }
                }
            }
        }
    }
    // Eject beside the links, but where it is the primary.
    let primary = primary(card);
    let accepted = card.found.accepted();
    let still = card.run.is_none() && !card.out && card.paused().is_none();
    if accepted && primary != Some(CardPrimary::Eject) && still {
        links.push((t!("dev.card.link_eject").to_string(), Act::CardEject));
    }
    rows.push(Row::Blank);
    // Under its gate or its sheet the card keeps its rows, as the design
    // draws it, and its primary keeps its place undrawn: a modal shorter
    // than the card leaves no half a button showing.
    let covered = page.card.gate.is_some() || page.card.sheet.is_some();
    let primary = primary.map(|p| {
        let word = p.word();
        match page.lock.as_ref() {
            Some(_) if !locked => (word, false, None),
            Some(_) => {
                let port = page.writing_port().map(short).unwrap_or_default();
                (word, false, Some(t!("dev.fw_one_at_a_time", port = port).to_string()))
            }
            None => (format!("{word} {}", g.forward), true, None),
        }
    });
    rows.push(Row::Actions { links: give_way(links, primary.as_ref(), content_w), primary, covered });
    if page.details {
        rows.extend(details_rows(card, w));
    }
    rows.push(Row::Blank);
    rows
}

/// The rows in `inside` rows at most (clause 4): short of rows the Card
/// view gives way its dim explanation lines first, the last first, then the
/// Converts row; never the head, Holds, Space or the bottom row.
pub(super) fn give_way_rows(rows: &mut Vec<Row>, inside: u16) {
    let height = |rows: &[Row]| rows.iter().map(Row::height).sum::<u16>();
    let dim_line = |row: &Row| match row {
        Row::Value { label: None, left, right: None } => {
            !left.spans.is_empty() && left.spans.iter().all(|s| s.style == dim())
        }
        _ => false,
    };
    let converts = t!("dev.card.label_converts").to_string();
    while height(rows) > inside {
        if let Some(at) = rows.iter().rposition(dim_line) {
            rows.remove(at);
            continue;
        }
        let at = rows.iter().position(|r| matches!(r, Row::Value { label: Some(l), .. } if *l == converts));
        let Some(at) = at else { break };
        rows.remove(at);
        while matches!(rows.get(at), Some(Row::Value { label: None, right: None, .. })) {
            rows.remove(at);
        }
    }
}

/// The bottom row's links, short of room before the primary: Send the
/// plays gives way first, then the links from the end (clause 44).
fn give_way(
    mut links: Vec<(String, Act)>,
    primary: Option<&(String, bool, Option<String>)>,
    w: u16,
) -> Vec<(String, Act)> {
    let room = usize::from(w).saturating_sub(primary.map_or(0, |(label, ..)| usize::from(kit::tall_width(label)) + 2));
    let width = |links: &[(String, Act)]| -> usize {
        links.iter().map(|(t, _)| kit::width(t)).sum::<usize>() + 3 * links.len().saturating_sub(1)
    };
    if width(&links) > room
        && let Some(at) = links.iter().position(|(_, a)| *a == Act::CardPlays)
    {
        links.remove(at);
    }
    links
}

/// Details (clause 44): the facts, a line each in the value column's `w`
/// cells, then the transfer's log.
fn details_rows(card: &CardView, w: u16) -> Vec<Row> {
    let mut rows = vec![Row::Rule];
    let fact = |label: String, value: String| Row::Fact { label, value, style: Style::default() };
    rows.push(fact(t!("dev.card.fact_drive").to_string(), drive_fact(card, w)));
    if let Some(facts) = card.facts() {
        if let Some((id, generation, _)) = facts.root {
            let version = facts.producer.rsplit(' ').next().unwrap_or_default().to_string();
            let words = t!("dev.card.fact_card_v", id = format!("{id:016x}"), rev = generation, version = version);
            rows.push(fact(t!("dev.label_card_fact").to_string(), words.to_string()));
        }
        let mut server = server_name(card);
        if card.server.link().is_some() {
            server.push_str(&format!(" · {}", t!("dev.card.fact_signed_in")));
        }
        rows.push(fact(t!("dev.card.label_server").to_string(), server));
        if let Some(plan) = plan_of(card).filter(|p| !p.removals.is_empty()) {
            let more = plan.removals.len() - 1;
            let words = if more == 0 { plan.removals[0].clone() } else { removes_fact(&plan.removals[0], more, w) };
            rows.push(fact(t!("dev.card.fact_removes").to_string(), words));
        }
        if let Some(tally) = plan_of(card).and_then(|p| p.tally.as_ref()) {
            // A card for someone else sends none (clause 61): said in place
            // of the counts, which would all be none.
            let gift = facts.state.as_ref().is_some_and(|s| s.settings.for_someone_else);
            let words = if gift || tally.gift > 0 {
                t!("dev.card.plays_gift").to_string()
            } else {
                t!(
                    "dev.card.fact_plays_v",
                    n = fmt_count(tally.due as u64),
                    old = fmt_count(tally.clock as u64),
                    mine = fmt_count(tally.own as u64)
                )
                .to_string()
            };
            rows.push(fact(t!("dev.card.label_plays").to_string(), words));
        }
        let d = &facts.device;
        if d.present {
            let adj = if d.read_mpdj > 0 { d.read_mpdj.to_string() } else { "-".to_string() };
            let (version, codecs) = (d.firmware.clone(), d.codecs.clone());
            let words =
                t!("dev.card.fact_player_v", name = "Core2", version = version, codecs = codecs, adj = adj);
            rows.push(fact(t!("dev.card.row_player").to_string(), words.to_string()));
        }
    }
    if let Some(Ended::Done { skipped, .. }) = &card.ended {
        for (i, (path, why)) in skipped.iter().enumerate() {
            let label = if i == 0 { t!("dev.card.fact_skipped").to_string() } else { String::new() };
            rows.push(Row::Fact { label, value: format!("{path} · {why}"), style: Style::default() });
        }
    }
    rows.push(Row::Log);
    let here = card.run.is_none() && !card.out && card.found.accepted();
    if here && card.facts().is_some_and(|f| f.set_up()) {
        rows.push(Row::Blank);
        let text = t!("dev.card.link_sync_again").to_string();
        rows.push(Row::Link { wide: true, text, act: Act::CardSyncAgain, note: None });
    }
    rows
}

/// The Drive fact in `w` cells, in the locale's words a piece at a time (a
/// card may lack its size, its clusters or its serial). Short of cells (a
/// volume's long name, on Linux or macOS) its serial gives way first, then
/// its clusters, each whole, never cut (clause 44).
fn drive_fact(card: &CardView, w: u16) -> String {
    let v = &card.found.volume;
    let table = v.table.name().to_uppercase();
    let fs = t!("dev.card.head_on", fs = v.fs.name(), table = table).to_string();
    let mut drive = vec![card.drive(), fs];
    // The pieces that give way, the last first.
    let mut spare = Vec::new();
    if let Some(space) = v.space {
        drive.push(size_words(space.size));
        if space.cluster > 0 {
            spare.push(t!("dev.card.fact_drive_kb", kb = space.cluster / 1024).to_string());
        }
    }
    if let Some(serial) = &v.serial {
        spare.push(serial.to_string());
    }
    loop {
        let words = drive.iter().chain(&spare).map(String::as_str).collect::<Vec<_>>().join(" · ");
        if kit::width(&words) <= usize::from(w) || spare.pop().is_none() {
            return words;
        }
    }
}

/// The Removes fact in `w` cells: the first path and how many more. Short
/// of cells the path gives way, cut with an ellipsis, never the count
/// (clause 44).
fn removes_fact(path: &str, more: usize, w: u16) -> String {
    let n = fmt_count(more as u64);
    let words = |path: &str| t!("dev.card.fact_removes_v", path = path, n = n.as_str()).to_string();
    let whole = words(path);
    let over = kit::width(&whole).saturating_sub(usize::from(w));
    if over == 0 {
        return whole;
    }
    words(&fit(path, kit::width(path).saturating_sub(over)))
}

// ── The board's card line (clause 20a) ──────────────────────────────────────

/// The line under a board's SD card row naming its card, from `cid=`.
pub(super) fn board_lines(page: &Page, board: &Board) -> Vec<String> {
    use crate::device::board::CardRoot;
    let Some(status) = board.status() else { return Vec::new() };
    match status.cid {
        CardRoot::Unknown => Vec::new(),
        CardRoot::None => match status.card {
            CardKind::Fat32 | CardKind::Fat16 => vec![t!("dev.card.board_unfilled").to_string()],
            _ => Vec::new(),
        },
        CardRoot::Loaded(r) => {
            let known = page.card.known.as_ref().and_then(|k| k.cards.get(&r.card_id));
            match known {
                Some(seen) if seen.synced > 0 => {
                    let server = if seen.server.is_empty() { "?".to_string() } else { seen.server.clone() };
                    let label = if seen.label.is_empty() { "?".to_string() } else { seen.label.clone() };
                    let when = ago(seen.synced, now());
                    let is = t!("dev.card.board_is", card = label, when = when, server = server);
                    let mut lines = vec![is.to_string()];
                    if r.generation < seen.generation {
                        lines.push(t!("dev.card.board_behind").to_string());
                    }
                    lines
                }
                _ => vec![t!("dev.card.board_elsewhere").to_string()],
            }
        }
    }
}

// ── The gates (clause 58) ───────────────────────────────────────────────────

/// The gate's words as [`gate_body`] wraps them, a line each: the tests'
/// way to read a gate whole.
#[cfg(test)]
pub(in crate::device::page) fn gate_words(gate: &CardGate, width: usize) -> Vec<String> {
    let words = |line: GLine| match line {
        GLine::Text(text) | GLine::Box(text) => text.to_string(),
        GLine::Blank => String::new(),
    };
    gate_body(gate, width).into_iter().map(words).collect()
}

/// A gate line: text, a blank that gives way, or the box.
enum GLine {
    Text(Line<'static>),
    Blank,
    Box(Line<'static>),
}

/// The gate's words, wrapped in `width` cells.
fn gate_body(gate: &CardGate, width: usize) -> Vec<GLine> {
    let gold = Style::default().fg(th().gold);
    let green = Style::default().fg(th().ok);
    let plain = Style::default();
    let mut lines = Vec::new();
    let text = |lines: &mut Vec<GLine>, words: String, style: Style| {
        for line in wrap_spans(&[(words, style)], width) {
            lines.push(GLine::Text(line));
        }
    };
    let title = |words: String| (words, gold.add_modifier(Modifier::BOLD));
    match &gate.kind {
        GateKind::Removals { card, server, n, first } => {
            let words = t!("dev.card.gate_remove_title", n = fmt_count(*n as u64), card = card.clone());
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_remove_why", server = server.clone()).to_string(), plain);
            for path in first {
                lines.push(GLine::Text(Line::from(fit(&format!("  {path}"), width))));
            }
            if *n > first.len() {
                let more = t!("dev.card.gate_remove_more", n = fmt_count((n - first.len()) as u64));
                lines.push(GLine::Text(Line::from(Span::styled(format!("  {more}"), dim()))));
            }
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_remove_stays", server = server.clone()).to_string(), green);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_remove_never").to_string(), plain);
        }
        GateKind::Half { card, server, n, total, libs, ticked } => {
            let (n, total) = (fmt_count(*n as u64), fmt_count(*total as u64));
            let words = t!("dev.card.gate_half_title", n = n, card = card.clone(), total = total);
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_half_why", server = server.clone()).to_string(), plain);
            if !libs.is_empty() {
                lines.push(GLine::Blank);
                for (lib, gone, of) in libs {
                    let (n, total) = (fmt_count(*gone as u64), fmt_count(*of as u64));
                    let words = t!("dev.card.gate_half_lib", n = n, total = total);
                    lines.push(GLine::Text(Line::from(fit(&format!("  {lib}  {words}"), width))));
                }
            }
            lines.push(GLine::Blank);
            let mark = if *ticked { glyphs().checked } else { "[ ]" };
            let words = t!("dev.card.gate_half_box", server = server.clone());
            lines.push(GLine::Box(Line::from(format!("{mark} {words}"))));
            text(&mut lines, t!("dev.card.gate_half_stays").to_string(), plain);
        }
        GateKind::Adopt { drive, server, n, all } => {
            let words = t!("dev.card.gate_adopt_title", n = fmt_count(*n as u64), server = server.clone());
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            let mb = (*n as u64 * 8 * 1024).div_ceil(1_000_000);
            let (every, some, size) = (fmt_count(*all as u64), fmt_count(*n as u64), fmt_count(mb));
            let words = t!(
                "dev.card.gate_adopt_body",
                drive = drive.clone(),
                all = every,
                n = some,
                server = server.clone(),
                mb = size
            );
            text(&mut lines, words.to_string(), plain);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_adopt_then").to_string(), plain);
            if all > n {
                text(&mut lines, t!("dev.card.gate_adopt_rest", n = fmt_count((all - n) as u64)).to_string(), green);
            }
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_adopt_nothing").to_string(), plain);
        }
        GateKind::Relay { server, bytes } => {
            let (t, s) = title(t!("dev.card.gate_relay_title", gb = gb(*bytes)).to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            // About 1.5 MB/s over the relay, 20 MB/s on the same network.
            let (slow, fast) = (span(bytes / 1_500_000), span(bytes / 20_000_000));
            let words = t!("dev.card.gate_relay_body", server = server.clone(), t = slow, t2 = fast);
            text(&mut lines, words.to_string(), plain);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_relay_ok").to_string(), green);
        }
        GateKind::Proxy { peer, server, .. } => {
            let words = t!("dev.card.gate_proxy_title", peer = peer.clone(), server = server.clone());
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            let words = t!("dev.card.gate_proxy_body", peer = peer.clone(), server = server.clone());
            text(&mut lines, words.to_string(), plain);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_proxy_once").to_string(), plain);
        }
        GateKind::Server { card, old, new, rows, plays, .. } => {
            let (t, s) = title(t!("dev.card.gate_server_title", new = new.clone()).to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            let words = t!("dev.card.gate_server_body", card = card.clone(), old = old.clone(), new = new.clone());
            text(&mut lines, words.to_string(), plain);
            let drop = t!("dev.card.gate_server_drop", n = fmt_count(*rows as u64), old = old.clone());
            lines.push(GLine::Text(Line::from(fit(&format!("  {drop}"), width))));
            if *plays > 0 {
                lines.push(GLine::Blank);
                let words = t!("dev.card.gate_server_plays", n = fmt_count(*plays as u64), old = old.clone());
                text(&mut lines, words.to_string(), plain);
            }
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_server_old", old = old.clone()).to_string(), plain);
        }
    }
    lines.push(GLine::Blank);
    lines
}

/// The gate's buttons: the keep, named for what stays, then the go; the
/// stronger gate's go only once its box is ticked.
pub(in crate::device::page) fn gate_buttons_of(gate: &CardGate) -> (String, Option<String>) {
    let back = glyphs().back;
    let (keep, go) = match &gate.kind {
        GateKind::Removals { n, .. } => {
            let go = t!("dev.card.gate_remove_do", n = fmt_count(*n as u64)).to_string();
            (t!("dev.card.gate_not_now").to_string(), Some(go))
        }
        GateKind::Half { n, ticked, .. } => (
            t!("dev.card.gate_not_now").to_string(),
            ticked.then(|| t!("dev.card.gate_remove_do", n = fmt_count(*n as u64)).to_string()),
        ),
        GateKind::Adopt { n, .. } => (
            t!("dev.card.gate_adopt_keep").to_string(),
            Some(t!("dev.card.gate_adopt_do", n = fmt_count(*n as u64)).to_string()),
        ),
        GateKind::Relay { .. } => (
            t!("dev.card.gate_not_now").to_string(),
            Some(t!("dev.card.gate_relay_do").to_string()),
        ),
        GateKind::Proxy { peer, server, .. } => (
            t!("dev.card.gate_proxy_keep", peer = peer.clone()).to_string(),
            Some(t!("dev.card.gate_proxy_do", server = server.clone()).to_string()),
        ),
        GateKind::Server { old, new, .. } => (
            t!("dev.card.gate_server_keep_btn", old = old.clone()).to_string(),
            Some(t!("dev.card.gate_server_do", new = new.clone()).to_string()),
        ),
    };
    (format!("{back} {keep}"), go)
}

/// The kit's gold modal over the card (clause 58's grammar: clause 23's).
pub(super) fn draw_card_gate(frame: &mut Frame, page: &mut Page, area: Rect) {
    let Some(gate) = page.card.gate.clone() else { return };
    let width = GATE_W.min(area.width.saturating_sub(4));
    if width < 12 || area.height < 5 {
        return;
    }
    let text_w = usize::from(width.saturating_sub(4));
    let mut lines = gate_body(&gate, text_w);
    let most = usize::from(area.height.saturating_sub(2)).saturating_sub(3);
    while lines.len() > most {
        let Some(at) = lines.iter().rposition(|l| matches!(l, GLine::Blank)) else { break };
        lines.remove(at);
    }
    let height = (lines.len() + 3) as u16;
    let inner = kit::modal_frame_on(frame, &mut page.ui, area, width, height, th().gold);
    if inner.height == 0 {
        return;
    }
    let body = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(1),
    };
    for (i, line) in lines.iter().enumerate() {
        let y = body.y + i as u16;
        if y >= body.bottom() {
            break;
        }
        let at = Rect { y, height: 1, ..body };
        match line {
            GLine::Blank => {}
            GLine::Text(line) => frame.render_widget(Paragraph::new(line.clone()), at),
            GLine::Box(line) => {
                let rect = Rect { width: (line.width() as u16).min(at.width), ..at };
                let style = if page.ui.hovers(rect) { Style::default().fg(th().bright) } else { Style::default() };
                frame.render_widget(Paragraph::new(line.clone().style(style)), rect);
                page.ui.click(rect, Act::CardBox);
            }
        }
    }
    let y = inner.bottom().saturating_sub(1);
    let (keep, go) = gate_buttons_of(&gate);
    let keep_w = kit::width(&keep) as u16 + 4;
    let go_w = go.as_ref().map_or(0, |g| kit::width(g) as u16 + 4 + 2);
    let keep_x = inner.right().saturating_sub(go_w + keep_w).max(inner.x);
    let at = Rect { x: keep_x, y, width: inner.right() - keep_x, height: 1 };
    let keep_rect = kit::button(frame, &mut page.ui, at, &keep, true, Act::CardKeep);
    if let Some(go) = go {
        let go_x = keep_rect.right() + 2;
        if go_x < inner.right() {
            let at = Rect { x: go_x, y, width: inner.right() - go_x, height: 1 };
            kit::button(frame, &mut page.ui, at, &go, false, Act::CardGo);
        }
    }
}

// ── The card's Settings sheet (clauses 48–50) ───────────────────────────────

/// The fit line's words (clause 49) from a plan's fit and the card's size.
fn fit_line(fit: &Fit, size: u64) -> (Vec<(String, Style)>, Option<String>) {
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let used = size.saturating_sub(fit.room).saturating_add(fit.needed);
    if fit.over_cap() {
        let words = t!("dev.card.fit_cap", n = fmt_count(fit.tracks as u64), max = fmt_count(TRACK_CAP as u64));
        return (vec![(format!("{} ", g.no), gold), (words.to_string(), gold)], None);
    }
    if !fit.fits() {
        let head = t!("dev.card.fit_no", gb = gb(fit.short()));
        let over = t!("dev.card.fit_over", used = gb(used), size = gb(size));
        return (vec![(format!("{} ", g.no), gold), (format!("{head} · {over}"), gold)], None);
    }
    let spare = fit.room.saturating_sub(fit.needed);
    let after = t!("dev.card.fit_after", used = gb(used), size = gb(size), spare = gb(spare));
    if size > 0 && spare * 20 < size {
        let pieces = vec![(format!("{} ", g.warn), gold), (format!("{} · {after}", t!("dev.card.fit_tight")), gold)];
        return (pieces, Some(t!("dev.card.fit_tight_2").to_string()));
    }
    let ok = Style::default().fg(th().ok);
    (vec![(format!("{} ", g.ok), ok), (format!("{} · {after}", t!("dev.card.fit_ok")), Style::default())], None)
}

/// The help line's words, about the focused row (clause 48).
fn sheet_help(sheet: &CardSheet, server: &str) -> String {
    row_help(&sheet.row(), &sheet.offered, server)
}

/// The help about `row`, its library's size from what the server `offered`.
pub(in crate::device::page) fn row_help(
    row: &SRow,
    offered: &[(String, Option<(usize, u64)>)],
    server: &str,
) -> String {
    match row {
        SRow::Name => t!("dev.card.help_name").to_string(),
        SRow::Removals => t!("dev.card.help_removals", server = server).to_string(),
        SRow::Into => t!("dev.card.help_into", server = server).to_string(),
        SRow::Player => t!("dev.card.help_player").to_string(),
        SRow::Library(lib) => {
            let size = offered.iter().find(|(l, _)| l == lib).and_then(|(_, s)| *s);
            match size {
                Some((n, _)) => {
                    t!("dev.card.help_lib_short", lib = lib, server = server, n = fmt_count(n as u64)).to_string()
                }
                None => t!("dev.card.help_lib_unknown", lib = lib, server = server).to_string(),
            }
        }
        SRow::Budget => t!("dev.card.help_budget").to_string(),
        SRow::Convert | SRow::Covers | SRow::AutoDj | SRow::For => t!("dev.card.help_writing").to_string(),
        _ => t!("dev.card.help_from").to_string(),
    }
}

/// The help in two lines of `tw` cells, an ellipsis past them (clause 48).
pub(in crate::device::page) fn help_lines(text: &str, tw: usize) -> Vec<Line<'static>> {
    let help = wrap_spans(&[(text.to_string(), dim())], tw);
    let more = help.len() > 2;
    let cut = |line: Line<'static>| Line::from(Span::styled(fit(&format!("{line} …"), tw), dim()));
    help.into_iter().take(2).enumerate().map(|(i, line)| if more && i == 1 { cut(line) } else { line }).collect()
}

/// The sheet's width in an area `area_w` cells wide (the kit's modal: 74,
/// the area's less four), and its text's inside its border and padding.
pub(in crate::device::page) fn sheet_widths(area_w: u16) -> (u16, u16) {
    let width = GATE_W.min(area_w.saturating_sub(4));
    (width, width.saturating_sub(4))
}

/// The sheet's rows' height, its tallest state: its top is held there.
const CARD_SHEET_MAX: u16 = 22;

/// A sheet row's label in its 14 cells, padded by display width: a label in
/// Japanese or Chinese takes two cells a character, and padding by
/// characters left its row's value out of line with the others.
pub(in crate::device::page) fn sheet_label(words: &str) -> String {
    let label = fit(words, 13);
    let pad = 14usize.saturating_sub(kit::width(&label));
    format!("{label}{}", " ".repeat(pad))
}

/// The sheet over the card (clause 48): the kit's neutral modal, two tabs,
/// the fit line under both, the help, the buttons.
pub(super) fn draw_card_sheet(frame: &mut Frame, page: &mut Page, area: Rect) {
    let Some(sheet) = page.card.sheet.clone() else { return };
    let Some(card) = page.card_of(&sheet.key) else { return };
    let (width, text_w) = sheet_widths(area.width);
    if width < 24 || area.height < 8 {
        return;
    }
    let servers = page.servers();
    let server = sheet
        .want
        .server
        .as_ref()
        .and_then(|id| servers.iter().find(|s| &s.id == id))
        .map(|s| s.name.clone())
        .unwrap_or_else(|| server_name(&card));
    let tw = usize::from(text_w);
    let g = glyphs();
    // The lines: (text pieces, its row's act, whether it is the keyboard's).
    enum L {
        Title,
        Blank,
        Tabs,
        Row(usize, Line<'static>),
        Plain(Line<'static>),
        Rule,
        Help(Line<'static>),
        Buttons,
    }
    let mut lines: Vec<L> = vec![L::Title, L::Blank];
    if !sheet.setup {
        lines.push(L::Tabs);
        lines.push(L::Blank);
    }
    let focus = sheet.focus.min(sheet.rows().len() - 1);
    let label = |key: &str| sheet_label(&t!(key));
    let on = |b: bool| if b { g.radio_on } else { "( )" };
    let tick = |b: bool| if b { g.checked } else { "[ ]" };
    let mut peer_head: Option<u32> = None;
    for (i, row) in sheet.rows().iter().enumerate() {
        let lit = i == focus && sheet.field.is_none() && sheet.list.is_none();
        let style = if lit { accent().add_modifier(Modifier::BOLD) } else { Style::default() };
        let line: Line<'static> = match row {
            SRow::Name => {
                let value = if sheet.want.name.is_empty() { String::new() } else { sheet.want.name.clone() };
                Line::from(vec![Span::styled(label("dev.card.row_name"), dim()), Span::styled(value, style)])
            }
            SRow::From => {
                let picked = sheet.want.server.as_ref().and_then(|id| servers.iter().find(|s| &s.id == id));
                let name = picked.map_or_else(|| "—".to_string(), |s| s.name.clone());
                let value = Span::styled(format!("{name} {}", g.open), style);
                Line::from(vec![Span::styled(label("dev.card.grp_from"), dim()), value])
            }
            SRow::Library(lib) => {
                let ticked = sheet.libraries().contains(lib);
                let size = sheet.offered.iter().find(|(l, _)| l == lib).and_then(|(_, s)| *s);
                let left = format!("  {} {lib}", tick(ticked));
                let right = size
                    .map(|(n, b)| {
                        let tracks = t!("dev.card_tracks", n = fmt_count(n as u64));
                        format!("{} · {tracks}", size_words(b))
                    })
                    .unwrap_or_default();
                let pad = tw.saturating_sub(kit::width(&left) + kit::width(&right));
                Line::from(vec![Span::styled(left, style), Span::raw(" ".repeat(pad)), Span::styled(right, dim())])
            }
            SRow::PeerLibrary(id, lib) => {
                if peer_head != Some(*id) {
                    peer_head = Some(*id);
                    let peer = sheet.peers.iter().find(|(p, ..)| p == id);
                    let name = peer.map(|(_, n, _)| n.clone()).unwrap_or_default();
                    let mb = fmt_count(sheet.want.budget_mb.unwrap_or(crate::device::card::budget::DEFAULT_MB));
                    let head = t!("dev.card.grp_peer", peer = name, server = server.clone(), mb = mb).to_string();
                    lines.push(L::Plain(Line::from(Span::styled(fit(&head, tw), dim()))));
                }
                let ticked = sheet.want.peers.iter().any(|(p, libs)| p == id && libs.contains(lib));
                Line::from(Span::styled(format!("  {} {lib}", tick(ticked)), style))
            }
            SRow::Convert => {
                let needed = sheet.want.convert == Convert::Unplayable;
                let (one, all) = (t!("dev.card.opt_needed"), t!("dev.card.opt_all"));
                let words = format!("{} {one}   {} {all}", on(needed), on(!needed));
                Line::from(vec![Span::styled(label("dev.card.row_convert"), dim()), Span::styled(words, style)])
            }
            SRow::Into => {
                let words = format!("{} {}", codec_words(sheet.want.codec, sheet.want.bitrate), g.open);
                Line::from(vec![Span::styled(label("dev.card.row_into"), dim()), Span::styled(words, style)])
            }
            SRow::Removals => {
                let follow = sheet.want.removal == Removal::Mirror;
                let (mirror, never) = (t!("dev.card.opt_follow", server = server.clone()), t!("dev.card.opt_never"));
                let words = format!("{} {mirror}   {} {never}", on(follow), on(!follow));
                Line::from(vec![Span::styled(label("dev.card.row_removals"), dim()), Span::styled(words, style)])
            }
            SRow::Covers => {
                let words = format!("{} {}", tick(sheet.want.covers), t!("dev.card.opt_covers"));
                Line::from(vec![Span::styled(label("dev.card.row_covers"), dim()), Span::styled(words, style)])
            }
            SRow::AutoDj => {
                let words = format!("{} {}", tick(sheet.want.auto_dj), t!("dev.card.opt_index"));
                Line::from(vec![Span::styled(label("dev.card.row_autodj"), dim()), Span::styled(words, style)])
            }
            SRow::For => {
                let me = !sheet.want.for_someone_else;
                let words = format!("{} {}   {} {}", on(me), t!("dev.card.opt_me"), on(!me), t!("dev.card.opt_else"));
                Line::from(vec![Span::styled(label("dev.card.row_for"), dim()), Span::styled(words, style)])
            }
            SRow::Player => {
                let d = card.facts().map(|f| f.device.clone());
                let words = match d.filter(|d| d.present) {
                    Some(d) => t!("dev.card.player_txt", name = "M5Stack Core2", version = d.firmware).to_string(),
                    None => t!("dev.card.player_none").to_string(),
                };
                Line::from(vec![Span::styled(label("dev.card.row_player"), dim()), Span::raw(words)])
            }
            SRow::Budget => {
                let mb = sheet.want.budget_mb.unwrap_or(crate::device::card::budget::DEFAULT_MB);
                let filled = (mb / 300).min(10) as usize;
                let bar = format!("{}{}", g.filled.repeat(filled), g.empty.repeat(10 - filled));
                let words = format!("- {bar} +  {}", t!("dev.card.budget_mb", mb = fmt_count(mb)));
                Line::from(vec![Span::styled(label("dev.card.row_budget"), dim()), Span::styled(words, style)])
            }
            SRow::Buttons => continue,
        };
        lines.push(L::Row(i, line));
    }
    lines.push(L::Blank);
    // The fit line: the sheet's own answer, else the card's last look.
    let fit_next = card.fit.as_ref().filter(|(seq, _)| *seq == sheet.seq).map(|(_, n)| n.clone());
    let size = card.found.volume.space.map_or(0, |s| s.size);
    match fit_next.as_ref().or(Some(&card.next)) {
        Some(Next::Asking) => {
            let words = t!("dev.card.sync_planning").to_string();
            let spans: Vec<Span<'static>> = scan(None, &words).into_iter().map(|(t, s)| Span::styled(t, s)).collect();
            lines.push(L::Plain(Line::from(spans)));
        }
        Some(Next::Plan(plan)) => {
            if let Some(fit) = plan.fit {
                let (pieces, more) = fit_line(&fit, size);
                for line in wrap_spans(&pieces, tw) {
                    lines.push(L::Plain(line));
                }
                if let Some(more) = more {
                    lines.push(L::Plain(Line::from(Span::styled(format!("  {more}"), dim()))));
                }
            }
        }
        _ => {}
    }
    lines.push(L::Rule);
    lines.extend(help_lines(&sheet_help(&sheet, &server), tw).into_iter().map(L::Help));
    lines.push(L::Buttons);
    // The frame takes at most two rows short of the area (kit::modal_rect), and
    // its own two borders.
    let most = usize::from(area.height.saturating_sub(4));
    if lines.len() > most {
        lines.retain(|l| !matches!(l, L::Rule | L::Help(_)));
    }
    while lines.len() > most {
        let Some(at) = lines.iter().rposition(|l| matches!(l, L::Blank)) else { break };
        lines.remove(at);
    }
    if lines.len() > most {
        let buttons = lines.pop();
        lines.truncate(most.saturating_sub(1));
        lines.extend(buttons);
    }
    let height = (lines.len() + 2) as u16;
    let inner = kit::modal_frame_anchored_on(frame, &mut page.ui, area, width, height, CARD_SHEET_MAX, th().accent);
    if inner.height == 0 || inner.width < 4 {
        return;
    }
    let content = Rect { x: inner.x + 1, y: inner.y, width: inner.width - 2, height: inner.height };
    let mut list_at: Option<Rect> = None;
    for (i, line) in lines.iter().enumerate() {
        let y = content.y + i as u16;
        if y >= content.bottom() {
            break;
        }
        let row = Rect { y, height: 1, ..content };
        match line {
            L::Title => {
                let title = if sheet.setup {
                    t!("dev.card.setup_title", drive = card.drive()).to_string()
                } else {
                    t!("dev.card.set_title", card = card_name(&card)).to_string()
                };
                let at = Rect { width: row.width.saturating_sub(4), ..row };
                let title = Span::styled(fit(&title, usize::from(at.width)), accent().add_modifier(Modifier::BOLD));
                frame.render_widget(Paragraph::new(title), at);
                kit::modal_close(frame, &mut page.ui, inner, Act::CardSheetClose);
            }
            L::Blank => {}
            L::Tabs => {
                let mut x = row.x + 1;
                let tabs = [("dev.card.tab_music", SheetTab::Music), ("dev.card.tab_writing", SheetTab::Writing)];
                for (i, (key, tab)) in tabs.iter().enumerate() {
                    let words = format!(" {} ", t!(*key));
                    let w = kit::width(&words) as u16;
                    let rect = Rect { x, y, width: w.min(row.right().saturating_sub(x)), height: 1 };
                    let style = if sheet.tab == *tab {
                        Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
                    } else if page.ui.hovers(rect) {
                        hover()
                    } else {
                        dim()
                    };
                    frame.render_widget(Paragraph::new(Span::styled(words, style)), rect);
                    page.ui.click(rect, Act::CardSheetTab(i));
                    x += w + 2;
                }
                if let Some(space) = card.found.volume.space {
                    let right = format!("{} · {}", card.drive(), size_words(space.size));
                    let w = kit::width(&right) as u16;
                    if x + 2 + w <= row.right() {
                        let at = Rect { x: row.right() - w, y, width: w, height: 1 };
                        frame.render_widget(Paragraph::new(Span::styled(right, dim())), at);
                    }
                }
            }
            L::Row(i, line) => {
                if sheet.field.is_some() && sheet.rows().get(*i) == Some(&SRow::Name) {
                    let at = Rect { x: row.x + 14, width: row.width.saturating_sub(14), ..row };
                    frame.render_widget(Paragraph::new(Span::styled(label("dev.card.row_name"), dim())), row);
                    if let Some(field) = &sheet.field {
                        let (value, cursor) = (field.value(), field.cursor());
                        let shown = kit::field_display(&mut page.ui, at.x, at.y, value, cursor, at.width, None);
                        frame.render_widget(Paragraph::new(Span::styled(shown, accent())), at);
                    }
                } else {
                    let cut = Line::from(line.spans.clone());
                    frame.render_widget(Paragraph::new(cut), row);
                }
                page.ui.click(row, Act::CardSheetRow(*i));
                if sheet.list.is_some() && *i == focus {
                    list_at = Some(Rect { x: row.x + 14, y: row.y + 1, ..row });
                }
            }
            L::Plain(line) => frame.render_widget(Paragraph::new(line.clone()), row),
            L::Rule => {
                let rule = Span::styled(g.rule.repeat(usize::from(row.width)), dim());
                frame.render_widget(Paragraph::new(rule), row)
            }
            L::Help(line) => frame.render_widget(Paragraph::new(line.clone()), row),
            L::Buttons => {
                let (other, go) = if sheet.setup {
                    (t!("dev.card.btn_cancel").to_string(), t!("dev.card.btn_do_setup").to_string())
                } else {
                    (t!("dev.card.btn_defaults").to_string(), t!("dev.card.btn_apply").to_string())
                };
                let ow = kit::width(&other) as u16 + 4;
                let gw = kit::width(&go) as u16 + 4;
                let x = row.right().saturating_sub(ow + 2 + gw).max(row.x);
                let at = Rect { x, width: row.right() - x, ..row };
                let focused = sheet.row() == SRow::Buttons;
                let other_act = if sheet.setup { Act::CardSheetClose } else { Act::CardDefaults };
                let r = kit::button(frame, &mut page.ui, at, &other, focused && !sheet.on_apply, other_act);
                let gx = r.right() + 2;
                if gx < row.right() {
                    let at = Rect { x: gx, width: row.right() - gx, ..row };
                    kit::button(frame, &mut page.ui, at, &go, true, Act::CardApply);
                }
            }
        }
    }
    // The hanging list over the sheet's lower rows.
    if let (Some(cursor), Some(at)) = (sheet.list, list_at) {
        let entries = list_entries(&sheet, &servers);
        let w = entries.iter().map(|e| kit::width(e)).max().unwrap_or(4) as u16 + 6;
        let h = (entries.len() as u16 + 2).min(area.bottom().saturating_sub(at.y));
        if h > 2 {
            let width = w.min(area.right().saturating_sub(at.x));
            let rect = Rect { x: at.x.saturating_sub(1), y: at.y, width, height: h };
            kit::blank(frame, rect);
            let inner = kit::frame_at(frame, rect, th().accent);
            for (i, entry) in entries.iter().enumerate() {
                let y = inner.y + i as u16;
                if y >= inner.bottom() {
                    break;
                }
                let r = Rect { y, height: 1, ..inner };
                let lit = Style::default().fg(th().on_accent).bg(th().accent);
                let style = if i == cursor { lit } else { Style::default() };
                let words = fit(&format!(" {entry}"), usize::from(r.width));
                frame.render_widget(Paragraph::new(Span::styled(words, style)), r);
                page.ui.click(r, Act::CardSheetPick(i));
            }
        }
    }
}
