//! Add from YouTube's pure half: what a pasted link is and the one URL the
//! server gets for it, the tags a lookup reads into (the title cleaned of
//! upload noise, the artist split off it, every removal kept so it can be
//! put back), where the song lands (the YouTube layouts and their bracket
//! traps, on the layout engine the Discover picker already mirrors in
//! [`crate::api::types`]), the formats the menu offers, the file name the
//! server gives the song and whether the folder already has it, how a
//! failed lookup, folder-make, add or job reads, and why Add is off. No
//! I/O: the room, the window and the job tracker wire it to the server.
//!
//! A port of the webapp's `webapp/alpha/ytdl-core.js` (mStream #1123),
//! itself a port of the phone's `lib/util/ytdl_{link,guess,layout,formats,
//! errors}.dart` — behaviour, not syntax — plus what the webapp has no use
//! for, because its server is always its own release: the phone's
//! `ytdl_filename.dart` (yt-dlp's restricted file name, for a server whose
//! lookup names no `fileStem`) and the phone's older-server rules (the
//! live-stream hedge, the re-tag clash that blocks Add before 6.30, the
//! hedged job failure). The tests carry both records' fixtures; where the
//! terminal differs on purpose the comment says so.
//!
//! Text never leaves here as English: a [`Label`] is a locale key under
//! `gui.ytdl.` with its params; every key is in `KEYS`, the tests' list
//! of them. The surface renders it ([`Label::text`]).

mod failure;
mod filename;
mod formats;
mod guess;
/// The job tracker the room, the window and the list share
/// (`crate::ytdl::jobs::Tracker`): no I/O, driven from the GUI's frame.
pub(crate) mod jobs;
mod layout;
mod link;

pub(crate) use failure::*;
pub(crate) use filename::*;
pub(crate) use formats::*;
pub(crate) use guess::*;
pub(crate) use layout::*;
pub(crate) use link::*;

use rust_i18n::t;

/// Words for a surface to show: a locale key and the values of its
/// `%{name}` placeholders, in the order the rule named them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Label {
    pub key: &'static str,
    pub params: Vec<(&'static str, String)>,
}

impl Label {
    pub(crate) fn new(key: &'static str) -> Label {
        Label { key, params: Vec::new() }
    }

    /// The label with one more placeholder's value.
    pub(crate) fn with(mut self, name: &'static str, value: impl Into<String>) -> Label {
        self.params.push((name, value.into()));
        self
    }

    /// A placeholder's value, if the label carries it.
    pub(crate) fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }

    /// The words in the running locale.
    pub(crate) fn text(&self) -> String {
        self.fill(&t!(self.key))
    }

    /// `template` with each `%{name}` this label carries filled in, in one
    /// pass: a value that itself reads `%{…}` (a video title can) is never
    /// filled again. A placeholder the label has no value for stays as it
    /// is, so a missing param shows rather than vanishing.
    pub(crate) fn fill(&self, template: &str) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(at) = rest.find("%{") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 2..];
            match after.find('}').and_then(|end| self.param(&after[..end]).map(|v| (end, v))) {
                Some((end, value)) => {
                    out.push_str(value);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push_str("%{");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// How a line under the link field, the tags or the folder reads. The
/// surface draws it (the calmer grammar: [`Tone::Refusal`] and
/// [`Tone::Warn`] are the GOLD `! ` lines, the rest DIM or OK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// The link can't be added as it is (not YouTube, a playlist).
    Refusal,
    /// Worth a look before adding (a live stream, a clash, a trap).
    Warn,
    /// For information (just the video, letters dropped).
    Note,
    /// Confirmed (tags from YouTube Music).
    Ok,
}

/// Which surface a failure is worded for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    /// The Settings room: inline above its primary.
    Room,
    /// The Files window (`[ Add here ]`): a lost library's first sentence
    /// only — the window can't pick another library.
    Window,
    /// A row of the From YouTube list.
    List,
}

/// What a server offers Add from YouTube: the API's own reading
/// ([`crate::api::ytdl::YtdlTier`]) — the phone's two tiers below the
/// ping's generation 1, and the new job list from it. Older is also the
/// reading while the tier is not known yet (the phone's rule).
pub(crate) use crate::api::ytdl::YtdlTier as Tier;

