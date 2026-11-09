//! Where the song lands. The server's /ytdl takes a finished directory and
//! renders nothing, so the client renders the layout with the engine the
//! Discover picker already mirrors ([`crate::api::types::layout_folders`]
//! and its checks, path-template.js): the folders shown are the folders
//! mkdir makes and the add names. Then the YouTube rules on top (the
//! webapp's ytdl-core.js, the phone's `lib/util/ytdl_layout.dart`): four
//! variables, three that are always empty for a YouTube song, the bracket
//! traps an empty one leaves, and "Where new songs land"'s presets, which
//! read the account's Discover destination and never write it.

use std::sync::OnceLock;

use regex::Regex;

use crate::api::types::{
    CollectionDestinationView, DEFAULT_LAYOUT, LAYOUT_VARS, LayoutProblem, LayoutTags, MAX_TEMPLATE_LEN, SERVER_FOLDER,
    TorrentTemplates, base_folders, fold_base, layout_folders, sanitize_segment, validate_layout, validate_resolved_path,
};

use super::{Label, SongTags, path_crumbs};

/// The variables a YouTube layout may name, in the variables row's order.
pub(crate) const YTDL_VARS: [&str; 4] = ["ARTIST", "ALBUMARTIST", "ALBUM", "YEAR"];
/// Always empty for a YouTube song: a layout naming one is refused in
/// Custom, and drops the folder where a library's template names it.
pub(crate) const NEVER_VARS: [&str; 3] = ["GENRE", "PEER", SERVER_FOLDER];
pub(crate) const LAYOUT_ARTIST_ALBUM: &str = DEFAULT_LAYOUT;
pub(crate) const LAYOUT_ARTIST: &str = "{{ALBUMARTIST}}";
pub(crate) const LAYOUT_ALBUM: &str = "{{ALBUM}}";
/// "Top of the library" / "Right here": no folders. Never handed to the
/// engine, which refuses an empty layout.
pub(crate) const LAYOUT_TOP: &str = "";

/// Every variable the engine knows (the six, and `{{SERVERFOLDER}}`).
fn engine_knows(name: &str) -> bool {
    LAYOUT_VARS.contains(&name) || name == SERVER_FOLDER
}

struct Regexes {
    token: Regex,
    only_brackets: Regex,
    empty_pair: Regex,
    dangling_edge: Regex,
}

fn regexes() -> &'static Regexes {
    static CELL: OnceLock<Regexes> = OnceLock::new();
    CELL.get_or_init(|| Regexes {
        token: Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}").expect("the token pattern"),
        only_brackets: Regex::new(r"^[\s()\[\]{}【】\-–—_.,·&+]+$").expect("the brackets pattern"),
        empty_pair: Regex::new(r"[(\[{【]\s*[)\]}】]").expect("the pair pattern"),
        dangling_edge: Regex::new(r"^[-–—_,·&+]|[-–—_,·&+]$").expect("the edge pattern"),
    })
}

/// A layout's `{{NAME}}` tokens: the name upper-cased, and the token as
/// typed.
fn tokens(layout: &str) -> impl Iterator<Item = (String, &str)> {
    regexes().token.captures_iter(layout).map(|m| (m[1].to_ascii_uppercase(), m.get(0).map_or("", |t| t.as_str())))
}

/// Characters a folder name cannot hold on Windows. The engine folds them
/// out of tag values but keeps them in literal text, and the mkdir route
/// only normalises the path, so a Windows server would answer a bare 500.
fn bad_literal(c: char) -> bool {
    matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')
}

/// A layout as typed, checked: the engine's own check first, then the
/// YouTube allow-list, then `: * ? " < > |` in the literal text. `""` is
/// valid here (Top of the library).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LayoutCheck {
    Valid,
    /// A token the engine doesn't know, as typed: `{{ title }}`.
    UnknownVariable(String),
    /// `{{GENRE}}`, `{{PEER}}` or `{{SERVERFOLDER}}`, as typed.
    NeverVariable(String),
    /// The first refused character of the literal text.
    BadChars(char),
    /// The engine's refusal.
    Engine(LayoutProblem),
}

pub(crate) fn layout_check(layout: &str) -> LayoutCheck {
    if layout.is_empty() {
        return LayoutCheck::Valid;
    }
    match validate_layout(layout, &engine_knows) {
        Err(LayoutProblem::UnknownVariable { token }) => return LayoutCheck::UnknownVariable(token),
        Err(problem) => return LayoutCheck::Engine(problem),
        Ok(()) => {}
    }
    if let Some((_, raw)) = tokens(layout).find(|(name, _)| NEVER_VARS.contains(&name.as_str())) {
        return LayoutCheck::NeverVariable(raw.to_string());
    }
    match regexes().token.replace_all(layout, "").chars().find(|c| bad_literal(*c)) {
        Some(c) => LayoutCheck::BadChars(c),
        None => LayoutCheck::Valid,
    }
}

/// Custom's error line, or `None` when the layout is valid.
pub(crate) fn layout_problem(check: &LayoutCheck) -> Option<Label> {
    let vars = YTDL_VARS.join(", ");
    Some(match check {
        LayoutCheck::Valid => return None,
        LayoutCheck::UnknownVariable(token) => {
            Label::new("gui.ytdl.where.err.unknown_var").with("token", token.clone()).with("vars", vars)
        }
        LayoutCheck::NeverVariable(token) => {
            Label::new("gui.ytdl.where.err.never_var").with("token", token.clone()).with("vars", vars)
        }
        LayoutCheck::BadChars(_) => Label::new("gui.ytdl.where.err.bad_char"),
        // The engine's refusals, worded for a YouTube layout; the two drive
        // letter codes and the two absolute codes read as one each.
        LayoutCheck::Engine(problem) => match problem {
            LayoutProblem::UnbalancedBraces => Label::new("gui.ytdl.where.err.braces").with("example", "{{ARTIST}}"),
            LayoutProblem::Absolute => Label::new("gui.ytdl.where.err.absolute"),
            LayoutProblem::Traversal => Label::new("gui.ytdl.where.err.traversal"),
            LayoutProblem::EmptyPath => Label::new("gui.ytdl.where.err.empty"),
            LayoutProblem::Home => Label::new("gui.ytdl.where.err.home"),
            LayoutProblem::DriveLetter => Label::new("gui.ytdl.where.err.drive"),
            LayoutProblem::TemplateTooLong | LayoutProblem::PathTooLong => {
                Label::new("gui.ytdl.where.err.too_long").with("max", MAX_TEMPLATE_LEN.to_string())
            }
            LayoutProblem::InvalidChars => Label::new("gui.ytdl.where.err.control"),
            LayoutProblem::EmptyTemplate | LayoutProblem::UnknownVariable { .. } => Label::new("gui.ytdl.where.err.generic"),
        },
    })
}

/// The engine's tags for a YouTube song: ALBUMARTIST and ARTIST are both
/// the Artist field; GENRE and the rest have no value.
fn engine_tags(tags: &SongTags) -> LayoutTags {
    LayoutTags {
        artist: Some(tags.artist.clone()),
        album: Some(tags.album.clone()),
        year: Some(tags.year.clone()),
        album_artist: Some(tags.artist.clone()),
        ..LayoutTags::default()
    }
}

/// A variable's rendered value for a YouTube song.
fn value_of(name: &str, tags: &SongTags) -> String {
    match name {
        "ARTIST" | "ALBUMARTIST" => sanitize_segment(Some(&tags.artist)),
        "ALBUM" => sanitize_segment(Some(&tags.album)),
        "YEAR" => sanitize_segment(Some(&tags.year)),
        _ => String::new(),
    }
}

