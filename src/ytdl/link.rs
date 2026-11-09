//! Links: the first YouTube link in what was pasted, and the one URL the
//! server gets for it.
//!
//! The server's /api/v1/ytdl routes take a URL whose host is youtube.com, a
//! subdomain of it, or youtu.be, then keep its scheme, host and path with
//! only the `v` parameter (mStream `src/api/ytdl.js` sanitizeYoutubeUrl).
//! A youtu.be, /shorts/, /live/ or /embed/ link has no `v`, so the server
//! answers 400 "missing video ID": those are rewritten to a watch link
//! first. The URL sent is one sanitizeYoutubeUrl leaves as it is, so a
//! job's `url` and a download record's `origin` match it character for
//! character. The parse is WHATWG's, as the server's `new URL()` is: the
//! `url` crate implements the same standard.

use url::Url;

use super::{Label, Tier, Tone, js_space};

/// What was pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkKind {
    /// Nothing, or only blanks.
    Empty,
    /// No YouTube link in it, or a YouTube page that names no video.
    NotYoutube,
    /// A playlist with no video of its own.
    Playlist,
    /// One video: [`Link::url`] is what the server gets.
    Video,
}

/// A pasted link, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub kind: LinkKind,
    /// `<scheme>://<host>/watch?v=<id>`: the host lower-cased, a default
    /// port dropped, every other parameter and the fragment gone. Video
    /// only.
    pub url: Option<String>,
    pub video_id: Option<String>,
    /// youtu.be, /shorts/, /live/, /embed/ or youtube-nocookie.com,
    /// rewritten to `https://www.youtube.com/watch?v=<id>`.
    pub short_form: bool,
    /// Typed without `https://`.
    pub scheme_added: bool,
    /// A /live/ link.
    pub live: bool,
    /// The link named a playlist too (`list=`): just the video goes.
    pub in_playlist: bool,
    /// For a helper line: the URL minus its scheme and a leading `www.`.
    pub shown: String,
}

impl Link {
    fn of(kind: LinkKind) -> Link {
        Link {
            kind,
            url: None,
            video_id: None,
            short_form: false,
            scheme_added: false,
            live: false,
            in_playlist: false,
            shown: String::new(),
        }
    }

    fn video(url: String, id: &str, short_form: bool, scheme_added: bool, live: bool, in_playlist: bool) -> Link {
        let bare = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
        let shown = bare.strip_prefix("www.").unwrap_or(bare).to_string();
        Link {
            kind: LinkKind::Video,
            url: Some(url),
            video_id: Some(id.to_string()),
            short_form,
            scheme_added,
            live,
            in_playlist,
            shown,
        }
    }
}

/// The hosts a candidate may end in (any subdomain of each).
const DOMAINS: [&str; 3] = ["youtube.com", "youtu.be", "youtube-nocookie.com"];

/// A character a link may not follow (the phone's lookbehind
/// `(?<![\w.@/-])`, ASCII `\w` as in JS): `notyoutube.com`, `me@youtube.com`
/// and `//youtube.com` are not links to YouTube.
fn not_before(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '/' | '-')
}

fn label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// A path character of a copied link: printable ASCII but `"`, `<` and `>`,
/// so a link run straight into text ("…/dQw4w9WgXcQ。看看") ends where the
/// text starts.
fn path_char(c: char) -> bool {
    ('!'..='~').contains(&c) && !matches!(c, '"' | '<' | '>')
}

/// Punctuation a link in a sentence ends with that is not part of it.
fn trailing_punctuation(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '>' | '\'' | '"' | '»' | '”' | '’')
}

fn starts_with_ignore_case(chars: &[char], at: usize, word: &str) -> bool {
    let n = word.chars().count();
    at + n <= chars.len() && chars[at..at + n].iter().zip(word.chars()).all(|(a, b)| a.eq_ignore_ascii_case(&b))
}

