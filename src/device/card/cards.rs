//! `cards.json`, beside `stats.json` in the player's config folder: the
//! cards this computer has seen, by card id — each one's label, when it was
//! last seen, and its revision then (its root's generation and commit id).
//! It is a cache of what each card says of itself (the firmware repo's
//! docs/METADATA.md 2.12.7: a card whose state file is gone has its
//! settings taken from here), and it is the second sign of a copied card
//! (the transfer plan's 2.10): a byte-level clone keeps the volume serial,
//! so only a card id seen at one generation with another commit shows it
//! ([`Cards::diverged`]). Disposable, as stats.json is: a file that doesn't
//! read is no cache at all, and the next save writes it over; one of a newer
//! player is read as none and never written over. Every member this player
//! doesn't know is kept.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::state::{Copied, from_hex, hex};

/// The file's schema.
pub(crate) const VERSION: u64 = 1;

/// The members of a card's entry this player reads and writes.
const KNOWN: [&str; 4] = ["label", "lastSeen", "generation", "commitId"];

/// A card as this computer last saw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Seen {
    pub(crate) label: String,
    /// Unix seconds.
    pub(crate) last_seen: u64,
    /// Its root then.
    pub(crate) generation: u32,
    pub(crate) commit_id: u64,
    /// A newer player's members, by name (a multi-card set's, say).
    pub(crate) keep: BTreeMap<String, Value>,
}

/// The cache.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Cards {
    pub(crate) cards: BTreeMap<u64, Seen>,
    keep: BTreeMap<String, Value>,
    /// A newer player's file: read as none, and never written over.
    newer: bool,
}

impl Cards {
    /// The cache a file holds: none when it doesn't read as one.
    pub(crate) fn parse(text: &str) -> Cards {
        let Ok(Value::Object(top)) = serde_json::from_str::<Value>(text) else {
            return Cards::default();
        };
        match top.get("version").and_then(Value::as_u64) {
            Some(VERSION) => {}
            Some(v) if v > VERSION => return Cards { newer: true, ..Cards::default() },
            _ => return Cards::default(),
        }
        let mut cards = BTreeMap::new();
        for (id, entry) in top.get("cards").and_then(Value::as_object).into_iter().flatten() {
            let (Some(id), Some(entry)) = (from_hex(id), entry.as_object()) else { continue };
            let number = |key: &str| entry.get(key).and_then(Value::as_u64);
            let text = |key: &str| entry.get(key).and_then(Value::as_str);
            let keep = entry.iter().filter(|(name, _)| !KNOWN.contains(&name.as_str()));
            let seen = Seen {
                label: text("label").unwrap_or_default().to_string(),
                last_seen: number("lastSeen").unwrap_or(0),
                generation: number("generation").and_then(|g| u32::try_from(g).ok()).unwrap_or(0),
                commit_id: text("commitId").and_then(from_hex).unwrap_or(0),
                keep: keep.map(|(name, value)| (name.clone(), value.clone())).collect(),
            };
            cards.insert(id, seen);
        }
        let keep = top.iter().filter(|(name, _)| !matches!(name.as_str(), "version" | "cards"));
        let keep = keep.map(|(name, value)| (name.clone(), value.clone())).collect();
        Cards { cards, keep, newer: false }
    }

    /// The file's text.
    pub(crate) fn text(&self) -> String {
        let mut cards = Map::new();
        for (id, seen) in &self.cards {
            let mut entry: Map<String, Value> = seen.keep.clone().into_iter().collect();
            entry.insert("label".into(), seen.label.as_str().into());
            entry.insert("lastSeen".into(), seen.last_seen.into());
            entry.insert("generation".into(), seen.generation.into());
            entry.insert("commitId".into(), hex(seen.commit_id).into());
            cards.insert(hex(*id), Value::Object(entry));
        }
        let mut top: Map<String, Value> = self.keep.clone().into_iter().collect();
        top.insert("version".into(), json!(VERSION));
        top.insert("cards".into(), Value::Object(cards));
        serde_json::to_string_pretty(&Value::Object(top)).unwrap_or_default() + "\n"
    }

    /// This computer's cache; none when there is no file or it doesn't
    /// read.
    pub(crate) fn load() -> Cards {
        match crate::config::load_cards_file() {
            Ok(Some(text)) => Cards::parse(&text),
            _ => Cards::default(),
        }
    }

    /// Written back, through config.rs's atomic write; a newer player's file
    /// is left as it is.
    pub(crate) fn save(&self) -> Result<(), String> {
        if self.newer {
            return Ok(());
        }
        crate::config::save_cards_file(&self.text())
    }