/// Why a song has nowhere to land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetError {
    /// The engine refuses the layout, or a folder it renders, or the base
    /// folders (`Same as Discover`).
    Layout(LayoutProblem),
    /// `: * ? " < > |` in the layout's own text.
    BadChars,
    /// The explorer's path names no library (its root).
    NoLibrary,
}

/// What an empty variable did to a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrapKind {
    /// Only brackets or separators left: "()".
    EmptyFolder,
    /// An empty pair or a dangling separator, left by the year alone:
    /// "Discovery ()".
    MissingYear,
    /// The same, left by another tag.
    Other,
}

/// The first folder an empty variable leaves as only brackets or
/// separators, or with an empty pair or a separator at its edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Trap {
    pub kind: TrapKind,
    pub folder: String,
}

/// Where a song lands: a library and the folders below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub library: String,
    /// Below the library: the base folders, then the rendered layout.
    pub segments: Vec<String>,
    /// The variables with no value (their folders drop out), upper case,
    /// the never-variables left out.
    pub missing: Vec<String>,
    /// The never-variables the layout names.
    pub never: Vec<String>,
    pub trap: Option<Trap>,
    pub error: Option<TargetError>,
}

impl Target {
    fn invalid(library: &str, error: TargetError, missing: Vec<String>, never: Vec<String>) -> Target {
        Target { library: library.to_string(), segments: Vec::new(), missing, never, trap: None, error: Some(error) }
    }

    fn valid(library: &str, segments: Vec<String>, missing: Vec<String>, never: Vec<String>, trap: Option<Trap>) -> Target {
        Target { library: library.to_string(), segments, missing, never, trap, error: None }
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.error.is_none()
    }

    /// The folders below the library, `/` between.
    pub(crate) fn rel_dir(&self) -> String {
        self.segments.join("/")
    }

    /// What POST /ytdl and mkdir get: `/<library>/<folders>/`, or `/<library>/`;
    /// `""` when there is nowhere to land.
    pub(crate) fn directory(&self) -> String {
        if !self.is_valid() {
            return String::new();
        }
        directory_of(&self.library, &self.segments)
    }

    /// The library, then its folders.
    pub(crate) fn crumbs(&self) -> Vec<String> {
        std::iter::once(self.library.clone()).chain(self.segments.iter().cloned()).collect()
    }
}

fn directory_of(library: &str, segments: &[String]) -> String {
    if segments.is_empty() { format!("/{library}/") } else { format!("/{library}/{}/", segments.join("/")) }
}

/// The never-variables `layout` names, each once, in its order.
fn never_in(layout: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, _) in tokens(layout) {
        if NEVER_VARS.contains(&name.as_str()) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The variables `layout` names that render nothing for these tags, each
/// once, in its order, the never-variables left out.
fn missing_in(layout: &str, tags: &SongTags) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, _) in tokens(layout) {
        if !NEVER_VARS.contains(&name.as_str()) && value_of(&name, tags).is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The first folder of `layout` an empty variable leaves as a trap. Each
/// folder renders as the engine renders it (a value never holds a `/`, so
/// a layout's folder is one rendered folder); one with no empty variable
/// in it is the user's own text and never a trap.
fn trap_in(layout: &str, tags: &SongTags) -> Option<Trap> {
    let r = regexes();
    for folder in layout.split(['/', '\\']).filter(|f| !f.is_empty()) {
        let mut emptied: Vec<String> = Vec::new();
        let rendered = r.token.replace_all(folder, |m: &regex::Captures| {
            let name = m[1].to_ascii_uppercase();
            let value = value_of(&name, tags);
            if value.is_empty() {
                emptied.push(name);
            }
            value
        });
        let rendered = super::js_trim(&rendered);
        if rendered.is_empty() || emptied.is_empty() {
            continue;
        }
        if r.only_brackets.is_match(rendered) {
            return Some(Trap { kind: TrapKind::EmptyFolder, folder: rendered.to_string() });
        }
        if r.empty_pair.is_match(rendered) || r.dangling_edge.is_match(rendered) {
            let kind = if emptied.iter().all(|n| n == "YEAR") { TrapKind::MissingYear } else { TrapKind::Other };
            return Some(Trap { kind, folder: rendered.to_string() });
        }
    }
    None
}

/// Where a song lands in library `vpath`: Discover's base folders `base`
/// first for "Same as Discover" (`""` otherwise), then `layout` rendered
/// for the song's tags by the engine (the webapp's `previewTarget`); `None`
/// or `""` is Top of the library, the base folders only. A refused base
/// folder or layout names no missing variables; a rendered folder the
/// server would refuse does.
pub(crate) fn target(vpath: &str, base: &str, layout: Option<&str>, tags: &SongTags) -> Target {
    let layout = layout.unwrap_or(LAYOUT_TOP);
    let mut folders = base_folders(base);
    let base_path = folders.join("/");
    let never = if layout == LAYOUT_TOP { Vec::new() } else { never_in(layout) };
    if layout != LAYOUT_TOP && matches!(layout_check(layout), LayoutCheck::BadChars(_)) {
        return Target::invalid(vpath, TargetError::BadChars, Vec::new(), never);
    }
    if !base_path.is_empty()
        && let Err(problem) = validate_resolved_path(&base_path)
    {
        return Target::invalid(vpath, TargetError::Layout(problem), Vec::new(), never);
    }
    if layout == LAYOUT_TOP {
        return Target::valid(vpath, folders, Vec::new(), Vec::new(), None);
    }
    if let Err(problem) = validate_layout(layout, &engine_knows) {
        return Target::invalid(vpath, TargetError::Layout(problem), Vec::new(), never);
    }
    let own: Vec<String> = layout_folders(layout, &engine_tags(tags), &engine_knows)
        .into_iter()
        .map(|folder| folder.into_iter().map(|(text, _)| text).collect())
        .collect();
    let missing = missing_in(layout, tags);
    let path = own.join("/");
    if !path.is_empty()
        && let Err(problem) = validate_resolved_path(&path)
    {
        return Target::invalid(vpath, TargetError::Layout(problem), missing, never);
    }
    folders.extend(own);
    Target::valid(vpath, folders, missing, never, trap_in(layout, tags))
}

/// The Files window's target under the folder on screen. `current` is
/// the explorer's path in either spelling (`Music/Daft Punk` or
/// `/Music/Daft Punk/`); `None` or `""` is "Right here". The folder on
/// screen exists, so its names go in front of the rendered layout as they
/// are, unchecked: the base-folder rules are for folders a layout would
/// create.
pub(crate) fn folder_target(current: &str, layout: Option<&str>, tags: &SongTags) -> Target {
    let here = path_crumbs(current);
    let Some((library, below)) = here.split_first() else {
        return Target::invalid("", TargetError::NoLibrary, Vec::new(), Vec::new());
    };
    let layout = layout.unwrap_or(LAYOUT_TOP);
    if layout == LAYOUT_TOP {
        return Target::valid(library, below.to_vec(), Vec::new(), Vec::new(), None);
    }
    let mut t = target(library, "", Some(layout), tags);
    if t.is_valid() {
        t.segments = below.iter().cloned().chain(t.segments).collect();
    }
    t
}

/// One crumb as drawn: the library first; a folder that will be made
/// carries "new"; the trap's folder is the warning's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Crumb {
    pub text: String,
    pub is_new: bool,
    pub trap: bool,
}