/// Where a host starting at `start` ends: subdomain labels, then one of
/// [`DOMAINS`], not followed by a label character or by a dot and one — the
/// records' `(?:[a-z0-9-]+\.)*(?:youtube\.com|…)(?![a-z0-9-]|\.[a-z0-9])`,
/// tried in the order a backtracking engine tries it (the most labels
/// first, then each domain).
fn host_end(chars: &[char], start: usize) -> Option<usize> {
    let mut label_ends = vec![start];
    let mut at = start;
    loop {
        let mut end = at;
        while end < chars.len() && label_char(chars[end]) {
            end += 1;
        }
        if end == at || chars.get(end) != Some(&'.') {
            break;
        }
        at = end + 1;
        label_ends.push(at);
    }
    let free = |p: usize| match chars.get(p) {
        Some(c) if label_char(*c) => false,
        Some('.') => !chars.get(p + 1).is_some_and(|c| c.is_ascii_alphanumeric()),
        _ => true,
    };
    for &from in label_ends.iter().rev() {
        for domain in DOMAINS {
            let end = from + domain.len();
            if starts_with_ignore_case(chars, from, domain) && free(end) {
                return Some(end);
            }
        }
    }
    None
}

/// The first YouTube link in `chars`: its span, and whether it carried its
/// own scheme. The records' pattern, by hand — the `regex` crate has no
/// lookaround, and the host's lookahead steers the match.
fn first_candidate(chars: &[char]) -> Option<(usize, usize, bool)> {
    for start in 0..chars.len() {
        if start > 0 && not_before(chars[start - 1]) {
            continue;
        }
        let scheme = ["https://", "http://"].into_iter().find(|s| starts_with_ignore_case(chars, start, s));
        let host_start = start + scheme.map_or(0, str::len);
        let Some(mut end) = host_end(chars, host_start) else { continue };
        if chars.get(end) == Some(&':') && chars.get(end + 1).is_some_and(char::is_ascii_digit) {
            end += 1;
            while chars.get(end).is_some_and(char::is_ascii_digit) {
                end += 1;
            }
        }
        if chars.get(end).is_some_and(|c| matches!(c, '/' | '?' | '#')) {
            end += 1;
            while chars.get(end).is_some_and(|c| path_char(*c)) {
                end += 1;
            }
        }
        return Some((start, end, scheme.is_some()));
    }
    None
}

/// A video id: six or more of A–Z a–z 0–9 `_` `-`.
fn video_id(id: &str) -> bool {
    id.len() >= 6 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `decodeURIComponent`, or the segment as it is where that would throw (a
/// stray `%`, bytes that are not UTF-8).
fn decode_segment(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = segment.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok());
            let Some(byte) = hex else { return segment.to_string() };
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| segment.to_string())
}

