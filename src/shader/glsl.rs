//! One pass of a preset, as GLSL that naga accepts.
//!
//! The presets are GLSL ES 3.00 written for Android's shader engine, which
//! hands them to the phone's GL driver behind a preamble declaring the
//! ShaderToy uniforms. Here the compiler is naga — inside wgpu, so the same
//! source becomes Metal, HLSL, SPIR-V or desktop GLSL — and naga reads
//! Vulkan-flavoured GLSL 4.50 with three differences that matter, each
//! found by compiling all fifteen passes of the nine presets through every
//! backend (PLAN.md, Phase 10):
//!
//! - **No combined samplers.** A `uniform sampler2D` is refused outright.
//!   So each channel is a `texture2D` beside one shared `sampler`, and
//!   `iChannelN` is a macro that recombines them at every use — which is
//!   exactly what a `texture(iChannel0, uv)` in the source needs.
//! - **No `sampler2D` parameters.** A function taking one is refused;
//!   one taking a `texture2D` and a `sampler` is fine. So such parameters
//!   are split in two, and every call hands over the pair ([`split`]).
//!   MountainBytes is built on this; the Flutter port could not run it.
//! - **A swizzle into an `inout` parameter** panics naga 30's SPIR-V
//!   writer ("Expression [N] is not cached!"). A plain local does not. So
//!   such an argument is copied into a local before the statement and back
//!   after it — which is what `inout` means anyway ([`hoist`]).
//!
//! All three are rewrites of the token stream rather than of text matched
//! by pattern: the mobile app imports user shaders, and a rewrite that can
//! be fooled by a comment or a string of lookalike text would change
//! someone's program. Where a rewrite cannot be sure it is exact — a
//! sampler that arrives some other way, an `inout` swizzle in a loop
//! header — it refuses with the reason rather than guessing.
//!
//! The uniforms mirror Android's engine rather than ShaderToy's where the
//! two differ, because the presets were tuned on Android: `iResolution.z`
//! is the aspect ratio, `iChannelTime[]` is the engine clock, `iSampleRate`
//! is 44100, `iMouse` and `iDate` are zero.

use std::collections::HashMap;

use super::preset::{MAX_PARAMS, Pass, Preset};

/// Where everything the translation declares is bound — the renderer's half
/// of the contract. The offsets are std140's for the `ShaderToy` block, and
/// a test reads them back out of naga to hold the two together.
pub mod layout {
    pub const UNIFORM_GROUP: u32 = 0;
    pub const UNIFORM_BINDING: u32 = 0;
    /// iChannelN's texture is binding N of this group; the sampler the four
    /// share comes after them.
    pub const CHANNEL_GROUP: u32 = 1;
    pub const SAMPLER_BINDING: u32 = 4;

    pub const RESOLUTION: usize = 0;
    pub const TIME: usize = 12;
    pub const MOUSE: usize = 16;
    pub const DATE: usize = 32;
    pub const TIME_DELTA: usize = 48;
    pub const FRAME: usize = 52;
    pub const SAMPLE_RATE: usize = 56;
    pub const PARAMS: usize = 64;
    pub const CHANNEL_TIME: usize = 96;
    pub const CHANNEL_RESOLUTION: usize = 112;
    pub const SIZE: usize = 176;
}

/// One pass's uniforms for one frame, as Android's engine sets them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uniforms {
    /// The size of what this pass draws into: the output for the image
    /// pass, the buffer for a buffer pass (1×1 for a fixed-size one).
    pub resolution: [f32; 2],
    /// Seconds since the preset started.
    pub time: f32,
    pub time_delta: f32,
    pub frame: i32,
    pub params: [f32; MAX_PARAMS],
    /// What `iChannelResolution[]` reports. Android gives all four the
    /// output's size whatever they hold, and so does this.
    pub channel_resolution: [f32; 2],
}