/// The crumbs of `target`, `new_segments` (indexes into its segments, the
/// folder check's answer) marked new.
pub(crate) fn crumb_items(target: &Target, new_segments: &[usize]) -> Vec<Crumb> {
    let mut out = vec![Crumb { text: target.library.clone(), is_new: false, trap: false }];
    let mut trapped = false;
    for (i, text) in target.segments.iter().enumerate() {
        let trap = !trapped && target.trap.as_ref().is_some_and(|t| &t.folder == text);
        trapped |= trap;
        out.push(Crumb { text: text.clone(), is_new: new_segments.contains(&i), trap });
    }
    out
}

/// The tag a trap's key word takes the room to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrapFix {
    /// "e edit tags", the Album field.
    EditTags,
    /// "y add a year", the Year field.
    AddYear,
}

/// The bracket warning for a trap, and its way on. Save stays allowed:
/// songs with every tag render fine with that layout.
pub(crate) fn trap_label(trap: &Trap) -> (Label, Option<TrapFix>) {
    let folder = trap.folder.clone();
    match trap.kind {
        TrapKind::EmptyFolder => (Label::new("gui.ytdl.where.trap.empty").with("folder", folder), Some(TrapFix::EditTags)),
        TrapKind::MissingYear => (Label::new("gui.ytdl.where.trap.year").with("folder", folder), Some(TrapFix::AddYear)),
        TrapKind::Other => (Label::new("gui.ytdl.where.trap.other").with("folder", folder), None),
    }
}

// ── the new-folder check ────────────────────────────────────────────────────

/// A folder's listing, names only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Listing {
    pub directories: Vec<String>,
    pub files: Vec<String>,
}

/// What the new-folder check found: `known` — the folders that will be
/// made are `new_segments` (indexes into the target's segments); `deepest`
/// the deepest folder that listed ("i save to … instead"); `files` the
/// target's own file names, for the name check (`None` when it didn't list
/// or is new).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderCheck {
    pub known: bool,
    pub new_segments: Vec<usize>,
    pub deepest: Option<String>,
    pub files: Option<Vec<String>>,
}

impl FolderCheck {
    fn unknown(deepest: Option<String>) -> FolderCheck {
        FolderCheck { known: false, new_segments: Vec::new(), deepest, files: None }
    }
}

/// List the target folder; on any failure walk up until a listing works.
/// The folder below the one that listed is new when that listing has no
/// entry of its name (ignoring case), and so is every folder after it. A
/// file of that name, or no listing at all, is unknown. `list` answers a
/// directory (`/Music/Daft Punk/`) or `None` for one that wouldn't list;
/// `shown` is the rows already on screen for a directory (the Files
/// window's own folder), used when the target won't list. The caller runs
/// this off the GUI thread and drops a stale answer.
pub(crate) fn check_folders(
    target: &Target,
    list: &mut dyn FnMut(&str) -> Option<Listing>,
    shown: &dyn Fn(&str) -> Option<Vec<String>>,
) -> FolderCheck {
    if !target.is_valid() {
        return FolderCheck::unknown(None);
    }
    let segments = &target.segments;
    for depth in (0..=segments.len()).rev() {
        let dir = directory_of(&target.library, &segments[..depth]);
        let Some(listing) = list(&dir) else {
            if depth == segments.len()
                && let Some(rows) = shown(&dir)
            {
                return FolderCheck { known: true, new_segments: Vec::new(), deepest: Some(dir), files: Some(rows) };
            }
            continue;
        };
        if depth == segments.len() {
            return FolderCheck { known: true, new_segments: Vec::new(), deepest: Some(dir), files: Some(listing.files) };
        }
        let next = segments[depth].to_lowercase();
        let there = listing.directories.iter().chain(&listing.files).any(|name| name.to_lowercase() == next);
        if there {
            return FolderCheck::unknown(Some(dir));
        }
        return FolderCheck { known: true, new_segments: (depth..segments.len()).collect(), deepest: Some(dir), files: None };
    }
    FolderCheck::unknown(None)
}

// ── "Where new songs land": the presets ─────────────────────────────────────

/// A layout radio's kind, as `config.toml`'s `ytdl_layout_kind` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutKind {
    ArtistAlbum,
    Artist,
    Top,
    /// "Same as Discover": the account's own destination, followed while
    /// it is known.
    Discover,
    /// "Library layout": the library's admin template, followed likewise.
    Library,
    Custom,
}

impl LayoutKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LayoutKind::ArtistAlbum => "artistAlbum",
            LayoutKind::Artist => "artist",
            LayoutKind::Top => "top",
            LayoutKind::Discover => "discover",
            LayoutKind::Library => "library",
            LayoutKind::Custom => "custom",
        }
    }

    pub(crate) const ALL: [LayoutKind; 6] = [
        LayoutKind::ArtistAlbum,
        LayoutKind::Artist,
        LayoutKind::Top,
        LayoutKind::Discover,
        LayoutKind::Library,
        LayoutKind::Custom,
    ];

    pub(crate) fn parse(word: &str) -> Option<LayoutKind> {
        LayoutKind::ALL.into_iter().find(|k| k.as_str() == word)
    }

    /// The radio's words.
    pub(crate) fn label(self) -> Label {
        Label::new(match self {
            LayoutKind::ArtistAlbum => "gui.ytdl.where.preset.artist_album",
            LayoutKind::Artist => "gui.ytdl.where.preset.artist",
            LayoutKind::Top => "gui.ytdl.where.preset.top",
            LayoutKind::Discover => "gui.ytdl.where.preset.discover",
            LayoutKind::Library => "gui.ytdl.where.preset.library",
            LayoutKind::Custom => "gui.ytdl.where.preset.custom",
        })
    }

    /// The kind a layout with no remembered kind reads as.
    fn of_layout(layout: &str) -> LayoutKind {
        match layout {
            LAYOUT_TOP => LayoutKind::Top,
            LAYOUT_ARTIST_ALBUM => LayoutKind::ArtistAlbum,
            LAYOUT_ARTIST => LayoutKind::Artist,
            _ => LayoutKind::Custom,
        }
    }
}

/// The account's own Discover destination: "Same as Discover".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoverPlace {
    pub vpath: String,
    pub base: String,
    pub layout: String,
}

/// What the account offers beside the fixed presets, once read: Discover's
/// destination when it is the user's own (source `user`), and each
/// library's admin template (non-blank).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Presets {
    pub discover: Option<DiscoverPlace>,
    pub templates: Vec<(String, String)>,
}

impl Presets {
    /// The presets from the destination view (asked only where the server
    /// has Discover's plug-ins) and, when it names no template, the torrent
    /// path templates — the phone's fallback for "Library layout".
    pub(crate) fn of(view: Option<&CollectionDestinationView>, torrent: Option<&TorrentTemplates>) -> Presets {
        let discover = view.and_then(|v| v.destination.as_ref()).filter(|d| d.source.as_deref() == Some("user")).map(|d| {
            DiscoverPlace { vpath: d.vpath.clone(), base: d.base.clone(), layout: d.layout.clone() }
        });
        let mut templates: Vec<(String, String)> = view
            .map(|v| v.libraries.iter().filter_map(|l| Some((l.vpath.clone(), l.template.clone()?))).collect())
            .unwrap_or_default();
        templates.retain(|(_, t)| !t.trim().is_empty());
        if templates.is_empty()
            && let Some(torrent) = torrent
        {
            templates = torrent.vpaths.iter().filter_map(|(v, e)| Some((v.clone(), e.template.clone()?))).collect();
            templates.retain(|(_, t)| !t.trim().is_empty());
            templates.sort();
        }
        Presets { discover, templates }
    }

    /// The library's admin template, or `None` when it has none.
    pub(crate) fn template_of(&self, vpath: &str) -> Option<&str> {
        self.templates.iter().find(|(v, _)| v == vpath).map(|(_, t)| t.as_str()).filter(|t| !t.trim().is_empty())
    }
}

