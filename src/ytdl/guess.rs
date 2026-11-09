//! Reading a lookup into tags: the title cleaned of upload noise, the
//! artist taken from "Artist - Title" or from YouTube's artist field, and
//! every removal kept so it can be put back.
//!
//! The readers at the top are the server's own (mStream
//! `src/discovery-plugins/plugins/youtube.js`), so Discover and Add from
//! YouTube read a title the same way. The guess adds the phone's
//! deliberate differences (`lib/util/ytdl_guess.dart`): 【…】 groups and the
//! word MV count as noise, a noise group that also says Remix, Live,
//! Remaster(ed) or Version keeps those words, and a trailing "ft. X" becomes
//! "(feat. X)". The records' patterns are JavaScript regexes: here `\s` is
//! JavaScript's ([`super::JS_SPACE`]), `\b` ASCII's, `.` stops at line
//! terminators as JavaScript's does, and digits are ASCII. One drift is
//! left: `(?i)` folds `k` to the Kelvin sign and `s` to `ſ`, as JavaScript's
//! `/i` does not — no noise word is spelled with either. Indexes count
//! characters where the records count UTF-16 units: a title has no
//! fragment boundary inside a character either way.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;

use super::{JS_SPACE, Label, Tone, js_space, js_trim, squeeze};

struct Regexes {
    bracket_noise: Regex,
    trailing_noise: Regex,
    artist_title: Regex,
    topic: Regex,
    topic_suffix: Regex,
    group: Regex,
    trailing: Regex,
    keep_word: Regex,
    noise_phrase: Regex,
    separator_run: Regex,
    edge_separators: Regex,
    feat_tail: Regex,
    vevo: Regex,
    official: Regex,
}

fn regexes() -> &'static Regexes {
    static CELL: OnceLock<Regexes> = OnceLock::new();
    CELL.get_or_init(|| {
        // The records' `\s`, `\S` and `.`, and their word boundary.
        let s = format!("[{JS_SPACE}]");
        let ns = format!("[^{JS_SPACE}]");
        let dot = r"[^\n\r\x{2028}\x{2029}]";
        let b = r"(?-u:\b)";
        let re = |pattern: String| Regex::new(&pattern).expect("a ytdl title pattern");
        // The noise words of a bracket group: the server's, and the phone's
        // with MV.
        let server_words =
            format!(r"official|lyrics?|audio|video|visuali[sz]er|hd|hq|4k|1080p|720p|explicit|clean|full{s}+(?:song|track)");
        let phone_words = format!(r"{server_words}|mv|m/v");
        // A trailing " - Official HD Music Video": the server's words, and
        // the phone's.
        let tail = |words: &str| {
            format!(r"(?i){s}*[-–—|:]{s}*(?:official{s}+)?(?:hd{s}+|hq{s}+)?(?:{words}){s}*$")
        };
        let tail_words = format!(r"music{s}+video|lyric{s}+video|lyrics?|audio|video|visuali[sz]er|audio{s}+only");
        // What a kept group loses: the longer phrases first, as the phone
        // lists them.
        let phrases = format!(r"official|full{s}+(?:song|track)|(?:music|lyrics?){s}+video|audio{s}+only");
        let words = r"lyrics?|audio|video|visuali[sz]er|hd|hq|4k|1080p|720p|explicit|clean|mv|m/v";
        let separators = "-–—|:,/&+·";
        Regexes {
            bracket_noise: re(format!(r"(?i)[(\[][^)\]]*{b}(?:{server_words}){b}[^)\]]*[)\]]")),
            trailing_noise: re(tail(&tail_words)),
            artist_title: re(format!(r"^({dot}{{1,80}}?){s}+([-–—:]){s}+({dot}{{1,160}})$")),
            topic: re(format!(r"(?i)-{s}*Topic$")),
            topic_suffix: re(format!(r"(?i){s}*-{s}*Topic$")),
            group: re(format!(
                r"(?i)[(\[][^)\]]*{b}(?:{phone_words}){b}[^)\]]*[)\]]|【[^】]*{b}(?:{phone_words}){b}[^】]*】"
            )),
            trailing: re(tail(&format!("{tail_words}|mv|m/v"))),
            keep_word: re(format!(r"(?i){b}(?:remix|live|remaster(?:ed)?|version){b}")),
            noise_phrase: re(format!(r"(?i){b}(?:{phrases}|{words}){b}")),
            separator_run: re(format!(r"{s}*([{separators}])(?:{s}*[{separators}])+")),
            edge_separators: re(format!(r"^[{JS_SPACE}\-–—|:,/&+·]+|[{JS_SPACE}\-–—|:,/&+·]+$")),
            feat_tail: re(format!(r"(?i)^({dot}*?{ns}){s}+(?:feat\.|ft\.|featuring){s}+([^()\[\]【】]+)$")),
            vevo: re(format!(r"(?i)^({dot}*?{ns}){s}*VEVO$")),
            official: re(format!(r"(?i)^({dot}*?{ns}){s}+(?:[-–—|:·]{s}+)?Official$")),
        }
    })
}

// ── the server's title readers ──────────────────────────────────────────────

/// The server's `cleanTitle`: bracketed edition noise out, then a trailing
/// " - Official Video", blanks squeezed.
pub(crate) fn clean_title(raw: &str) -> String {
    let r = regexes();
    let s = r.bracket_noise.replace_all(raw, " ");
    let s = r.trailing_noise.replace(&s, "");
    squeeze(&s)
}

/// The server's `splitArtistTitle`: "Artist - Title" → (artist, title).
pub(crate) fn split_artist_title(title: &str) -> Option<(String, String)> {
    let m = regexes().artist_title.captures(js_trim(title))?;
    Some((js_trim(&m[1]).to_string(), js_trim(&m[3]).to_string()))
}

/// A YouTube Music auto-channel: "Daft Punk - Topic".
pub(crate) fn is_topic_channel(channel: &str) -> bool {
    regexes().topic.is_match(js_trim(channel))
}

/// The artist a channel name stands for: " - Topic" off, or nothing left.
pub(crate) fn channel_artist(channel: &str) -> Option<String> {
    let s = regexes().topic_suffix.replace(channel, "");
    let s = js_trim(&s);
    (!s.is_empty()).then(|| s.to_string())
}

// ── the guess ───────────────────────────────────────────────────────────────

/// Where the tags came from, which decides the source line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuessTier {
    /// The lookup had an album (yt-dlp sets one only for music tracks):
    /// "Tags from YouTube Music".
    Music,
    /// "Artist - Title" in the video title.
    Split,
    /// YouTube's artist field, maybe the channel: "check it".
    YoutubeArtist,
    /// No lookup: the fields are the user's.
    None,
}

/// What a fragment of the raw title was cut as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cut {
    /// Upload noise: a bracket group, or a trailing " - Official Video".
    Noise,
    /// The separator the artist was split off at.
    Split,
}

/// One piece the guess took out of the raw title: "(Official Video)",
/// "[HD]", "-" for the split. Putting it back restores it in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fragment {
    pub text: String,
    pub kind: Cut,
}

/// The raw title cut into runs: kept text (`owner` `None`), or a run of one
/// fragment's characters, with what stands in for it while it is out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Piece {
    text: String,
    owner: Option<usize>,
    stand: String,
}

/// A lookup read into tags. Two readings of one answer compare equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Guess {
    pub tier: GuessTier,
    /// The lookup's title, untouched.
    pub raw_title: String,
    /// Cleaned, and the right half after a split.
    pub title: String,
    /// `""` when none.
    pub artist: String,
    pub album: String,
    /// Four digits or `""`.
    pub year: String,
    /// In the order they stand in the raw title.
    pub fragments: Vec<Fragment>,
    /// YouTube's artist field as it came.
    pub youtube_artist_raw: String,
    /// YouTube's artist with a trailing " - Topic", "VEVO" or "Official"
    /// removed; [`Guess::artist_strip`] says which.
    pub youtube_artist: String,
    pub artist_strip: Option<&'static str>,
    /// The split's artist is YouTube's, ignoring case.
    pub artist_agrees: bool,
    /// Something was taken out of the title.
    pub cleaned: bool,
    pieces: Vec<Piece>,
    /// The split fragment's index among [`Guess::fragments`].
    split: Option<usize>,
}

/// What the room keeps beside a guess: the fragments put back (by index
/// into [`Guess::fragments`]; Undo puts them all back), "Swap artist and
/// title", and YouTube's artist with its strip restored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GuessState {
    pub restored: BTreeSet<usize>,
    pub swapped: bool,
    pub strip_restored: bool,
}

