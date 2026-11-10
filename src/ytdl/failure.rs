//! Failures: why a lookup, a folder-make, an add or a job failed, by the
//! status, the server's `{error}` words and — on a server with the new job
//! list — its `reason` code (the phone's `lib/util/ytdl_errors.dart` and the
//! webapp's classifiers, which add the 409 and the reasons). Then each
//! failure's words, its ways on, and why Add is off.
//!
//! The terminal can tell a refused connection from a dropped one (the
//! webapp can't): a call that never connected certainly started nothing
//! ("Can't reach …"), as on the phone. It has no "You're offline": not
//! knowing whose network it is reads as the server's, the webapp's rule.

use super::{Label, Surface, Tier, format_name, last_folder};

/// How a call failed, as the classifiers read it — what the API layer's
/// error carries, shorn of transport detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallFailure {
    /// A status outside 2xx (or a 2xx the lookup could not read, given
    /// here as its status). `words`: the body's `error` string as sent;
    /// `reason` and `file_name` from a 409.
    Answered { status: u16, words: Option<String>, reason: Option<String>, file_name: Option<String> },
    /// Our own bound ran out.
    TimedOut,
    /// No connection could be made: refused, no such host, no route. The
    /// request never left.
    NeverConnected,
    /// Connected, then no answer (dropped, reset).
    Dropped,
    /// A 2xx whose body would not parse.
    Unreadable,
    /// Not a request failure at all (no client to send with).
    NotSent,
}

impl CallFailure {
    /// The server's answer, read: its status, whether it sent words (an
    /// `error` of only blanks is none — a bare 400/404/500 reads as one,
    /// never as an empty quote), the words, the reason, the file name.
    fn answer(&self) -> Option<Answer<'_>> {
        let CallFailure::Answered { status, words, reason, file_name } = self else { return None };
        let message = words.as_deref().filter(|w| !w.trim().is_empty());
        Some(Answer {
            status: *status,
            message,
            reason: reason.as_deref(),
            file_name: file_name.as_deref().filter(|n| !n.is_empty()),
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Answer<'a> {
    status: u16,
    /// The server's own words; `None` when it sent none.
    message: Option<&'a str>,
    reason: Option<&'a str>,
    file_name: Option<&'a str>,
}

impl Answer<'_> {
    fn words(&self) -> &str {
        self.message.unwrap_or_default()
    }

    /// "yt-dlp is not installed" (6.30+) · "Error - failed to find yt-dlp"
    /// (older).
    fn ytdlp_missing(&self) -> bool {
        let m = self.words().to_lowercase();
        m.contains("yt-dlp") && (m.contains("not installed") || m.contains("failed to find"))
    }

    fn ffmpeg_not_ready(&self) -> bool {
        self.words().to_lowercase().starts_with("ffmpeg is not available")
    }

    /// The server's catch-all ("Server Error"), or no words at all.
    fn bare_server_error(&self) -> bool {
        self.message.is_none_or(|m| m.trim() == "Server Error")
    }
}

/// Which part of an add failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The add or its mkdir.
    Start,
    Lookup,
    /// A job the server took.
    Download,
}

/// Why it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Problem {
    YtdlpMissing,
    /// The add's 500 "FFmpeg is not available yet": the ping said ready and
    /// the route disagreed.
    FfmpegNotReady,
    /// 403: the server's gate, the account's, a jukebox session.
    UploadsOff,
    /// mkdir's 403.
    MkdirOff,
    /// Any other mkdir failure, the folder named.
    MkdirFailed,
    /// A 404 naming the library: gone, or this account lost it.
    LibraryGone,
    /// Express's own 404: no such route.
    NoRoute,
    /// A 400 naming `outputCodec`: the server turned the format off.
    BadFormat,
    /// Any other validation 400, never quoted.
    BadRequest,
    /// A bare 500.
    ServerError,
    /// The lookup's bare 500: yt-dlp exited non-zero.
    LookupFailed,
    TimedOut,
    /// No connection to the server.
    Unreachable,
    /// The add went out and no answer came back: it may be running.
    StartUnsure,
    /// The file name is taken: the 409 before downloading, or a job's
    /// reason after.
    Exists,
    TooBig,
    TooLong,
    Live,
    Unavailable,
    /// Anything else: the server's words, or `HTTP <status>`.
    Other,
    /// A job that failed with no words at all.
    Unknown,
}

impl Problem {
    const ALL: [Problem; 21] = [
        Problem::YtdlpMissing,
        Problem::FfmpegNotReady,
        Problem::UploadsOff,
        Problem::MkdirOff,
        Problem::MkdirFailed,
        Problem::LibraryGone,
        Problem::NoRoute,
        Problem::BadFormat,
        Problem::BadRequest,
        Problem::ServerError,
        Problem::LookupFailed,
        Problem::TimedOut,
        Problem::Unreachable,
        Problem::StartUnsure,
        Problem::Exists,
        Problem::TooBig,
        Problem::TooLong,
        Problem::Live,
        Problem::Unavailable,
        Problem::Other,
        Problem::Unknown,
    ];

    /// Its name in `ytdl.json`, the phone's and the webapp's spelling.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Problem::YtdlpMissing => "ytdlpMissing",
            Problem::FfmpegNotReady => "ffmpegNotReady",
            Problem::UploadsOff => "uploadsOff",
            Problem::MkdirOff => "mkdirOff",
            Problem::MkdirFailed => "mkdirFailed",
            Problem::LibraryGone => "libraryGone",
            Problem::NoRoute => "noRoute",
            Problem::BadFormat => "badFormat",
            Problem::BadRequest => "badRequest",
            Problem::ServerError => "serverError",
            Problem::LookupFailed => "lookupFailed",
            Problem::TimedOut => "timedOut",
            Problem::Unreachable => "unreachable",
            Problem::StartUnsure => "startUnsure",
            Problem::Exists => "exists",
            Problem::TooBig => "tooBig",
            Problem::TooLong => "tooLong",
            Problem::Live => "live",
            Problem::Unavailable => "unavailable",
            Problem::Other => "other",
            Problem::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(word: &str) -> Option<Problem> {
        Problem::ALL.into_iter().find(|p| p.as_str() == word)
    }
}

/// A classified failure. `detail`: for [`Problem::Other`] the words to
/// quote (the server's, or `HTTP <status>`; `None` only for what was no
/// request at all); for [`Problem::Exists`] the file name; for
/// [`Problem::LibraryGone`] the library; for [`Problem::MkdirFailed`] and
/// [`Problem::MkdirOff`] the folder. `reason`: the server's code when it
/// sent one; `text`: the server's own words when it sent any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub problem: Problem,
    pub detail: Option<String>,
    pub reason: Option<String>,
    pub text: Option<String>,
    pub phase: Phase,
}