impl Uniforms {
    /// The block's bytes, little-endian — which every wgpu backend is.
    pub fn to_bytes(&self) -> [u8; layout::SIZE] {
        let mut bytes = [0u8; layout::SIZE];
        let mut put = |offset: usize, value: [u8; 4]| {
            bytes[offset..offset + 4].copy_from_slice(&value);
        };
        let [w, h] = self.resolution;
        put(layout::RESOLUTION, w.to_le_bytes());
        put(layout::RESOLUTION + 4, h.to_le_bytes());
        // Android's z: the aspect ratio, where ShaderToy has pixel aspect.
        put(layout::RESOLUTION + 8, (w / h.max(1.0)).to_le_bytes());
        put(layout::TIME, self.time.to_le_bytes());
        put(layout::TIME_DELTA, self.time_delta.to_le_bytes());
        put(layout::FRAME, self.frame.to_le_bytes());
        put(layout::SAMPLE_RATE, 44100.0f32.to_le_bytes());
        // iMouse and iDate stay zero: Android never sets them, and a preset
        // tuned there never saw anything else.
        for offset in [layout::MOUSE, layout::DATE] {
            for lane in 0..4 {
                put(offset + 4 * lane, 0.0f32.to_le_bytes());
            }
        }
        for (i, value) in self.params.iter().enumerate() {
            put(layout::PARAMS + 4 * i, value.to_le_bytes());
        }
        for i in 0..4 {
            put(layout::CHANNEL_TIME + 4 * i, self.time.to_le_bytes());
            let [cw, ch] = self.channel_resolution;
            put(layout::CHANNEL_RESOLUTION + 16 * i, cw.to_le_bytes());
            put(layout::CHANNEL_RESOLUTION + 16 * i + 4, ch.to_le_bytes());
            put(layout::CHANNEL_RESOLUTION + 16 * i + 8, 1.0f32.to_le_bytes());
        }
        bytes
    }
}

/// Everything above the preset's own code. The arrays ShaderToy code
/// indexes as floats live in the block as vec4s — std140 would otherwise
/// give each float a 16-byte slot — and are unpacked into globals of the
/// shapes the code expects before `mainImage` runs.
const PREAMBLE: &str = "\
#version 450
layout(set = 0, binding = 0, std140) uniform ShaderToy {
    vec3  iResolution;
    float iTime;
    vec4  iMouse;
    vec4  iDate;
    float iTimeDelta;
    int   iFrame;
    float iSampleRate;
    float mstream_pad;
    vec4  mstream_params[2];
    vec4  mstream_channel_time;
    vec4  mstream_channel_resolution[4];
};
layout(set = 1, binding = 0) uniform texture2D mstream_channel0;
layout(set = 1, binding = 1) uniform texture2D mstream_channel1;
layout(set = 1, binding = 2) uniform texture2D mstream_channel2;
layout(set = 1, binding = 3) uniform texture2D mstream_channel3;
layout(set = 1, binding = 4) uniform sampler mstream_sampler;
#define iChannel0 sampler2D(mstream_channel0, mstream_sampler)
#define iChannel1 sampler2D(mstream_channel1, mstream_sampler)
#define iChannel2 sampler2D(mstream_channel2, mstream_sampler)
#define iChannel3 sampler2D(mstream_channel3, mstream_sampler)
float iParams[8];
float iChannelTime[4];
vec3  iChannelResolution[4];
";

/// The entry point. ShaderToy's `fragCoord` counts up from the bottom, and a
/// wgpu framebuffer counts down from the top — but only the image pass is
/// ever looked at. Buffer passes are left counting down: each is read only
/// by passes that sample it with the same convention it was written with,
/// so a feedback read lands where it was written, as it does in GL. The
/// image pass alone turns over, and it alone is made opaque; a buffer's
/// alpha is data.
fn entry_point(image: bool) -> String {
    let (flip, alpha) = if image {
        ("    mstream_coord.y = iResolution.y - mstream_coord.y;\n", "vec4(mstream_rgba.rgb, 1.0)")
    } else {
        ("", "mstream_rgba")
    };
    format!(
        "
layout(location = 0) out vec4 mstream_out;
void main() {{
    for (int i = 0; i < 8; i++) {{ iParams[i] = mstream_params[i / 4][i % 4]; }}
    for (int i = 0; i < 4; i++) {{
        iChannelTime[i] = mstream_channel_time[i];
        iChannelResolution[i] = mstream_channel_resolution[i].xyz;
    }}
    vec2 mstream_coord = gl_FragCoord.xy;
{flip}    vec4 mstream_rgba = vec4(0.0);
    mainImage(mstream_rgba, mstream_coord);
    mstream_out = {alpha};
}}
"
    )
}