/// What a lookup answered, as the guess reads it. The year is the answer's
/// text (a number answer is its integer digits, read by the API layer).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Lookup<'a> {
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub year: Option<&'a str>,
}

/// What a noise group keeps: "(Official Live Video)" → "(Live)". `None`
/// when it goes whole — no keep word, or nothing but "Version" left ("(Clean
/// Version)" says nothing once "Clean" is gone).
fn keep_of(group: &[char]) -> Option<String> {
    let r = regexes();
    let inner: String = group[1..group.len() - 1].iter().collect();
    if !r.keep_word.is_match(&inner) {
        return None;
    }
    let kept = r.noise_phrase.replace_all(&inner, " ");
    let kept = r.separator_run.replace_all(&kept, " $1 ");
    let kept = squeeze(&kept);
    let kept = r.edge_separators.replace_all(&kept, "");
    if kept.is_empty() || kept.to_lowercase() == "version" {
        return None;
    }
    Some(format!("{}{kept}{}", group[0], group[group.len() - 1]))
}

/// "Song ft. X" → "Song (feat. X)", only for a credit at the very end and
/// outside brackets. "feat" and "ft" need their dots: "A Feat of Strength"
/// and "6 ft under" are titles, and the rewrite has no chip to undo it.
fn featured(title: &str) -> String {
    let Some(m) = regexes().feat_tail.captures(title) else { return title.to_string() };
    let who = js_trim(&m[2]);
    if who.is_empty() { title.to_string() } else { format!("{} (feat. {who})", &m[1]) }
}

/// YouTube's artist field with a trailing " - Topic", "VEVO" or word
/// "Official" removed, and which one. A leading "Official" is a name
/// ("Official髭男dism"), never removed.
fn strip_artist(raw: &str) -> (String, Option<&'static str>) {
    let r = regexes();
    let s = squeeze(raw);
    if is_topic_channel(&s) {
        return match channel_artist(&s) {
            Some(a) => (a, Some("- Topic")),
            None => (s, None),
        };
    }
    if let Some(m) = r.vevo.captures(&s) {
        return (m[1].to_string(), Some("VEVO"));
    }
    if let Some(m) = r.official.captures(&s) {
        return (m[1].to_string(), Some("Official"));
    }
    (s, None)
}

fn four_digits(s: &str) -> bool {
    s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit())
}

fn year_of(year: Option<&str>) -> String {
    let s = js_trim(year.unwrap_or_default());
    if four_digits(s) { s.to_string() } else { String::new() }
}

/// A regex match's span in characters.
fn char_span(text: &str, start: usize, end: usize) -> (usize, usize) {
    let a = text[..start].chars().count();
    (a, a + text[start..end].chars().count())
}

/// The cutting of one raw title: which characters each fragment owns.
/// Fragments are added as found (groups, the trailing noise, the split);
/// [`Cutting::laid_out`] orders them as they stand in the title.
struct Cutting {
    raw: Vec<char>,
    owner: Vec<Option<usize>>,
    kinds: Vec<Cut>,
    texts: Vec<String>,
    stands: Vec<String>,
    starts: Vec<usize>,
}

impl Cutting {
    fn add(&mut self, kind: Cut, text: String, stand: String, start: usize) -> usize {
        self.kinds.push(kind);
        self.texts.push(text);
        self.stands.push(stand);
        self.starts.push(start);
        self.kinds.len() - 1
    }

    /// The raw title with every fragment replaced by what stands in for it,
    /// and for each character the raw index it came from (`None`:
    /// stand-in text).
    fn render(&self) -> (String, Vec<Option<usize>>) {
        let mut out = String::new();
        let mut from = Vec::new();
        for i in 0..self.raw.len() {
            match self.owner[i] {
                None => {
                    out.push(self.raw[i]);
                    from.push(Some(i));
                }
                Some(f) if i == 0 || self.owner[i - 1] != Some(f) => {
                    let stand = if i == self.starts[f] { self.stands[f].as_str() } else { " " };
                    out.push_str(stand);
                    from.extend(stand.chars().map(|_| None));
                }
                Some(_) => {}
            }
        }
        (out, from)
    }

    fn new(raw: &str) -> Cutting {
        let r = regexes();
        let mut cut = Cutting {
            raw: raw.chars().collect(),
            owner: vec![None; raw.chars().count()],
            kinds: Vec::new(),
            texts: Vec::new(),
            stands: Vec::new(),
            starts: Vec::new(),
        };
        for m in r.group.find_iter(raw) {
            let (a, z) = char_span(raw, m.start(), m.end());
            let keep = keep_of(&cut.raw[a..z]);
            let f = cut.add(Cut::Noise, m.as_str().to_string(), keep.unwrap_or_else(|| " ".into()), a);
            cut.owner[a..z].fill(Some(f));
        }
        // The trailing rule reads the title with its groups replaced, as
        // the server's second step does; what it matches goes back to the
        // raw characters it came from.
        let (text, from) = cut.render();
        if let Some(m) = r.trailing.find(&text) {
            let (a, z) = char_span(&text, m.start(), m.end());
            let owned: Vec<usize> = from[a..z].iter().flatten().copied().collect();
            if let Some(&first) = owned.first() {
                let words: String = owned.iter().map(|&i| cut.raw[i]).collect();
                let f = cut.add(Cut::Noise, squeeze(&words), " ".into(), first);
                for i in owned {
                    cut.owner[i] = Some(f);
                }
            }
        }
        cut
    }

    /// The server's split, run on the cleaned title: the separator becomes
    /// a fragment. False when the title does not split (or would split at a
    /// separator inside kept bracket text, which the raw title lacks).
    fn split(&mut self) -> bool {
        let (text, from) = self.render();
        // The cleaned title keeping each character's raw index: blank runs
        // squeezed, the ends trimmed.
        let mut chars = String::new();
        let mut at: Vec<Option<usize>> = Vec::new();
        let mut gap = false;
        for (c, f) in text.chars().zip(from) {
            if js_space(c) {
                gap = !chars.is_empty();
                continue;
            }
            if gap {
                chars.push(' ');
                at.push(None);
                gap = false;
            }
            chars.push(c);
            at.push(f);
        }
        let Some(m) = regexes().artist_title.captures(&chars) else { return false };
        let list: Vec<char> = chars.chars().collect();
        let mut k = m[1].chars().count();
        while list.get(k).copied().is_some_and(js_space) {
            k += 1;
        }
        let Some(Some(p)) = at.get(k).copied() else { return false };
        let f = self.add(Cut::Split, m[2].to_string(), String::new(), p);
        self.owner[p] = Some(f);
        true
    }

    /// The fragments in raw order, the pieces titles are rebuilt from, and
    /// the split's index among the fragments.
    fn laid_out(self) -> (Vec<Fragment>, Vec<Piece>, Option<usize>) {
        let mut order: Vec<usize> = (0..self.kinds.len()).collect();
        order.sort_by_key(|&f| self.starts[f]);
        let mut rank = vec![0; order.len()];
        for (r, &f) in order.iter().enumerate() {
            rank[f] = r;
        }
        let mut pieces = Vec::new();
        let mut i = 0;
        while i < self.raw.len() {
            let f = self.owner[i];
            let mut j = i + 1;
            while j < self.raw.len() && self.owner[j] == f {
                j += 1;
            }
            let text: String = self.raw[i..j].iter().collect();
            pieces.push(match f {
                None => Piece { text, owner: None, stand: String::new() },
                Some(f) => {
                    let stand = if i == self.starts[f] { self.stands[f].clone() } else { " ".into() };
                    Piece { text, owner: Some(rank[f]), stand }
                }
            });
            i = j;
        }
        let split = self.kinds.iter().position(|k| *k == Cut::Split).map(|f| rank[f]);
        let fragments = order.iter().map(|&f| Fragment { text: self.texts[f].clone(), kind: self.kinds[f] }).collect();
        (fragments, pieces, split)
    }
}

/// The raw title rebuilt from `pieces` with the `restored` fragments back,
/// cut at the separator (the fragment `split`) while the split is out.
fn rebuild(pieces: &[Piece], split: Option<usize>, restored: &BTreeSet<usize>) -> (String, String) {
    let split_out = split.filter(|s| !restored.contains(s));
    let mut left = String::new();
    let mut right = String::new();
    let mut past = split_out.is_none();
    for p in pieces {
        if split_out.is_some() && p.owner == split_out {
            past = true;
            continue;
        }
        let piece = match p.owner {
            Some(o) if !restored.contains(&o) => &p.stand,
            _ => &p.text,
        };
        if past { right.push_str(piece) } else { left.push_str(piece) }
    }
    (left, right)
}