impl Failure {
    fn of(problem: Problem, phase: Phase) -> Failure {
        Failure { problem, detail: None, reason: None, text: None, phase }
    }

    fn detail(mut self, detail: impl Into<String>) -> Failure {
        self.detail = Some(detail.into());
        self
    }

    fn text(mut self, text: Option<&str>) -> Failure {
        self.text = text.map(str::to_string);
        self
    }
}

fn other(a: &Answer<'_>, phase: Phase) -> Failure {
    let detail = a.message.map_or_else(|| format!("HTTP {}", a.status), str::to_string);
    Failure::of(Problem::Other, phase).detail(detail).text(a.message)
}

/// vpath.js's two 404s, with the library's name in them.
fn library_named(words: &str) -> Option<&str> {
    let m = words.trim();
    m.strip_prefix("Library '").and_then(|rest| rest.strip_suffix("' not found")).filter(|n| !n.is_empty()).or_else(|| {
        m.strip_prefix("User does not have access to path ").filter(|n| !n.is_empty())
    })
}

/// A 404. Express's own (no words) is no such route; one naming the
/// library is a lost library. Any other 404 with words is quoted: on the
/// phone it read as a lost library too, but the new server also says
/// "Folder not found" for a folder that went away, which a lost library
/// would misname.
fn not_found(a: &Answer<'_>, phase: Phase) -> Failure {
    let Some(words) = a.message else { return Failure::of(Problem::NoRoute, phase) };
    match library_named(words) {
        Some(library) => Failure::of(Problem::LibraryGone, phase).detail(library).text(a.message),
        None => other(a, phase),
    }
}

/// Joi's own message starts with the quoted field (`"url" must be …`); the
/// sanitizer says "Invalid YouTube URL - missing video ID". A 400 with no
/// words is taken as the same. Anything else ("Not A Directory") is quoted.
fn bad_request(a: &Answer<'_>, phase: Phase) -> Failure {
    let m = a.words().trim();
    if a.message.is_none() || m.starts_with('"') || m.contains("missing video ID") {
        Failure::of(Problem::BadRequest, phase).text(a.message)
    } else {
        other(a, phase)
    }
}

/// A failed POST /api/v1/ytdl/. An add that went out and got no answer is
/// [`Problem::StartUnsure`] — a client timeout doesn't stop a job the
/// server took, and an unreadable 2xx did start — so it is looked for,
/// never retried (a second POST is a second job). A 504 is a proxy in
/// front of mStream that stopped waiting, so the same. One that never
/// connected certainly started nothing.
pub(crate) fn classify_add(err: &CallFailure) -> Failure {
    let phase = Phase::Start;
    let Some(a) = err.answer() else {
        return Failure::of(
            match err {
                CallFailure::NeverConnected => Problem::Unreachable,
                CallFailure::NotSent => Problem::Other,
                _ => Problem::StartUnsure,
            },
            phase,
        );
    };
    if a.ytdlp_missing() {
        return Failure::of(Problem::YtdlpMissing, phase).text(a.message);
    }
    match a.status {
        // The name check, before anything downloaded.
        409 if a.reason == Some("exists") || a.file_name.is_some() => {
            let f = Failure { reason: Some("exists".into()), ..Failure::of(Problem::Exists, phase).text(a.message) };
            match a.file_name {
                Some(name) => f.detail(name),
                None => f,
            }
        }
        403 => Failure::of(Problem::UploadsOff, phase).text(a.message),
        404 => not_found(&a, phase),
        400 if a.words().contains("outputCodec") => Failure::of(Problem::BadFormat, phase).text(a.message),
        400 => bad_request(&a, phase),
        500 if a.ffmpeg_not_ready() => Failure::of(Problem::FfmpegNotReady, phase).text(a.message),
        500 if a.bare_server_error() => Failure::of(Problem::ServerError, phase).text(a.message),
        504 => Failure::of(Problem::StartUnsure, phase),
        _ => other(&a, phase),
    }
}

/// A failed GET /api/v1/ytdl/metadata. The route has no upload or ffmpeg
/// gate, so a 403 or a 404 with words is quoted.
pub(crate) fn classify_lookup(err: &CallFailure) -> Failure {
    let phase = Phase::Lookup;
    let Some(a) = err.answer() else {
        return Failure::of(
            match err {
                CallFailure::TimedOut => Problem::TimedOut,
                CallFailure::NeverConnected | CallFailure::Dropped => Problem::Unreachable,
                // A 200 this client could not read: as good as no reading.
                CallFailure::Unreadable => Problem::LookupFailed,
                _ => Problem::Other,
            },
            phase,
        );
    };
    if a.ytdlp_missing() {
        return Failure::of(Problem::YtdlpMissing, phase).text(a.message);
    }
    match a.status {
        404 if a.message.is_none() => Failure::of(Problem::NoRoute, phase),
        // Any 400 here is about the link: the query takes nothing else.
        400 => Failure::of(Problem::BadRequest, phase).text(a.message),
        // yt-dlp exiting non-zero is a plain throw on the server: a 500
        // without words reads the same way.
        500 if a.bare_server_error() => Failure::of(Problem::LookupFailed, phase).text(a.message),
        200..=299 => Failure::of(Problem::LookupFailed, phase).text(a.message),
        _ => other(&a, phase),
    }
}

/// A failed POST /api/v1/file-explorer/mkdir, as the server answered it.
pub(crate) fn classify_mkdir(err: &CallFailure) -> Failure {
    let phase = Phase::Start;
    let Some(a) = err.answer() else {
        return Failure::of(
            match err {
                CallFailure::TimedOut => Problem::TimedOut,
                CallFailure::NeverConnected | CallFailure::Dropped => Problem::Unreachable,
                _ => Problem::Other,
            },
            phase,
        );
    };
    match a.status {
        403 => Failure::of(Problem::MkdirOff, phase).text(a.message),
        404 => not_found(&a, phase),
        400 => bad_request(&a, phase),
        // A file in the way of the folder.
        500 if a.bare_server_error() => Failure::of(Problem::ServerError, phase).text(a.message),
        _ => other(&a, phase),
    }
}

/// A failed mkdir as the add shows it: refused (the folder named), a lost
/// library or no connection stay what they are; anything else is
/// "Couldn't make the folder “Discovery”." — the folder named, not the
/// status. `folder`: the first new folder's name (else `directory`'s last).
/// `None` for a 2xx whose body would not parse: any 2xx means the folder is
/// there (the route answers `{}`), so the add goes on.
pub(crate) fn mkdir_failure(err: &CallFailure, folder: Option<&str>, directory: &str) -> Option<Failure> {
    if *err == CallFailure::Unreadable {
        return None;
    }
    let f = classify_mkdir(err);
    let folder = folder.map(str::to_string).filter(|f| !f.is_empty()).unwrap_or_else(|| last_folder(directory));
    Some(match f.problem {
        Problem::MkdirOff => {
            let refused = Failure { text: f.text, ..Failure::of(Problem::MkdirOff, Phase::Start) };
            if folder.is_empty() { refused } else { refused.detail(folder) }
        }
        Problem::Unreachable | Problem::LibraryGone => f,
        _ => Failure { text: f.text, ..Failure::of(Problem::MkdirFailed, Phase::Start).detail(folder) },
    })
}

