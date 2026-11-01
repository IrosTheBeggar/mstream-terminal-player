//! The mood filters (mStream #1092): the server scores every song with
//! mood heads, and Albums, Artists and the Discover page filter by them.
//!
//! A filter has up to four groups, and a group left out does not filter:
//! - `valence`: happy · neutral · sad
//! - `energy`: relaxed · neutral · aggressive
//! - `voice`: instrumental · vocals
//! - `tags`: any of acoustic, party, danceable — every picked one must hold
//!
//! `/api/v1/db/albums` and `/api/v1/db/artists` take it as `moods` in a POST
//! body (a peer's proxy forwards bodies, not query strings); the Discover
//! page's taste body takes the same key, and so do the song lists,
//! `/api/v1/db/rated` and `/api/v1/db/recent/added`. A server says it can
//! filter with the ping's `moodFilters` (scores to filter by) — and the
//! taste body, which refuses keys it does not know, only from
//! `discoveryTaste` 2.
//!
//! A score is the model's confidence, not an intensity. The filter's level
//! (mStream #1099) says how sure every group must be: `threshold` inside
//! `moods` — Strict 0.7, Loose 0.3, Standard the server's default and sent
//! as nothing — and only beside a group, since a bar alone filters nothing.
//! A filtered song list's songs each carry `moodMatch`, 0 to 1, how surely
//! the song fits; the list keeps its own order, and a client's Sort by
//! match puts the best first ([`match_order`]).

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use super::Track;

/// The three single-choice scales, in the panel's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoodScale {
    Valence,
    Energy,
    Voice,
}

impl MoodScale {
    pub const ALL: [MoodScale; 3] = [MoodScale::Valence, MoodScale::Energy, MoodScale::Voice];

    /// Its values in the panel's order — the wire's words too.
    pub fn choices(self) -> &'static [&'static str] {
        match self {
            MoodScale::Valence => &["happy", "neutral", "sad"],
            MoodScale::Energy => &["relaxed", "neutral", "aggressive"],
            MoodScale::Voice => &["instrumental", "vocals"],
        }
    }

    /// The wire key.
    pub fn key(self) -> &'static str {
        match self {
            MoodScale::Valence => "valence",
            MoodScale::Energy => "energy",
            MoodScale::Voice => "voice",
        }
    }
}

/// The tags, any of which can be on, in the panel's (and the wire's) order.
pub const MOOD_TAGS: [&str; 3] = ["acoustic", "party", "danceable"];

/// How sure every group must be: the server's `threshold`. Standard is the
/// server's default, so it sends none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MoodLevel {
    Strict,
    #[default]
    Standard,
    Loose,
}

impl MoodLevel {
    pub const ALL: [MoodLevel; 3] = [MoodLevel::Strict, MoodLevel::Standard, MoodLevel::Loose];

    /// The bar the wire names — the webapp's and the phone's Strict and
    /// Loose; none for Standard.
    pub fn threshold(self) -> Option<f64> {
        match self {
            MoodLevel::Strict => Some(0.7),
            MoodLevel::Standard => None,
            MoodLevel::Loose => Some(0.3),
        }
    }
}

/// A song list's order under a filter: its own (Rated by rating, Recent
/// newest first) or Match, the surest fits first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MoodSort {
    #[default]
    Own,
    Match,
}

/// One room's filter: each scale's chosen value as an index into its
/// [`MoodScale::choices`], the tags as bits in [`MOOD_TAGS`] order, and the
/// level. `Copy` and `Eq`, because it is part of a library view's identity
/// (`LibraryNode::Albums`), so an answer asked for under another filter is
/// dropped when it lands — in its [`Self::wire`] form, so a level picked
/// with no mood set changes no identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MoodFilter {
    valence: Option<u8>,
    energy: Option<u8>,
    voice: Option<u8>,
    tags: u8,
    level: MoodLevel,
}

impl MoodFilter {
    /// No filter: every song, at the Standard level.
    pub const NONE: MoodFilter =
        MoodFilter { valence: None, energy: None, voice: None, tags: 0, level: MoodLevel::Standard };

    /// Whether nothing filters: no scale's value and no tag. A level alone
    /// raises no bar, so it is empty too.
    pub fn is_empty(&self) -> bool {
        self.valence.is_none() && self.energy.is_none() && self.voice.is_none() && self.tags == 0
    }

    pub fn level(&self) -> MoodLevel {
        self.level
    }

    /// The choices as they are, at `level`.
    pub fn with_level(self, level: MoodLevel) -> MoodFilter {
        MoodFilter { level, ..self }
    }

    /// What is asked for: [`Self::NONE`] when nothing filters, else the
    /// filter whole. The one form that goes into a node, a request or a
    /// cache key, so a lone level never asks or tells anything apart.
    pub fn wire(self) -> MoodFilter {
        if self.is_empty() { MoodFilter::NONE } else { self }
    }

    /// Every scale and tag let go, the level back to Standard.
    pub fn cleared(self) -> MoodFilter {
        MoodFilter::NONE
    }

