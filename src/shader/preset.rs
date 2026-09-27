//! The preset format, as the mobile app's shader engine reads it.
//!
//! One file is one preset. A file with no pass markers is a single image
//! pass reading the audio texture on `iChannel0` — every ShaderToy shader
//! ever pasted into the app. A file with markers is split on them:
//!
//! ```text
//! // === channel image.0 = music        routing, in the header only
//! // === size bufferc = 1x1             a fixed render size, buffers only
//! // === pass: common ===               shared code, prepended to each pass
//! // === pass: buffera ===              buffers A-D run first, in order
//! // === pass: image ===                the picture, last
//! ```
//!
//! The rules are Android's `parseShader`
//! (mstream_music `android/app/src/main/cpp/shader_engine.cpp`), ported
//! rule for rule rather than improved on: the same file has to mean the
//! same thing on a phone and here. Where Android is lenient — an unknown
//! pass name, a channel line after the first marker, a size nobody could
//! allocate — this is lenient in the same place. The `// param:` knobs
//! follow the Dart side's `parseShaderParams`, and the title the Dart
//! side's `titleOf`, for the same reason.

/// How many `iChannelN` samplers a pass has — ShaderToy's four.
pub const CHANNELS: usize = 4;

/// How many `// param:` knobs a preset may declare: the length of the
/// `iParams[]` uniform array on every engine that runs these files.
pub const MAX_PARAMS: usize = 8;

/// The largest side a `// === size` line may ask for. Android's clamp, which
/// it added because imported shaders are user-supplied: an unbounded size is
/// an unbounded allocation, twice over for a ping-pong pair.
const MAX_PASS_SIDE: u32 = 8192;

/// How far down the file the header metadata is looked for — the Dart
/// side's window. The header sits above the code; a `// title:` further
/// down is a comment about something else.
const HEADER_LINES: usize = 20;

/// Which section of the file a pass is, in the order they run each frame:
/// the buffers first, A to D, then the image. A pass reading a buffer that
/// ran earlier this frame sees this frame's pixels; reading itself, or a
/// buffer still to run, it sees last frame's — ShaderToy's feedback rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PassName {
    BufferA,
    BufferB,
    BufferC,
    BufferD,
    Image,
}

impl PassName {
    pub const ORDER: [PassName; 5] =
        [PassName::BufferA, PassName::BufferB, PassName::BufferC, PassName::BufferD, PassName::Image];

    /// The name as the file spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            PassName::BufferA => "buffera",
            PassName::BufferB => "bufferb",
            PassName::BufferC => "bufferc",
            PassName::BufferD => "bufferd",
            PassName::Image => "image",
        }
    }

    /// A buffer is drawn offscreen for other passes to read; the image pass
    /// is the one that is shown.
    pub fn is_buffer(self) -> bool {
        self != PassName::Image
    }

    /// From a label already folded by [`fold`]. `common` is not a pass — it
    /// has no output — so it is not one of these either.
    fn from_label(label: &str) -> Option<PassName> {
        PassName::ORDER.into_iter().find(|pass| pass.as_str() == label)
    }
}

/// What one of a pass's four samplers reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// Nothing declared: the renderer binds black, as Android does.
    #[default]
    Unbound,
    /// The 512×2 audio texture — see [`super::audio`].
    Audio,
    /// A buffer pass's output. Never [`PassName::Image`]: ShaderToy has no
    /// way to name it, and Android's parser has none either.
    Buffer(PassName),
}

impl Channel {
    /// From a label already folded by [`fold`]. The four spellings of the
    /// audio texture are Android's (`mic` among them, from the days the
    /// microphone was the only input); anything unrecognised binds nothing.
    fn from_label(label: &str) -> Channel {
        match label {
            "audio" | "music" | "musicstream" | "mic" => Channel::Audio,
            _ => match PassName::from_label(label) {
                Some(pass) if pass.is_buffer() => Channel::Buffer(pass),
                _ => Channel::Unbound,
            },
        }
    }
}

/// One section of the file, ready to be compiled.
#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub name: PassName,
    /// The section's own code: from the line after its marker to the line
    /// before the next. The `common` section is not in here — see
    /// [`Preset::common`].
    pub source: String,
    /// What `iChannel0`..`iChannel3` read.
    pub channels: [Channel; CHANNELS],
    /// A fixed render size from a `// === size` line — for a buffer that
    /// writes one constant value to every pixel, 1×1 instead of the whole
    /// window. `None` means the output's size. Never set on the image pass:
    /// it is the output.
    pub size: Option<(u32, u32)>,
}