/// The GLSL naga compiles for one pass: the preamble, the preset's common
/// code and the pass's own, rewritten as the module note describes, and an
/// entry point that calls `mainImage`.
pub fn translate(preset: &Preset, pass: &Pass) -> Result<String, String> {
    let code = format!("{}\n{}", preset.common, pass.source);
    let code = rewrite(&code)?;
    Ok(format!("{PREAMBLE}{code}{}", entry_point(!pass.name.is_buffer())))
}

// ── Tokens ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Space,
    /// `//` to the end of the line, or `/* */`.
    Comment,
    /// A preprocessor line, continuations included. Left to naga's
    /// preprocessor, except that `#define` aliases of a channel are read.
    Directive,
    Ident,
    Number,
    /// One character. Nothing here needs `+=` or `<<` as a unit.
    Punct,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

fn tokenize(src: &str) -> Vec<Token> {
    let bytes = src.as_bytes();
    let at = |i: usize| bytes.get(i).copied().unwrap_or(0);
    let mut tokens = Vec::new();
    let mut i = 0;
    // Whether only whitespace has been seen since the last newline: a `#`
    // is a directive only there.
    let mut line_start = true;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        let kind = if b.is_ascii_whitespace() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                if bytes[i] == b'\n' {
                    line_start = true;
                }
                i += 1;
            }
            Kind::Space
        } else if b == b'/' && at(i + 1) == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            Kind::Comment
        } else if b == b'/' && at(i + 1) == b'*' {
            i += 2;
            while i < bytes.len() && !(bytes[i] == b'*' && at(i + 1) == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            Kind::Comment
        } else if b == b'#' && line_start {
            while i < bytes.len() && bytes[i] != b'\n' {
                // A backslash before the newline carries the directive on.
                if bytes[i] == b'\\' && at(i + 1) == b'\n' {
                    i += 1;
                }
                i += 1;
            }
            Kind::Directive
        } else if b.is_ascii_alphabetic() || b == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            Kind::Ident
        } else if b.is_ascii_digit() || (b == b'.' && at(i + 1).is_ascii_digit()) {
            let hex = b == b'0' && matches!(at(i + 1), b'x' | b'X');
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_alphanumeric() || c == b'.' || c == b'_' {
                    i += 1;
                } else if matches!(c, b'+' | b'-') && !hex && matches!(bytes[i - 1], b'e' | b'E') {
                    i += 1;
                } else {
                    break;
                }
            }
            Kind::Number
        } else {
            // One character, however many bytes it takes: stray UTF-8 is
            // invalid GLSL, but slicing through the middle of it would
            // not be the place to find that out.
            i += src[i..].chars().next().map_or(1, char::len_utf8);
            Kind::Punct
        };
        if kind != Kind::Space {
            line_start = false;
        }
        tokens.push(Token { kind, start, end: i });
    }
    tokens
}

/// The tokens that are code — not space, comments or directives — so that
/// "the next token" means the next one that says something.
struct Code<'a> {
    src: &'a str,
    tokens: Vec<Token>,
}

impl<'a> Code<'a> {
    fn new(src: &'a str) -> Code<'a> {
        let tokens = tokenize(src)
            .into_iter()
            .filter(|t| matches!(t.kind, Kind::Ident | Kind::Number | Kind::Punct))
            .collect();
        Code { src, tokens }
    }

    fn text(&self, i: usize) -> &'a str {
        self.tokens.get(i).map_or("", |t| &self.src[t.start..t.end])
    }

    fn is(&self, i: usize, text: &str) -> bool {
        self.text(i) == text
    }

    fn is_ident(&self, i: usize) -> bool {
        self.tokens.get(i).is_some_and(|t| t.kind == Kind::Ident)
    }

    /// The token closing the bracket opened at `open`.
    fn closing(&self, open: usize) -> Option<usize> {
        let (up, down) = match self.text(open) {
            "(" => ("(", ")"),
            "[" => ("[", "]"),
            "{" => ("{", "}"),
            _ => return None,
        };
        let mut depth = 0;
        for i in open..self.tokens.len() {
            if self.is(i, up) {
                depth += 1;
            } else if self.is(i, down) {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    /// The comma-separated items between the brackets at `open` and
    /// `close`, as inclusive token ranges. Empty for `()`.
    fn items(&self, open: usize, close: usize) -> Vec<(usize, usize)> {
        let mut items = Vec::new();
        let mut depth = 0;
        let mut first = open + 1;
        for i in open + 1..close {
            match self.text(i) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                "," if depth == 0 => {
                    items.push((first, i - 1));
                    first = i + 1;
                }
                _ => {}
            }
        }
        if first < close {
            items.push((first, close - 1));
        }
        items
    }

    /// The source text of an inclusive token range, comments and all.
    fn slice(&self, first: usize, last: usize) -> &'a str {
        &self.src[self.tokens[first].start..self.tokens[last].end]
    }
}

// ── What the code declares ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    In,
    /// `out` or `inout`: the argument is written back.
    Out,
}