    fn slot(&self, scale: MoodScale) -> Option<u8> {
        match scale {
            MoodScale::Valence => self.valence,
            MoodScale::Energy => self.energy,
            MoodScale::Voice => self.voice,
        }
    }

    /// The scale's chosen value, if any.
    pub fn value_of(&self, scale: MoodScale) -> Option<&'static str> {
        self.slot(scale).map(|i| scale.choices()[i as usize])
    }

    /// `value` pressed on `scale`: chosen — or let go, when it was the one
    /// chosen. A word the scale does not have changes nothing.
    pub fn press(self, scale: MoodScale, value: &str) -> MoodFilter {
        let Some(i) = scale.choices().iter().position(|c| *c == value) else { return self };
        let i = i as u8;
        let next = if self.slot(scale) == Some(i) { None } else { Some(i) };
        let mut out = self;
        match scale {
            MoodScale::Valence => out.valence = next,
            MoodScale::Energy => out.energy = next,
            MoodScale::Voice => out.voice = next,
        }
        out
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        MOOD_TAGS.iter().position(|t| *t == tag).is_some_and(|i| self.tags & (1 << i) != 0)
    }

    /// `tag` on if it was off, off if it was on; an unknown tag changes
    /// nothing.
    pub fn toggle_tag(self, tag: &str) -> MoodFilter {
        let Some(i) = MOOD_TAGS.iter().position(|t| *t == tag) else { return self };
        MoodFilter { tags: self.tags ^ (1 << i), ..self }
    }

    /// The picked tags, in [`MOOD_TAGS`] order.
    pub fn tags(&self) -> Vec<&'static str> {
        MOOD_TAGS.iter().copied().filter(|t| self.has_tag(t)).collect()
    }
}

/// The wire form: only the groups that filter, tags in a fixed order, then
/// the level's `threshold` when it has one —
/// `{"energy":"relaxed","tags":["acoustic"],"threshold":0.7}`. A level
/// alone writes nothing. A caller sends the key only for a filter that is
/// not empty.
impl Serialize for MoodFilter {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        for scale in MoodScale::ALL {
            if let Some(v) = self.value_of(scale) {
                map.serialize_entry(scale.key(), v)?;
            }
        }
        let tags = self.tags();
        if !tags.is_empty() {
            map.serialize_entry("tags", &tags)?;
        }
        if let Some(threshold) = self.level.threshold().filter(|_| !self.is_empty()) {
            map.serialize_entry("threshold", &threshold)?;
        }
        map.end()
    }
}

/// A filtered song list's row: the track, and how surely it fits the
/// filter (`moodMatch`, 0 to 1 in three places) — `None` for a song with
/// no scores, whether the server wrote `null` or nothing. A field of its
/// own rather than one more on [`Track`], which every list shares.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MoodSong {
    #[serde(flatten)]
    pub track: Track,
    #[serde(rename = "moodMatch", default)]
    pub mood_match: Option<f64>,
}

/// A match as an ordering key: the best (1.000) is 0 and the least (0) is
/// 1000, read in the wire's three places; none is `u32::MAX`, last.
pub fn match_key(m: Option<f64>) -> u32 {
    match m {
        Some(m) if m.is_finite() => (1000.0 - (m.clamp(0.0, 1.0) * 1000.0).round()) as u32,
        _ => u32::MAX,
    }
}