/// One tuning knob, bound to `iParams[i]` for the i-th declared.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub min: f32,
    pub max: f32,
    /// What the preset draws with until someone turns the knob. Not clamped
    /// to the range, because the Dart side does not clamp it either.
    pub default: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Preset {
    pub title: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    /// The `common` section, prepended to every pass. Empty when there is
    /// none, which is every single-pass file.
    pub common: String,
    /// In [`PassName::ORDER`], so the image pass is always last.
    pub passes: Vec<Pass>,
    /// In declaration order, which is `iParams[]` order.
    pub params: Vec<Param>,
}

impl Preset {
    /// Read one preset file. The one thing that fails is a multi-pass file
    /// with no image pass — there would be nothing to show. Everything else
    /// a phone would accept, this accepts; whether the code compiles is
    /// [`super::glsl`]'s question.
    pub fn parse(source: &str) -> Result<Preset, String> {
        let mut preset = Preset {
            title: header_value(source, "title"),
            author: header_value(source, "author"),
            license: header_value(source, "license"),
            params: params(source),
            ..Preset::default()
        };

        // Android asks exactly this before it looks for any marker: a file
        // spelled `=== pass :` never reaches the splitter there, so it must
        // not here either.
        if !source.contains("=== pass:") {
            let mut channels = [Channel::Unbound; CHANNELS];
            channels[0] = Channel::Audio;
            preset.passes.push(Pass {
                name: PassName::Image,
                source: source.to_string(),
                channels,
                size: None,
            });
            return Ok(preset);
        }

        let mut routing = [[Channel::Unbound; CHANNELS]; PassName::ORDER.len()];
        let mut sizes: [Option<(u32, u32)>; 5] = [None; 5];
        let mut bodies: [Option<String>; 5] = Default::default();
        let mut common: Option<String> = None;

        let mut section = Section::Header;
        let mut body = String::new();
        for line in source.lines() {
            if let Some(label) = pass_marker(line) {
                close(section, &mut body, &mut bodies, &mut common);
                let label = fold(label);
                section = if label == "common" {
                    Section::Common
                } else {
                    match PassName::from_label(&label) {
                        Some(pass) => Section::Pass(pass),
                        None => {
                            tracing::warn!(pass = %label, "visualizer preset: unknown pass, dropped");
                            Section::Dropped
                        }
                    }
                };
                continue;
            }
            if section == Section::Header {
                // Routing is read here and only here. The same line inside
                // a pass is a comment — Android's rule, and a preset that
                // documents its wiring in the image pass's comments (04
                // does) would otherwise rewire itself.
                if let Some((pass, index, channel)) = channel_marker(line) {
                    routing[slot(pass)][index] = channel;
                } else if let Some((pass, size)) = size_marker(line) {
                    sizes[slot(pass)] = size;
                }
                continue;
            }
            body.push_str(line);
            body.push('\n');
        }
        close(section, &mut body, &mut bodies, &mut common);

        preset.common = common.unwrap_or_default();
        for pass in PassName::ORDER {
            if let Some(source) = bodies[slot(pass)].take() {
                preset.passes.push(Pass {
                    name: pass,
                    source,
                    channels: routing[slot(pass)],
                    size: if pass.is_buffer() { sizes[slot(pass)] } else { None },
                });
            }
        }
        if preset.pass(PassName::Image).is_none() {
            return Err("a multi-pass preset needs an `// === pass: image ===` section".into());
        }
        Ok(preset)
    }

    pub fn pass(&self, name: PassName) -> Option<&Pass> {
        self.passes.iter().find(|pass| pass.name == name)
    }

    /// `iParams[]` as the preset draws before anyone tunes it: each declared
    /// knob's default, zero past the last.
    pub fn default_params(&self) -> [f32; MAX_PARAMS] {
        let mut values = [0.0; MAX_PARAMS];
        for (value, param) in values.iter_mut().zip(&self.params) {
            *value = param.default;
        }
        values
    }
}

// ── The splitter ────────────────────────────────────────────────────────────

/// Where the lines being read belong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Above the first marker: metadata, comments, routing.
    Header,
    Common,
    Pass(PassName),
    /// Under a marker naming no pass this format has. Android keeps reading
    /// to the next marker and throws the lot away; so does this.
    Dropped,
}