/// Every key this module can return, for the locale files and their
/// tests: each `locales/*.yml` must carry every one.
#[cfg(test)]
pub(crate) const KEYS: &[&str] = &[
    "gui.ytdl.link.not_youtube",
    "gui.ytdl.link.playlist",
    "gui.ytdl.link.in_playlist",
    "gui.ytdl.link.live",
    "gui.ytdl.link.live_older",
    "gui.ytdl.lookup.failed",
    "gui.ytdl.lookup.timed_out",
    "gui.ytdl.server.no_ytdlp",
    "gui.ytdl.server.unreachable",
    "gui.ytdl.source.music",
    "gui.ytdl.source.channel",
    "gui.ytdl.source.cleaned",
    "gui.ytdl.help.music",
    "gui.ytdl.help.video_title",
    "gui.ytdl.help.video_title_cleaned",
    "gui.ytdl.help.youtube_artist",
    "gui.ytdl.help.youtube_artist_check",
    "gui.ytdl.help.youtube_artist_stripped",
    "gui.ytdl.help.video_release",
    "gui.ytdl.help.edited",
    "gui.ytdl.blank.title",
    "gui.ytdl.blank.artist_music",
    "gui.ytdl.blank.artist_guess",
    "gui.ytdl.blank.artist_guess_unnamed",
    "gui.ytdl.blank.album_single",
    "gui.ytdl.blank.album_keeps",
    "gui.ytdl.blank.year",
    "gui.ytdl.year_invalid",
    "gui.ytdl.where.err.unknown_var",
    "gui.ytdl.where.err.never_var",
    "gui.ytdl.where.err.bad_char",
    "gui.ytdl.where.err.braces",
    "gui.ytdl.where.err.absolute",
    "gui.ytdl.where.err.traversal",
    "gui.ytdl.where.err.empty",
    "gui.ytdl.where.err.home",
    "gui.ytdl.where.err.drive",
    "gui.ytdl.where.err.too_long",
    "gui.ytdl.where.err.control",
    "gui.ytdl.where.err.generic",
    "gui.ytdl.where.fix_layout",
    "gui.ytdl.where.trap.empty",
    "gui.ytdl.where.trap.year",
    "gui.ytdl.where.trap.other",
    "gui.ytdl.where.preset.artist_album",
    "gui.ytdl.where.preset.artist",
    "gui.ytdl.where.preset.top",
    "gui.ytdl.where.preset.discover",
    "gui.ytdl.where.preset.library",
    "gui.ytdl.where.preset.custom",
    "gui.ytdl.sort.here",
    "gui.ytdl.sort.album",
    "gui.ytdl.sort.artist_album",
    "gui.ytdl.sort.custom",
    "gui.ytdl.format.mp3",
    "gui.ytdl.format.m4a",
    "gui.ytdl.format.opus",
    "gui.ytdl.format.flac",
    "gui.ytdl.format.wav",
    "gui.ytdl.file.clash",
    "gui.ytdl.file.clash_older",
    "gui.ytdl.file.letters_dropped",
    "gui.ytdl.no_mkdir.new",
    "gui.ytdl.no_mkdir.unchecked",
    "gui.ytdl.err.ffmpeg",
    "gui.ytdl.err.uploads_off",
    "gui.ytdl.err.mkdir",
    "gui.ytdl.err.library_gone",
    "gui.ytdl.err.library_gone_short",
    "gui.ytdl.err.no_route",
    "gui.ytdl.err.format_off",
    "gui.ytdl.err.bad_request",
    "gui.ytdl.err.server",
    "gui.ytdl.err.said",
    "gui.ytdl.err.exists",
    "gui.ytdl.err.add_unreachable",
    "gui.ytdl.err.start_unsure",
    "gui.ytdl.list.said",
    "gui.ytdl.list.server",
    "gui.ytdl.list.landed",
    "gui.ytdl.list.landed_unnamed",
    "gui.ytdl.list.too_big",
    "gui.ytdl.list.too_long",
    "gui.ytdl.list.live",
    "gui.ytdl.list.unavailable",
    "gui.ytdl.list.hedged",
    "gui.ytdl.list.hedged_older",
    "gui.ytdl.list.didnt_start",
    "gui.ytdl.list.not_sure_started",
    "gui.ytdl.already_adding",
    "gui.ytdl.add.to_library",
    "gui.ytdl.add.here",
    "gui.ytdl.add.anyway",
    "gui.ytdl.add.adding",
];

/// JavaScript's `\s` as a regex class body (the records' patterns are JS
/// regexes): Rust's `\s` also takes U+0085, and leaves out U+FEFF.
const JS_SPACE: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// A JavaScript `\s` character.
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}'
            | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// JavaScript's `String.prototype.trim`.
fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// `s.replace(/\s+/g, ' ').trim()`, the records' last step.
fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if js_space(c) {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(' ');
        }
        gap = false;
        out.push(c);
    }
    out
}