/// A job entry's failure (GET /api/v1/ytdl/downloads: `status` `error`,
/// `reason`, `error`, `fileName`), or `None` while it hasn't failed. A
/// reason code reads as its own words; without one the server's words are
/// the answer (the bot check and a stale yt-dlp are deliberately reasonless
/// on the server); without words either — every older server — the hedge.
pub(crate) fn entry_failure(
    status: &str,
    reason: Option<&str>,
    error: Option<&str>,
    file_name: Option<&str>,
) -> Option<Failure> {
    if status != "error" {
        return None;
    }
    let text = error.map(str::trim).filter(|e| !e.is_empty());
    let problem = match reason {
        Some("exists") => Some(Problem::Exists),
        Some("too-big") => Some(Problem::TooBig),
        Some("too-long") => Some(Problem::TooLong),
        Some("live") => Some(Problem::Live),
        Some("unavailable") => Some(Problem::Unavailable),
        _ => None,
    };
    let phase = Phase::Download;
    Some(match (problem, text) {
        (Some(problem), _) => {
            let f = Failure { reason: reason.map(str::to_string), ..Failure::of(problem, phase).text(text) };
            match file_name.filter(|n| problem == Problem::Exists && !n.is_empty()) {
                Some(name) => f.detail(name),
                None => f,
            }
        }
        (None, Some(words)) => Failure::of(Problem::Other, phase).detail(words).text(text),
        (None, None) => Failure::of(Problem::Unknown, phase),
    })
}

/// The server's own words to quote for [`Problem::Other`] ("The server
/// said: “…”"); `None` when it sent none ("HTTP 502"): "Something went
/// wrong on the server." then.
pub(crate) fn server_words(f: &Failure) -> Option<&str> {
    if f.problem != Problem::Other {
        return None;
    }
    let d = f.detail.as_deref()?.trim();
    let bare_status = d.strip_prefix("HTTP ").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (!d.is_empty() && !bare_status).then_some(d)
}

/// Where the lookup stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LookupState {
    Idle,
    Loading,
    /// Still looking after 8 s.
    Slow,
    Found,
    Failed,
    TimedOut,
    YtdlpMissing,
    Unreachable,
}

/// Where the lookup stands after a failure.
pub(crate) fn lookup_state_of(f: &Failure) -> LookupState {
    match f.problem {
        Problem::YtdlpMissing => LookupState::YtdlpMissing,
        Problem::TimedOut => LookupState::TimedOut,
        Problem::Unreachable => LookupState::Unreachable,
        _ => LookupState::Failed,
    }
}

/// What a failure's words depend on besides the failure: the surface, the
/// server's name ("Can't reach Home server"), the format the failed add
/// asked for ("doesn't save Opus"), and the tier (an older server's job
/// failures come with no words, so their hedge is its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Words<'a> {
    pub surface: Surface,
    pub server: &'a str,
    pub codec: &'a str,
    pub tier: Tier,
}

/// A failure in words. A job's failure reads the same wherever it shows; a
/// lookup's as a lookup's.
pub(crate) fn failure_label(f: &Failure, w: &Words<'_>) -> Label {
    let said = |words: Option<&str>| match words {
        Some(text) => Label::new("gui.ytdl.err.said").with("text", text),
        None => Label::new("gui.ytdl.err.server"),
    };
    let text = f.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let detail = f.detail.clone().unwrap_or_default();
    match f.phase {
        Phase::Download => match f.problem {
            Problem::Exists if !detail.is_empty() => Label::new("gui.ytdl.list.landed").with("name", detail),
            Problem::Exists => Label::new("gui.ytdl.list.landed_unnamed"),
            Problem::TooBig => Label::new("gui.ytdl.list.too_big"),
            Problem::TooLong => Label::new("gui.ytdl.list.too_long"),
            Problem::Live => Label::new("gui.ytdl.list.live"),
            Problem::Unavailable => Label::new("gui.ytdl.list.unavailable"),
            Problem::Other => match server_words(f) {
                Some(text) => Label::new("gui.ytdl.list.said").with("text", text),
                None => Label::new("gui.ytdl.list.server"),
            },
            // No words at all: the new server always sends some; before
            // it, the phone's hedge — the common causes from 6.30 (live
            // streams, YouTube refusing the server, 500 MB, an hour), only
            // YouTube's refusal before (no live or size checks there).
            _ => Label::new(match w.tier {
                Tier::New => "gui.ytdl.list.server",
                Tier::Full => "gui.ytdl.list.hedged",
                Tier::Older => "gui.ytdl.list.hedged_older",
            }),
        },
        Phase::Lookup => match f.problem {
            Problem::TimedOut => Label::new("gui.ytdl.lookup.timed_out"),
            Problem::YtdlpMissing => Label::new("gui.ytdl.server.no_ytdlp"),
            Problem::Unreachable => Label::new("gui.ytdl.server.unreachable").with("server", w.server),
            Problem::NoRoute => Label::new("gui.ytdl.err.no_route"),
            Problem::BadRequest => Label::new("gui.ytdl.err.bad_request"),
            Problem::Other => said(server_words(f)),
            _ => Label::new("gui.ytdl.lookup.failed"),
        },
        Phase::Start => {
            let list = w.surface == Surface::List;
            match f.problem {
                Problem::YtdlpMissing => Label::new("gui.ytdl.server.no_ytdlp"),
                Problem::FfmpegNotReady => Label::new("gui.ytdl.err.ffmpeg"),
                Problem::UploadsOff => Label::new("gui.ytdl.err.uploads_off"),
                Problem::MkdirOff if !detail.is_empty() => Label::new("gui.ytdl.no_mkdir.new").with("folder", detail),
                Problem::MkdirOff => Label::new("gui.ytdl.no_mkdir.unchecked"),
                Problem::MkdirFailed => Label::new("gui.ytdl.err.mkdir").with("folder", detail),
                // The window and a row can't pick another library: the
                // first sentence only.
                Problem::LibraryGone => {
                    let key =
                        if w.surface == Surface::Room { "gui.ytdl.err.library_gone" } else { "gui.ytdl.err.library_gone_short" };
                    Label::new(key).with("library", detail)
                }
                Problem::NoRoute => Label::new("gui.ytdl.err.no_route"),
                Problem::BadFormat => Label::new("gui.ytdl.err.format_off").with("format", format_name(w.codec)),
                Problem::BadRequest => Label::new("gui.ytdl.err.bad_request"),
                Problem::Exists if !detail.is_empty() => Label::new("gui.ytdl.err.exists").with("name", detail),
                Problem::Exists => said(text),
                Problem::Unreachable if list => Label::new("gui.ytdl.list.didnt_start"),
                Problem::Unreachable => Label::new("gui.ytdl.err.add_unreachable").with("server", w.server),
                Problem::StartUnsure if list => Label::new("gui.ytdl.list.not_sure_started"),
                Problem::StartUnsure => Label::new("gui.ytdl.err.start_unsure"),
                Problem::Other => said(server_words(f)),
                _ => Label::new("gui.ytdl.err.server"),
            }
        }
    }
}