impl Guess {
    /// The empty guess, for details entered by hand.
    pub(crate) fn none() -> Guess {
        Guess::blank(String::new())
    }

    fn blank(year: String) -> Guess {
        Guess {
            tier: GuessTier::None,
            raw_title: String::new(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            year,
            fragments: Vec::new(),
            youtube_artist_raw: String::new(),
            youtube_artist: String::new(),
            artist_strip: None,
            artist_agrees: false,
            cleaned: false,
            pieces: Vec::new(),
            split: None,
        }
    }

    /// The title half of the rebuilt title, "ft. X" made "(feat. X)" — or,
    /// with every fragment back, the video's own title (its blanks
    /// squeezed), "ft." and all.
    pub(crate) fn title_with(&self, restored: &BTreeSet<usize>) -> String {
        let count = self.fragments.len();
        if count > 0 && (0..count).all(|i| restored.contains(&i)) {
            return squeeze(&self.raw_title);
        }
        featured(&squeeze(&rebuild(&self.pieces, self.split, restored).1))
    }

    /// The artist when the split fragment is put back (YouTube's artist,
    /// stripped) or not (the text left of the separator, with any of its
    /// restored fragments back). Outside the split tier: the guess's
    /// artist.
    pub(crate) fn artist_with(&self, restored: &BTreeSet<usize>) -> String {
        if self.tier != GuessTier::Split {
            return self.artist.clone();
        }
        if self.split.is_some_and(|s| restored.contains(&s)) {
            return self.youtube_artist.clone();
        }
        squeeze(&rebuild(&self.pieces, self.split, restored).0)
    }

    fn split_out(&self, state: &GuessState) -> bool {
        self.tier == GuessTier::Split && !self.split.is_some_and(|s| state.restored.contains(&s))
    }

    fn noise_out(&self, state: &GuessState) -> bool {
        self.fragments.iter().enumerate().any(|(i, f)| f.kind == Cut::Noise && !state.restored.contains(&i))
    }

    /// The title and artist the guess gives now.
    pub(crate) fn fields(&self, state: &GuessState) -> (String, String) {
        let title = self.title_with(&state.restored);
        let youtube =
            if state.strip_restored { js_trim(&self.youtube_artist_raw).to_string() } else { self.youtube_artist.clone() };
        let artist = match self.tier {
            GuessTier::Music => self.artist.clone(),
            GuessTier::Split if self.split_out(state) => self.artist_with(&state.restored),
            GuessTier::Split | GuessTier::YoutubeArtist => youtube,
            GuessTier::None => String::new(),
        };
        if state.swapped { (artist, title) } else { (title, artist) }
    }

    /// What the room may offer around the fields. `artist_edited`: the user
    /// typed in the Artist field.
    pub(crate) fn view(&self, state: &GuessState, artist_edited: bool) -> GuessView {
        let split_out = self.split_out(state);
        let can_undo_all = self.cleaned && state.restored.len() < self.fragments.len();
        let mut chips = Vec::new();
        if !state.swapped {
            for kind in [Cut::Noise, Cut::Split] {
                for (i, f) in self.fragments.iter().enumerate() {
                    if f.kind == kind && !state.restored.contains(&i) {
                        chips.push((i, f.clone()));
                    }
                }
            }
        }
        GuessView {
            split_out,
            noise_out: self.noise_out(state),
            can_swap: split_out,
            can_undo_all,
            cleaned: can_undo_all && self.fragments.iter().any(|f| f.kind == Cut::Noise),
            chips,
            can_restore_strip: self.artist_strip.is_some()
                && !state.strip_restored
                && !artist_edited
                && (self.tier == GuessTier::YoutubeArtist || (self.tier == GuessTier::Split && !split_out)),
            or_youtube_artist: (self.tier == GuessTier::Split
                && split_out
                && !self.artist_agrees
                && !self.youtube_artist.is_empty()
                && !artist_edited)
                .then(|| self.youtube_artist.clone()),
        }
    }

    /// Where the tags came from.
    pub(crate) fn source(&self, state: &GuessState) -> Source {
        match self.tier {
            GuessTier::None => Source::None,
            GuessTier::Music => Source::Music,
            _ if self.split_out(state) => Source::VideoTitle { agrees: self.artist_agrees },
            _ => Source::Channel,
        }
    }

    /// "Cleaned from “Daft Punk - One More Time (Official Video)”".
    pub(crate) fn cleaned_label(&self) -> Label {
        Label::new("gui.ytdl.source.cleaned").with("title", self.raw_title.clone())
    }
}

/// The room's reading of a guess and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuessView {
    pub split_out: bool,
    pub noise_out: bool,
    pub can_swap: bool,
    /// Something is still out: "u undo" puts it all back.
    pub can_undo_all: bool,
    /// A noise fragment is out: "Cleaned from “…”" shows.
    pub cleaned: bool,
    /// The pieces still out, noise first, none while swapped.
    pub chips: Vec<(usize, Fragment)>,
    pub can_restore_strip: bool,
    /// The split's artist differs from YouTube's: its name.
    pub or_youtube_artist: Option<String>,
}

/// The source line under the tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// No lookup.
    None,
    /// "Tags from YouTube Music".
    Music,
    /// The split stands ("Artist and title from the video title" on the
    /// phone): the terminal draws no line for it (TermRoomStates C2 shows
    /// only "Cleaned from …"), so it has no words here.
    VideoTitle { agrees: bool },
    /// "Artist may be the channel name — check it": YouTube's artist
    /// field, also once a split is undone.
    Channel,
}

impl Source {
    /// The line, with its tone; `None` where the room draws none.
    pub(crate) fn line(self) -> Option<(Tone, Label)> {
        match self {
            Source::Music => Some((Tone::Ok, Label::new("gui.ytdl.source.music"))),
            Source::Channel => Some((Tone::Warn, Label::new("gui.ytdl.source.channel"))),
            Source::None | Source::VideoTitle { .. } => None,
        }
    }
}

/// Read a lookup answer into a guess. `title` is cleaned (and the right
/// half after a split); the year is four digits or `""`.
pub(crate) fn guess(answer: Lookup<'_>) -> Guess {
    let raw_title = answer.title.unwrap_or_default().to_string();
    let youtube_artist_raw = answer.artist.unwrap_or_default().to_string();
    let album = squeeze(answer.album.unwrap_or_default());
    let year = year_of(answer.year);
    let (youtube_artist, artist_strip) = strip_artist(&youtube_artist_raw);

    if squeeze(&raw_title).is_empty() && squeeze(&youtube_artist_raw).is_empty() && album.is_empty() {
        return Guess::blank(year);
    }

    let mut cut = Cutting::new(&raw_title);
    // The split tier needs no album and a title the server's rule splits.
    let tier = if !album.is_empty() {
        GuessTier::Music
    } else if cut.split() {
        GuessTier::Split
    } else {
        GuessTier::YoutubeArtist
    };
    let (fragments, pieces, split) = cut.laid_out();
    let none = BTreeSet::new();
    let artist = match tier {
        GuessTier::Music => squeeze(&youtube_artist_raw),
        GuessTier::Split => squeeze(&rebuild(&pieces, split, &none).0),
        _ => youtube_artist.clone(),
    };
    let artist_agrees =
        tier == GuessTier::Split && !youtube_artist.is_empty() && youtube_artist.to_lowercase() == artist.to_lowercase();
    let mut g = Guess {
        tier,
        raw_title,
        title: String::new(),
        artist,
        album,
        year,
        cleaned: !fragments.is_empty(),
        fragments,
        youtube_artist_raw,
        youtube_artist,
        artist_strip,
        artist_agrees,
        pieces,
        split,
    };
    g.title = g.title_with(&none);
    g
}

// ── the fields ──────────────────────────────────────────────────────────────

/// One of the four tags the room edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagField {
    Title,
    Artist,
    Album,
    Year,
}

/// The four tags as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
}

impl Tags {
    /// The tags as the layout reads them (trimmed).
    pub(crate) fn song(&self) -> SongTags {
        SongTags {
            artist: js_trim(&self.artist).to_string(),
            album: js_trim(&self.album).to_string(),
            year: js_trim(&self.year).to_string(),
        }
    }

    /// POST /api/v1/ytdl/'s `metadata`: only the non-blank title, artist and
    /// album and a four-digit year, trimmed, in that order — the route's
    /// Joi refuses any other key, and null, and with it the whole add; an
    /// empty list means no `metadata` key at all.
    pub(crate) fn request(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (key, value) in [("title", &self.title), ("artist", &self.artist), ("album", &self.album)] {
            let v = js_trim(value);
            if !v.is_empty() {
                out.push((key, v.to_string()));
            }
        }
        let year = js_trim(&self.year);
        if four_digits(year) {
            out.push(("year", year.to_string()));
        }
        out
    }
}