/// A path's folders, as the explorer and the records spell them
/// (`pathCrumbs`): `/Music/Daft Punk/` and `Music/Daft Punk` alike.
pub(crate) fn path_crumbs(path: &str) -> Vec<String> {
    path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The last folder of a path: the folder a failed mkdir names.
fn last_folder(path: &str) -> String {
    path_crumbs(path).pop().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_fills_its_placeholders_once_and_keeps_the_ones_it_lacks() {
        let l = Label::new("gui.ytdl.err.said").with("text", "%{text} and 50% off");
        assert_eq!(l.fill("The server said: “%{text}”"), "The server said: “%{text} and 50% off”");
        assert_eq!(l.fill("%{other} · %{text"), "%{other} · %{text");
        assert_eq!(Label::new("k").fill("plain"), "plain");
        assert_eq!(l.param("text"), Some("%{text} and 50% off"));
        assert_eq!(l.param("name"), None);
    }

    #[test]
    fn javascript_space_is_not_rusts() {
        assert!(!js_space('\u{85}'), "NEL is no JS space");
        assert!(js_space('\u{FEFF}'));
        assert_eq!(squeeze("  a \u{A0}\t b  "), "a b");
        assert_eq!(squeeze("a\u{85}b "), "a\u{85}b");
        assert_eq!(js_trim("\u{3000}x\u{85}"), "x\u{85}");
    }

    #[test]
    fn path_crumbs_read_either_spelling() {
        assert_eq!(path_crumbs("/Music/Daft Punk/"), ["Music", "Daft Punk"]);
        assert_eq!(path_crumbs("Music/Daft Punk"), ["Music", "Daft Punk"]);
        assert!(path_crumbs("/").is_empty());
        assert_eq!(last_folder("/Music/Daft Punk/Discovery/"), "Discovery");
        assert_eq!(last_folder(""), "");
    }

    /// A label from every branch the rules have.
    fn every_label() -> Vec<Label> {
        let mut out = Vec::new();
        for (input, live, tier) in [
            ("https://vimeo.com/1", false, Tier::New),
            ("https://www.youtube.com/playlist?list=PLx", false, Tier::New),
            ("https://www.youtube.com/live/jfKfPfyJRdk?list=PLx", false, Tier::New),
            ("https://www.youtube.com/watch?v=jfKfPfyJRdk", true, Tier::Older),
        ] {
            out.extend(link_notes(&parse_link(input), live, tier).into_iter().map(|(_, l)| l));
        }

        let gorillaz = guess(Lookup { title: Some("Gorillaz - Feel Good Inc. (Official Video) [HD]"), artist: Some("GorillazVEVO"), ..Lookup::default() });
        let plain = guess(Lookup { title: Some("Song"), artist: Some("Some Uploader"), ..Lookup::default() });
        let music = guess(Lookup { title: Some("One More Time"), artist: Some("Daft Punk"), album: Some("Discovery"), year: Some("2001") });
        out.push(gorillaz.cleaned_label());
        out.extend([Source::Music, Source::Channel].into_iter().filter_map(|s| s.line()).map(|(_, l)| l));
        let undone = GuessState { restored: [0].into(), ..GuessState::default() };
        let stripped = GuessState { strip_restored: true, ..undone.clone() };
        let none = GuessState::default();
        for field in [TagField::Title, TagField::Artist, TagField::Album, TagField::Year] {
            for (g, state, edited, value, album) in [
                (&music, &none, false, "x", None),
                (&music, &none, false, "", None),
                (&gorillaz, &none, false, "x", None),
                (&gorillaz, &none, true, "x", None),
                (&gorillaz, &undone, false, "x", None),
                (&gorillaz, &stripped, false, "x", None),
                (&plain, &none, false, "x", None),
                (&plain, &none, false, "", Some("An album")),
                (&Guess::none(), &none, false, "", None),
            ] {
                out.push(field_help(field, g, state, edited, value, album));
            }
        }

        for check in [
            layout_check("{{TITLE}}"),
            layout_check("{{GENRE}}"),
            layout_check("Singles: 2024"),
            layout_check("{A}"),
            layout_check("/a"),
            layout_check("../a"),
            layout_check(" "),
            layout_check("~/a"),
            layout_check("C:"),
            layout_check(&"x".repeat(501)),
            layout_check("a\u{1}b"),
            LayoutCheck::Engine(crate::api::types::LayoutProblem::EmptyTemplate),
        ] {
            out.extend(layout_problem(&check));
        }
        for kind in [TrapKind::EmptyFolder, TrapKind::MissingYear, TrapKind::Other] {
            out.push(trap_label(&Trap { kind, folder: "()".into() }).0);
        }
        for kind in ["artistAlbum", "artist", "top", "discover", "library", "custom"] {
            out.push(LayoutKind::parse(kind).expect("a kind").label());
        }
        out.extend(FolderSort::ALL.iter().map(|s| s.label()));
        out.extend(FORMAT_ORDER.iter().filter_map(|f| format_help(f)));

        let there = vec!["One_More_Time.mp3".to_string()];
        for (title, stem, tier) in [("One More Time", Some("One_More_Time"), Tier::New), ("One More Time", None, Tier::Older), ("Кино", Some("_"), Tier::New)] {
            out.extend(name_check(title, stem, "mp3", Some(&there), tier).line.map(|(_, l)| l));
        }

        let answered = |status: u16, words: Option<&str>, file_name: Option<&str>| CallFailure::Answered {
            status,
            words: words.map(str::to_string),
            reason: file_name.map(|_| "exists".to_string()),
            file_name: file_name.map(str::to_string),
        };
        let mut failures = vec![
            classify_add(&answered(500, Some("yt-dlp is not installed"), None)),
            classify_add(&answered(500, Some("FFmpeg is not available yet"), None)),
            classify_add(&answered(403, None, None)),
            classify_add(&answered(404, Some("Library 'Music' not found"), None)),
            classify_add(&answered(404, None, None)),
            classify_add(&answered(400, Some("\"outputCodec\" must be one of [mp3]"), None)),
            classify_add(&answered(400, None, None)),
            classify_add(&answered(409, Some("words"), Some("a.mp3"))),
            classify_add(&answered(409, Some("words"), None)),
            classify_add(&answered(500, Some("disk full"), None)),
            classify_add(&answered(502, None, None)),
            classify_add(&CallFailure::NeverConnected),
            classify_add(&CallFailure::TimedOut),
            classify_lookup(&CallFailure::TimedOut),
            classify_lookup(&CallFailure::NeverConnected),
            classify_lookup(&answered(500, None, None)),
            classify_lookup(&answered(500, Some("yt-dlp is not installed"), None)),
            classify_lookup(&answered(404, None, None)),
            classify_lookup(&answered(400, None, None)),
            classify_lookup(&answered(403, Some("words"), None)),
        ];
        failures.extend(mkdir_failure(&answered(403, None, None), Some("Discovery"), "/Music/Discovery/"));
        failures.extend(mkdir_failure(&answered(403, None, None), None, ""));
        failures.extend(mkdir_failure(&answered(500, None, None), Some("Discovery"), "/Music/Discovery/"));
        for (reason, error, name) in
            [(Some("exists"), None, Some("a.mp3")), (Some("exists"), None, None), (Some("too-big"), None, None), (Some("too-long"), None, None), (Some("live"), None, None), (Some("unavailable"), None, None), (None, Some("words"), None), (None, None, None)]
        {
            failures.extend(entry_failure("error", reason, error, name));
        }
        for f in &failures {
            for surface in [Surface::Room, Surface::Window, Surface::List] {
                for tier in [Tier::Older, Tier::Full, Tier::New] {
                    out.push(failure_label(f, &Words { surface, server: "Home server", codec: "opus", tier }));
                }
            }
        }

        for block in [AddBlock::ConvertOff, AddBlock::Unreachable, AddBlock::YtdlpMissing, AddBlock::UploadsOff, AddBlock::AlreadyAdding, AddBlock::InvalidLayout, AddBlock::YearInvalid] {
            out.extend(block_label(block, "Home server"));
        }
        for (surface, clash, starting) in [(Surface::Room, false, false), (Surface::Window, false, false), (Surface::Room, true, false), (Surface::Room, false, true)] {
            out.push(add_button_label(surface, clash, starting));
        }
        out
    }

    #[test]
    fn every_key_is_returned_by_some_rule_and_every_label_returned_is_a_key() {
        let labels = every_label();
        let mut params: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
        for l in &labels {
            assert!(KEYS.contains(&l.key), "{} (returned) is in KEYS", l.key);
            let names: Vec<&str> = l.params.iter().map(|(n, _)| *n).collect();
            let seen = params.entry(l.key).or_insert_with(|| names.clone());
            assert_eq!(*seen, names, "{} always gets the same placeholders", l.key);
        }
        let missing: Vec<&&str> = KEYS.iter().filter(|k| !params.contains_key(**k)).collect();
        assert!(missing.is_empty(), "keys no rule returned: {missing:?}");
    }

    #[test]
    fn keys_are_unique_snake_case_gui_ytdl_keys() {
        let mut seen = std::collections::BTreeSet::new();
        for key in KEYS {
            assert!(seen.insert(*key), "{key} twice");
            let rest = key.strip_prefix("gui.ytdl.").unwrap_or_else(|| panic!("{key} is outside gui.ytdl"));
            assert!(rest.split('.').all(|part| !part.is_empty()
                && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')), "{key}");
        }
    }
}