/// What `config.toml` remembers for a server (`ytdl_library`,
/// `ytdl_layout`, `ytdl_layout_kind`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Remembered {
    pub library: Option<String>,
    pub layout: Option<String>,
    pub kind: Option<LayoutKind>,
}

/// A choice new songs land by: a library, a layout (`""` Top of the
/// library), its kind, and Discover's base folders for "Same as Discover".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub library: String,
    pub layout: String,
    pub kind: LayoutKind,
    pub base: String,
}

/// One layout radio: its kind, its layout (`None` for Custom: the field's),
/// Discover's base folders, the library it lands in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preset {
    pub kind: LayoutKind,
    pub layout: Option<String>,
    pub base: String,
    pub library: String,
}

/// The "Library layout" radio's layout: the library's admin template,
/// unless it is one of the presets above it (Artist / Album, Artist) —
/// still offered on the library a "Library layout" choice was saved for.
pub(crate) fn library_template(presets: Option<&Presets>, vpath: &str, saved: Option<&Remembered>) -> Option<String> {
    let template = presets?.template_of(vpath)?;
    let kept = saved.is_some_and(|s| s.kind == Some(LayoutKind::Library) && s.library.as_deref() == Some(vpath));
    let dupe = template == LAYOUT_ARTIST_ALBUM || template == LAYOUT_ARTIST;
    (!dupe || kept).then(|| template.to_string())
}

/// The layout radios for `library`, in order: Artist / Album, Artist, Top
/// of the library, Same as Discover (its library is Discover's), Library
/// layout (only where the library has its own), Custom. `presets` is
/// `None` until they are read.
pub(crate) fn layout_choices(presets: Option<&Presets>, library: &str, saved: Option<&Remembered>) -> Vec<Preset> {
    let preset = |kind, layout: Option<&str>, base: &str, library: &str| Preset {
        kind,
        layout: layout.map(str::to_string),
        base: base.to_string(),
        library: library.to_string(),
    };
    let mut out = vec![
        preset(LayoutKind::ArtistAlbum, Some(LAYOUT_ARTIST_ALBUM), "", library),
        preset(LayoutKind::Artist, Some(LAYOUT_ARTIST), "", library),
        preset(LayoutKind::Top, Some(LAYOUT_TOP), "", library),
    ];
    if let Some(d) = presets.and_then(|p| p.discover.as_ref()) {
        out.push(preset(LayoutKind::Discover, Some(&d.layout), &d.base, &d.vpath));
    }
    if let Some(t) = library_template(presets, library, saved) {
        out.push(preset(LayoutKind::Library, Some(&t), "", library));
    }
    out.push(preset(LayoutKind::Custom, None, "", library));
    out
}

/// The choice new songs land by, from what `config.toml` remembers for the
/// server and the presets (`None` while they haven't answered). Library:
/// the remembered one while the account still has it (`libraries`, the
/// ping's); else Discover's when its destination is the user's own; else
/// the first. Layout: the remembered one, else Artist / Album — never
/// inherited, so the first song can't make an "Album ()" folder. "Same as
/// Discover" and "Library layout" follow their preset while it is known,
/// and read Custom once the presets answered without it.
pub(crate) fn remembered_choice(presets: Option<&Presets>, saved: Option<&Remembered>, libraries: &[String]) -> Choice {
    let none = Remembered::default();
    let s = saved.unwrap_or(&none);
    let discover = presets.and_then(|p| p.discover.as_ref());
    let library = s
        .library
        .clone()
        .filter(|l| libraries.contains(l))
        .or_else(|| discover.map(|d| d.vpath.clone()).filter(|v| libraries.contains(v)))
        .or_else(|| libraries.first().cloned())
        .unwrap_or_default();
    let layout = s.layout.clone().unwrap_or_else(|| LAYOUT_ARTIST_ALBUM.to_string());
    let mut kind = s.kind.unwrap_or_else(|| LayoutKind::of_layout(&layout));
    let template = presets.and_then(|p| p.template_of(&library)).map(str::to_string);
    if let Some(p) = presets {
        if kind == LayoutKind::Discover && p.discover.is_none() {
            kind = LayoutKind::Custom;
        }
        if kind == LayoutKind::Library && template.is_none() {
            kind = LayoutKind::Custom;
        }
    }
    match (kind, discover, template) {
        (LayoutKind::Discover, Some(d), _) => {
            Choice { library: d.vpath.clone(), layout: d.layout.clone(), kind, base: d.base.clone() }
        }
        (LayoutKind::Library, _, Some(t)) => Choice { library, layout: t, kind, base: String::new() },
        _ => Choice { library, layout, kind, base: String::new() },
    }
}

/// What to remember for a choice (Save): a "Same as Discover" choice keeps
/// its base folders in front of its layout ([`fold_base`]), the copy used
/// while Discover's destination is not known.
pub(crate) fn choice_to_save(choice: &Choice) -> Remembered {
    let discover = choice.kind == LayoutKind::Discover;
    let layout = if discover { fold_base(&choice.base, &choice.layout) } else { choice.layout.clone() };
    Remembered { library: Some(choice.library.clone()), layout: Some(layout), kind: Some(choice.kind) }
}

/// A choice's target for the song's tags: only "Same as Discover" has
/// base folders.
pub(crate) fn choice_target(choice: &Choice, tags: &SongTags) -> Target {
    let base = if choice.kind == LayoutKind::Discover { choice.base.as_str() } else { "" };
    target(&choice.library, base, Some(&choice.layout), tags)
}

/// "s subfolders" in the Files window (not remembered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderSort {
    Here,
    Album,
    ArtistAlbum,
    Custom,
}

impl FolderSort {
    pub(crate) const ALL: [FolderSort; 4] =
        [FolderSort::Here, FolderSort::Album, FolderSort::ArtistAlbum, FolderSort::Custom];