/// The tags a layout renders a song's folders from: ARTIST and ALBUMARTIST
/// are both the Artist field (the room has one).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SongTags {
    pub artist: String,
    pub album: String,
    pub year: String,
}

/// Non-empty and not four digits: "A year is 4 digits, like 2001".
pub(crate) fn year_invalid(year: &str) -> bool {
    let y = js_trim(year);
    !y.is_empty() && !four_digits(y)
}

/// The focused field's one help line: its source, "Edited" once typed in,
/// or — empty, typed or not — what a blank field does. `edited`: this
/// field was typed in; `lookup_album`: the lookup's own album.
pub(crate) fn field_help(
    field: TagField,
    guess: &Guess,
    state: &GuessState,
    edited: bool,
    value: &str,
    lookup_album: Option<&str>,
) -> Label {
    let blank = js_trim(value).is_empty();
    if !blank && edited {
        return Label::new("gui.ytdl.help.edited");
    }
    let split_out = guess.split_out(state);
    let noise_out = guess.noise_out(state);
    let music = guess.tier == GuessTier::Music;
    let key = match field {
        TagField::Title if blank => "gui.ytdl.blank.title",
        TagField::Title if state.swapped => "gui.ytdl.help.video_title",
        TagField::Title if noise_out => "gui.ytdl.help.video_title_cleaned",
        TagField::Title if music => "gui.ytdl.help.music",
        TagField::Title => "gui.ytdl.help.video_title",
        TagField::Artist if blank => {
            if music {
                "gui.ytdl.blank.artist_music"
            } else {
                let raw = js_trim(&guess.youtube_artist_raw);
                if raw.is_empty() {
                    "gui.ytdl.blank.artist_guess_unnamed"
                } else {
                    return Label::new("gui.ytdl.blank.artist_guess").with("name", raw);
                }
            }
        }
        // Swapped, the artist holds the title's half — cleaned, if noise
        // came out of it.
        TagField::Artist if state.swapped && noise_out => "gui.ytdl.help.video_title_cleaned",
        TagField::Artist if state.swapped => "gui.ytdl.help.video_title",
        TagField::Artist if music => "gui.ytdl.help.music",
        TagField::Artist if split_out => "gui.ytdl.help.video_title",
        TagField::Artist => match guess.artist_strip {
            Some(removed) if !state.strip_restored => {
                return Label::new("gui.ytdl.help.youtube_artist_stripped").with("removed", removed);
            }
            _ if guess.tier == GuessTier::YoutubeArtist => "gui.ytdl.help.youtube_artist_check",
            _ => "gui.ytdl.help.youtube_artist",
        },
        TagField::Album if blank => {
            if !guess.album.is_empty() || lookup_album.is_some_and(|a| !a.is_empty()) {
                "gui.ytdl.blank.album_keeps"
            } else {
                "gui.ytdl.blank.album_single"
            }
        }
        // An album only ever comes with the music tier.
        TagField::Album => "gui.ytdl.help.music",
        TagField::Year if blank => "gui.ytdl.blank.year",
        TagField::Year if music => "gui.ytdl.help.music",
        TagField::Year => "gui.ytdl.help.video_release",
    };
    Label::new(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(title: &str, artist: Option<&str>) -> Guess {
        guess(Lookup { title: Some(title), artist, ..Lookup::default() })
    }

    fn set(indexes: &[usize]) -> BTreeSet<usize> {
        indexes.iter().copied().collect()
    }

    fn all(guess: &Guess) -> BTreeSet<usize> {
        (0..guess.fragments.len()).collect()
    }

    fn texts(guess: &Guess) -> Vec<&str> {
        guess.fragments.iter().map(|f| f.text.as_str()).collect()
    }

    fn noise(text: &str) -> Fragment {
        Fragment { text: text.into(), kind: Cut::Noise }
    }

    fn sep(text: &str) -> Fragment {
        Fragment { text: text.into(), kind: Cut::Split }
    }

    /// What the server's own code made of each title: cleanTitle, then
    /// splitArtistTitle of that, isTopicChannel and channelArtist of the
    /// title (the webapp's SERVER_ANSWERS, held to youtube.js there).
    const SERVER_ANSWERS: &[(&str, &str, Option<(&str, &str)>, bool, Option<&str>)] = &[
        ("Gorillaz - Feel Good Inc. (Official Video) [HD]", "Gorillaz - Feel Good Inc.", Some(("Gorillaz", "Feel Good Inc.")), false, Some("Gorillaz - Feel Good Inc. (Official Video) [HD]")),
        ("Sweater Weather (Official Video)", "Sweater Weather", None, false, Some("Sweater Weather (Official Video)")),
        ("Кино – Группа крови", "Кино – Группа крови", Some(("Кино", "Группа крови")), false, Some("Кино – Группа крови")),
        ("Daft Punk - Around the World (Official Audio)", "Daft Punk - Around the World", Some(("Daft Punk", "Around the World")), false, Some("Daft Punk - Around the World (Official Audio)")),
        ("Song (Official Video (HD))", "Song )", None, false, Some("Song (Official Video (HD))")),
        ("Song (Official Video]", "Song", None, false, Some("Song (Official Video]")),
        ("Song [Lyrics] (Live)", "Song (Live)", None, false, Some("Song [Lyrics] (Live)")),
        ("Song - Official HD Music Video", "Song", None, false, Some("Song - Official HD Music Video")),
        ("Song : Audio Only", "Song", None, false, Some("Song : Audio Only")),
        ("Song — visualiser", "Song", None, false, Some("Song — visualiser")),
        ("Song (Full Song)", "Song", None, false, Some("Song (Full Song)")),
        ("Song [4K Remastered]", "Song", None, false, Some("Song [4K Remastered]")),
        ("Artist: Song Name", "Artist: Song Name", None, false, Some("Artist: Song Name")),
        ("A - B - C", "A - B - C", Some(("A", "B - C")), false, Some("A - B - C")),
        ("Artist -Song", "Artist -Song", None, false, Some("Artist -Song")),
        ("  Spaced   -   Out  (Audio)  ", "Spaced - Out", Some(("Spaced", "Out")), false, Some("Spaced   -   Out  (Audio)")),
        ("Neon Harbor - Topic", "Neon Harbor - Topic", Some(("Neon Harbor", "Topic")), true, Some("Neon Harbor")),
        ("Neon Harbor-Topic", "Neon Harbor-Topic", None, true, Some("Neon Harbor")),
        ("Neon Harbor - topic ", "Neon Harbor - topic", Some(("Neon Harbor", "topic")), true, Some("Neon Harbor - topic")),
        ("- Topic", "- Topic", None, true, None),
        ("Song\u{85}(Official Audio)", "Song\u{85}", None, false, Some("Song\u{85}(Official Audio)")),
        ("Videogame (Lyric)", "Videogame", None, false, Some("Videogame (Lyric)")),
        ("Song (Explicit)", "Song", None, false, Some("Song (Explicit)")),
        ("Song (Clean)", "Song", None, false, Some("Song (Clean)")),
        ("Song (Official Live Video)", "Song", None, false, Some("Song (Official Live Video)")),
        ("Song (Live)", "Song (Live)", None, false, Some("Song (Live)")),
        ("【MV】米津玄師 - Lemon", "【MV】米津玄師 - Lemon", Some(("【MV】米津玄師", "Lemon")), false, Some("【MV】米津玄師 - Lemon")),
        ("Song (MV)", "Song (MV)", None, false, Some("Song (MV)")),
        ("Artist - Song ft. Other (Official Video)", "Artist - Song ft. Other", Some(("Artist", "Song ft. Other")), false, Some("Artist - Song ft. Other (Official Video)")),
        ("Song (Official Video) - Lyrics", "Song", None, false, Some("Song (Official Video) - Lyrics")),
        ("Song | HD Video", "Song", None, false, Some("Song | HD Video")),
        ("Song (HQ Audio) [1080p]", "Song", None, false, Some("Song (HQ Audio) [1080p]")),
    ];

    /// Titles the phone (and so the webapp and this) reads differently on
    /// purpose.
    const PHONE_DIFFERS: &[&str] =
        &["Song (Official Live Video)", "Song [4K Remastered]", "【MV】米津玄師 - Lemon", "Song (MV)", "Artist - Song ft. Other (Official Video)"];

    #[test]
    fn the_title_readers_are_the_servers() {
        for (title, cleaned, split, topic, artist) in SERVER_ANSWERS {
            assert_eq!(clean_title(title), *cleaned, "{title}");
            assert_eq!(split_artist_title(cleaned).as_ref().map(|(a, t)| (a.as_str(), t.as_str())), *split, "{title}");
            assert_eq!(is_topic_channel(title), *topic, "{title}");
            assert_eq!(channel_artist(title).as_deref(), *artist, "{title}");
        }
        assert_eq!(clean_title("Salt & Static (2019 Remaster)"), "Salt & Static (2019 Remaster)", "remaster is no noise");
        assert_eq!(clean_title("Neon Harbor - Salt & Static (Official Audio)"), "Neon Harbor - Salt & Static");
        assert_eq!(clean_title("Salt & Static [Official Music Video] (HD)"), "Salt & Static");
        assert_eq!(clean_title("Salt & Static | Official Lyric Video"), "Salt & Static");
        assert_eq!(clean_title("Salt & Static - Lyrics"), "Salt & Static");
        assert_eq!(split_artist_title("Neon Harbor – Salt & Static"), Some(("Neon Harbor".into(), "Salt & Static".into())));
        assert_eq!(channel_artist(""), None);
        assert_eq!(clean_title(""), "");
        assert_eq!(split_artist_title(""), None);
    }

    #[test]
    fn the_guess_cleans_and_splits_as_the_server_does_where_it_makes_no_change_of_its_own() {
        for (title, cleaned, split, _, _) in SERVER_ANSWERS {
            if PHONE_DIFFERS.contains(title) {
                continue;
            }
            let guess = g(title, Some("Uploader"));
            match split {
                None => {
                    assert_eq!(guess.tier, GuessTier::YoutubeArtist, "{title}");
                    assert_eq!(guess.title, *cleaned, "{title}");
                }
                Some((artist, t)) => {
                    assert_eq!(guess.tier, GuessTier::Split, "{title}");
                    assert_eq!((guess.artist.as_str(), guess.title.as_str()), (*artist, *t), "{title}");
                }
            }
        }
    }

    /// The webapp's differential table against the phone's own guess
    /// (`ytdlGuess`, run on the Dart VM over 3,660 answers): title, artist,
    /// tier, title out, artist out, fragments, the strip.
    const PHONE_GUESSES: &[(Option<&str>, Option<&str>, GuessTier, &str, &str, &[&str], Option<&str>)] = &[
        (Some("Artist - Title (Official Video) [HD]"), Some("ArtistVEVO"), GuessTier::Split, "Title", "Artist", &["-", "(Official Video)", "[HD]"], Some("VEVO")),
        (Some("AC/DC - Thunderstruck (Official Video)"), Some("acdcVEVO"), GuessTier::Split, "Thunderstruck", "AC/DC", &["-", "(Official Video)"], Some("VEVO")),
        (Some("Кино - Группа крови (Official Audio)"), Some("Кино - Topic"), GuessTier::Split, "Группа крови", "Кино", &["-", "(Official Audio)"], Some("- Topic")),
        (Some("Song"), Some("Daft Punk - Topic"), GuessTier::YoutubeArtist, "Song", "Daft Punk", &[], Some("- Topic")),
        (Some("Song"), Some("Gorillaz - Official"), GuessTier::YoutubeArtist, "Song", "Gorillaz", &[], Some("Official")),
        (Some("Song"), Some("Official髭男dism"), GuessTier::YoutubeArtist, "Song", "Official髭男dism", &[], None),
        (Some("Song"), Some("VEVO"), GuessTier::YoutubeArtist, "Song", "VEVO", &[], None),
        (Some("Song"), Some("- Topic"), GuessTier::YoutubeArtist, "Song", "- Topic", &[], None),
        (Some("Artist - Title ft. Someone (Official Video)"), None, GuessTier::Split, "Title (feat. Someone)", "Artist", &["-", "(Official Video)"], None),
        (Some("Artist feat. Other - Title"), None, GuessTier::Split, "Title", "Artist feat. Other", &["-"], None),
        (Some("Artist - Title (feat. Other) [Official Video]"), None, GuessTier::Split, "Title (feat. Other)", "Artist", &["-", "[Official Video]"], None),
        (Some("Artist - Title featuring A & B"), None, GuessTier::Split, "Title (feat. A & B)", "Artist", &["-"], None),
        (Some("Artist - 6 ft under"), None, GuessTier::Split, "6 ft under", "Artist", &["-"], None),
        (Some("Artist - A Feat of Strength"), None, GuessTier::Split, "A Feat of Strength", "Artist", &["-"], None),
        (Some("Artist - Title | Official Lyric Video"), None, GuessTier::Split, "Title", "Artist", &["-", "| Official Lyric Video"], None),
        (Some("Artist — Title (Live at Wembley) [Official Video]"), None, GuessTier::Split, "Title (Live at Wembley)", "Artist", &["—", "[Official Video]"], None),
        (Some("Artist - Title (Remix) [Official Audio]"), None, GuessTier::Split, "Title (Remix)", "Artist", &["-", "[Official Audio]"], None),
        (Some("Artist - Title (Official Remix Video)"), None, GuessTier::Split, "Title (Remix)", "Artist", &["-", "(Official Remix Video)"], None),
        (Some("Artist - Title (Official Video · Live)"), None, GuessTier::Split, "Title (Live)", "Artist", &["-", "(Official Video · Live)"], None),
        (Some("Artist - Title (Live | Official Video)"), None, GuessTier::Split, "Title (Live)", "Artist", &["-", "(Live | Official Video)"], None),
        (Some("Artist - Title [Official Video] (Remastered 2009)"), None, GuessTier::Split, "Title (Remastered 2009)", "Artist", &["-", "[Official Video]"], None),
        (Some("a-ha - Take On Me (Official Video) [Remastered in 4K]"), Some("a-ha"), GuessTier::Split, "Take On Me [Remastered in]", "a-ha", &["-", "(Official Video)", "[Remastered in 4K]"], None),
        (Some("Artist - Title (Clean Version)"), None, GuessTier::Split, "Title", "Artist", &["-", "(Clean Version)"], None),
        (Some("Artist - Title (Acoustic Version) (Official Video)"), None, GuessTier::Split, "Title (Acoustic Version)", "Artist", &["-", "(Official Video)"], None),
        (Some("Artist - Title 【Official Music Video】"), None, GuessTier::Split, "Title", "Artist", &["-", "【Official Music Video】"], None),
        (Some("【MV】Artist - Title"), None, GuessTier::Split, "Title", "Artist", &["【MV】", "-"], None),
        (Some("Artist - Title M/V"), None, GuessTier::Split, "Title M/V", "Artist", &["-"], None),
        (Some("Artist「Title」MV"), None, GuessTier::YoutubeArtist, "Artist「Title」MV", "", &[], None),
        (Some("米津玄師 MV「Lemon」"), Some("Kenshi Yonezu 米津玄師"), GuessTier::YoutubeArtist, "米津玄師 MV「Lemon」", "Kenshi Yonezu 米津玄師", &[], None),
        (Some("YOASOBI「夜に駆ける」Official Music Video"), Some("Ayase / YOASOBI"), GuessTier::YoutubeArtist, "YOASOBI「夜に駆ける」Official Music Video", "Ayase / YOASOBI", &[], None),
        (Some("BTS (방탄소년단) 'Dynamite' Official MV"), Some("HYBE LABELS"), GuessTier::YoutubeArtist, "BTS (방탄소년단) 'Dynamite' Official MV", "HYBE LABELS", &[], None),
        (Some("周杰倫 Jay Chou【告白氣球 Love Confession】Official MV"), Some("周杰倫 Jay Chou"), GuessTier::YoutubeArtist, "周杰倫 Jay Chou【告白氣球 Love Confession】Official MV", "周杰倫 Jay Chou", &[], None),
        (Some("Björk : Army of Me"), None, GuessTier::Split, "Army of Me", "Björk", &[":"], None),
        (Some("Artist - Title"), None, GuessTier::Split, "Title", "Artist", &["-"], None),
        (Some("Title ((Official Video))"), None, GuessTier::YoutubeArtist, "Title )", "", &["((Official Video)"], None),
        (Some("(Official Video)"), Some("Uploader"), GuessTier::YoutubeArtist, "", "Uploader", &["(Official Video)"], None),
        (Some("   "), Some("Uploader"), GuessTier::YoutubeArtist, "", "Uploader", &[], None),
        (Some(""), Some(""), GuessTier::None, "", "", &[], None),
        (None, None, GuessTier::None, "", "", &[], None),
    ];

    #[test]
    fn adversarial_titles_and_artists_read_as_the_phones_guess_reads_them() {
        let long = format!("{} - Title", "A".repeat(81));
        let mut rows: Vec<(Option<&str>, Option<&str>, GuessTier, String, &str, &[&str], Option<&str>)> =
            PHONE_GUESSES.iter().map(|(t, a, tier, title, artist, f, s)| (*t, *a, *tier, title.to_string(), *artist, *f, *s)).collect();
        rows.push((Some(&long), None, GuessTier::YoutubeArtist, long.clone(), "", &[], None));
        for (title, artist, tier, out_title, out_artist, fragments, strip) in rows {
            let guess = guess(Lookup { title, artist, ..Lookup::default() });
            let at = format!("{title:?} / {artist:?}");
            assert_eq!(guess.tier, tier, "{at}");
            assert_eq!(guess.title, out_title, "{at}");
            assert_eq!(guess.artist, out_artist, "{at}");
            assert_eq!(texts(&guess), fragments, "{at}");
            assert_eq!(guess.artist_strip, strip, "{at}");
            assert_eq!(guess.title_with(&all(&guess)), squeeze(title.unwrap_or_default()), "{at}: Undo is the raw title");
        }
    }

    #[test]
    fn music_tier_takes_the_tags_as_given() {
        let guess = guess(Lookup { title: Some("One More Time"), artist: Some("Daft Punk"), album: Some("Discovery"), year: Some("2001") });
        assert_eq!(guess.tier, GuessTier::Music);
        assert_eq!([guess.title.as_str(), guess.artist.as_str(), guess.album.as_str(), guess.year.as_str()], ["One More Time", "Daft Punk", "Discovery", "2001"]);
        assert!(guess.fragments.is_empty());
        assert!(!guess.cleaned);
        assert_eq!(guess.raw_title, "One More Time");
        assert_eq!(guess.youtube_artist_raw, "Daft Punk");
        assert_eq!(guess.youtube_artist, "Daft Punk");
        assert_eq!(guess.artist_strip, None);
        assert!(!guess.artist_agrees);
        assert_eq!(guess.title_with(&set(&[])), "One More Time");
        assert_eq!(guess.artist_with(&set(&[])), "Daft Punk");
        let cleaned = super::guess(Lookup {
            title: Some("Daft Punk - One More Time (Official Audio)"),
            artist: Some("Daft Punk"),
            album: Some("Discovery"),
            year: None,
        });
        assert_eq!(cleaned.tier, GuessTier::Music, "a music track never splits");
        assert_eq!(cleaned.title, "Daft Punk - One More Time");
        assert_eq!(cleaned.fragments, [noise("(Official Audio)")]);
    }

    #[test]
    fn split_tier_names_the_separator_and_whether_youtube_agrees() {
        let gorillaz = g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("Gorillaz"));
        assert_eq!(gorillaz.tier, GuessTier::Split);
        assert_eq!([gorillaz.title.as_str(), gorillaz.artist.as_str(), gorillaz.album.as_str(), gorillaz.year.as_str()], ["Feel Good Inc.", "Gorillaz", "", ""]);
        assert!(gorillaz.artist_agrees);
        assert!(gorillaz.cleaned);
        assert_eq!(gorillaz.fragments, [sep("-"), noise("(Official Video)"), noise("[HD]")]);
        let kino = g("Кино – Группа крови", Some("Кино"));
        assert_eq!((kino.artist.as_str(), kino.title.as_str()), ("Кино", "Группа крови"));
        assert_eq!(kino.fragments, [sep("–")]);
        assert!(kino.artist_agrees);
        assert!(g("GORILLAZ - Feel Good Inc.", Some("gorillaz")).artist_agrees, "ignoring case");
        assert!(!g("Gorillaz - Feel Good Inc.", Some("Some Uploader")).artist_agrees);
        assert!(!g("Gorillaz - Feel Good Inc.", None).artist_agrees);
        assert!(g("Gorillaz - Feel Good Inc.", Some("GorillazVEVO")).artist_agrees, "the stripped artist");
        let several = g("Daft Punk, Pharrell Williams - Get Lucky", Some("Daft Punk"));
        assert_eq!((several.artist.as_str(), several.title.as_str()), ("Daft Punk, Pharrell Williams", "Get Lucky"));
        assert_eq!(g("Lily Allen & Giggs - Something's Not Right", None).artist, "Lily Allen & Giggs");
        assert_eq!(g("Björk : Army of Me", None).artist, "Björk");
        assert_eq!(g("Artist -Song", None).tier, GuessTier::YoutubeArtist);
    }

    #[test]
    fn youtube_artist_tier_strips_topic_vevo_and_a_trailing_official() {
        let vevo = g("Sweater Weather (Official Video)", Some("TheNeighbourhoodVEVO"));
        assert_eq!(vevo.tier, GuessTier::YoutubeArtist);
        assert_eq!((vevo.title.as_str(), vevo.artist.as_str()), ("Sweater Weather", "TheNeighbourhood"));
        assert_eq!(vevo.youtube_artist_raw, "TheNeighbourhoodVEVO");
        assert_eq!(vevo.artist_strip, Some("VEVO"));
        assert_eq!(vevo.fragments, [noise("(Official Video)")]);
        assert!(!vevo.artist_agrees);
        for raw in ["Daft Punk - Topic", "Daft Punk-Topic", "Daft Punk -  topic"] {
            let guess = g("One More Time", Some(raw));
            assert_eq!(guess.artist, "Daft Punk", "{raw}");
            assert_eq!(guess.artist_strip, Some("- Topic"), "{raw}");
        }
        assert_eq!(g("Song", Some("Taylor Swift VEVO")).artist, "Taylor Swift");
        assert_eq!(g("Song", Some("TaylorSwiftVevo")).artist, "TaylorSwift");
        assert_eq!(g("Song", Some("VEVO")).artist_strip, None, "nothing would be left");
        let pretender = g("Pretender", Some("Official髭男dism"));
        assert_eq!((pretender.artist.as_str(), pretender.artist_strip), ("Official髭男dism", None));
        let official = g("Song", Some("Daft Punk Official"));
        assert_eq!((official.artist.as_str(), official.artist_strip), ("Daft Punk", Some("Official")));
        assert_eq!(g("Song", Some("BLACKPINK - Official")).artist, "BLACKPINK");
        assert_eq!(g("Song", Some("Unofficial")).artist_strip, None);
        assert_eq!(g("Song", Some("Official")).artist, "Official");
        let none = g("Sweater Weather (Official Video)", None);
        assert_eq!((none.tier, none.artist.as_str(), none.youtube_artist_raw.as_str()), (GuessTier::YoutubeArtist, "", ""));
    }

    #[test]
    fn no_lookup_is_the_empty_guess_and_the_year_is_four_digits_or_nothing() {
        let none = Guess::none();
        assert_eq!(none.tier, GuessTier::None);
        for v in [&none.raw_title, &none.title, &none.artist, &none.album, &none.year, &none.youtube_artist] {
            assert_eq!(v, "");
        }
        assert!(none.fragments.is_empty());
        assert!(!none.cleaned);
        assert_eq!(none.title_with(&set(&[0])), "");
        assert_eq!(none.artist_with(&set(&[0])), "");
        assert_eq!(guess(Lookup::default()), Guess::none());
        assert_eq!(guess(Lookup { title: Some("  "), artist: Some(""), ..Lookup::default() }).tier, GuessTier::None);
        let year = guess(Lookup { year: Some("2001"), ..Lookup::default() });
        assert_eq!((year.tier, year.year.as_str()), (GuessTier::None, "2001"));
        let with = |y: &str| guess(Lookup { title: Some("x"), album: Some("A"), year: Some(y), ..Lookup::default() }).year;
        assert_eq!(with("2001"), "2001");
        assert_eq!(with(" 2001 "), "2001");
        for y in ["", "01", "20011", "2001-03-12", "MMI", "12", "٢٠٠١", "2001.5"] {
            assert_eq!(with(y), "", "{y}");
        }
    }

    #[test]
    fn the_phones_own_cleaning_keeps_words_and_drops_a_lone_version() {
        let live = g("Sweater Weather (Official Live Video)", Some("X"));
        assert_eq!(live.title, "Sweater Weather (Live)");
        assert_eq!(live.fragments, [noise("(Official Live Video)")]);
        assert_eq!(live.title_with(&set(&[0])), "Sweater Weather (Official Live Video)");
        assert_eq!(g("Song (Official Remix Video)", None).title, "Song (Remix)");
        assert_eq!(g("Song (Remastered 2009 - Official Audio)", None).title, "Song (Remastered 2009)");
        assert_eq!(g("Song [Live at Wembley | Official Music Video]", None).title, "Song [Live at Wembley]");
        assert_eq!(g("Song (Extended Version) (Official Audio)", None).title, "Song (Extended Version)");
        assert_eq!(g("Song (Live - Acoustic Version - Official Video)", None).title, "Song (Live - Acoustic Version)");
        assert_eq!(g("Song [4K Remastered]", None).title, "Song [Remastered]");
        let clean = g("Song (Clean Version)", None);
        assert_eq!(clean.title, "Song");
        assert_eq!(clean.fragments, [noise("(Clean Version)")]);
        assert_eq!(g("Song [Live]", None).title, "Song [Live]");
        assert_eq!(g("Song [2011 Remaster]", None).title, "Song [2011 Remaster]");
        assert!(g("Song [2011 Remaster]", None).fragments.is_empty());
        assert_eq!(g("Song (Radio Edit)", None).title, "Song (Radio Edit)");
    }

    #[test]
    fn corner_bracket_groups_follow_the_bracket_rule_and_mv_is_noise() {
        let lemon = g("【MV】米津玄師 - Lemon", Some("米津玄師"));
        assert_eq!(lemon.tier, GuessTier::Split);
        assert_eq!((lemon.artist.as_str(), lemon.title.as_str()), ("米津玄師", "Lemon"));
        assert_eq!(lemon.fragments, [noise("【MV】"), sep("-")]);
        assert_eq!(g("Lemon【公式MV】", None).title, "Lemon");
        assert_eq!(g("Lemon【初音ミク】", None).title, "Lemon【初音ミク】");
        assert_eq!(g("Song (MV)", None).title, "Song");
        assert_eq!(g("Song [M/V]", None).title, "Song");
        assert_eq!(g("Song - Official MV", None).title, "Song");
        assert_eq!(g("Mvula (Live)", None).title, "Mvula (Live)", "MV is a word");
    }

    #[test]
    fn a_trailing_credit_becomes_feat_and_anywhere_else_is_left_alone() {
        assert_eq!(g("Get Lucky feat. Pharrell Williams", None).title, "Get Lucky (feat. Pharrell Williams)");
        assert_eq!(g("Get Lucky ft. Pharrell Williams", None).title, "Get Lucky (feat. Pharrell Williams)");
        assert_eq!(g("Get Lucky Featuring Pharrell Williams, Nile Rodgers", None).title, "Get Lucky (feat. Pharrell Williams, Nile Rodgers)");
        let lucky = g("Daft Punk - Get Lucky ft. Pharrell Williams (Official Audio)", Some("Daft Punk"));
        assert_eq!((lucky.artist.as_str(), lucky.title.as_str()), ("Daft Punk", "Get Lucky (feat. Pharrell Williams)"));
        assert_eq!(g("Get Lucky (feat. Pharrell)", None).title, "Get Lucky (feat. Pharrell)");
        assert_eq!(g("Get Lucky feat. Pharrell (Radio Edit)", None).title, "Get Lucky feat. Pharrell (Radio Edit)");
        let left = g("Daft Punk ft. Pharrell - Get Lucky", None);
        assert_eq!((left.artist.as_str(), left.title.as_str()), ("Daft Punk ft. Pharrell", "Get Lucky"));
        assert_eq!(g("6 ft under", None).title, "6 ft under", "\"ft\" needs its dot");
        assert_eq!(g("A Feat of Strength", None).title, "A Feat of Strength");
        assert_eq!(g("Get Lucky feat Pharrell", None).title, "Get Lucky feat Pharrell");
        assert_eq!(g("Feat. Nobody", None).title, "Feat. Nobody");
    }

    #[test]
    fn putting_fragments_back_restores_each_in_place() {
        let gorillaz = g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("Gorillaz"));
        assert_eq!(gorillaz.title_with(&set(&[1])), "Feel Good Inc. (Official Video)");
        assert_eq!(gorillaz.title_with(&set(&[2])), "Feel Good Inc. [HD]");
        assert_eq!(gorillaz.title_with(&set(&[1, 2])), "Feel Good Inc. (Official Video) [HD]");
        assert_eq!(gorillaz.title_with(&set(&[0])), "Gorillaz - Feel Good Inc.");
        assert_eq!(gorillaz.artist_with(&set(&[0])), "Gorillaz");
        assert_eq!(gorillaz.artist_with(&set(&[])), "Gorillaz");
        assert_eq!(gorillaz.title_with(&all(&gorillaz)), "Gorillaz - Feel Good Inc. (Official Video) [HD]");
        assert_eq!(gorillaz.title_with(&set(&[7])), "Feel Good Inc.", "indexes that are no fragment change nothing");
        let kino = g("Кино – Группа крови", Some("Kino Official"));
        assert_eq!(kino.artist_with(&set(&[])), "Кино");
        assert_eq!(kino.artist_with(&set(&[0])), "Kino", "YouTube's artist, stripped");
        assert_eq!(kino.title_with(&set(&[0])), "Кино – Группа крови");
        let feat = g("Daft Punk - Get Lucky ft. Pharrell", Some("Daft Punk"));
        assert_eq!(feat.title, "Get Lucky (feat. Pharrell)");
        assert_eq!(feat.title_with(&all(&feat)), "Daft Punk - Get Lucky ft. Pharrell");
        let mv = g("【MV】米津玄師 - Lemon", Some("米津玄師"));
        assert_eq!(mv.artist_with(&set(&[0])), "【MV】米津玄師", "a fragment on the artist side goes back into the artist");
        assert_eq!(mv.title_with(&set(&[0])), "Lemon");
        assert_eq!(mv.title_with(&set(&[1])), "米津玄師 - Lemon");
        let music = guess(Lookup { title: Some("One More Time"), album: Some("Discovery"), ..Lookup::default() });
        assert_eq!(music.title_with(&set(&[0])), "One More Time");
        assert_eq!(g("Sweater Weather (Official Video)", Some("TheNeighbourhoodVEVO")).artist_with(&set(&[0])), "TheNeighbourhood");
    }

    #[test]
    fn a_trailing_official_video_is_one_fragment_even_around_a_group() {
        let song = g("Song - Official Video (HD)", None);
        assert_eq!(song.title, "Song");
        assert_eq!(song.fragments, [noise("- Official Video"), noise("(HD)")]);
        assert_eq!(song.title_with(&set(&[0])), "Song - Official Video");
        assert_eq!(song.title_with(&set(&[1])), "Song (HD)");
        let around = g("Song - (HD) Official Video", None);
        assert_eq!(around.title, "Song");
        assert_eq!(around.title_with(&set(&[1])), "Song (HD)");
        assert_eq!(around.title_with(&all(&around)), "Song - (HD) Official Video");
    }

    #[test]
    fn two_readings_of_one_answer_are_equal_and_a_different_one_is_not() {
        let read = || g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("GorillazVEVO"));
        assert_eq!(read(), read());
        assert_ne!(read(), g("Gorillaz - Feel Good Inc. (Official Video)", Some("GorillazVEVO")));
        assert_ne!(read(), g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("Gorillaz")));
    }

    fn state(restored: &[usize], swapped: bool, strip_restored: bool) -> GuessState {
        GuessState { restored: set(restored), swapped, strip_restored }
    }

    #[test]
    fn the_fields_follow_restores_the_strip_and_the_swap() {
        let gorillaz = g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("GorillazVEVO"));
        let vevo = g("Sweater Weather (Official Video)", Some("TheNeighbourhoodVEVO"));
        let music = guess(Lookup { title: Some("One More Time"), artist: Some("Daft Punk"), album: Some("Discovery"), year: Some("2001") });
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(gorillaz.fields(&GuessState::default()), pair("Feel Good Inc.", "Gorillaz"));
        assert_eq!(gorillaz.fields(&state(&[0], false, false)), pair("Gorillaz - Feel Good Inc.", "Gorillaz"), "the split undone: YouTube's artist");
        assert_eq!(gorillaz.fields(&state(&[0], false, true)), pair("Gorillaz - Feel Good Inc.", "GorillazVEVO"));
        assert_eq!(gorillaz.fields(&state(&[], true, false)), pair("Gorillaz", "Feel Good Inc."));
        assert_eq!(vevo.fields(&state(&[], false, true)), pair("Sweater Weather", "TheNeighbourhoodVEVO"));
        assert_eq!(music.fields(&GuessState::default()), pair("One More Time", "Daft Punk"));
        assert_eq!(Guess::none().fields(&GuessState::default()), pair("", ""));
    }

    #[test]
    fn the_view_offers_chips_undo_the_swap_and_the_strip() {
        let gorillaz = g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("GorillazVEVO"));
        let v = gorillaz.view(&GuessState::default(), false);
        assert_eq!(v.chips, [(1, noise("(Official Video)")), (2, noise("[HD]")), (0, sep("-"))]);
        assert!(v.split_out && v.can_swap && v.can_undo_all && v.cleaned);
        assert!(!v.can_restore_strip, "the strip shows once the split is undone");
        assert_eq!(v.or_youtube_artist, None, "the split agrees with YouTube");
        assert!(gorillaz.view(&state(&[], true, false), false).chips.is_empty(), "none while swapped");
        let undone = gorillaz.view(&state(&[0], false, false), false);
        assert!(undone.can_restore_strip && !undone.can_swap);
        assert!(!gorillaz.view(&state(&[0], false, false), true).can_restore_strip, "not over an edited artist");
        let everything = gorillaz.view(&state(&[0, 1, 2], false, false), false);
        assert!(!everything.can_undo_all && !everything.cleaned && !everything.noise_out);
        let kino = g("Кино – Группа крови", Some("Kino Official"));
        assert!(!kino.view(&GuessState::default(), false).cleaned, "a split alone is no clean-up");
        assert_eq!(kino.view(&GuessState::default(), false).or_youtube_artist.as_deref(), Some("Kino"));
        assert_eq!(kino.view(&GuessState::default(), true).or_youtube_artist, None);
        let vevo = g("Sweater Weather (Official Video)", Some("TheNeighbourhoodVEVO"));
        assert!(vevo.view(&GuessState::default(), false).can_restore_strip);
        assert!(!vevo.view(&state(&[], false, true), false).can_restore_strip);
        let cleaned = gorillaz.cleaned_label();
        assert_eq!((cleaned.key, cleaned.param("title")), ("gui.ytdl.source.cleaned", Some("Gorillaz - Feel Good Inc. (Official Video) [HD]")));
    }

    #[test]
    fn the_source_line_is_youtube_music_or_check_it_and_nothing_for_the_split() {
        let gorillaz = g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("GorillazVEVO"));
        let music = guess(Lookup { title: Some("One More Time"), artist: Some("Daft Punk"), album: Some("Discovery"), year: None });
        assert_eq!(music.source(&GuessState::default()), Source::Music);
        assert_eq!(Source::Music.line().map(|(t, l)| (t, l.key)), Some((Tone::Ok, "gui.ytdl.source.music")));
        assert_eq!(gorillaz.source(&GuessState::default()), Source::VideoTitle { agrees: true });
        assert_eq!(gorillaz.source(&state(&[], true, false)), Source::VideoTitle { agrees: true }, "swapping keeps the split");
        assert_eq!(g("Кино – Группа крови", Some("Kino Official")).source(&GuessState::default()), Source::VideoTitle { agrees: false });
        assert_eq!(Source::VideoTitle { agrees: true }.line(), None);
        assert_eq!(gorillaz.source(&state(&[0], false, false)), Source::Channel);
        assert_eq!(g("Sweater Weather (Official Video)", Some("TheNeighbourhoodVEVO")).source(&GuessState::default()), Source::Channel);
        assert_eq!(Source::Channel.line().map(|(t, l)| (t, l.key)), Some((Tone::Warn, "gui.ytdl.source.channel")));
        assert_eq!(Guess::none().source(&GuessState::default()), Source::None);
        assert_eq!(Source::None.line(), None);
    }

    #[test]
    fn the_focused_fields_help_line() {
        let gorillaz = g("Gorillaz - Feel Good Inc. (Official Video) [HD]", Some("GorillazVEVO"));
        let vevo = g("Sweater Weather (Official Video)", Some("TheNeighbourhoodVEVO"));
        let music = guess(Lookup { title: Some("One More Time"), artist: Some("Daft Punk"), album: Some("Discovery"), year: Some("2001") });
        let none = GuessState::default();
        let help = |field, guess: &Guess, state: &GuessState, edited, value, album| field_help(field, guess, state, edited, value, album);
        let key = |l: Label| l.key;
        use TagField::*;
        assert_eq!(key(help(Title, &music, &none, false, "One More Time", None)), "gui.ytdl.help.music");
        assert_eq!(key(help(Title, &gorillaz, &none, false, "Feel Good Inc.", None)), "gui.ytdl.help.video_title_cleaned");
        assert_eq!(key(help(Title, &gorillaz, &state(&[1, 2], false, false), false, "x", None)), "gui.ytdl.help.video_title");
        assert_eq!(key(help(Title, &gorillaz, &state(&[], true, false), false, "x", None)), "gui.ytdl.help.video_title");
        assert_eq!(key(help(Title, &gorillaz, &none, true, "Mine", None)), "gui.ytdl.help.edited");
        assert_eq!(key(help(Title, &gorillaz, &none, true, " ", None)), "gui.ytdl.blank.title", "emptied: the blank line wins");
        assert_eq!(key(help(Artist, &music, &none, false, "Daft Punk", None)), "gui.ytdl.help.music");
        assert_eq!(key(help(Artist, &gorillaz, &none, false, "Gorillaz", None)), "gui.ytdl.help.video_title");
        assert_eq!(key(help(Artist, &gorillaz, &state(&[], true, false), false, "x", None)), "gui.ytdl.help.video_title_cleaned");
        let stripped = help(Artist, &gorillaz, &state(&[0], false, false), false, "Gorillaz", None);
        assert_eq!((stripped.key, stripped.param("removed")), ("gui.ytdl.help.youtube_artist_stripped", Some("VEVO")));
        assert_eq!(key(help(Artist, &gorillaz, &state(&[0], false, true), false, "x", None)), "gui.ytdl.help.youtube_artist");
        assert_eq!(key(help(Artist, &g("Song", Some("Some Uploader")), &none, false, "x", None)), "gui.ytdl.help.youtube_artist_check");
        assert_eq!(key(help(Artist, &music, &none, false, "", None)), "gui.ytdl.blank.artist_music");
        let guessed = help(Artist, &vevo, &none, false, "", None);
        assert_eq!((guessed.key, guessed.param("name")), ("gui.ytdl.blank.artist_guess", Some("TheNeighbourhoodVEVO")));
        assert_eq!(key(help(Artist, &g("Song", None), &none, false, "", None)), "gui.ytdl.blank.artist_guess_unnamed");
        assert_eq!(key(help(Album, &music, &none, false, "Discovery", None)), "gui.ytdl.help.music");
        assert_eq!(key(help(Album, &music, &none, false, "", None)), "gui.ytdl.blank.album_keeps");
        assert_eq!(key(help(Album, &vevo, &none, false, "", Some("Wiped Out!"))), "gui.ytdl.blank.album_keeps");
        assert_eq!(key(help(Album, &vevo, &none, false, "", None)), "gui.ytdl.blank.album_single");
        assert_eq!(key(help(Year, &music, &none, false, "2001", None)), "gui.ytdl.help.music");
        assert_eq!(key(help(Year, &vevo, &none, false, "2013", None)), "gui.ytdl.help.video_release");
        assert_eq!(key(help(Year, &vevo, &none, false, "", None)), "gui.ytdl.blank.year");
    }

    #[test]
    fn the_year_check_the_layout_tags_and_the_request_tags() {
        for y in ["", "  ", "2001", " 2001 "] {
            assert!(!year_invalid(y), "{y:?}");
        }
        for y in ["01", "20011", "2001-03", "MMI", "٢٠٠١"] {
            assert!(year_invalid(y), "{y}");
        }
        let tags = Tags { title: "x".into(), artist: " Daft Punk ".into(), album: "Discovery ".into(), year: "2001".into() };
        assert_eq!(tags.song(), SongTags { artist: "Daft Punk".into(), album: "Discovery".into(), year: "2001".into() });
        assert_eq!(Tags::default().song(), SongTags::default());
        let full = Tags { title: " One More Time ".into(), artist: "Daft Punk".into(), album: "Discovery".into(), year: "2001".into() };
        let owned = |pairs: &[(&'static str, &str)]| pairs.iter().map(|(k, v)| (*k, v.to_string())).collect::<Vec<_>>();
        assert_eq!(full.request(), owned(&[("title", "One More Time"), ("artist", "Daft Punk"), ("album", "Discovery"), ("year", "2001")]));
        let some = Tags { title: "".into(), artist: "Daft Punk".into(), album: "  ".into(), year: "01".into() };
        assert_eq!(some.request(), owned(&[("artist", "Daft Punk")]));
        let none = Tags { title: " ".into(), year: "MMI".into(), ..Tags::default() };
        assert!(none.request().is_empty(), "no metadata key when none is left");
    }
}