/// A failure's ways on, Dismiss last. The list words them: "a try again",
/// "Change format and retry", "c choose a folder", "Check again", "x
/// dismiss".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Retry,
    ChangeFormat,
    ChooseFolder,
    TopOfLibrary,
    CheckAgain,
    Dismiss,
}

/// The ways on. A retry that would fail the same way after another full
/// download is not offered (the phone's rule for a clash): a name taken or
/// a file over the cap gets Change format instead — and that only when the
/// server has another format (`formats`, the menu's options; `None`: not
/// known, offered); a video YouTube won't give the server, Dismiss alone.
pub(crate) fn failure_actions(f: &Failure, formats: Option<&[&str]>) -> Vec<Action> {
    let another = formats.is_none_or(|f| f.len() > 1);
    let change_format = if another { vec![Action::ChangeFormat, Action::Dismiss] } else { vec![Action::Dismiss] };
    if f.phase == Phase::Download {
        return match f.problem {
            Problem::Exists | Problem::TooBig => change_format,
            Problem::Unavailable => vec![Action::Dismiss],
            _ => vec![Action::Retry, Action::Dismiss],
        };
    }
    match f.problem {
        Problem::MkdirFailed => vec![Action::Retry, Action::ChooseFolder, Action::Dismiss],
        Problem::MkdirOff => vec![Action::TopOfLibrary, Action::ChooseFolder, Action::Dismiss],
        Problem::UploadsOff | Problem::LibraryGone | Problem::NoRoute => vec![Action::Dismiss],
        Problem::BadFormat | Problem::Exists => change_format,
        Problem::StartUnsure => vec![Action::CheckAgain, Action::Dismiss],
        _ => vec![Action::Retry, Action::Dismiss],
    }
}

// ── why Add is off ──────────────────────────────────────────────────────────

/// Why Add is off, the first that holds in the phone's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddBlock {
    None,
    /// The server's audio conversion is off (a re-ping after "FFmpeg is
    /// not available yet" said so).
    ConvertOff,
    NoLink,
    Unreachable,
    YtdlpMissing,
    /// Until found, or "t type the details".
    LookingUp,
    UploadsOff,
    AlreadyAdding,
    InvalidLayout,
    YearInvalid,
    /// The account can't make folders and one is new or unchecked.
    MkdirBlocked,
    /// Before 6.30 a same-name file would be re-tagged, not added.
    ClashOlder,
}

/// What [`add_block`] weighs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AddState {
    pub convert_off: bool,
    /// The link field holds one video.
    pub video: bool,
    pub lookup: LookupState,
    /// The details are being typed by hand ("t type the details").
    pub manual: bool,
    pub uploads_off: bool,
    /// This surface's own add is starting: the open job is its own.
    pub starting: bool,
    /// An open job adds this link to this folder.
    pub already_adding: bool,
    pub target_valid: bool,
    pub year_invalid: bool,
    pub mkdir_blocked: bool,
    /// [`super::NameCheck::blocks`].
    pub clash_older: bool,
}

pub(crate) fn add_block(s: &AddState) -> AddBlock {
    if s.convert_off {
        return AddBlock::ConvertOff;
    }
    if !s.video {
        return AddBlock::NoLink;
    }
    match s.lookup {
        LookupState::Unreachable => return AddBlock::Unreachable,
        LookupState::YtdlpMissing => return AddBlock::YtdlpMissing,
        LookupState::Idle | LookupState::Loading | LookupState::Slow if !s.manual => return AddBlock::LookingUp,
        _ => {}
    }
    if s.uploads_off {
        return AddBlock::UploadsOff;
    }
    if !s.starting && s.already_adding {
        return AddBlock::AlreadyAdding;
    }
    if !s.target_valid {
        return AddBlock::InvalidLayout;
    }
    if s.year_invalid {
        return AddBlock::YearInvalid;
    }
    if s.mkdir_blocked {
        return AddBlock::MkdirBlocked;
    }
    if s.clash_older {
        return AddBlock::ClashOlder;
    }
    AddBlock::None
}

/// The line a block shows (the reason is never only in a hover), or
/// `None` where something else says it: looking up (the reading line), no
/// link (the link line), can't make folders (its own line), an older
/// clash (the file line), nothing.
pub(crate) fn block_label(block: AddBlock, server: &str) -> Option<Label> {
    Some(match block {
        AddBlock::ConvertOff => Label::new("gui.ytdl.err.ffmpeg"),
        AddBlock::Unreachable => Label::new("gui.ytdl.server.unreachable").with("server", server),
        AddBlock::YtdlpMissing => Label::new("gui.ytdl.server.no_ytdlp"),
        AddBlock::UploadsOff => Label::new("gui.ytdl.err.uploads_off"),
        AddBlock::AlreadyAdding => Label::new("gui.ytdl.already_adding"),
        AddBlock::InvalidLayout => Label::new("gui.ytdl.where.fix_layout"),
        AddBlock::YearInvalid => Label::new("gui.ytdl.year_invalid"),
        AddBlock::None | AddBlock::NoLink | AddBlock::LookingUp | AddBlock::MkdirBlocked | AddBlock::ClashOlder => {
            return None;
        }
    })
}