#[derive(Debug, Clone)]
struct Param {
    /// The whole declaration, as an inclusive token range.
    first: usize,
    last: usize,
    direction: Direction,
    ty: String,
    name: Option<String>,
}

#[derive(Debug, Clone)]
struct Function {
    name: String,
    params: Vec<Param>,
    /// The braces of the body; `None` for a prototype.
    body: Option<(usize, usize)>,
}

/// Every function declared at file scope: `<type> <name> ( ... )` followed
/// by a body or a semicolon. A variable initialised with a constructor
/// never has two identifiers before its parenthesis, so is never mistaken
/// for one.
fn functions(code: &Code) -> Vec<Function> {
    let mut found = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < code.tokens.len() {
        match code.text(i) {
            "{" => depth += 1,
            "}" => depth -= 1,
            _ => {}
        }
        if depth == 0 && code.is_ident(i) && code.is_ident(i + 1) && code.is(i + 2, "(") {
            if let Some(close) = code.closing(i + 2) {
                let body = if code.is(close + 1, "{") {
                    code.closing(close + 1).map(|end| (close + 1, end))
                } else {
                    None
                };
                if body.is_some() || code.is(close + 1, ";") {
                    found.push(Function {
                        name: code.text(i + 1).to_string(),
                        params: params(code, i + 2, close),
                        body,
                    });
                    // Past the body, so its braces do not count here.
                    i = body.map_or(close + 1, |(_, end)| end) + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    found
}

fn params(code: &Code, open: usize, close: usize) -> Vec<Param> {
    code.items(open, close)
        .into_iter()
        .filter(|&(first, last)| !(first == last && code.is(first, "void")))
        .map(|(first, last)| {
            // `[qualifiers] <type> [<name>] [<array>]`: the words before any
            // bracket, of which the last two are the type and the name — or
            // the last alone is the type, in a prototype that names nothing.
            let words: Vec<usize> =
                (first..=last).take_while(|&i| !code.is(i, "[")).filter(|&i| code.is_ident(i)).collect();
            let qualifiers = |word: &str| words.iter().any(|&i| code.is(i, word));
            let direction =
                if qualifiers("out") || qualifiers("inout") { Direction::Out } else { Direction::In };
            let types: Vec<&str> = words
                .iter()
                .map(|&i| code.text(i))
                .filter(|w| !QUALIFIERS.contains(w))
                .collect();
            let (ty, name) = match types[..] {
                [.., ty, name] => (ty.to_string(), Some(name.to_string())),
                [ty] => (ty.to_string(), None),
                [] => (String::new(), None),
            };
            Param { first, last, direction, ty, name }
        })
        .collect()
}

const QUALIFIERS: [&str; 8] = ["in", "out", "inout", "const", "highp", "mediump", "lowp", "precise"];

/// Channel names as a call argument may spell them: `iChannelN` itself, or
/// a word `#define`d to one — MountainBytes names its two noise textures
/// `hifbm` and `lofbm` that way.
fn channel_aliases(src: &str) -> HashMap<String, usize> {
    let mut defines: HashMap<String, String> = HashMap::new();
    for token in tokenize(src).iter().filter(|t| t.kind == Kind::Directive) {
        let text = &src[token.start..token.end];
        let Some(rest) = text.trim_start().strip_prefix('#') else { continue };
        let Some(rest) = rest.trim_start().strip_prefix("define") else { continue };
        let mut words = rest.split_whitespace();
        if let (Some(name), Some(value)) = (words.next(), words.next()) {
            // Object-like and a single word: `NAME(` would be a function-
            // like macro, and a longer body is not a plain rename.
            let body_rest = words.next();
            if !name.contains('(') && (body_rest.is_none() || body_rest.is_some_and(|w| w.starts_with("//")))
            {
                defines.insert(name.to_string(), value.to_string());
            }
        }
    }
    let channel = |word: &str| -> Option<usize> {
        let index = word.strip_prefix("iChannel")?;
        let index: usize = index.parse().ok()?;
        (index < 4 && word.len() == "iChannel0".len()).then_some(index)
    };
    let mut aliases: HashMap<String, usize> = (0..4).map(|i| (format!("iChannel{i}"), i)).collect();
    for name in defines.keys() {
        // Followed as far as a chain of renames goes, and no further than
        // eight: a macro that ends in itself is not a channel.
        let mut word = name.as_str();
        for _ in 0..8 {
            if let Some(index) = channel(word) {
                aliases.insert(name.clone(), index);
                break;
            }
            match defines.get(word) {
                Some(next) => word = next,
                None => break,
            }
        }
    }
    aliases
}

// ── The rewrites ────────────────────────────────────────────────────────────

/// One change to the source: `text` replaces the bytes `start..end`, or is
/// inserted there when the two are equal.
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

fn rewrite(src: &str) -> Result<String, String> {
    let code = Code::new(src);
    let functions = functions(&code);
    let mut edits = Vec::new();
    split(&code, &functions, &channel_aliases(src), &mut edits)?;
    hoist(&code, &functions, &mut edits)?;

    // Applied back to front, so each edit's offsets still mean what they
    // meant; insertions at one offset keep the order they were made in.
    edits.sort_by_key(|edit| (edit.start, edit.end));
    let mut out = String::with_capacity(src.len() + 256);
    let mut done = 0;
    for edit in &edits {
        if edit.start < done {
            return Err("internal: two rewrites of the same code".into());
        }
        out.push_str(&src[done..edit.start]);
        out.push_str(&edit.text);
        done = edit.end;
    }
    out.push_str(&src[done..]);
    Ok(out)
}

/// The calls in the token range `first..=last`: a name followed by `(`,
/// not after a `.` (a method-like swizzle is not a call), and naming a
/// function the file declares. Returns (name token, open paren, close).
fn calls<'f>(
    code: &Code,
    functions: &'f [Function],
    first: usize,
    last: usize,
) -> Vec<(usize, usize, usize, Vec<&'f Function>)> {
    let mut found = Vec::new();
    for i in first..=last {
        if !code.is_ident(i) || !code.is(i + 1, "(") || (i > 0 && code.is(i - 1, ".")) {
            continue;
        }
        let name = code.text(i);
        let overloads: Vec<&Function> = functions.iter().filter(|f| f.name == name).collect();
        if overloads.is_empty() {
            continue;
        }
        if let Some(close) = code.closing(i + 1) {
            found.push((i, i + 1, close, overloads));
        }
    }
    found
}

/// The overloads a call with `arity` arguments could mean. GLSL overloads
/// by type, which a token stream does not know; by count is as far as this
/// can tell them apart, so a rewrite acts only where every candidate
/// agrees.
fn by_arity<'f>(overloads: &[&'f Function], arity: usize) -> Vec<&'f Function> {
    overloads.iter().copied().filter(|f| f.params.len() == arity).collect()
}