fn slot(pass: PassName) -> usize {
    PassName::ORDER.iter().position(|p| *p == pass).expect("every pass is in ORDER")
}

/// File the body read so far under the section it was read for. A second
/// section of the same name replaces the first, which is what Android does.
fn close(
    section: Section,
    body: &mut String,
    bodies: &mut [Option<String>; 5],
    common: &mut Option<String>,
) {
    let text = std::mem::take(body);
    match section {
        Section::Common => *common = Some(text),
        Section::Pass(pass) => bodies[slot(pass)] = Some(text),
        Section::Header | Section::Dropped => {}
    }
}

/// Android's three line patterns, ported as they are written (ECMAScript
/// `regex_search`, each anchored at the start of the line):
///
/// ```text
/// pass     ^\s*//\s*===\s*pass\s*:\s*([a-zA-Z]+)\s*===
/// channel  ^\s*//\s*===\s*channel\s+([a-zA-Z]+)\s*\.\s*(\d+)\s*=\s*([a-zA-Z]+)
/// size     ^\s*//\s*===\s*size\s+([a-zA-Z]+)\s*=\s*(\d+)\s*[xX]\s*(\d+)
/// ```
///
/// Anything may follow a match. Each capture is a maximal run, and nothing
/// the patterns allow after one could have matched a shorter run instead, so
/// a scanner that never backtracks accepts exactly what the regexes do.
struct Scan<'a> {
    rest: &'a str,
}

impl<'a> Scan<'a> {
    /// `^\s*//\s*===\s*` — what all three open with.
    fn open(line: &'a str) -> Option<Scan<'a>> {
        let mut scan = Scan { rest: line };
        scan.spaces();
        scan.literal("//")?;
        scan.spaces();
        scan.literal("===")?;
        scan.spaces();
        Some(scan)
    }

    /// `\s*`
    fn spaces(&mut self) {
        self.rest = self.rest.trim_start();
    }

    /// `\s+`
    fn some_spaces(&mut self) -> Option<()> {
        let trimmed = self.rest.trim_start();
        (trimmed.len() < self.rest.len()).then(|| self.rest = trimmed)
    }

    fn literal(&mut self, text: &str) -> Option<()> {
        self.rest = self.rest.strip_prefix(text)?;
        Some(())
    }

    fn run(&mut self, class: fn(&char) -> bool) -> Option<&'a str> {
        let end = self.rest.find(|c: char| !class(&c)).unwrap_or(self.rest.len());
        let (run, rest) = self.rest.split_at(end);
        self.rest = rest;
        (!run.is_empty()).then_some(run)
    }

    /// `[a-zA-Z]+`
    fn letters(&mut self) -> Option<&'a str> {
        self.run(char::is_ascii_alphabetic)
    }

    /// `\d+`
    fn digits(&mut self) -> Option<&'a str> {
        self.run(char::is_ascii_digit)
    }
}

/// A pass marker, and the name it gives.
fn pass_marker(line: &str) -> Option<&str> {
    let mut scan = Scan::open(line)?;
    scan.literal("pass")?;
    scan.spaces();
    scan.literal(":")?;
    scan.spaces();
    let name = scan.letters()?;
    scan.spaces();
    scan.literal("===")?;
    Some(name)
}

/// `// === channel <pass>.<n> = <source>`. A pass this format does not have,
/// or a sampler past the fourth, routes nothing; a source it does not know
/// routes nothing to that sampler, which is not the same thing.
fn channel_marker(line: &str) -> Option<(PassName, usize, Channel)> {
    let mut scan = Scan::open(line)?;
    scan.literal("channel")?;
    scan.some_spaces()?;
    let target = scan.letters()?;
    scan.spaces();
    scan.literal(".")?;
    scan.spaces();
    let index = scan.digits()?;
    scan.spaces();
    scan.literal("=")?;
    scan.spaces();
    let source = scan.letters()?;

    let pass = PassName::from_label(&fold(target))?;
    // Parsed without overflowing, which Android's std::stoi would not
    // survive: a digit run that long names no sampler.
    let index: usize = index.parse().ok()?;
    (index < CHANNELS).then(|| (pass, index, Channel::from_label(&fold(source))))
}