    /// The second sign of a copied card: this computer saw the card id at
    /// this generation with another commit.
    pub(crate) fn diverged(&self, card_id: u64, generation: u32, commit_id: u64) -> Option<Copied> {
        let seen = self.cards.get(&card_id)?;
        let other = seen.generation == generation && seen.commit_id != commit_id;
        other.then_some(Copied::Diverged { generation, seen: seen.commit_id, now: commit_id })
    }

    /// The card as it is now: its label, the time, its root's revision —
    /// unless that revision [`diverged`](Cards::diverged) from the one kept:
    /// the card seen first keeps its revision here, so the other one is the
    /// copy at every look (and the first is not), until one of them takes an
    /// identity of its own.
    pub(crate) fn saw(&mut self, id: u64, label: &str, now: u64, generation: u32, commit: u64) {
        let diverged = self.diverged(id, generation, commit).is_some();
        let seen = self.cards.entry(id).or_default();
        seen.label = label.to_string();
        seen.last_seen = now;
        if !diverged {
            seen.generation = generation;
            seen.commit_id = commit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_reads_back_what_it_wrote_and_keeps_what_it_doesnt_know() {
        let mut cards = Cards::default();
        cards.saw(0x5EED_C0DE_0A1B_2C3D, "Car", 1_791_900_000, 1, 0xC0FF_EE00);
        let text = cards.text();
        assert!(text.contains("\"0x5EEDC0DE0A1B2C3D\""));
        assert!(text.contains("\"commitId\": \"0x00000000C0FFEE00\""), "{text}");
        assert_eq!(Cards::parse(&text), cards);

        // A newer player's members, in an entry and at the top, stay.
        let newer = r#"{"version": 1, "sets": {"a": 1}, "cards": {
            "0x0000000000000009": {"label": "Gym", "lastSeen": 5, "generation": 3, "commitId": "0x0A",
                                   "set": {"id": "0x01", "index": 2}},
            "not hex": {"label": "dropped"}}}"#;
        let mut read = Cards::parse(newer);
        assert_eq!(read.cards.len(), 1);
        let gym = &read.cards[&9];
        let fields = (gym.label.as_str(), gym.last_seen, gym.generation, gym.commit_id);
        assert_eq!(fields, ("Gym", 5, 3, 10));
        read.saw(9, "Gym bag", 6, 4, 11);
        let again = Cards::parse(&read.text());
        assert_eq!(again.cards[&9].keep["set"], json!({"id": "0x01", "index": 2}));
        assert_eq!(again.keep["sets"], json!({"a": 1}));
        assert_eq!(again.cards[&9].label, "Gym bag");

        // A file that doesn't read is no cache; a newer one is read as none
        // and never written over.
        for broken in ["", "[]", "{\"cards\": {}}", "{\"version\": 0}", "{\"version\": \"1\"}"] {
            assert_eq!(Cards::parse(broken), Cards::default(), "{broken}");
        }
        let future = Cards::parse(r#"{"version": 2, "cards": {"0x01": {"label": "x"}}}"#);
        assert!(future.cards.is_empty() && future.newer);
    }

    #[test]
    fn one_card_id_at_one_generation_with_two_commits_is_two_cards() {
        let mut cards = Cards::default();
        assert_eq!(cards.diverged(9, 1, 100), None, "never seen");
        cards.saw(9, "Car", 1, 1, 100);
        assert_eq!(cards.diverged(9, 1, 100), None, "the commit seen");
        assert_eq!(cards.diverged(9, 2, 200), None, "a later commit: a sync since");
        let sign = Copied::Diverged { generation: 1, seen: 100, now: 101 };
        assert_eq!(cards.diverged(9, 1, 101), Some(sign.clone()));
        assert_eq!(cards.diverged(8, 1, 101), None, "another card");

        // The copy seen keeps the revision seen first: the copy is the one
        // said to be a copy at every look, and the first card never is.
        cards.saw(9, "Car copy", 2, 1, 101);
        let seen = &cards.cards[&9];
        let fields = (seen.label.as_str(), seen.last_seen, seen.generation, seen.commit_id);
        assert_eq!(fields, ("Car copy", 2, 1, 100));
        assert_eq!(cards.diverged(9, 1, 100), None);
        assert_eq!(cards.diverged(9, 1, 101), Some(sign));
        // A sync since, of either: its next generation is the one kept.
        cards.saw(9, "Car", 3, 2, 200);
        assert_eq!((cards.cards[&9].generation, cards.cards[&9].commit_id), (2, 200));
    }
}