/// The first YouTube link in `text`, with or without a scheme. youtu.be,
/// /shorts/, /live/, /embed/ and youtube-nocookie.com are rewritten to
/// `https://www.youtube.com/watch?v=<id>`; music.youtube.com keeps its host
/// and http stays http.
pub(crate) fn parse_link(text: &str) -> Link {
    if text.chars().all(js_space) {
        return Link::of(LinkKind::Empty);
    }
    let chars: Vec<char> = text.chars().collect();
    let Some((start, mut end, has_scheme)) = first_candidate(&chars) else { return Link::of(LinkKind::NotYoutube) };
    while end > start && trailing_punctuation(chars[end - 1]) {
        end -= 1;
    }
    let raw: String = chars[start..end].iter().collect();
    let scheme_added = !has_scheme;
    let parsed = if scheme_added { Url::parse(&format!("https://{raw}")) } else { Url::parse(&raw) };
    let Ok(uri) = parsed else { return Link::of(LinkKind::NotYoutube) };
    let host = uri.host_str().unwrap_or_default().to_string();
    let segments: Vec<String> = uri.path().split('/').filter(|s| !s.is_empty()).map(decode_segment).collect();
    let query = |name: &str| uri.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    let v = query("v");
    let in_playlist = query("list").is_some_and(|l| !l.is_empty());

    let rewritten = |id: Option<&str>, live: bool| match id {
        Some(id) if video_id(id) => {
            Link::video(format!("https://www.youtube.com/watch?v={id}"), id, true, scheme_added, live, in_playlist)
        }
        _ => Link::of(LinkKind::NotYoutube),
    };
    let playlist_or_nothing = || Link::of(if in_playlist { LinkKind::Playlist } else { LinkKind::NotYoutube });

    let first = segments.first().map_or("", String::as_str);
    let second = segments.get(1).map(String::as_str);
    if host == "youtu.be" || host == "www.youtu.be" {
        return rewritten(segments.first().map(String::as_str), false);
    }
    // An embedded playlist names no video: /embed/videoseries?list=…, on
    // either host. (The phone checks this after the nocookie host, so its
    // privacy-mode playlist embed reads as a video with the id
    // "videoseries"; the webapp and this read it as the playlist it is.)
    if first == "embed" && second == Some("videoseries") {
        return playlist_or_nothing();
    }
    if host == "youtube-nocookie.com" || host.ends_with(".youtube-nocookie.com") {
        // Only embeds live there; the server refuses the host, so even a
        // watch link on it is sent as www.youtube.com.
        let id = match first {
            "embed" => second,
            "watch" => v.as_deref(),
            _ => None,
        };
        return rewritten(id, false);
    }
    match first {
        "shorts" | "embed" => rewritten(second, false),
        "live" => rewritten(second, true),
        "playlist" => playlist_or_nothing(),
        "watch" if segments.len() == 1 => match v {
            None => playlist_or_nothing(),
            Some(id) if !video_id(&id) => Link::of(LinkKind::NotYoutube),
            Some(id) => {
                // Scheme and host kept, a port only when it is not the
                // scheme's own.
                let port = uri.port().map(|p| format!(":{p}")).unwrap_or_default();
                let url = format!("{}://{host}{port}/watch?v={id}", uri.scheme());
                Link::video(url, &id, false, scheme_added, false, in_playlist)
            }
        },
        _ => Link::of(LinkKind::NotYoutube),
    }
}

/// A lookup answer for a stream (the server's isLiveEntry): live now,
/// upcoming, or just ended and not yet processed. Only a server with the
/// new job list sends these.
pub(crate) fn live_meta(is_live: bool, live_status: Option<&str>) -> bool {
    is_live || matches!(live_status, Some("is_live" | "is_upcoming" | "post_live"))
}