    /// The layout under the folder on screen; `None` for Custom (the
    /// field's).
    pub(crate) fn layout(self) -> Option<&'static str> {
        match self {
            FolderSort::Here => Some(LAYOUT_TOP),
            FolderSort::Album => Some(LAYOUT_ALBUM),
            FolderSort::ArtistAlbum => Some(LAYOUT_ARTIST_ALBUM),
            FolderSort::Custom => None,
        }
    }

    pub(crate) fn label(self) -> Label {
        Label::new(match self {
            FolderSort::Here => "gui.ytdl.sort.here",
            FolderSort::Album => "gui.ytdl.sort.album",
            FolderSort::ArtistAlbum => "gui.ytdl.sort.artist_album",
            FolderSort::Custom => "gui.ytdl.sort.custom",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::api::types::{CollectionDestination, DestinationLibrary, TorrentTemplateEntry};

    fn tags(artist: &str, album: &str, year: &str) -> SongTags {
        SongTags { artist: artist.into(), album: album.into(), year: year.into() }
    }

    fn daft_punk() -> SongTags {
        tags("Daft Punk", "Discovery", "2001")
    }

    fn gorillaz() -> SongTags {
        tags("Gorillaz", "", "")
    }

    const LIBRARY_LAYOUT: &str = "{{ARTIST}}/{{ALBUM}} ({{YEAR}})";

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn expect(t: &Target, crumbs: &[&str], directory: &str, missing: &[&str], never: &[&str], trap: Option<(TrapKind, &str)>) {
        assert!(t.is_valid(), "{t:?}");
        assert_eq!(t.crumbs(), strings(crumbs), "{t:?}");
        assert_eq!(t.library, crumbs[0]);
        assert_eq!(t.segments, strings(&crumbs[1..]));
        assert_eq!(t.rel_dir(), crumbs[1..].join("/"));
        assert_eq!(t.directory(), directory);
        assert_eq!(t.missing, strings(missing), "{t:?}");
        assert_eq!(t.never, strings(never), "{t:?}");
        assert_eq!(t.trap, trap.map(|(kind, folder)| Trap { kind, folder: folder.into() }), "{t:?}");
    }

    #[test]
    fn the_presets_and_every_one_passes_the_check() {
        assert_eq!(LAYOUT_ARTIST_ALBUM, "{{ALBUMARTIST}}/{{ALBUM}}", "the server's own default");
        for l in [LAYOUT_ARTIST_ALBUM, LAYOUT_ARTIST, LAYOUT_ALBUM, LAYOUT_TOP] {
            assert_eq!(layout_check(l), LayoutCheck::Valid, "{l}");
        }
        let mut both: Vec<&str> = YTDL_VARS.iter().chain(&NEVER_VARS).copied().collect();
        both.sort();
        let mut engine: Vec<&str> = LAYOUT_VARS.iter().copied().chain([SERVER_FOLDER]).collect();
        engine.sort();
        assert_eq!(both, engine, "between them, every variable the engine knows");
    }

    #[test]
    fn a_song_lands_by_its_tags() {
        let music = |layout, t: &SongTags| target("Music", "", Some(layout), t);
        expect(&music(LAYOUT_ARTIST_ALBUM, &daft_punk()), &["Music", "Daft Punk", "Discovery"], "/Music/Daft Punk/Discovery/", &[], &[], None);
        expect(&music(LAYOUT_ARTIST_ALBUM, &gorillaz()), &["Music", "Gorillaz"], "/Music/Gorillaz/", &["ALBUM"], &[], None);
        expect(&music(LAYOUT_ARTIST_ALBUM, &tags("Кино", "", "")), &["Music", "Кино"], "/Music/Кино/", &["ALBUM"], &[], None);
        expect(&music(LAYOUT_ARTIST_ALBUM, &SongTags::default()), &["Music"], "/Music/", &["ALBUMARTIST", "ALBUM"], &[], None);
        expect(&target("Mixes", "", Some(LAYOUT_ARTIST), &daft_punk()), &["Mixes", "Daft Punk"], "/Mixes/Daft Punk/", &[], &[], None);
        expect(&music(LAYOUT_TOP, &daft_punk()), &["Music"], "/Music/", &[], &[], None);
        expect(&target("Music", "", None, &daft_punk()), &["Music"], "/Music/", &[], &[], None);
        expect(
            &music(LAYOUT_ARTIST_ALBUM, &tags("AC/DC", "Back In Black?", "")),
            &["Music", "AC-DC", "Back In Black-"],
            "/Music/AC-DC/Back In Black-/",
            &[],
            &[],
            None,
        );
    }

    #[test]
    fn same_as_discover_puts_the_accounts_base_folders_first() {
        expect(
            &target("Music", "From Discover", Some(LAYOUT_ARTIST_ALBUM), &daft_punk()),
            &["Music", "From Discover", "Daft Punk", "Discovery"],
            "/Music/From Discover/Daft Punk/Discovery/",
            &[],
            &[],
            None,
        );
        expect(
            &target("Music", "Added\\ From Discover /", Some(LAYOUT_ALBUM), &daft_punk()),
            &["Music", "Added", "From Discover", "Discovery"],
            "/Music/Added/From Discover/Discovery/",
            &[],
            &[],
            None,
        );
        expect(&target("Music", "From Discover", Some(""), &daft_punk()), &["Music", "From Discover"], "/Music/From Discover/", &[], &[], None);
        let t = target("Music", "..", Some(""), &daft_punk());
        assert_eq!(t.error, Some(TargetError::Layout(LayoutProblem::Traversal)));
        assert_eq!(t.directory(), "");
        assert_eq!(t.crumbs(), ["Music"]);
        assert_eq!(target("Music", "..", Some(LAYOUT_ARTIST), &daft_punk()).error, Some(TargetError::Layout(LayoutProblem::Traversal)));
    }

    #[test]
    fn an_empty_variable_can_leave_a_folder_of_brackets() {
        let music = |layout, t: &SongTags| target("Music", "", Some(layout), t);
        expect(&music(LIBRARY_LAYOUT, &gorillaz()), &["Music", "Gorillaz", "()"], "/Music/Gorillaz/()/", &["ALBUM", "YEAR"], &[], Some((TrapKind::EmptyFolder, "()")));
        expect(
            &music(LIBRARY_LAYOUT, &tags("Daft Punk", "Discovery", "")),
            &["Music", "Daft Punk", "Discovery ()"],
            "/Music/Daft Punk/Discovery ()/",
            &["YEAR"],
            &[],
            Some((TrapKind::MissingYear, "Discovery ()")),
        );
        expect(&music(LIBRARY_LAYOUT, &daft_punk()), &["Music", "Daft Punk", "Discovery (2001)"], "/Music/Daft Punk/Discovery (2001)/", &[], &[], None);
        expect(&music("{{ARTIST}} - {{ALBUM}}", &gorillaz()), &["Music", "Gorillaz -"], "/Music/Gorillaz -/", &["ALBUM"], &[], Some((TrapKind::Other, "Gorillaz -")));
        let trap = |layout, t: &SongTags| music(layout, t).trap.map(|t| (t.kind, t.folder));
        assert_eq!(trap("{{ARTIST}}/[{{YEAR}}]", &gorillaz()), Some((TrapKind::EmptyFolder, "[]".into())));
        assert_eq!(trap("{{ALBUM}} - {{ARTIST}}", &gorillaz()), Some((TrapKind::Other, "- Gorillaz".into())));
        assert_eq!(trap("{{ALBUM}} - {{YEAR}}", &tags("", "Discovery", "")), Some((TrapKind::MissingYear, "Discovery -".into())));
        assert_eq!(trap("{{ARTIST}} - {{ALBUM}}", &SongTags::default()), Some((TrapKind::EmptyFolder, "-".into())));
        let genre = music("{{ALBUM}} ({{GENRE}})", &daft_punk());
        assert_eq!(genre.trap, Some(Trap { kind: TrapKind::Other, folder: "Discovery ()".into() }), "a never-variable that empties a folder is a trap too");
        assert_eq!(genre.never, ["GENRE"]);
        assert_eq!(trap("{{ARTIST}}/Singles ()", &gorillaz()), None, "brackets typed as text are the user's own");
        assert_eq!(trap("{{ARTIST}}/{{ALBUM}}", &gorillaz()), None, "an emptied folder is dropped, not a trap");
    }

    #[test]
    fn the_bracket_warning_and_its_way_on() {
        let (label, fix) = trap_label(&Trap { kind: TrapKind::EmptyFolder, folder: "()".into() });
        assert_eq!((label.key, label.param("folder"), fix), ("gui.ytdl.where.trap.empty", Some("()"), Some(TrapFix::EditTags)));
        let (label, fix) = trap_label(&Trap { kind: TrapKind::MissingYear, folder: "Discovery ()".into() });
        assert_eq!((label.key, label.param("folder"), fix), ("gui.ytdl.where.trap.year", Some("Discovery ()"), Some(TrapFix::AddYear)));
        let (label, fix) = trap_label(&Trap { kind: TrapKind::Other, folder: "Gorillaz -".into() });
        assert_eq!((label.key, label.param("folder"), fix), ("gui.ytdl.where.trap.other", Some("Gorillaz -"), None));
        let r = regexes();
        for f in ["()", "【】", "-", " [ ] ", "()-()"] {
            assert!(r.only_brackets.is_match(f), "{f}");
        }
        for f in ["", "Discovery ()", "a"] {
            assert!(!r.only_brackets.is_match(f), "{f}");
        }
    }

    #[test]
    fn the_crumbs_mark_new_folders_and_the_trap() {
        let t = target("Music", "", Some(LIBRARY_LAYOUT), &tags("Daft Punk", "Discovery", ""));
        let crumb = |text: &str, is_new, trap| Crumb { text: text.into(), is_new, trap };
        assert_eq!(
            crumb_items(&t, &[1]),
            [crumb("Music", false, false), crumb("Daft Punk", false, false), crumb("Discovery ()", true, true)]
        );
        assert_eq!(crumb_items(&target("Music", "", Some(""), &SongTags::default()), &[]), [crumb("Music", false, false)]);
    }

    #[test]
    fn a_custom_layout_and_its_three_errors() {
        assert_eq!(layout_check("{{ARTIST}}/Singles"), LayoutCheck::Valid);
        expect(&target("Music", "", Some("{{ARTIST}}/Singles"), &daft_punk()), &["Music", "Daft Punk", "Singles"], "/Music/Daft Punk/Singles/", &[], &[], None);

        let unknown = layout_check("{{ARTIST}}/{{TITLE}}");
        assert_eq!(unknown, LayoutCheck::UnknownVariable("{{TITLE}}".into()));
        assert_eq!(layout_check("{{artist}}/{{ title }}"), LayoutCheck::UnknownVariable("{{ title }}".into()), "the token as typed");
        let words = layout_problem(&unknown).expect("words");
        assert_eq!((words.key, words.param("token"), words.param("vars")), ("gui.ytdl.where.err.unknown_var", Some("{{TITLE}}"), Some("ARTIST, ALBUMARTIST, ALBUM, YEAR")));
        let t = target("Music", "", Some("{{ARTIST}}/{{TITLE}}"), &daft_punk());
        assert_eq!(t.error, Some(TargetError::Layout(LayoutProblem::UnknownVariable { token: "{{TITLE}}".into() })));
        assert_eq!(t.directory(), "");

        let never = layout_check("{{GENRE}}/{{ARTIST}}");
        assert_eq!(never, LayoutCheck::NeverVariable("{{GENRE}}".into()));
        assert_eq!(layout_check("{{ARTIST}}/{{ peer }}"), LayoutCheck::NeverVariable("{{ peer }}".into()));
        assert_eq!(layout_check("{{SERVERFOLDER}}"), LayoutCheck::NeverVariable("{{SERVERFOLDER}}".into()));
        let words = layout_problem(&never).expect("words");
        assert_eq!((words.key, words.param("token")), ("gui.ytdl.where.err.never_var", Some("{{GENRE}}")));

        assert_eq!(layout_check("Singles: 2024"), LayoutCheck::BadChars(':'));
        for l in ["a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "{{ARTIST}} | Live"] {
            assert!(matches!(layout_check(l), LayoutCheck::BadChars(_)), "{l}");
        }
        assert_eq!(layout_problem(&LayoutCheck::BadChars(':')).map(|l| l.key), Some("gui.ytdl.where.err.bad_char"));
        assert_eq!(target("Music", "", Some("Singles: 2024"), &daft_punk()).error, Some(TargetError::BadChars));
        assert!(matches!(layout_check("{{TITLE}}/{{GENRE}}"), LayoutCheck::UnknownVariable(_)), "the allow-list runs after the engine");
        assert!(matches!(layout_check("{{GENRE}}: x"), LayoutCheck::NeverVariable(_)), "the literal check last");
        assert_eq!(layout_problem(&layout_check("{{ARTIST}}")), None);
        assert_eq!(layout_check(""), LayoutCheck::Valid, "callers treat it as Top of the library");
    }

    #[test]
    fn the_engines_own_refusals_come_back_worded() {
        let long = "x".repeat(501);
        for (layout, problem, key) in [
            ("{ARTIST}", LayoutProblem::UnbalancedBraces, "gui.ytdl.where.err.braces"),
            ("/{{ARTIST}}", LayoutProblem::Absolute, "gui.ytdl.where.err.absolute"),
            ("C:", LayoutProblem::DriveLetter, "gui.ytdl.where.err.drive"),
            ("{{ARTIST}}/D:x", LayoutProblem::DriveLetter, "gui.ytdl.where.err.drive"),
            ("../{{ARTIST}}", LayoutProblem::Traversal, "gui.ytdl.where.err.traversal"),
            ("~/{{ARTIST}}", LayoutProblem::Home, "gui.ytdl.where.err.home"),
            (" ", LayoutProblem::EmptyPath, "gui.ytdl.where.err.empty"),
            (long.as_str(), LayoutProblem::TemplateTooLong, "gui.ytdl.where.err.too_long"),
            ("a\u{1}b", LayoutProblem::InvalidChars, "gui.ytdl.where.err.control"),
        ] {
            let check = layout_check(layout);
            assert_eq!(check, LayoutCheck::Engine(problem), "{layout}");
            assert_eq!(layout_problem(&check).map(|l| l.key), Some(key), "{layout}");
        }
        assert_eq!(layout_problem(&LayoutCheck::Engine(LayoutProblem::UnbalancedBraces)).unwrap().param("example"), Some("{{ARTIST}}"));
        assert_eq!(layout_problem(&LayoutCheck::Engine(LayoutProblem::TemplateTooLong)).unwrap().param("max"), Some("500"));
        assert_eq!(layout_problem(&LayoutCheck::Engine(LayoutProblem::EmptyTemplate)).map(|l| l.key), Some("gui.ytdl.where.err.generic"));
    }

    #[test]
    fn a_library_layout_naming_a_never_variable_still_renders() {
        let music = |layout| target("Music", "", Some(layout), &daft_punk());
        expect(&music("{{GENRE}}/{{ARTIST}}"), &["Music", "Daft Punk"], "/Music/Daft Punk/", &[], &["GENRE"], None);
        expect(&target("Music", "", Some("{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}"), &gorillaz()), &["Music", "Gorillaz"], "/Music/Gorillaz/", &["ALBUM"], &["PEER"], None);
        expect(&music("{{SERVERFOLDER}}"), &["Music"], "/Music/", &[], &["SERVERFOLDER"], None);
    }

    #[test]
    fn the_files_window_lands_under_the_folder_on_screen_in_either_spelling() {
        for here in ["/Music/Daft Punk/", "Music/Daft Punk/", "Music/Daft Punk"] {
            expect(&folder_target(here, Some(LAYOUT_TOP), &daft_punk()), &["Music", "Daft Punk"], "/Music/Daft Punk/", &[], &[], None);
            expect(&folder_target(here, Some(LAYOUT_ALBUM), &daft_punk()), &["Music", "Daft Punk", "Discovery"], "/Music/Daft Punk/Discovery/", &[], &[], None);
            expect(
                &folder_target(here, Some(LAYOUT_ARTIST_ALBUM), &daft_punk()),
                &["Music", "Daft Punk", "Daft Punk", "Discovery"],
                "/Music/Daft Punk/Daft Punk/Discovery/",
                &[],
                &[],
                None,
            );
            expect(&folder_target(here, Some(LAYOUT_ALBUM), &gorillaz()), &["Music", "Daft Punk"], "/Music/Daft Punk/", &["ALBUM"], &[], None);
        }
        expect(&folder_target("/Music/", Some(""), &daft_punk()), &["Music"], "/Music/", &[], &[], None);
        expect(
            &folder_target("/Music/~Odd C: Folder/", Some(LAYOUT_ALBUM), &daft_punk()),
            &["Music", "~Odd C: Folder", "Discovery"],
            "/Music/~Odd C: Folder/Discovery/",
            &[],
            &[],
            None,
        );
        assert_eq!(folder_target("/Music/", Some("Singles: 2024"), &daft_punk()).error, Some(TargetError::BadChars));
        assert!(matches!(folder_target("/Music/", Some("{{TITLE}}"), &daft_punk()).error, Some(TargetError::Layout(LayoutProblem::UnknownVariable { .. }))));
        let trap = folder_target("/Music/Daft Punk/", Some("{{ALBUM}} ({{YEAR}})"), &tags("", "Discovery", ""));
        assert_eq!(trap.directory(), "/Music/Daft Punk/Discovery ()/");
        assert_eq!(trap.trap, Some(Trap { kind: TrapKind::MissingYear, folder: "Discovery ()".into() }));
        assert_eq!(folder_target("", Some(""), &SongTags::default()).error, Some(TargetError::NoLibrary));
        assert_eq!(folder_target("/", Some(""), &SongTags::default()).error, Some(TargetError::NoLibrary));
        let sorts: Vec<_> = FolderSort::ALL.iter().map(|s| s.layout()).collect();
        assert_eq!(sorts, [Some(""), Some("{{ALBUM}}"), Some("{{ALBUMARTIST}}/{{ALBUM}}"), None]);
        assert_eq!(FolderSort::Here.label().key, "gui.ytdl.sort.here");
    }

    /// A library as the file explorer answers it.
    fn library() -> HashMap<&'static str, (Vec<&'static str>, Vec<&'static str>)> {
        HashMap::from([
            ("/Music/", (vec!["Daft Punk", "Gorillaz"], vec!["loose.mp3"])),
            ("/Music/Daft Punk/", (vec!["Homework"], vec!["One_More_Time.mp3", "Discovery"])),
            ("/Music/Gorillaz/", (vec![], vec!["Feel_Good_Inc.mp3"])),
        ])
    }

    fn walk(t: &Target, tree: &HashMap<&'static str, (Vec<&'static str>, Vec<&'static str>)>, shown: &dyn Fn(&str) -> Option<Vec<String>>) -> (FolderCheck, Vec<String>) {
        let mut asked = Vec::new();
        let mut list = |dir: &str| {
            asked.push(dir.to_string());
            tree.get(dir).map(|(d, f)| Listing { directories: strings(d), files: strings(f) })
        };
        let check = check_folders(t, &mut list, shown);
        (check, asked)
    }

    const NOTHING_SHOWN: &dyn Fn(&str) -> Option<Vec<String>> = &|_| None;

    #[test]
    fn a_target_that_lists_is_known_with_its_files() {
        let t = target("Music", "", Some(LAYOUT_ARTIST), &gorillaz());
        let (check, asked) = walk(&t, &library(), NOTHING_SHOWN);
        assert_eq!(check, FolderCheck { known: true, new_segments: vec![], deepest: Some("/Music/Gorillaz/".into()), files: Some(strings(&["Feel_Good_Inc.mp3"])) });
        assert_eq!(asked, ["/Music/Gorillaz/"]);
    }

    #[test]
    fn the_walk_goes_up_to_the_folder_that_lists() {
        let t = target("Music", "", Some("{{ARTIST}}/{{ALBUM}}/{{YEAR}}"), &tags("Gorillaz", "Demon Days", "2005"));
        let (check, asked) = walk(&t, &library(), NOTHING_SHOWN);
        assert_eq!(check, FolderCheck { known: true, new_segments: vec![1, 2], deepest: Some("/Music/Gorillaz/".into()), files: None });
        assert_eq!(asked, ["/Music/Gorillaz/Demon Days/2005/", "/Music/Gorillaz/Demon Days/", "/Music/Gorillaz/"]);
        let fresh = target("Music", "", Some(LAYOUT_ARTIST_ALBUM), &tags("Air", "Moon Safari", ""));
        assert_eq!(walk(&fresh, &library(), NOTHING_SHOWN).0, FolderCheck { known: true, new_segments: vec![0, 1], deepest: Some("/Music/".into()), files: None });
    }

    #[test]
    fn a_folder_in_another_case_is_not_new_and_a_file_of_its_name_is_unknown() {
        let mut tree = library();
        tree.remove("/Music/Daft Punk/");
        let lower = target("Music", "", Some("{{ARTIST}}/x"), &tags("daft punk", "", ""));
        assert_eq!(walk(&lower, &tree, NOTHING_SHOWN).0, FolderCheck::unknown(Some("/Music/".into())));
        let blocked = target("Music", "", Some(LAYOUT_ARTIST_ALBUM), &daft_punk());
        assert_eq!(walk(&blocked, &library(), NOTHING_SHOWN).0, FolderCheck::unknown(Some("/Music/Daft Punk/".into())));
    }

    #[test]
    fn nothing_listing_is_unknown_and_rows_on_screen_stand_in() {
        let t = target("Music", "", Some(LAYOUT_ARTIST), &daft_punk());
        assert_eq!(walk(&t, &HashMap::new(), NOTHING_SHOWN).0, FolderCheck::unknown(None));
        let shown = |dir: &str| (dir == "/Music/Daft Punk/").then(|| strings(&["One_More_Time.mp3"]));
        assert_eq!(
            walk(&t, &HashMap::new(), &shown).0,
            FolderCheck { known: true, new_segments: vec![], deepest: Some("/Music/Daft Punk/".into()), files: Some(strings(&["One_More_Time.mp3"])) }
        );
        let bad = target("Music", "", Some("{{TITLE}}"), &SongTags::default());
        let (check, asked) = walk(&bad, &library(), NOTHING_SHOWN);
        assert_eq!(check, FolderCheck::unknown(None));
        assert!(asked.is_empty(), "nowhere to land: nothing is asked");
    }

    fn view(destination: Option<CollectionDestination>, libraries: &[(&str, Option<&str>)]) -> CollectionDestinationView {
        CollectionDestinationView {
            destination,
            libraries: libraries.iter().map(|(v, t)| DestinationLibrary { vpath: v.to_string(), template: t.map(str::to_string) }).collect(),
            ..CollectionDestinationView::default()
        }
    }

    fn user_destination() -> Option<CollectionDestination> {
        Some(CollectionDestination { vpath: "Music".into(), base: "From Discover".into(), layout: LAYOUT_ARTIST_ALBUM.into(), source: Some("user".into()) })
    }

    fn presets() -> Presets {
        Presets::of(Some(&view(user_destination(), &[("Music", Some(LIBRARY_LAYOUT)), ("Mixes", None)])), None)
    }

    fn kinds(choices: &[Preset]) -> Vec<LayoutKind> {
        choices.iter().map(|c| c.kind).collect()
    }

    use LayoutKind::{Artist, ArtistAlbum, Custom, Discover, Library, Top};

    #[test]
    fn the_radios_offer_discover_and_the_library_layout_only_where_they_exist() {
        let p = presets();
        let c = layout_choices(Some(&p), "Music", None);
        assert_eq!(kinds(&c), [ArtistAlbum, Artist, Top, Discover, Library, Custom]);
        let keys: Vec<_> = c.iter().map(|x| x.kind.label().key).collect();
        assert_eq!(
            keys,
            [
                "gui.ytdl.where.preset.artist_album",
                "gui.ytdl.where.preset.artist",
                "gui.ytdl.where.preset.top",
                "gui.ytdl.where.preset.discover",
                "gui.ytdl.where.preset.library",
                "gui.ytdl.where.preset.custom"
            ]
        );
        assert_eq!(c[3], Preset { kind: Discover, layout: Some(LAYOUT_ARTIST_ALBUM.into()), base: "From Discover".into(), library: "Music".into() });
        assert_eq!(c[4].layout.as_deref(), Some(LIBRARY_LAYOUT));
        assert_eq!(c[5].layout, None, "Custom's layout is the field's");
        assert_eq!(kinds(&layout_choices(Some(&p), "Mixes", None)), [ArtistAlbum, Artist, Top, Discover, Custom]);
        let default_destination = CollectionDestination { source: Some("default".into()), base: String::new(), ..user_destination().unwrap() };
        let plain = Presets::of(Some(&view(Some(default_destination), &[("Music", Some(LIBRARY_LAYOUT))])), None);
        assert_eq!(kinds(&layout_choices(Some(&plain), "Music", None)), [ArtistAlbum, Artist, Top, Library, Custom], "the default destination is no Same as Discover");
        assert_eq!(kinds(&layout_choices(None, "Music", None)), [ArtistAlbum, Artist, Top, Custom]);
    }

    #[test]
    fn a_template_that_is_a_preset_is_not_offered_twice_but_where_it_was_saved() {
        let p = Presets::of(Some(&view(user_destination(), &[("Music", Some(LAYOUT_ARTIST_ALBUM)), ("Mixes", Some("  "))])), None);
        assert_eq!(library_template(Some(&p), "Music", None), None);
        let saved = |library: &str| Remembered { library: Some(library.into()), layout: None, kind: Some(Library) };
        assert_eq!(library_template(Some(&p), "Music", Some(&saved("Music"))).as_deref(), Some(LAYOUT_ARTIST_ALBUM));
        assert_eq!(library_template(Some(&p), "Music", Some(&saved("Mixes"))), None);
        assert_eq!(library_template(Some(&p), "Mixes", None), None, "a blank template is none");
        assert_eq!(library_template(Some(&presets()), "Nope", None), None);
        assert_eq!(presets().discover, Some(DiscoverPlace { vpath: "Music".into(), base: "From Discover".into(), layout: LAYOUT_ARTIST_ALBUM.into() }));
        assert_eq!(Presets::of(Some(&view(None, &[])), None).discover, None);
    }

    #[test]
    fn the_torrent_templates_stand_in_for_a_view_without_templates() {
        let torrent = TorrentTemplates {
            vpaths: HashMap::from([
                ("Music".to_string(), TorrentTemplateEntry { template: Some(LIBRARY_LAYOUT.into()) }),
                ("Mixes".to_string(), TorrentTemplateEntry { template: Some(" ".into()) }),
            ]),
            suggested_template: None,
        };
        let p = Presets::of(None, Some(&torrent));
        assert_eq!(p.template_of("Music"), Some(LIBRARY_LAYOUT));
        assert_eq!(p.template_of("Mixes"), None);
        let own = Presets::of(Some(&view(None, &[("Mixes", Some("{{ALBUM}}"))])), Some(&torrent));
        assert_eq!(own.templates, [("Mixes".to_string(), "{{ALBUM}}".to_string())], "the view's own templates win");
    }

    fn libs(names: &[&str]) -> Vec<String> {
        strings(names)
    }

    fn choice(library: &str, layout: &str, kind: LayoutKind, base: &str) -> Choice {
        Choice { library: library.into(), layout: layout.into(), kind, base: base.into() }
    }

    fn saved(library: &str, layout: &str, kind: Option<LayoutKind>) -> Remembered {
        Remembered { library: Some(library.into()), layout: Some(layout.into()), kind }
    }

    #[test]
    fn first_use_takes_discovers_library_else_the_first_and_always_artist_album() {
        let both = libs(&["Music", "Mixes"]);
        assert_eq!(remembered_choice(Some(&presets()), None, &both), choice("Music", LAYOUT_ARTIST_ALBUM, ArtistAlbum, ""));
        assert_eq!(remembered_choice(Some(&presets()), None, &libs(&["Mixes", "Music"])).library, "Music", "Discover's");
        let default_destination = CollectionDestination { source: Some("default".into()), ..user_destination().unwrap() };
        let plain = Presets::of(Some(&view(Some(default_destination), &[])), None);
        assert_eq!(remembered_choice(Some(&plain), None, &libs(&["Mixes", "Music"])).library, "Mixes");
        assert_eq!(remembered_choice(None, None, &[]), choice("", LAYOUT_ARTIST_ALBUM, ArtistAlbum, ""));
    }

    #[test]
    fn a_remembered_choice_follows_its_preset_or_reads_custom_once_it_is_gone() {
        let both = libs(&["Music", "Mixes"]);
        let p = presets();
        assert_eq!(remembered_choice(Some(&p), Some(&saved("Mixes", "", Some(Top))), &both), choice("Mixes", "", Top, ""));
        assert_eq!(remembered_choice(Some(&p), Some(&saved("Gone", "", Some(Top))), &both).library, "Music", "a library the account lost falls back");
        let folded = "From Discover/{{ALBUMARTIST}}/{{ALBUM}}";
        assert_eq!(remembered_choice(Some(&p), Some(&saved("Music", folded, Some(Discover))), &both), choice("Music", LAYOUT_ARTIST_ALBUM, Discover, "From Discover"));
        let gone = Presets::of(Some(&view(None, &[("Music", Some(LIBRARY_LAYOUT))])), None);
        assert_eq!(remembered_choice(Some(&gone), Some(&saved("Music", folded, Some(Discover))), &both), choice("Music", folded, Custom, ""), "the folded copy lives on in Custom");
        assert_eq!(remembered_choice(None, Some(&saved("Music", folded, Some(Discover))), &libs(&["Music"])), choice("Music", folded, Discover, ""), "until the presets answer, the copy");
        assert_eq!(remembered_choice(Some(&p), Some(&saved("Music", "old", Some(Library))), &both), choice("Music", LIBRARY_LAYOUT, Library, ""));
        assert_eq!(remembered_choice(Some(&p), Some(&saved("Mixes", "old", Some(Library))), &both).kind, Custom);
        assert_eq!(remembered_choice(Some(&p), Some(&saved("Music", "{{ALBUMARTIST}}", None)), &both).kind, Artist, "no kind: read off the layout");
        assert_eq!(remembered_choice(Some(&p), Some(&saved("Music", "Singles", None)), &both).kind, Custom);
    }

    #[test]
    fn save_keeps_discovers_base_folders_in_front_and_a_choice_lands_where_its_target_says() {
        let d = choice("Music", LAYOUT_ARTIST_ALBUM, Discover, "From Discover");
        assert_eq!(choice_to_save(&d), saved("Music", "From Discover/{{ALBUMARTIST}}/{{ALBUM}}", Some(Discover)));
        assert_eq!(choice_to_save(&choice("Mixes", "", Top, "")), saved("Mixes", "", Some(Top)));
        assert_eq!(choice_target(&d, &daft_punk()).directory(), "/Music/From Discover/Daft Punk/Discovery/");
        let again = remembered_choice(None, Some(&choice_to_save(&d)), &libs(&["Music"]));
        assert_eq!(choice_target(&again, &daft_punk()).directory(), "/Music/From Discover/Daft Punk/Discovery/", "the remembered copy lands in the same place");
        assert_eq!(choice_target(&choice("Music", "{{ALBUM}}", Custom, "ignored"), &daft_punk()).directory(), "/Music/Discovery/", "only Same as Discover has base folders");
    }

    #[test]
    fn layout_kinds_round_trip_their_config_words() {
        for kind in [ArtistAlbum, Artist, Top, Discover, Library, Custom] {
            assert_eq!(LayoutKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(LayoutKind::parse("folder"), None);
        assert_eq!(Discover.as_str(), "discover", "the phone's and the webapp's spelling");
    }
}