fn is_sampler(ty: &str) -> bool {
    ty == "sampler2D"
}

/// `sampler2D` parameters become a `texture2D` and a `sampler`; inside such
/// a function every use of the parameter recombines them, and every call to
/// one hands over a pair instead of a channel.
fn split(
    code: &Code,
    functions: &[Function],
    aliases: &HashMap<String, usize>,
    edits: &mut Vec<Edit>,
) -> Result<(), String> {
    for function in functions {
        // The declaration, prototype or definition.
        for param in function.params.iter().filter(|p| is_sampler(&p.ty)) {
            let name = param.name.clone().unwrap_or_else(|| "mstream_unnamed".into());
            edits.push(Edit {
                start: code.tokens[param.first].start,
                end: code.tokens[param.last].end,
                text: format!("texture2D {name}_t, sampler {name}_s"),
            });
        }

        let Some((open, close)) = function.body else { continue };
        let samplers: Vec<&str> = function
            .params
            .iter()
            .filter(|p| is_sampler(&p.ty))
            .filter_map(|p| p.name.as_deref())
            .collect();

        // Arguments bound for a sampler parameter: the pair.
        let mut handled = Vec::new();
        for (name_at, open_at, close_at, overloads) in calls(code, functions, open, close) {
            let args = code.items(open_at, close_at);
            let candidates = by_arity(&overloads, args.len());
            for (index, &(first, last)) in args.iter().enumerate() {
                let wanted = candidates.iter().filter(|f| is_sampler(&f.params[index].ty)).count();
                if wanted == 0 {
                    continue;
                }
                let word = code.text(first);
                let pair = if first != last || !code.is_ident(first) {
                    None
                } else if samplers.contains(&word) {
                    Some(format!("{word}_t, {word}_s"))
                } else {
                    aliases.get(word).map(|i| format!("mstream_channel{i}, mstream_sampler"))
                };
                match pair {
                    Some(text) => {
                        edits.push(Edit { start: code.tokens[first].start, end: code.tokens[last].end, text });
                        handled.push(first);
                    }
                    // Some overload takes something else here: leave it be.
                    None if wanted < candidates.len() => {}
                    None => {
                        return Err(format!(
                            "{}: argument {} of {}() must be a channel or a sampler parameter, not `{}`",
                            function.name,
                            index + 1,
                            code.text(name_at),
                            code.slice(first, last),
                        ));
                    }
                }
            }
        }

        // Every other use of a sampler parameter: recombined in place.
        for i in open..=close {
            if handled.contains(&i) || !code.is_ident(i) || (i > 0 && code.is(i - 1, ".")) {
                continue;
            }
            let word = code.text(i);
            if samplers.contains(&word) {
                edits.push(Edit {
                    start: code.tokens[i].start,
                    end: code.tokens[i].end,
                    text: format!("sampler2D({word}_t, {word}_s)"),
                });
            }
        }
    }
    Ok(())
}

