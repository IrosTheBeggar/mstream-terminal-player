//! The pipe between the player and its visualizer window (Phase 11.1): what
//! the parent writes down the child's stdin. Framed as one tag byte and a
//! fixed payload, so the reader never needs a length and the writer never
//! needs a flush boundary; EOF is the parent gone, the child's cue to quit.
//!
//! And the way back ([`Report`]): what the window's controls changed, one
//! line of text each on the child's stdout, for the player to apply and
//! keep. Lines rather than frames because this direction is rare — a pick,
//! a slider moving — and a line the reader does not understand is skipped
//! whole rather than tearing the stream.

use std::io::{self, Read, Write};

use crate::shader::audio::{Curve, HEIGHT, WIDTH};

/// The audio texture's bytes: 512 × 2.
pub const AUDIO_LEN: usize = WIDTH * HEIGHT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// The audio texture for the next frame.
    Audio(Vec<u8>),
    /// Show this preset, by its index in the library's order.
    Preset(u8),
    /// Bring the window to the front.
    Raise,
    /// Close the window.
    Quit,
}

const AUDIO: u8 = b'A';
const PRESET: u8 = b'P';
const RAISE: u8 = b'R';
const QUIT: u8 = b'Q';

impl Message {
    /// The message's bytes on the wire. An audio payload is always exactly
    /// the texture's size — short is padded, long is cut.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Message::Audio(bytes) => {
                let mut out = Vec::with_capacity(1 + AUDIO_LEN);
                out.push(AUDIO);
                out.extend_from_slice(&bytes[..bytes.len().min(AUDIO_LEN)]);
                out.resize(1 + AUDIO_LEN, 0);
                out
            }
            Message::Preset(index) => vec![PRESET, *index],
            Message::Raise => vec![RAISE],
            Message::Quit => vec![QUIT],
        }
    }
}