/// The rows' order under Match, as indices into `matches`: a stable sort by
/// [`match_key`] — the surest first, the unscored last, ties in the given
/// order (the webapp's and the phone's Sort by match).
pub fn match_order(matches: &[Option<f64>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..matches.len()).collect();
    order.sort_by_key(|&i| match_key(matches[i]));
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_names_only_the_groups_that_filter_tags_in_a_fixed_order() {
        assert_eq!(serde_json::to_value(MoodFilter::NONE).unwrap(), serde_json::json!({}));
        let f = MoodFilter::NONE
            .press(MoodScale::Valence, "neutral")
            .toggle_tag("danceable")
            .toggle_tag("acoustic");
        assert_eq!(
            serde_json::to_value(f).unwrap(),
            serde_json::json!({ "valence": "neutral", "tags": ["acoustic", "danceable"] })
        );
        let all = f.press(MoodScale::Energy, "relaxed").press(MoodScale::Voice, "vocals");
        assert_eq!(
            serde_json::to_value(all).unwrap(),
            serde_json::json!({
                "valence": "neutral", "energy": "relaxed", "voice": "vocals",
                "tags": ["acoustic", "danceable"]
            })
        );
    }

    #[test]
    fn one_value_per_scale_pressed_again_lets_go_and_tags_toggle() {
        let f = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        assert_eq!(f.value_of(MoodScale::Energy), Some("relaxed"));
        let f = f.press(MoodScale::Energy, "aggressive");
        assert_eq!(f.value_of(MoodScale::Energy), Some("aggressive"), "one value per scale");
        assert!(f.press(MoodScale::Energy, "aggressive").is_empty(), "pressed again, it lets go");
        assert_eq!(f.press(MoodScale::Energy, "sad"), f, "sad is not on that scale");
        let t = MoodFilter::NONE.toggle_tag("party").toggle_tag("acoustic");
        assert_eq!(t.tags(), vec!["acoustic", "party"]);
        assert_eq!(t.toggle_tag("party").tags(), vec!["acoustic"]);
        assert_eq!(t.toggle_tag("sad"), t);
        assert!(t.toggle_tag("party").toggle_tag("acoustic").is_empty());
    }

    #[test]
    fn a_level_rides_inside_moods_only_beside_a_mood() {
        assert_eq!(MoodLevel::Strict.threshold(), Some(0.7));
        assert_eq!(MoodLevel::Loose.threshold(), Some(0.3));
        assert_eq!(MoodLevel::Standard.threshold(), None);
        assert_eq!(MoodFilter::NONE.level(), MoodLevel::Standard);
        let relaxed = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        assert_eq!(
            serde_json::to_value(relaxed.with_level(MoodLevel::Strict)).unwrap(),
            serde_json::json!({ "energy": "relaxed", "threshold": 0.7 })
        );
        assert_eq!(
            serde_json::to_value(relaxed.with_level(MoodLevel::Standard)).unwrap(),
            serde_json::json!({ "energy": "relaxed" }),
            "Standard is the server's default: no threshold"
        );
        // After the tags, in the written text too.
        let tagged = relaxed.toggle_tag("party").with_level(MoodLevel::Loose);
        assert_eq!(
            serde_json::to_string(&tagged).unwrap(),
            r#"{"energy":"relaxed","tags":["party"],"threshold":0.3}"#
        );
        // A level alone filters nothing, says nothing, and asks as none.
        for level in MoodLevel::ALL {
            let lone = MoodFilter::NONE.with_level(level);
            assert!(lone.is_empty(), "{level:?}");
            assert_eq!(serde_json::to_value(lone).unwrap(), serde_json::json!({}), "{level:?}");
            assert_eq!(lone.wire(), MoodFilter::NONE, "{level:?}");
        }
        assert_eq!(tagged.wire(), tagged, "a filter that filters goes whole");
    }

    #[test]
    fn letting_the_last_mood_go_keeps_the_level_and_cleared_is_standard() {
        let strict = MoodFilter::NONE.with_level(MoodLevel::Strict);
        let f = strict.press(MoodScale::Valence, "happy").toggle_tag("acoustic");
        assert_eq!(f.level(), MoodLevel::Strict, "a press and a tag keep the level");
        let gone = f.press(MoodScale::Valence, "happy").toggle_tag("acoustic");
        assert!(gone.is_empty());
        assert_eq!(gone.level(), MoodLevel::Strict, "kept for the next mood");
        assert_eq!(gone, strict);
        assert_ne!(gone, MoodFilter::NONE, "the room's choice still holds its level");
        assert_eq!(f.cleared(), MoodFilter::NONE);
        assert_eq!(f.cleared().level(), MoodLevel::Standard);
    }

    #[test]
    fn match_order_is_stable_best_first_missing_last() {
        // Swing, Host, Civil Attack in the server's order: Host, Swing,
        // Civil Attack by match.
        assert_eq!(match_order(&[Some(0.555), Some(0.865), Some(0.524)]), vec![1, 0, 2]);
        // Ties keep the given order; the unscored go last, in theirs.
        assert_eq!(match_order(&[None, Some(0.5), Some(0.9), Some(0.5), None]), vec![2, 1, 3, 0, 4]);
        assert_eq!(match_order(&[None, None, None]), vec![0, 1, 2], "all unscored: the given order");
        assert_eq!(match_key(Some(1.0)), 0);
        assert_eq!(match_key(Some(0.0)), 1000);
        assert_eq!(match_key(Some(0.865)), 135);
        assert_eq!(match_key(None), u32::MAX);
    }

    #[test]
    fn a_songs_answer_reads_mood_match_null_and_absent_alike() {
        let songs: Vec<MoodSong> = serde_json::from_value(serde_json::json!([
            { "filepath": "demo/untagged/Host.mp3",
              "metadata": { "title": "Host", "artist": "ALM", "duration": 201.5, "rating": 8 },
              "moodMatch": 0.865 },
            { "filepath": "demo/untagged/Swing.mp3", "metadata": { "title": "Swing" }, "moodMatch": null },
            { "filepath": "demo/untagged/Civil Attack.mp3", "metadata": null },
        ]))
        .unwrap();
        assert_eq!(songs.iter().map(|s| s.mood_match).collect::<Vec<_>>(), vec![Some(0.865), None, None]);
        let host = &songs[0].track;
        assert_eq!(host.filepath, "demo/untagged/Host.mp3");
        assert_eq!(host.metadata.title.as_deref(), Some("Host"));
        assert_eq!(host.metadata.artist.as_deref(), Some("ALM"));
        assert_eq!((host.metadata.duration, host.metadata.rating), (Some(201.5), Some(8)));
        assert_eq!(songs[2].track.metadata.title, None, "a null metadata reads as none");
    }
}