/// A swizzle or an element handed to an `out`/`inout` parameter is copied
/// into a local before its statement and written back after it.
///
/// Exact only when nothing else in the statement touches the variable —
/// GLSL writes the argument back as the call returns, not as the statement
/// ends — and when the statement is a plain one that a declaration can be
/// put in front of. Anything else is refused by name.
fn hoist(code: &Code, functions: &[Function], edits: &mut Vec<Edit>) -> Result<(), String> {
    let mut serial = 0;
    for function in functions {
        let Some((open, close)) = function.body else { continue };
        for (name_at, open_at, close_at, overloads) in calls(code, functions, open, close) {
            let args = code.items(open_at, close_at);
            let candidates = by_arity(&overloads, args.len());
            let Some(first_candidate) = candidates.first() else { continue };
            for (index, &(first, last)) in args.iter().enumerate() {
                let param = &first_candidate.params[index];
                let agreed = candidates
                    .iter()
                    .all(|f| f.params[index].direction == Direction::Out && f.params[index].ty == param.ty);
                // A lone variable is what naga handles already; only a
                // path into one needs the copy.
                if !agreed || first == last {
                    continue;
                }
                if !lvalue_path(code, first, last) {
                    continue;
                }
                let callee = code.text(name_at);
                let (start, end) = statement(code, open, name_at).ok_or_else(|| {
                    format!(
                        "{}: `{}` is passed to {callee}()'s `inout` parameter where it cannot be copied \
                         around the statement (a loop header, a condition, or a return)",
                        function.name,
                        code.slice(first, last)
                    )
                })?;
                let base = code.text(first);
                let elsewhere = (start..=end).any(|i| {
                    !(first..=last).contains(&i)
                        && code.is(i, base)
                        && code.is_ident(i)
                        && !(i > 0 && code.is(i - 1, "."))
                });
                if elsewhere {
                    return Err(format!(
                        "{}: the statement passing `{}` to {callee}() also reads `{base}`, so a copy \
                         written back at its end would change what that read sees",
                        function.name,
                        code.slice(first, last)
                    ));
                }

                let local = format!("mstream_inout{serial}");
                serial += 1;
                let path = code.slice(first, last);
                edits.push(Edit {
                    start: code.tokens[start].start,
                    end: code.tokens[start].start,
                    text: format!("{} {local} = {path}; ", param.ty),
                });
                edits.push(Edit {
                    start: code.tokens[first].start,
                    end: code.tokens[last].end,
                    text: local.clone(),
                });
                edits.push(Edit {
                    start: code.tokens[end].end,
                    end: code.tokens[end].end,
                    text: format!(" {path} = {local};"),
                });
            }
        }
    }
    Ok(())
}