/// The lines under the link field, in order: a refusal (not YouTube, a
/// playlist), "Just this video, not the playlist." and the live-stream
/// warning — a /live/ link, or a lookup that says the video is a stream.
/// A server before 6.30 doesn't refuse a stream, so there the warning is
/// the phone's hedge. A rewritten link is just sent: no "Sent as …".
pub(crate) fn link_notes(link: &Link, live_meta: bool, tier: Tier) -> Vec<(Tone, Label)> {
    match link.kind {
        LinkKind::NotYoutube => return vec![(Tone::Refusal, Label::new("gui.ytdl.link.not_youtube"))],
        LinkKind::Playlist => return vec![(Tone::Refusal, Label::new("gui.ytdl.link.playlist"))],
        LinkKind::Empty => return Vec::new(),
        LinkKind::Video => {}
    }
    let mut out = Vec::new();
    if link.in_playlist {
        out.push((Tone::Note, Label::new("gui.ytdl.link.in_playlist")));
    }
    if link.live || live_meta {
        let key = if tier == Tier::Older { "gui.ytdl.link.live_older" } else { "gui.ytdl.link.live" };
        out.push((Tone::Warn, Label::new(key)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server's own reading of a URL it takes (`sanitizeYoutubeUrl`
    /// behind the route's host rule): the host youtube.com, a subdomain of
    /// it, or youtu.be; only `v` kept.
    fn server_keeps(url: &str) -> Option<String> {
        let mut parsed = Url::parse(url).ok()?;
        let host = parsed.host_str()?.to_string();
        if !(host == "youtube.com" || host.ends_with(".youtube.com") || host == "youtu.be") {
            return None;
        }
        let v = parsed.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned())?;
        if v.is_empty() {
            return None;
        }
        parsed.set_query(None);
        parsed.set_fragment(None);
        parsed.query_pairs_mut().append_pair("v", &v);
        Some(parsed.to_string())
    }

    #[derive(Default)]
    struct Flags {
        short_form: bool,
        scheme_added: bool,
        live: bool,
        in_playlist: bool,
    }

    const SHORT: Flags = Flags { short_form: true, scheme_added: false, live: false, in_playlist: false };
    const PLAIN: Flags = Flags { short_form: false, scheme_added: false, live: false, in_playlist: false };

    fn expect_video(input: &str, url: &str, id: &str, flags: Flags) {
        let link = parse_link(input);
        assert_eq!(link.kind, LinkKind::Video, "{input}");
        assert_eq!(link.url.as_deref(), Some(url), "{input}");
        assert_eq!(link.video_id.as_deref(), Some(id), "{input}");
        assert_eq!(link.short_form, flags.short_form, "{input} short_form");
        assert_eq!(link.scheme_added, flags.scheme_added, "{input} scheme_added");
        assert_eq!(link.live, flags.live, "{input} live");
        assert_eq!(link.in_playlist, flags.in_playlist, "{input} in_playlist");
        assert_eq!(server_keeps(url).as_deref(), Some(url), "{url} is what the server records");
    }

    #[test]
    fn empty_and_whitespace_are_empty() {
        for input in ["", "   \n\t"] {
            let link = parse_link(input);
            assert_eq!(link.kind, LinkKind::Empty);
            assert_eq!(link.url, None);
            assert_eq!(link.shown, "");
        }
    }

    #[test]
    fn another_site_or_a_look_alike_is_not_youtube() {
        for input in [
            "https://vimeo.com/76979871",
            "just some words",
            "https://notyoutube.com/watch?v=FGBhQbmPwH8",
            "https://youtube.com.evil.example/watch?v=FGBhQbmPwH8",
            "https://evil.example/youtube.com/watch?v=FGBhQbmPwH8",
            "https://youtube.co/watch?v=FGBhQbmPwH8",
            "me@youtube.com/watch?v=FGBhQbmPwH8",
        ] {
            let link = parse_link(input);
            assert_eq!(link.kind, LinkKind::NotYoutube, "{input}");
            assert_eq!(link.url, None);
            assert_eq!(link.video_id, None);
        }
    }

    #[test]
    fn a_youtube_page_that_names_no_video_is_not_a_link() {
        for input in ["https://www.youtube.com/", "https://www.youtube.com/@daftpunk", "https://www.youtube.com/channel/UCx", "https://youtu.be/", "youtube.com"] {
            assert_eq!(parse_link(input).kind, LinkKind::NotYoutube, "{input}");
        }
    }

    #[test]
    fn a_video_id_is_six_or_more_of_its_alphabet() {
        assert_eq!(parse_link("https://www.youtube.com/watch?v=abc").kind, LinkKind::NotYoutube);
        assert_eq!(parse_link("https://www.youtube.com/watch?v=FGBh%20QbmPwH8").kind, LinkKind::NotYoutube);
        assert_eq!(parse_link("https://youtu.be/ab*cdef").kind, LinkKind::NotYoutube);
        assert_eq!(parse_link("https://www.youtube.com/watch?v=").kind, LinkKind::NotYoutube);
        assert_eq!(parse_link("https://youtube.com:99999/watch?v=FGBhQbmPwH8").kind, LinkKind::NotYoutube, "a port the parse refuses");
        expect_video("https://www.youtube.com/watch?v=abc_-1", "https://www.youtube.com/watch?v=abc_-1", "abc_-1", PLAIN);
    }

    #[test]
    fn a_playlist_with_no_video_is_a_playlist() {
        let link = parse_link("https://www.youtube.com/playlist?list=PLx0sYbCqOb8TBPRdmBHs5Iftvv9TPboYG");
        assert_eq!(link.kind, LinkKind::Playlist);
        assert_eq!(link.url, None);
        assert_eq!(link.shown, "");
        assert_eq!(parse_link("https://www.youtube.com/watch?list=PLx0sYbCqOb8T").kind, LinkKind::Playlist);
        assert_eq!(parse_link("https://music.youtube.com/playlist?list=OLAK5uy_k").kind, LinkKind::Playlist);
        assert_eq!(parse_link("https://www.youtube.com/playlist").kind, LinkKind::NotYoutube);
        assert_eq!(parse_link("https://www.youtube.com/embed/videoseries?list=PLx0sYbCqOb8T").kind, LinkKind::Playlist);
        assert_eq!(parse_link("https://www.youtube.com/embed/videoseries").kind, LinkKind::NotYoutube);
        assert_eq!(parse_link("https://www.youtube-nocookie.com/embed/videoseries").kind, LinkKind::NotYoutube);
    }

    #[test]
    fn a_watch_link_keeps_scheme_host_and_its_own_port_and_only_v() {
        let w = "https://www.youtube.com/watch?v=FGBhQbmPwH8";
        expect_video(w, w, "FGBhQbmPwH8", PLAIN);
        expect_video("https://music.youtube.com/watch?v=FGBhQbmPwH8&si=Rk3vT0", "https://music.youtube.com/watch?v=FGBhQbmPwH8", "FGBhQbmPwH8", PLAIN);
        expect_video("https://m.youtube.com/watch?v=HyHNuVaZJ-k", "https://m.youtube.com/watch?v=HyHNuVaZJ-k", "HyHNuVaZJ-k", PLAIN);
        expect_video("http://www.youtube.com/watch?v=FGBhQbmPwH8", "http://www.youtube.com/watch?v=FGBhQbmPwH8", "FGBhQbmPwH8", PLAIN);
        expect_video("https://WWW.YouTube.com:443/watch?v=FGBhQbmPwH8&t=42s#frag", w, "FGBhQbmPwH8", PLAIN);
        expect_video("http://youtube.com:80/watch?v=FGBhQbmPwH8", "http://youtube.com/watch?v=FGBhQbmPwH8", "FGBhQbmPwH8", PLAIN);
        expect_video("https://youtube.com:8443/watch?v=FGBhQbmPwH8", "https://youtube.com:8443/watch?v=FGBhQbmPwH8", "FGBhQbmPwH8", PLAIN);
        expect_video("https://www.youtube.com/watch?feature=share&v=FGBhQbmPwH8&t=1", w, "FGBhQbmPwH8", PLAIN);
        expect_video("https://www.youtube.com/watch?v=FGBhQbmPwH8&v=other12345", w, "FGBhQbmPwH8", PLAIN);
        expect_video("https://www.youtube.com/watch/?v=HyHNuVaZJ-k", "https://www.youtube.com/watch?v=HyHNuVaZJ-k", "HyHNuVaZJ-k", PLAIN);
    }

    #[test]
    fn a_watch_link_the_server_takes_as_typed_comes_out_as_the_server_makes_it() {
        for input in [
            "https://www.youtube.com/watch?v=FGBhQbmPwH8",
            "https://music.youtube.com/watch?v=FGBhQbmPwH8&si=Rk3vT0",
            "http://www.youtube.com/watch?v=FGBhQbmPwH8",
            "http://youtube.com:80/watch?v=FGBhQbmPwH8",
            "https://youtube.com:8443/watch?v=FGBhQbmPwH8",
            "https://www.youtube.com/watch?feature=share&v=FGBhQbmPwH8&list=PL123",
            "https://www.youtube.com/watch?v=FGBhQbmPwH8&v=other12345",
            "https://m.youtube.com/watch?v=HyHNuVaZJ-k",
        ] {
            assert_eq!(parse_link(input).url, server_keeps(input), "{input}");
        }
    }

    #[test]
    fn a_video_in_a_playlist_is_just_the_video() {
        let w = "https://www.youtube.com/watch?v=FGBhQbmPwH8";
        expect_video("https://www.youtube.com/watch?v=FGBhQbmPwH8&list=OLAK5uy_kVQ2", w, "FGBhQbmPwH8", Flags { in_playlist: true, ..Flags::default() });
        expect_video("https://youtu.be/FGBhQbmPwH8?list=OLAK5uy_kVQ2", w, "FGBhQbmPwH8", Flags { short_form: true, in_playlist: true, ..Flags::default() });
    }

    #[test]
    fn short_forms_are_rewritten_to_a_watch_link() {
        let w = "https://www.youtube.com/watch?v=FGBhQbmPwH8";
        expect_video("https://youtu.be/FGBhQbmPwH8?si=Rk3vT0", w, "FGBhQbmPwH8", SHORT);
        assert_eq!(parse_link("https://youtu.be/FGBhQbmPwH8?si=Rk3vT0").shown, "youtube.com/watch?v=FGBhQbmPwH8");
        expect_video("https://youtu.be/HyHNuVaZJ-k?si=a81Bq", "https://www.youtube.com/watch?v=HyHNuVaZJ-k", "HyHNuVaZJ-k", SHORT);
        expect_video("http://youtu.be/kN0gRuPpa01", "https://www.youtube.com/watch?v=kN0gRuPpa01", "kN0gRuPpa01", SHORT);
        expect_video("https://www.youtube.com/shorts/FGBhQbmPwH8?feature=share", w, "FGBhQbmPwH8", SHORT);
        expect_video("https://m.youtube.com/shorts/FGBhQbmPwH8", w, "FGBhQbmPwH8", SHORT);
        let live = "https://www.youtube.com/watch?v=jfKfPfyJRdk";
        expect_video("https://www.youtube.com/live/jfKfPfyJRdk", live, "jfKfPfyJRdk", Flags { short_form: true, live: true, ..Flags::default() });
        expect_video("https://www.youtube.com/live/jfKfPfyJRdk?si=x", live, "jfKfPfyJRdk", Flags { short_form: true, live: true, ..Flags::default() });
        expect_video("https://www.youtube.com/embed/FGBhQbmPwH8?start=10", w, "FGBhQbmPwH8", SHORT);
        expect_video("https://www.youtube-nocookie.com/embed/FGBhQbmPwH8", w, "FGBhQbmPwH8", SHORT);
        assert_eq!(server_keeps("https://www.youtube-nocookie.com/embed/x?v=abcdef"), None, "the server refuses the nocookie host");
    }

    #[test]
    fn typed_without_https_the_scheme_is_added_and_the_host_lower_cased() {
        let added = Flags { scheme_added: true, ..Flags::default() };
        expect_video("YouTube.com/watch?v=FGBhQbmPwH8", "https://youtube.com/watch?v=FGBhQbmPwH8", "FGBhQbmPwH8", added);
        assert_eq!(parse_link("YouTube.com/watch?v=FGBhQbmPwH8").shown, "youtube.com/watch?v=FGBhQbmPwH8");
        let w = "https://www.youtube.com/watch?v=FGBhQbmPwH8";
        expect_video("youtu.be/FGBhQbmPwH8", w, "FGBhQbmPwH8", Flags { short_form: true, scheme_added: true, ..Flags::default() });
        expect_video("www.youtube.com/watch?v=FGBhQbmPwH8", w, "FGBhQbmPwH8", Flags { scheme_added: true, ..Flags::default() });
    }

    #[test]
    fn a_share_message_finds_the_link_and_leaves_its_punctuation() {
        expect_video("Check this out https://youtu.be/HyHNuVaZJ-k?si=a81Bq. so good", "https://www.youtube.com/watch?v=HyHNuVaZJ-k", "HyHNuVaZJ-k", SHORT);
        let w = "https://www.youtube.com/watch?v=FGBhQbmPwH8";
        expect_video("(https://www.youtube.com/watch?v=FGBhQbmPwH8)", w, "FGBhQbmPwH8", PLAIN);
        expect_video("  https://youtu.be/FGBhQbmPwH8  \n", w, "FGBhQbmPwH8", SHORT);
        for text in ["https://youtu.be/FGBhQbmPwH8。", "看看https://youtu.be/FGBhQbmPwH8，很好听", "「https://youtu.be/FGBhQbmPwH8」", "https://youtu.be/FGBhQbmPwH8”"] {
            expect_video(text, w, "FGBhQbmPwH8", SHORT);
        }
        expect_video("https://vimeo.com/1 then https://youtu.be/FGBhQbmPwH8 and https://youtu.be/HyHNuVaZJ-k", w, "FGBhQbmPwH8", SHORT);
        expect_video("notyoutube.com/watch?v=aaaaaaaa or youtu.be/FGBhQbmPwH8", w, "FGBhQbmPwH8", Flags { short_form: true, scheme_added: true, ..Flags::default() });
    }

    /// The webapp's differential table against the phone's parser
    /// (lib/util/ytdl_link.dart, run on the Dart VM on 2026-11-03): input,
    /// kind, url, the flags set.
    const PHONE_LINKS: &[(&str, LinkKind, Option<&str>, &[&str])] = &[
        ("https://music.youtube.com/watch?v=FGBhQbmPwH8&feature=share", LinkKind::Video, Some("https://music.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("music.youtube.com/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://music.youtube.com/watch?v=FGBhQbmPwH8"), &["schemeAdded"]),
        ("http://music.youtube.com/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("http://music.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("gaming.youtube.com/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://gaming.youtube.com/watch?v=FGBhQbmPwH8"), &["schemeAdded"]),
        ("https://m.youtube.com/watch?app=desktop&v=FGBhQbmPwH8", LinkKind::Video, Some("https://m.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://youtu.be/FGBhQbmPwH8?t=42", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        ("youtu.be/FGBhQbmPwH8?t=1m2s#x", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm", "schemeAdded"]),
        ("https://www.youtu.be/FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        ("https://m.youtu.be/FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("YOUTU.BE/FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm", "schemeAdded"]),
        ("https://youtu.be/FGBhQb", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQb"), &["shortForm"]),
        ("https://youtube.com/shorts/FGBhQbmPwH8?si=x", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        ("https://www.youtube.com/shorts/FGBhQbmPwH8/extra", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        ("https://youtube.com:8080/shorts/FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        ("https://www.youtube.com/shorts/", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com/live/", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com/embed/FGBhQbmPwH8?list=PLx", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm", "inPlaylist"]),
        ("https://www.youtube-nocookie.com/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        // The one difference, on purpose: the phone reads this as a video
        // whose id is "videoseries".
        ("https://www.youtube-nocookie.com/embed/videoseries?list=PLx", LinkKind::Playlist, None, &[]),
        ("https://www.youtube.com/playlist?list=", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com/watch?list=PLx&v=FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["inPlaylist"]),
        ("https://www.youtube.com/watch?v=&list=PLx", LinkKind::NotYoutube, None, &[]),
        ("two: https://www.youtube.com/playlist?list=PLx and https://youtu.be/FGBhQbmPwH8", LinkKind::Playlist, None, &[]),
        ("HTTPS://WWW.YOUTUBE.COM/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://www.YouTube.com/WATCH?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com/watch?V=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        (" https://youtu.be/FGBhQbmPwH8 ", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        ("youtube.com.evil.com/watch?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://youtube.comx/watch?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://youtube.com@evil.com/watch?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://user@youtube.com/watch?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com./watch?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("foo@youtu.be/FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("_youtu.be/FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("ftp://youtube.com/watch?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("//youtube.com/watch?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        // A YouTube link inside another link's query is still the link.
        ("https://evil.com/?u=youtube.com/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://youtube.com/watch?v=FGBhQbmPwH8"), &["schemeAdded"]),
        ("https:youtube.com/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://youtube.com/watch?v=FGBhQbmPwH8"), &["schemeAdded"]),
        ("日本youtu.be/FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm", "schemeAdded"]),
        ("https://youtu.be/FGBhQbmPwH8…", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &["shortForm"]),
        ("https://www.youtube.com:0443/watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://www.youtube.com/watch?v=FGBh%51bmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://www.youtube.com/watch#v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com/watch?v=FGBhQbmPwH8+extra", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com/watch?v=FGBhQbmPwH8'", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("[link](https://www.youtube.com/watch?v=FGBhQbmPwH8)", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://www.youtube.com/watch?v=FGBhQbmPwH8&amp;list=x", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://www.youtube.com/foo/../watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://www.youtube.com//watch?v=FGBhQbmPwH8", LinkKind::Video, Some("https://www.youtube.com/watch?v=FGBhQbmPwH8"), &[]),
        ("https://www.youtube.com/watch/FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://www.youtube.com/v/FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
        ("https://youtube.com/?v=FGBhQbmPwH8", LinkKind::NotYoutube, None, &[]),
    ];

    #[test]
    fn adversarial_links_read_as_the_phones_parser_reads_them() {
        for (input, kind, url, flags) in PHONE_LINKS {
            let link = parse_link(input);
            assert_eq!(link.kind, *kind, "{input}");
            assert_eq!(link.url.as_deref(), *url, "{input}");
            let set: Vec<&str> = [
                ("shortForm", link.short_form),
                ("schemeAdded", link.scheme_added),
                ("live", link.live),
                ("inPlaylist", link.in_playlist),
            ]
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(name, _)| name)
            .collect();
            assert_eq!(set, *flags, "{input}");
            if let Some(url) = url {
                assert_eq!(server_keeps(url).as_deref(), Some(*url), "{url} is what the server records");
            }
        }
    }

    #[test]
    fn shown_is_the_host_minus_www_with_no_scheme() {
        assert_eq!(parse_link("https://www.youtube.com/watch?v=FGBhQbmPwH8").shown, "youtube.com/watch?v=FGBhQbmPwH8");
        assert_eq!(parse_link("https://music.youtube.com/watch?v=FGBhQbmPwH8").shown, "music.youtube.com/watch?v=FGBhQbmPwH8");
        assert_eq!(parse_link("https://youtube.com:8443/watch?v=FGBhQbmPwH8").shown, "youtube.com:8443/watch?v=FGBhQbmPwH8");
        assert_eq!(parse_link("https://www.youtube.com/live/jfKfPfyJRdk").shown, "youtube.com/watch?v=jfKfPfyJRdk");
        assert_eq!(parse_link("https://vimeo.com/76979871").shown, "");
    }

    #[test]
    fn the_lines_under_the_field_a_refusal_the_playlist_note_the_live_warning() {
        let keys = |notes: Vec<(Tone, Label)>| notes.into_iter().map(|(tone, l)| (tone, l.key)).collect::<Vec<_>>();
        let notes = |input: &str, live: bool| keys(link_notes(&parse_link(input), live, Tier::New));
        assert_eq!(notes("https://vimeo.com/1", false), [(Tone::Refusal, "gui.ytdl.link.not_youtube")]);
        assert_eq!(notes("https://www.youtube.com/playlist?list=PLx", false), [(Tone::Refusal, "gui.ytdl.link.playlist")]);
        assert_eq!(notes("", false), []);
        assert_eq!(notes("https://youtu.be/FGBhQbmPwH8", false), [], "a rewritten link is just sent");
        assert_eq!(notes("https://youtu.be/FGBhQbmPwH8?list=PLx", false), [(Tone::Note, "gui.ytdl.link.in_playlist")]);
        assert_eq!(
            notes("https://www.youtube.com/live/jfKfPfyJRdk?list=PLx", false),
            [(Tone::Note, "gui.ytdl.link.in_playlist"), (Tone::Warn, "gui.ytdl.link.live")]
        );
        assert_eq!(notes("https://www.youtube.com/watch?v=jfKfPfyJRdk", true), [(Tone::Warn, "gui.ytdl.link.live")]);
        let older = keys(link_notes(&parse_link("https://www.youtube.com/live/jfKfPfyJRdk"), false, Tier::Older));
        assert_eq!(older, [(Tone::Warn, "gui.ytdl.link.live_older")], "a server before 6.30 doesn't stop streams");
        let full = keys(link_notes(&parse_link("https://www.youtube.com/live/jfKfPfyJRdk"), false, Tier::Full));
        assert_eq!(full, [(Tone::Warn, "gui.ytdl.link.live")]);
    }

    #[test]
    fn a_lookup_reads_a_stream_as_the_server_does() {
        assert!(live_meta(true, None));
        for status in ["is_live", "is_upcoming", "post_live"] {
            assert!(live_meta(false, Some(status)), "{status}");
        }
        assert!(!live_meta(false, Some("was_live")));
        assert!(!live_meta(false, Some("not_live")));
        assert!(!live_meta(false, None));
    }
}