/// The next message, or `Ok(None)` at a clean EOF. A tag the reader does
/// not know is a broken stream: both ends are the same binary, so it
/// cannot be a newer message.
pub fn read(input: &mut impl Read) -> io::Result<Option<Message>> {
    let mut tag = [0u8; 1];
    if let Err(e) = input.read_exact(&mut tag) {
        return if e.kind() == io::ErrorKind::UnexpectedEof { Ok(None) } else { Err(e) };
    }
    Ok(Some(match tag[0] {
        AUDIO => {
            let mut bytes = vec![0u8; AUDIO_LEN];
            input.read_exact(&mut bytes)?;
            Message::Audio(bytes)
        }
        PRESET => {
            let mut index = [0u8; 1];
            input.read_exact(&mut index)?;
            Message::Preset(index[0])
        }
        RAISE => Message::Raise,
        QUIT => Message::Quit,
        other => {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown message tag {other:#04x}")));
        }
    }))
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn write(output: &mut impl Write, message: &Message) -> io::Result<()> {
    output.write_all(&message.encode())
}

/// What the window tells the player (contract clauses 13–14).
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    /// The preset in front changed, by its file name.
    Preset(String),
    /// The tuning panel moved the response curve — the curve the player's
    /// texture is built with, so it applies this at once.
    Curve(Curve),
    /// A preset's knobs, the ones away from the file's defaults, by name.
    /// Empty is every knob at its default.
    Knobs { file: String, turned: Vec<(String, f32)> },
}

impl Report {
    /// The line, without its newline. File and knob names carry no spaces:
    /// the files are the library's, and `// param:` names are `[A-Za-z0-9_]`.
    pub fn line(&self) -> String {
        match self {
            Report::Preset(file) => format!("preset {file}"),
            Report::Curve(curve) => format!("curve {} {} {}", curve.min_db, curve.max_db, curve.smoothing),
            Report::Knobs { file, turned } => {
                let mut line = format!("knobs {file}");
                for (name, value) in turned {
                    line.push_str(&format!(" {name}={value}"));
                }
                line
            }
        }
    }

    /// A line back into a report. Anything else — a word the reader does
    /// not know, a number that is not one — is `None`, and only that line
    /// is lost.
    pub fn parse(line: &str) -> Option<Report> {
        let mut words = line.split_whitespace();
        let finite = |word: Option<&str>| word?.parse::<f32>().ok().filter(|v| v.is_finite());
        let report = match words.next()? {
            "preset" => Report::Preset(words.next()?.to_string()),
            "curve" => Report::Curve(Curve {
                min_db: finite(words.next())?,
                max_db: finite(words.next())?,
                smoothing: finite(words.next())?,
            }),
            "knobs" => {
                let file = words.next()?.to_string();
                let mut turned = Vec::new();
                for word in words.by_ref() {
                    let (name, value) = word.split_once('=')?;
                    turned.push((name.to_string(), finite(Some(value))?));
                }
                Report::Knobs { file, turned }
            }
            _ => return None,
        };
        words.next().is_none().then_some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_round_trips_and_eof_is_the_end() {
        let audio: Vec<u8> = (0..AUDIO_LEN).map(|i| (i % 251) as u8).collect();
        let sent = [Message::Audio(audio.clone()), Message::Preset(5), Message::Raise, Message::Quit];
        let mut wire = Vec::new();
        for message in &sent {
            write(&mut wire, message).unwrap();
        }
        let mut cursor = std::io::Cursor::new(wire);
        for message in &sent {
            assert_eq!(read(&mut cursor).unwrap().as_ref(), Some(message));
        }
        assert_eq!(read(&mut cursor).unwrap(), None, "a clean EOF");
    }

    #[test]
    fn a_short_texture_is_padded_and_a_long_one_cut() {
        assert_eq!(Message::Audio(vec![7; 3]).encode().len(), 1 + AUDIO_LEN);
        assert_eq!(Message::Audio(vec![7; AUDIO_LEN + 9]).encode().len(), 1 + AUDIO_LEN);
    }

    #[test]
    fn every_report_round_trips_as_one_line() {
        let sent = [
            Report::Preset("05-hex-marching.glsl".into()),
            Report::Curve(Curve { min_db: -80.5, max_db: -12.0, smoothing: 0.3 }),
            Report::Knobs {
                file: "01-spectrum-bars.glsl".into(),
                turned: vec![("contrast".into(), 2.25), ("bars".into(), 48.0)],
            },
            Report::Knobs { file: "03-plasma-pulse.glsl".into(), turned: Vec::new() },
        ];
        for report in sent {
            let line = report.line();
            assert!(!line.contains('\n'), "{line}");
            assert_eq!(Report::parse(&line), Some(report), "{line}");
        }
        // f32's shortest spelling reads back as the same bits.
        let odd = Report::Curve(Curve { min_db: -69.7, max_db: -20.7, smoothing: 0.27 });
        assert_eq!(Report::parse(&odd.line()), Some(odd));
    }

    #[test]
    fn a_line_the_player_does_not_understand_is_nothing_not_a_guess() {
        for line in [
            "",
            "hello",
            "preset",
            "preset a b",
            "curve -70 -20",
            "curve -70 -20 x",
            "curve -70 -20 NaN",
            "curve -70 -20 inf",
            "knobs",
            "knobs 01-spectrum-bars.glsl bars",
            "knobs 01-spectrum-bars.glsl bars=wide",
            "logging to /tmp/x.log",
        ] {
            assert_eq!(Report::parse(line), None, "{line:?}");
        }
    }

    #[test]
    fn a_torn_message_and_an_unknown_tag_are_errors_not_silence() {
        let mut torn = std::io::Cursor::new(vec![b'A', 1, 2, 3]);
        assert!(read(&mut torn).is_err(), "an audio frame cut short");
        let mut odd = std::io::Cursor::new(vec![b'Z']);
        assert_eq!(read(&mut odd).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