/// `// === size <pass> = <w>x<h>`. Both sides must be above zero; a side
/// over [`MAX_PASS_SIDE`] is clamped to it, and a side too long to parse is
/// a size nobody means — the pass keeps the output's size, as it does when
/// Android's overflow catch fires.
fn size_marker(line: &str) -> Option<(PassName, Option<(u32, u32)>)> {
    let mut scan = Scan::open(line)?;
    scan.literal("size")?;
    scan.some_spaces()?;
    let target = scan.letters()?;
    scan.spaces();
    scan.literal("=")?;
    scan.spaces();
    let w = scan.digits()?;
    scan.spaces();
    scan.literal("x").or_else(|| scan.literal("X"))?;
    scan.spaces();
    let h = scan.digits()?;

    let pass = PassName::from_label(&fold(target))?;
    let side = |digits: &str| -> Option<u32> {
        let value: u32 = digits.parse().ok()?;
        (value > 0).then(|| value.min(MAX_PASS_SIDE))
    };
    Some((pass, side(w).zip(side(h))))
}

/// Lowercased, with everything that is not a letter or digit dropped:
/// Android's `lowerAlnum`, applied to each capture before it is compared.
fn fold(label: &str) -> String {
    label.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

// ── Header and knobs ────────────────────────────────────────────────────────

/// `// <key>: <value>` in the comment block the file opens with. The Dart
/// rule: blank lines are skipped, the block ends at the first line that is
/// not a comment, only the first [`HEADER_LINES`] are looked at, the key is
/// case-insensitive and an empty value is no value.
fn header_value(source: &str, key: &str) -> Option<String> {
    for line in source.lines().take(HEADER_LINES) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(comment) = line.strip_prefix("//") else { break };
        let comment = comment.trim_start();
        let Some((name, value)) = comment.split_once(':') else { continue };
        if name.eq_ignore_ascii_case(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Every `// param: <name> <min> <max> <default>` line, anywhere in the
/// file, in order — the i-th one well-formed is `iParams[i]`. A malformed
/// line, or one whose range is empty, is skipped and takes no index; past
/// [`MAX_PARAMS`] the rest are ignored. The Dart side's rules exactly.
fn params(source: &str) -> Vec<Param> {
    let mut found = Vec::new();
    for line in source.lines() {
        if found.len() == MAX_PARAMS {
            break;
        }
        let Some(rest) = line.trim_start().strip_prefix("//") else { continue };
        let Some(rest) = rest.trim_start().strip_prefix("param:") else { continue };
        let words: Vec<&str> = rest.split_whitespace().collect();
        let [name, min, max, default] = words[..] else { continue };
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let (Some(min), Some(max), Some(default)) = (number(min), number(max), number(default))
        else {
            continue;
        };
        if max <= min {
            continue;
        }
        found.push(Param { name: name.to_string(), min, max, default });
    }
    found
}

/// A number as the Dart pattern `-?[0-9]*\.?[0-9]+` spells one: an optional
/// minus, then digits with at most one point, and at least one digit after
/// the point. So `.5` and `-2` are numbers; `1.`, `+1` and `1e3` are not.
fn number(text: &str) -> Option<f32> {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => ("", unsigned),
    };
    let all_digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if fraction.is_empty() || !all_digits(whole) || !all_digits(fraction) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::shader::library;

    fn vendored(file: &str) -> Preset {
        let source = library::vendored_source(file).expect("a vendored preset");
        Preset::parse(&source).unwrap_or_else(|e| panic!("{file}: {e}"))
    }

    fn wiring(preset: &Preset, pass: PassName) -> [Channel; CHANNELS] {
        preset.pass(pass).unwrap_or_else(|| panic!("no {} pass", pass.as_str())).channels
    }

    use Channel::{Audio, Buffer, Unbound};
    use PassName::{BufferA, BufferB, BufferC, Image};

    #[test]
    fn a_file_without_markers_is_one_image_pass_reading_the_music() {
        let preset = vendored("01-spectrum-bars.glsl");
        assert_eq!(preset.passes.len(), 1);
        let pass = &preset.passes[0];
        assert_eq!(pass.name, Image);
        assert_eq!(pass.channels, [Audio, Unbound, Unbound, Unbound]);
        assert_eq!(pass.size, None);
        assert!(pass.source.contains("void mainImage"), "the whole file is the pass");
        assert!(preset.common.is_empty());
    }

    #[test]
    fn the_header_wires_the_passes_the_way_the_flutter_port_wired_them_by_hand() {
        // lib/visualizer/shader_visualizer_screen.dart hand-writes each
        // multi-pass preset's PassDefs. The files say the same thing in
        // their headers, and reading them has to agree with the port.
        let fuji = vendored("04-cyber-fuji.glsl");
        assert_eq!(fuji.passes.iter().map(|p| p.name).collect::<Vec<_>>(), [BufferA, Image]);
        assert_eq!(wiring(&fuji, BufferA)[..2], [Audio, Buffer(BufferA)]);
        assert_eq!(wiring(&fuji, Image)[..2], [Audio, Buffer(BufferA)]);
        assert_eq!(fuji.pass(BufferA).unwrap().size, Some((1, 1)));

        let hex = vendored("05-hex-marching.glsl");
        assert_eq!(
            hex.passes.iter().map(|p| p.name).collect::<Vec<_>>(),
            [BufferA, BufferB, BufferC, Image]
        );
        assert_eq!(wiring(&hex, BufferA), [Unbound; CHANNELS]);
        assert_eq!(wiring(&hex, BufferB)[..2], [Buffer(BufferA), Buffer(BufferB)]);
        assert_eq!(wiring(&hex, BufferC)[..2], [Audio, Buffer(BufferC)]);
        assert_eq!(wiring(&hex, Image)[..2], [Buffer(BufferB), Buffer(BufferC)]);
        assert_eq!(hex.pass(BufferC).unwrap().size, Some((1, 1)));
        assert_eq!(hex.pass(BufferB).unwrap().size, None);
    }

    #[test]
    fn a_common_section_is_kept_apart_from_the_passes() {
        let mountain = vendored("09-mountainbytes.glsl");
        assert!(!mountain.common.is_empty());
        assert_eq!(wiring(&mountain, Image)[..3], [Buffer(BufferA), Buffer(BufferB), Audio]);
        for pass in &mountain.passes {
            assert!(!pass.source.contains("=== pass:"), "markers are not code");
        }
    }

    #[test]
    fn routing_after_the_first_marker_is_only_a_comment() {
        let source = "\
// === channel image.0 = music
// === pass: image ===
// === channel image.1 = buffera
void mainImage(out vec4 c, in vec2 p) { c = vec4(0.0); }
";
        let preset = Preset::parse(source).unwrap();
        assert_eq!(wiring(&preset, Image), [Audio, Unbound, Unbound, Unbound]);
        // ...and the line stays in the pass, where GLSL reads it as the
        // comment it is.
        assert!(preset.passes[0].source.contains("// === channel image.1"));
    }

    #[test]
    fn labels_are_compared_case_blind_and_spaced_as_android_allows() {
        let source = "\
// ===channel   Image . 2 =  BufferB   (the rest of the line is anyone's)
// === size BufferB = 4X3
// ===   pass :  BufferB ===
void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }
// === pass: IMAGE===
void mainImage(out vec4 c, in vec2 p) { c = vec4(0.0); }
";
        let preset = Preset::parse(source).unwrap();
        assert_eq!(wiring(&preset, Image)[2], Buffer(BufferB));
        assert_eq!(preset.pass(BufferB).unwrap().size, Some((4, 3)));
    }

    #[test]
    fn a_line_the_patterns_do_not_match_is_not_a_marker() {
        let source = "\
// === channel image.0x = music
// === channel image . 1 = 2music
// === size buffera extra = 1x1
// === pass: buffera ===
// === pass: image
// === passage: image ===
void mainImage(out vec4 c, in vec2 p) { c = vec4(0.0); }
// === pass: image ===
void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }
";
        let preset = Preset::parse(source).unwrap();
        // No routing and no size: each of those three lines breaks its
        // pattern somewhere.
        assert_eq!(wiring(&preset, Image), [Unbound; CHANNELS]);
        assert_eq!(preset.pass(BufferA).unwrap().size, None);
        // Without its closing `===` a marker is a comment, so the first
        // mainImage belongs to buffer A.
        assert!(preset.pass(BufferA).unwrap().source.contains("vec4(0.0)"));
        assert!(preset.pass(Image).unwrap().source.contains("vec4(1.0)"));
    }

    #[test]
    fn an_unknown_pass_is_dropped_with_everything_under_it() {
        let source = "\
// === pass: sound ===
float nope;
// === pass: image ===
void mainImage(out vec4 c, in vec2 p) { c = vec4(0.0); }
";
        let preset = Preset::parse(source).unwrap();
        assert_eq!(preset.passes.len(), 1);
        assert!(!preset.passes[0].source.contains("nope"));
    }

    #[test]
    fn a_multi_pass_file_needs_something_to_show() {
        let source = "// === pass: buffera ===\nvoid mainImage(out vec4 c, in vec2 p) {}\n";
        assert!(Preset::parse(source).is_err());
    }

    #[test]
    fn a_spelling_android_never_splits_is_one_pass_here_too() {
        // `=== pass :` would satisfy the marker pattern, but Android's
        // up-front check looks for `=== pass:` and never gets that far.
        let source = "// === pass : buffera ===\nvoid mainImage(out vec4 c, in vec2 p) {}\n";
        let preset = Preset::parse(source).unwrap();
        assert_eq!(preset.passes.len(), 1);
        assert_eq!(preset.passes[0].name, Image);
    }

    #[test]
    fn sizes_are_clamped_and_nonsense_leaves_the_output_size() {
        let size_of = |line: &str| {
            let source = format!("{line}\n// === pass: bufferc ===\n\n// === pass: image ===\n\n");
            Preset::parse(&source).unwrap().pass(BufferC).unwrap().size
        };
        assert_eq!(size_of("// === size bufferc = 1x1"), Some((1, 1)));
        assert_eq!(size_of("// === size bufferc = 99999x2"), Some((MAX_PASS_SIDE, 2)));
        assert_eq!(size_of("// === size bufferc = 0x5"), None);
        assert_eq!(size_of("// === size bufferc = 99999999999999999999x2"), None);
        assert_eq!(size_of("// === size bufferc = wide"), None);

        // The image pass is the output; a size for it means nothing.
        let source = "// === size image = 2x2\n// === pass: image ===\n\n";
        assert_eq!(Preset::parse(source).unwrap().pass(Image).unwrap().size, None);
    }

    #[test]
    fn a_channel_index_past_the_fourth_sampler_is_ignored() {
        let source = "\
// === channel image.4 = music
// === channel image.99999999999999999999 = music
// === pass: image ===
";
        assert_eq!(wiring(&Preset::parse(source).unwrap(), Image), [Unbound; CHANNELS]);
    }

    #[test]
    fn params_take_indexes_in_order_and_malformed_lines_take_none() {
        let source = "\
// param: first 0.5 3.0 1.51
// param: bad-name 0 1 0.5
// param: trailing 1. 2 1.5
// param: empty 1 1 1
// param: exponent 1e3 2 1
// param: second -2 .5 0
//param:third 0 1 0.25
";
        let preset = Preset::parse(source).unwrap();
        let names: Vec<_> = preset.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["first", "second", "third"]);
        assert_eq!(preset.params[1], Param { name: "second".into(), min: -2.0, max: 0.5, default: 0.0 });
        let defaults = preset.default_params();
        assert_eq!(defaults[..4], [1.51, 0.0, 0.25, 0.0]);
    }

    #[test]
    fn params_stop_at_eight() {
        let source: String = (0..12).map(|i| format!("// param: p{i} 0 1 0.{i}\n")).collect();
        let preset = Preset::parse(&source).unwrap();
        assert_eq!(preset.params.len(), MAX_PARAMS);
        assert_eq!(preset.params[7].name, "p7");
    }

    #[test]
    fn the_header_is_the_comment_block_the_file_opens_with() {
        let source = "\
// Title: Spectrum Bars

// author: someone
void mainImage(out vec4 c, in vec2 p) { c = vec4(0.0); }
// license: not in the header
";
        let preset = Preset::parse(source).unwrap();
        assert_eq!(preset.title.as_deref(), Some("Spectrum Bars"));
        assert_eq!(preset.author.as_deref(), Some("someone"));
        assert_eq!(preset.license, None);
    }

    #[test]
    fn every_vendored_preset_parses_with_its_title_and_license() {
        for (file, source) in library::vendored() {
            let preset = Preset::parse(&source).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(preset.title.is_some(), "{file} has a title");
            assert!(preset.license.is_some(), "{file} says under what terms it came");
            assert_eq!(preset.passes.last().map(|p| p.name), Some(Image), "{file} ends on its image");
        }
    }
}