/// The primary's words: "Add to library" (the room) / "Add here" (the
/// window), "Add anyway" over a clash, "Adding…" while the server hasn't
/// answered.
pub(crate) fn add_button_label(surface: Surface, clash: bool, starting: bool) -> Label {
    Label::new(if starting {
        "gui.ytdl.add.adding"
    } else if clash {
        "gui.ytdl.add.anyway"
    } else if surface == Surface::Window {
        "gui.ytdl.add.here"
    } else {
        "gui.ytdl.add.to_library"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(status: u16, words: &str) -> CallFailure {
        CallFailure::Answered { status, words: Some(words.into()), reason: None, file_name: None }
    }

    fn said_409(words: &str, reason: Option<&str>, file_name: Option<&str>) -> CallFailure {
        CallFailure::Answered { status: 409, words: Some(words.into()), reason: reason.map(str::to_string), file_name: file_name.map(str::to_string) }
    }

    fn bare(status: u16) -> CallFailure {
        CallFailure::Answered { status, words: None, reason: None, file_name: None }
    }

    fn pick(f: &Failure) -> (Problem, Option<&str>) {
        (f.problem, f.detail.as_deref())
    }

    use Problem::*;

    #[test]
    fn the_add_reads_the_phones_table_and_the_new_409() {
        let rows: Vec<(CallFailure, (Problem, Option<&str>))> = vec![
            // yt-dlp: 6.30+ wording and the older one.
            (said(500, "yt-dlp is not installed"), (YtdlpMissing, None)),
            (said(500, "Error - failed to find yt-dlp"), (YtdlpMissing, None)),
            (said(500, "YT-DLP is not installed"), (YtdlpMissing, None)),
            (said(500, "FFmpeg is not available yet"), (FfmpegNotReady, None)),
            (said(403, "Uploading Disabled"), (UploadsOff, None)),
            (said(403, "Forbidden"), (UploadsOff, None)),
            (said(403, "not available in a jukebox session"), (UploadsOff, None)),
            (bare(403), (UploadsOff, None)),
            (said(404, "Library 'Music' not found"), (LibraryGone, Some("Music"))),
            (said(404, "User does not have access to path Music"), (LibraryGone, Some("Music"))),
            (said(404, "Library 'Old Vinyl' not found"), (LibraryGone, Some("Old Vinyl"))),
            (said(404, "Folder not found"), (Other, Some("Folder not found"))),
            (said(404, "something else not found"), (Other, Some("something else not found"))),
            (bare(404), (NoRoute, None)),
            (said(400, "\"outputCodec\" must be one of [mp3, flac, wav, ogg, aac, m4a, m4b, opus]"), (BadFormat, None)),
            (said(400, "\"url\" must be a valid uri with a scheme matching the http|https pattern"), (BadRequest, None)),
            (said(400, "\"url\" failed custom validation because URL must be a YouTube link"), (BadRequest, None)),
            (said(400, "\"url\" is required"), (BadRequest, None)),
            (said(400, "\"metadata.year\" must be a string"), (BadRequest, None)),
            (said(400, "\"foo\" is not allowed"), (BadRequest, None)),
            (said(400, "Invalid YouTube URL - missing video ID"), (BadRequest, None)),
            (bare(400), (BadRequest, None)),
            (said(400, "Not A Directory"), (Other, Some("Not A Directory"))),
            (said_409("“One_More_Time.mp3” is already in this folder, so nothing was added.", Some("exists"), Some("One_More_Time.mp3")), (Exists, Some("One_More_Time.mp3"))),
            (said(409, "busy"), (Other, Some("busy"))),
            (said(500, "Server Error"), (ServerError, None)),
            (said(500, "  Server Error  "), (ServerError, None)),
            (bare(500), (ServerError, None)),
            (said(500, "Server Error: boom"), (Other, Some("Server Error: boom"))),
            (said(500, "disk full"), (Other, Some("disk full"))),
            (said(429, "slow down"), (Other, Some("slow down"))),
            (bare(502), (Other, Some("HTTP 502"))),
            // Words of only blanks are no words.
            (said(400, ""), (BadRequest, None)),
            (said(404, "  "), (NoRoute, None)),
            (said(500, ""), (ServerError, None)),
            (said(401, " "), (Other, Some("HTTP 401"))),
            (said_409("", Some("exists"), Some("One_More_Time.mp3")), (Exists, Some("One_More_Time.mp3"))),
            // Went out, no answer: it may have started.
            (CallFailure::TimedOut, (StartUnsure, None)),
            (CallFailure::Dropped, (StartUnsure, None)),
            (CallFailure::Unreadable, (StartUnsure, None)),
            (bare(504), (StartUnsure, None)),
            (said(504, "Gateway Timeout"), (StartUnsure, None)),
            // Never left: certainly nothing started.
            (CallFailure::NeverConnected, (Unreachable, None)),
            (CallFailure::NotSent, (Other, None)),
        ];
        for (err, want) in rows {
            let f = classify_add(&err);
            assert_eq!(pick(&f), want, "{err:?}");
            assert_eq!(f.phase, Phase::Start);
        }
        let f = classify_add(&said_409("“One_More_Time.mp3” is already in this folder, so nothing was added.", Some("exists"), Some("One_More_Time.mp3")));
        assert_eq!(f.reason.as_deref(), Some("exists"));
        assert_eq!(f.text.as_deref(), Some("“One_More_Time.mp3” is already in this folder, so nothing was added."));
        assert_eq!(pick(&classify_add(&said_409("x", None, Some("a.mp3")))), (Exists, Some("a.mp3")));
    }

    #[test]
    fn the_lookup_reads_its_own_table() {
        let rows: Vec<(CallFailure, (Problem, Option<&str>))> = vec![
            (said(500, "yt-dlp is not installed"), (YtdlpMissing, None)),
            (said(500, "Error - failed to find yt-dlp"), (YtdlpMissing, None)),
            (said(500, "Server Error"), (LookupFailed, None)),
            (bare(500), (LookupFailed, None)),
            (bare(200), (LookupFailed, None)),
            (CallFailure::Unreadable, (LookupFailed, None)),
            (said(400, "Invalid YouTube URL - missing video ID"), (BadRequest, None)),
            (said(400, "\"url\" is required"), (BadRequest, None)),
            (bare(400), (BadRequest, None)),
            (bare(404), (NoRoute, None)),
            (said(403, "Forbidden"), (Other, Some("Forbidden"))),
            (said(404, "Library 'Music' not found"), (Other, Some("Library 'Music' not found"))),
            (said(500, "FFmpeg is not available yet"), (Other, Some("FFmpeg is not available yet"))),
            (bare(503), (Other, Some("HTTP 503"))),
            (said(500, "   "), (LookupFailed, None)),
            (said(404, ""), (NoRoute, None)),
            (said(403, " "), (Other, Some("HTTP 403"))),
            (CallFailure::TimedOut, (TimedOut, None)),
            (CallFailure::NeverConnected, (Unreachable, None)),
            (CallFailure::Dropped, (Unreachable, None)),
            (CallFailure::NotSent, (Other, None)),
        ];
        for (err, want) in rows {
            let f = classify_lookup(&err);
            assert_eq!(pick(&f), want, "{err:?}");
            assert_eq!(f.phase, Phase::Lookup);
        }
        for (p, state) in [
            (YtdlpMissing, LookupState::YtdlpMissing),
            (TimedOut, LookupState::TimedOut),
            (Unreachable, LookupState::Unreachable),
            (LookupFailed, LookupState::Failed),
            (BadRequest, LookupState::Failed),
            (NoRoute, LookupState::Failed),
            (Other, LookupState::Failed),
        ] {
            assert_eq!(lookup_state_of(&Failure::of(p, Phase::Lookup)), state, "{p:?}");
        }
    }

    #[test]
    fn mkdir_reads_its_table_and_names_the_folder_as_the_add_shows_it() {
        let rows: Vec<(CallFailure, (Problem, Option<&str>))> = vec![
            (said(403, "Create Folder Disabled"), (MkdirOff, None)),
            (said(404, "Library 'Music' not found"), (LibraryGone, Some("Music"))),
            (said(404, "User does not have access to path Music"), (LibraryGone, Some("Music"))),
            (bare(404), (NoRoute, None)),
            (said(500, "Server Error"), (ServerError, None)),
            (bare(500), (ServerError, None)),
            (said(400, "\"directory\" is required"), (BadRequest, None)),
            (said(500, "EACCES"), (Other, Some("EACCES"))),
            (said(500, " "), (ServerError, None)),
            (said(400, ""), (BadRequest, None)),
            (CallFailure::TimedOut, (TimedOut, None)),
            (CallFailure::NeverConnected, (Unreachable, None)),
        ];
        for (err, want) in rows {
            assert_eq!(pick(&classify_mkdir(&err)), want, "{err:?}");
        }
        let dir = "/Music/Daft Punk/Discovery/";
        let shown = |err: &CallFailure| mkdir_failure(err, Some("Discovery"), dir).map(|f| (f.problem, f.detail));
        assert_eq!(shown(&said(403, "Create Folder Disabled")), Some((MkdirOff, Some("Discovery".into()))));
        assert_eq!(shown(&said(404, "Library 'Music' not found")), Some((LibraryGone, Some("Music".into()))));
        assert_eq!(shown(&CallFailure::NeverConnected), Some((Unreachable, None)));
        for err in [said(500, "Server Error"), said(500, "EACCES"), CallFailure::TimedOut, bare(404), said(400, "\"directory\" is required"), CallFailure::NotSent] {
            assert_eq!(shown(&err), Some((MkdirFailed, Some("Discovery".into()))), "{err:?}");
        }
        assert_eq!(mkdir_failure(&bare(500), None, dir).map(|f| f.detail), Some(Some("Discovery".into())), "the path's last folder");
        assert_eq!(mkdir_failure(&CallFailure::Unreadable, Some("Discovery"), dir), None, "any 2xx: the folder is there");
        assert_eq!(mkdir_failure(&said(403, "x"), None, "").map(|f| f.detail), Some(None), "no folder to name");
    }

    #[test]
    fn a_jobs_entry_reads_its_reason_its_words_or_nothing() {
        let e = |reason: Option<&str>, error: Option<&str>, name: Option<&str>| entry_failure("error", reason, error, name);
        let pick_e = |f: Option<Failure>| f.map(|f| (f.problem, f.detail));
        assert_eq!(pick_e(e(Some("exists"), Some("a file named One_More_Time.mp3 already exists in Music — nothing was replaced"), Some("One_More_Time.mp3"))), Some((Exists, Some("One_More_Time.mp3".into()))));
        assert_eq!(pick_e(e(Some("exists"), Some("words"), None)), Some((Exists, None)));
        assert_eq!(pick_e(e(Some("too-big"), Some("words"), None)), Some((TooBig, None)));
        assert_eq!(pick_e(e(Some("too-long"), Some("words"), None)), Some((TooLong, None)));
        assert_eq!(pick_e(e(Some("live"), Some("words"), None)), Some((Live, None)));
        assert_eq!(pick_e(e(Some("unavailable"), Some("words"), Some("x.mp3"))), Some((Unavailable, None)), "only exists names a file");
        let bot = "ERROR: [youtube] x: Sign in to confirm you’re not a bot.";
        assert_eq!(pick_e(e(None, Some(bot), None)), Some((Other, Some(bot.into()))));
        assert_eq!(pick_e(e(Some("adding"), Some("a code from a newer server"), None)), Some((Other, Some("a code from a newer server".into()))));
        assert_eq!(pick_e(e(None, Some("  "), None)), Some((Unknown, None)));
        assert_eq!(pick_e(e(None, None, None)), Some((Unknown, None)));
        assert_eq!(entry_failure("downloading", None, None, None), None);
        assert_eq!(entry_failure("complete", None, Some("x"), None), None);
        let f = e(Some("too-long"), Some("yt-dlp took longer than 60 minutes — stopped"), None).expect("a failure");
        assert_eq!((f.phase, f.reason.as_deref(), f.text.as_deref()), (Phase::Download, Some("too-long"), Some("yt-dlp took longer than 60 minutes — stopped")));
    }

    #[test]
    fn the_words_to_quote_are_the_servers_never_a_bare_status() {
        let o = |d: Option<&str>| Failure { detail: d.map(str::to_string), ..Failure::of(Other, Phase::Start) };
        assert_eq!(server_words(&o(Some("Not A Directory"))), Some("Not A Directory"));
        assert_eq!(server_words(&o(Some("HTTP 502"))), None);
        assert_eq!(server_words(&o(Some("HTTP 5x2"))), Some("HTTP 5x2"));
        assert_eq!(server_words(&o(Some("  "))), None);
        assert_eq!(server_words(&o(None)), None);
        assert_eq!(server_words(&Failure::of(LibraryGone, Phase::Start).detail("Music")), None);
    }

    fn words(surface: Surface, tier: Tier) -> Words<'static> {
        Words { surface, server: "Home server", codec: "opus", tier }
    }

    fn says(l: Label) -> (&'static str, Vec<(&'static str, String)>) {
        (l.key, l.params)
    }

    fn key(k: &'static str) -> (&'static str, Vec<(&'static str, String)>) {
        (k, Vec::new())
    }

    fn keyed(k: &'static str, name: &'static str, value: &str) -> (&'static str, Vec<(&'static str, String)>) {
        (k, vec![(name, value.to_string())])
    }

    #[test]
    fn a_start_failure_reads_inline_above_the_rooms_primary() {
        let room = words(Surface::Room, Tier::New);
        let dir = "/Music/Daft Punk/Discovery/";
        let rows = vec![
            (classify_add(&said(500, "yt-dlp is not installed")), key("gui.ytdl.server.no_ytdlp")),
            (classify_add(&said(500, "FFmpeg is not available yet")), key("gui.ytdl.err.ffmpeg")),
            (classify_add(&said(403, "Uploading Disabled")), key("gui.ytdl.err.uploads_off")),
            (mkdir_failure(&said(403, "Create Folder Disabled"), Some("Discovery"), dir).unwrap(), keyed("gui.ytdl.no_mkdir.new", "folder", "Discovery")),
            (mkdir_failure(&said(403, "Create Folder Disabled"), None, "").unwrap(), key("gui.ytdl.no_mkdir.unchecked")),
            (mkdir_failure(&bare(500), Some("Discovery"), dir).unwrap(), keyed("gui.ytdl.err.mkdir", "folder", "Discovery")),
            (classify_add(&said(404, "Library 'Music' not found")), keyed("gui.ytdl.err.library_gone", "library", "Music")),
            (classify_add(&bare(404)), key("gui.ytdl.err.no_route")),
            (classify_add(&said(400, "\"outputCodec\" must be one of [mp3]")), keyed("gui.ytdl.err.format_off", "format", "Opus")),
            (classify_add(&said(400, "\"url\" is required")), key("gui.ytdl.err.bad_request")),
            (classify_add(&said_409("x", Some("exists"), Some("One_More_Time.mp3"))), keyed("gui.ytdl.err.exists", "name", "One_More_Time.mp3")),
            (classify_add(&said_409("“x” is already in this folder", Some("exists"), None)), keyed("gui.ytdl.err.said", "text", "“x” is already in this folder")),
            (mkdir_failure(&CallFailure::NeverConnected, None, dir).unwrap(), keyed("gui.ytdl.err.add_unreachable", "server", "Home server")),
            (classify_add(&CallFailure::NeverConnected), keyed("gui.ytdl.err.add_unreachable", "server", "Home server")),
            (classify_add(&CallFailure::TimedOut), key("gui.ytdl.err.start_unsure")),
            (classify_add(&said(400, "Not A Directory")), keyed("gui.ytdl.err.said", "text", "Not A Directory")),
            (classify_add(&bare(502)), key("gui.ytdl.err.server")),
            (classify_add(&said(500, "Server Error")), key("gui.ytdl.err.server")),
            (classify_add(&CallFailure::NotSent), key("gui.ytdl.err.server")),
            (classify_mkdir(&CallFailure::TimedOut), key("gui.ytdl.err.server")),
        ];
        for (f, want) in rows {
            assert_eq!(says(failure_label(&f, &room)), want, "{f:?}");
        }
    }

    #[test]
    fn the_window_and_a_row_give_a_lost_library_its_first_sentence_and_no_connection_didnt_start() {
        let gone = classify_add(&said(404, "Library 'Music' not found"));
        for surface in [Surface::Window, Surface::List] {
            assert_eq!(says(failure_label(&gone, &words(surface, Tier::New))), keyed("gui.ytdl.err.library_gone_short", "library", "Music"));
        }
        let list = words(Surface::List, Tier::New);
        assert_eq!(says(failure_label(&classify_add(&CallFailure::NeverConnected), &list)), key("gui.ytdl.list.didnt_start"));
        assert_eq!(says(failure_label(&classify_add(&CallFailure::Dropped), &list)), key("gui.ytdl.list.not_sure_started"));
        assert_eq!(says(failure_label(&classify_add(&said(400, "Not A Directory")), &list)), keyed("gui.ytdl.err.said", "text", "Not A Directory"));
        assert_eq!(says(failure_label(&classify_add(&CallFailure::NeverConnected), &words(Surface::Window, Tier::New))).0, "gui.ytdl.err.add_unreachable");
    }

    #[test]
    fn a_lookup_failure_reads_as_one_anywhere() {
        let look = |err: CallFailure| says(failure_label(&classify_lookup(&err), &words(Surface::Room, Tier::New)));
        assert_eq!(look(said(500, "Server Error")), key("gui.ytdl.lookup.failed"));
        assert_eq!(look(CallFailure::TimedOut), key("gui.ytdl.lookup.timed_out"));
        assert_eq!(look(said(500, "yt-dlp is not installed")), key("gui.ytdl.server.no_ytdlp"));
        assert_eq!(look(CallFailure::NeverConnected), keyed("gui.ytdl.server.unreachable", "server", "Home server"));
        assert_eq!(look(bare(404)), key("gui.ytdl.err.no_route"));
        assert_eq!(look(said(400, "\"url\" is required")), key("gui.ytdl.err.bad_request"));
        assert_eq!(look(said(403, "not available in a jukebox session")), keyed("gui.ytdl.err.said", "text", "not available in a jukebox session"));
        assert_eq!(look(bare(503)), key("gui.ytdl.err.server"));
        let window = says(failure_label(&classify_lookup(&CallFailure::TimedOut), &words(Surface::Window, Tier::New)));
        assert_eq!(window, key("gui.ytdl.lookup.timed_out"));
    }

    #[test]
    fn a_jobs_failure_reads_the_same_everywhere_and_an_older_server_gets_the_hedge() {
        let e = |reason: Option<&str>, error: Option<&str>, name: Option<&str>| entry_failure("error", reason, error, name).expect("failed");
        let rows = vec![
            (e(Some("exists"), Some("x"), Some("One_More_Time.mp3")), keyed("gui.ytdl.list.landed", "name", "One_More_Time.mp3")),
            (e(Some("exists"), Some("x"), None), key("gui.ytdl.list.landed_unnamed")),
            (e(Some("too-big"), Some("x"), None), key("gui.ytdl.list.too_big")),
            (e(Some("too-long"), Some("x"), None), key("gui.ytdl.list.too_long")),
            (e(Some("live"), Some("x"), None), key("gui.ytdl.list.live")),
            (e(Some("unavailable"), Some("x"), None), key("gui.ytdl.list.unavailable")),
            (e(None, Some("Sign in to confirm you’re not a bot."), None), keyed("gui.ytdl.list.said", "text", "Sign in to confirm you’re not a bot.")),
            (e(None, None, None), key("gui.ytdl.list.server")),
            (e(None, Some("HTTP 502"), None), key("gui.ytdl.list.server")),
        ];
        for (f, want) in rows {
            for surface in [Surface::Room, Surface::Window, Surface::List] {
                assert_eq!(says(failure_label(&f, &words(surface, Tier::New))), want, "{f:?} {surface:?}");
            }
        }
        let silent = e(None, None, None);
        assert_eq!(says(failure_label(&silent, &words(Surface::List, Tier::Full))), key("gui.ytdl.list.hedged"));
        assert_eq!(says(failure_label(&silent, &words(Surface::List, Tier::Older))), key("gui.ytdl.list.hedged_older"));
        // The full tier's after-download clash, as the tracker settles it.
        let clash = Failure { detail: None, ..Failure::of(Exists, Phase::Download) };
        assert_eq!(says(failure_label(&clash, &words(Surface::List, Tier::Full))), key("gui.ytdl.list.landed_unnamed"));
    }

    #[test]
    fn ways_on_never_offer_a_retry_that_would_fail_the_same_way() {
        let formats: &[&str] = &["mp3", "opus"];
        let acts = |f: &Failure| failure_actions(f, Some(formats));
        let e = |reason: Option<&str>, error: Option<&str>| entry_failure("error", reason, error, None).expect("failed");
        use Action::*;
        assert_eq!(acts(&e(Some("exists"), Some("x"))), [ChangeFormat, Dismiss]);
        assert_eq!(failure_actions(&e(Some("exists"), Some("x")), Some(&["mp3"])), [Dismiss]);
        assert_eq!(acts(&e(Some("too-big"), Some("x"))), [ChangeFormat, Dismiss]);
        assert_eq!(acts(&e(Some("unavailable"), Some("x"))), [Dismiss]);
        assert_eq!(acts(&e(Some("too-long"), Some("x"))), [Retry, Dismiss]);
        assert_eq!(acts(&e(Some("live"), Some("x"))), [Retry, Dismiss]);
        assert_eq!(acts(&e(None, Some("Sign in"))), [Retry, Dismiss]);
        assert_eq!(acts(&e(None, None)), [Retry, Dismiss]);
        let dir = "/Music/x/";
        assert_eq!(acts(&mkdir_failure(&bare(500), Some("Discovery"), dir).unwrap()), [Retry, ChooseFolder, Dismiss]);
        assert_eq!(acts(&mkdir_failure(&said(403, "Create Folder Disabled"), Some("Discovery"), dir).unwrap()), [TopOfLibrary, ChooseFolder, Dismiss]);
        for err in [said(403, "Uploading Disabled"), said(404, "Library 'Music' not found"), bare(404)] {
            assert_eq!(acts(&classify_add(&err)), [Dismiss], "{err:?}");
        }
        assert_eq!(acts(&classify_add(&said(400, "\"outputCodec\" must be one of [mp3]"))), [ChangeFormat, Dismiss]);
        assert_eq!(acts(&classify_add(&said_409("x", Some("exists"), Some("a.mp3")))), [ChangeFormat, Dismiss]);
        assert_eq!(acts(&classify_add(&CallFailure::TimedOut)), [CheckAgain, Dismiss]);
        assert_eq!(acts(&mkdir_failure(&CallFailure::NeverConnected, None, dir).unwrap()), [Retry, Dismiss]);
        assert_eq!(acts(&classify_add(&said(500, "disk full"))), [Retry, Dismiss]);
        assert_eq!(failure_actions(&classify_add(&said_409("x", Some("exists"), Some("a.mp3"))), None), [ChangeFormat, Dismiss], "formats unknown: offered");
        assert_eq!(acts(&classify_lookup(&CallFailure::TimedOut)), [Retry, Dismiss], "a try again");
    }

    fn ready() -> AddState {
        AddState {
            convert_off: false,
            video: true,
            lookup: LookupState::Found,
            manual: false,
            uploads_off: false,
            starting: false,
            already_adding: false,
            target_valid: true,
            year_invalid: false,
            mkdir_blocked: false,
            clash_older: false,
        }
    }

    #[test]
    fn nothing_in_the_way_is_none_and_each_reason_wins_over_every_later_one() {
        assert_eq!(add_block(&ready()), AddBlock::None);
        type Set = fn(&mut AddState);
        let order: [(AddBlock, Set); 11] = [
            (AddBlock::ConvertOff, |s| s.convert_off = true),
            (AddBlock::NoLink, |s| s.video = false),
            (AddBlock::Unreachable, |s| s.lookup = LookupState::Unreachable),
            (AddBlock::YtdlpMissing, |s| s.lookup = LookupState::YtdlpMissing),
            (AddBlock::LookingUp, |s| s.lookup = LookupState::Loading),
            (AddBlock::UploadsOff, |s| s.uploads_off = true),
            (AddBlock::AlreadyAdding, |s| s.already_adding = true),
            (AddBlock::InvalidLayout, |s| s.target_valid = false),
            (AddBlock::YearInvalid, |s| s.year_invalid = true),
            (AddBlock::MkdirBlocked, |s| s.mkdir_blocked = true),
            (AddBlock::ClashOlder, |s| s.clash_older = true),
        ];
        for i in 0..order.len() {
            // This reason and every later one hold: this one is the answer.
            // (Applied last to first, so of the lookup states the one under
            // test is the one set.)
            let mut s = ready();
            for (_, set) in order[i..].iter().rev() {
                set(&mut s);
            }
            assert_eq!(add_block(&s), order[i].0, "{:?}", order[i].0);
        }
    }

    #[test]
    fn looking_up_holds_until_found_unless_the_details_are_typed() {
        for lookup in [LookupState::Idle, LookupState::Loading, LookupState::Slow] {
            assert_eq!(add_block(&AddState { lookup, ..ready() }), AddBlock::LookingUp, "{lookup:?}");
        }
        assert_eq!(add_block(&AddState { lookup: LookupState::Slow, manual: true, ..ready() }), AddBlock::None);
        assert_eq!(add_block(&AddState { lookup: LookupState::Failed, ..ready() }), AddBlock::None, "the fields are open");
        assert_eq!(add_block(&AddState { lookup: LookupState::TimedOut, ..ready() }), AddBlock::None);
        assert_eq!(add_block(&AddState { lookup: LookupState::Unreachable, manual: true, ..ready() }), AddBlock::Unreachable, "no connection blocks a hand-made add too");
        assert_eq!(add_block(&AddState { already_adding: true, starting: true, ..ready() }), AddBlock::None, "this surface's own add");
    }

    #[test]
    fn the_reason_is_on_screen_and_the_primary_says_what_it_does() {
        let rows = [
            (AddBlock::ConvertOff, Some(key("gui.ytdl.err.ffmpeg"))),
            (AddBlock::Unreachable, Some(keyed("gui.ytdl.server.unreachable", "server", "Home server"))),
            (AddBlock::YtdlpMissing, Some(key("gui.ytdl.server.no_ytdlp"))),
            (AddBlock::UploadsOff, Some(key("gui.ytdl.err.uploads_off"))),
            (AddBlock::AlreadyAdding, Some(key("gui.ytdl.already_adding"))),
            (AddBlock::InvalidLayout, Some(key("gui.ytdl.where.fix_layout"))),
            (AddBlock::YearInvalid, Some(key("gui.ytdl.year_invalid"))),
            (AddBlock::LookingUp, None),
            (AddBlock::NoLink, None),
            (AddBlock::MkdirBlocked, None),
            (AddBlock::ClashOlder, None),
            (AddBlock::None, None),
        ];
        for (block, want) in rows {
            assert_eq!(block_label(block, "Home server").map(says), want, "{block:?}");
        }
        assert_eq!(add_button_label(Surface::Room, false, false).key, "gui.ytdl.add.to_library");
        assert_eq!(add_button_label(Surface::Window, false, false).key, "gui.ytdl.add.here");
        assert_eq!(add_button_label(Surface::Room, true, false).key, "gui.ytdl.add.anyway");
        assert_eq!(add_button_label(Surface::Window, true, false).key, "gui.ytdl.add.anyway");
        assert_eq!(add_button_label(Surface::Window, true, true).key, "gui.ytdl.add.adding");
    }

    #[test]
    fn a_problem_round_trips_its_saved_name() {
        for p in Problem::ALL {
            assert_eq!(Problem::parse(p.as_str()), Some(p));
        }
        assert_eq!(Problem::parse("offline"), None, "the terminal has no offline");
    }
}
