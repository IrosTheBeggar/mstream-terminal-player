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
//! page's taste body takes the same key. A server says it can filter with
//! the ping's `moodFilters` (scores to filter by) — and the taste body,
//! which refuses keys it does not know, only from `discoveryTaste` 2.

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

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

/// One room's filter: each scale's chosen value as an index into its
/// [`MoodScale::choices`], the tags as bits in [`MOOD_TAGS`] order. `Copy`
/// and `Eq`, because it is part of a library view's identity
/// (`LibraryNode::Albums`), so an answer asked for under another filter is
/// dropped when it lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MoodFilter {
    valence: Option<u8>,
    energy: Option<u8>,
    voice: Option<u8>,
    tags: u8,
}

impl MoodFilter {
    /// No filter: every song.
    pub const NONE: MoodFilter = MoodFilter { valence: None, energy: None, voice: None, tags: 0 };

    pub fn is_empty(&self) -> bool {
        *self == MoodFilter::NONE
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

/// The wire form: only the groups that filter, tags in a fixed order —
/// `{"energy":"relaxed","tags":["acoustic"]}`. A caller sends the key only
/// for a filter that is not empty.
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
        map.end()
    }
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
}