/// `name` followed by `.field`s and `[index]`es, and nothing that could
/// have an effect when evaluated twice: no call, no assignment, no `++`.
fn lvalue_path(code: &Code, first: usize, last: usize) -> bool {
    if !code.is_ident(first) {
        return false;
    }
    let mut depth = 0;
    let mut previous = "";
    for i in first + 1..=last {
        let text = code.text(i);
        match text {
            "[" => depth += 1,
            "]" => depth -= 1,
            "(" | ")" | "=" => return false,
            "+" | "-" if previous == text => return false,
            "." | "+" | "-" | "*" | "/" => {}
            _ if code.is_ident(i) || code.tokens[i].kind == Kind::Number => {}
            _ => return false,
        }
        // Outside an index only `.field` may follow the name.
        if depth == 0 && !matches!(text, "." | "]") && !code.is(i - 1, ".") {
            return false;
        }
        previous = text;
    }
    depth == 0
}

/// The plain statement around the call at `at`, as the tokens from its
/// first to its closing `;` — or `None` when it is not one a declaration
/// can go in front of: a call inside `for (...)`, `if (...)`, `while (...)`
/// or `switch (...)`, a braceless body, or a `return`.
///
/// Read forward from the body's brace, because only from that side is it
/// known which parentheses are open around the call: the `;` inside a `for`
/// header looks like any other until its parenthesis is seen.
fn statement(code: &Code, body_open: usize, at: usize) -> Option<(usize, usize)> {
    let mut open: Vec<usize> = Vec::new();
    let mut start = body_open + 1;
    for i in body_open + 1..at {
        match code.text(i) {
            "(" | "[" => open.push(i),
            ")" | "]" => {
                open.pop();
            }
            ";" | "{" | "}" if open.is_empty() => start = i + 1,
            _ => {}
        }
    }
    let header = |paren: usize| {
        code.is(paren, "(") && matches!(code.text(paren.wrapping_sub(1)), "for" | "if" | "while" | "switch")
    };
    if open.iter().any(|&paren| header(paren)) {
        return None;
    }
    if matches!(
        code.text(start),
        "if" | "for" | "while" | "do" | "return" | "else" | "switch" | "case" | "default"
    ) {
        return None;
    }
    let mut depth = open.len();
    for end in at..code.tokens.len() {
        match code.text(end) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth = depth.checked_sub(1)?,
            ";" if depth == 0 => return Some((start, end)),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::library;
    use crate::shader::preset::PassName;

    fn translated(file: &str, pass: PassName) -> String {
        let source = library::vendored_source(file).expect("vendored");
        let preset = Preset::parse(&source).unwrap();
        translate(&preset, preset.pass(pass).unwrap()).unwrap_or_else(|e| panic!("{file}: {e}"))
    }

    fn rewritten(src: &str) -> String {
        rewrite(src).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_preamble_declares_separate_textures_and_names_them_as_channels() {
        let out = translated("01-spectrum-bars.glsl", PassName::Image);
        assert!(!out.contains("uniform sampler2D"), "naga refuses combined samplers");
        assert!(out.contains("#define iChannel0 sampler2D(mstream_channel0, mstream_sampler)"));
        assert!(out.contains("void mainImage"));
        assert!(out.ends_with("}\n"));
    }

    #[test]
    fn only_the_image_pass_turns_over_and_turns_opaque() {
        let image = translated("04-cyber-fuji.glsl", PassName::Image);
        let buffer = translated("04-cyber-fuji.glsl", PassName::BufferA);
        assert!(image.contains("mstream_coord.y = iResolution.y - mstream_coord.y"));
        assert!(!buffer.contains("mstream_coord.y ="));
        assert!(image.contains("vec4(mstream_rgba.rgb, 1.0)"));
        assert!(buffer.contains("mstream_out = mstream_rgba;"));
    }

    #[test]
    fn a_sampler_parameter_becomes_a_pair_and_its_uses_recombine_it() {
        let out = rewritten(
            "#define lo iChannel1\n\
             float hf(sampler2D s, vec2 p) { return texture(s, p).x; }\n\
             float march(sampler2D s, vec2 p) { return hf(s, p) + hf(s, p * 2.0); }\n\
             void mainImage(out vec4 c, in vec2 p) { c = vec4(march(lo, p), march(iChannel0, p), 0.0, 1.0); }\n",
        );
        assert!(out.contains("float hf(texture2D s_t, sampler s_s, vec2 p)"));
        assert!(out.contains("texture(sampler2D(s_t, s_s), p)"));
        assert!(out.contains("hf(s_t, s_s, p) + hf(s_t, s_s, p * 2.0)"), "forwarded as a pair:\n{out}");
        assert!(out.contains("march(mstream_channel1, mstream_sampler, p)"), "an alias resolves:\n{out}");
        assert!(out.contains("march(mstream_channel0, mstream_sampler, p)"));
    }

    #[test]
    fn a_parameter_named_sampler_is_renamed_out_of_the_keywords_way() {
        // MountainBytes' own spelling. `sampler` is a keyword in 4.50.
        let out = translated("09-mountainbytes.glsl", PassName::Image);
        assert!(!out.contains("sampler2D sampler"));
        assert!(out.contains("texture2D sampler_t, sampler sampler_s"));
        assert!(out.contains("rayMarch(mstream_channel1, mstream_sampler,"));
        assert!(out.contains("normal(mstream_channel0, mstream_sampler,"));
    }

    #[test]
    fn a_sampler_arriving_any_other_way_is_refused_with_the_reason() {
        let err = rewrite(
            "float hf(sampler2D s, vec2 p) { return texture(s, p).x; }\n\
             void mainImage(out vec4 c, in vec2 p) { c = vec4(hf(true ? iChannel0 : iChannel1, p)); }\n",
        )
        .unwrap_err();
        assert!(err.contains("must be a channel or a sampler parameter"), "{err}");
    }

    #[test]
    fn a_swizzle_into_an_inout_parameter_is_copied_around_its_statement() {
        let out = rewritten(
            "float mod1(inout float p, float size) { p = mod(p, size); return p; }\n\
             void f(vec2 p) {\n    float n = mod1(p.x, 2.0);\n    mod1(p.y, 3.0);\n    float m = mod1(n, 1.0);\n}\n",
        );
        assert!(out.contains("float mstream_inout0 = p.x; float n = mod1(mstream_inout0, 2.0); p.x = mstream_inout0;"), "{out}");
        assert!(out.contains("float mstream_inout1 = p.y; mod1(mstream_inout1, 3.0); p.y = mstream_inout1;"), "{out}");
        assert!(out.contains("float m = mod1(n, 1.0);"), "a plain local is left alone");
    }

    #[test]
    fn an_inout_swizzle_the_copy_cannot_be_exact_for_is_refused() {
        let inout = "float mod1(inout float p, float s) { p = mod(p, s); return p; }\n";
        for (body, why) in [
            ("if (mod1(p.x, 2.0) > 0.0) p.y = 1.0;", "cannot be copied"),
            ("for (int i = 0; i < int(mod1(p.x, 2.0)); i++) {}", "cannot be copied"),
            ("float n = mod1(p.x, 2.0) + p.x;", "also reads `p`"),
        ] {
            let err = rewrite(&format!("{inout}void f(vec2 p) {{ {body} }}\n")).unwrap_err();
            assert!(err.contains(why), "{body}: {err}");
        }
    }

    #[test]
    fn comments_and_lookalikes_are_never_rewritten() {
        let src = "float mod1(inout float p, float s) { return p; }\n\
                   // mod1(p.x, 2.0); in a comment\n\
                   /* hf(iChannel0, p) */\n\
                   void f(vec2 p) { float q = p.mod1; }\n";
        assert_eq!(rewritten(src), src);
    }

    #[test]
    fn the_uniform_bytes_sit_where_the_block_says() {
        let uniforms = Uniforms {
            resolution: [640.0, 360.0],
            time: 2.5,
            time_delta: 0.25,
            frame: 7,
            params: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            channel_resolution: [640.0, 360.0],
        };
        let bytes = uniforms.to_bytes();
        let f = |offset: usize| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert_eq!((f(layout::RESOLUTION), f(layout::RESOLUTION + 4)), (640.0, 360.0));
        assert!((f(layout::RESOLUTION + 8) - 640.0 / 360.0).abs() < 1e-6, "z is the aspect");
        assert_eq!(f(layout::TIME), 2.5);
        assert_eq!(i32::from_le_bytes(bytes[layout::FRAME..layout::FRAME + 4].try_into().unwrap()), 7);
        assert_eq!(f(layout::SAMPLE_RATE), 44100.0);
        assert_eq!(f(layout::PARAMS + 4 * 5), 6.0);
        assert_eq!(f(layout::CHANNEL_TIME + 4 * 3), 2.5);
        assert_eq!(f(layout::CHANNEL_RESOLUTION + 16 * 3 + 8), 1.0);
    }
}
