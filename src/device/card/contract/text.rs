//! Text (docs/METADATA.md 2.3.6) and the string run (2.6.5).
//!
//! Tag text is stored as the reader decoded it, never normalised: two
//! producers reading the same bytes write the same bytes, and the builder on
//! the device folds later. A field with several values separates them with
//! U+001F, as mStream's `genres_concat` does. [`build_field`] applies
//! 2.3.6's steps to a field's raw values; a [`Run`] is a record's twelve
//! fields as stored, and its encoding is the run a record points to.

/// The longest one value may be, in bytes.
pub(crate) const VALUE_MAX: usize = 255;
/// The longest a list may be, the separators counted.
pub(crate) const LIST_MAX: usize = 1023;
/// The most values a list holds.
pub(crate) const LIST_VALUES: usize = 16;
/// U+001F, between a list's values.
pub(crate) const SEPARATOR: u8 = 0x1F;

/// The run's fields in readRules 1, in their order (2.6.5).
pub(crate) const RUN_FIELDS: usize = 12;
pub(crate) const TITLE: usize = 0;
pub(crate) const ARTIST: usize = 1;
pub(crate) const ALBUM: usize = 2;
pub(crate) const ALBUM_ARTIST: usize = 3;
pub(crate) const GENRE: usize = 4;
pub(crate) const COMPOSER: usize = 5;
pub(crate) const TITLE_SORT: usize = 6;
pub(crate) const ARTIST_SORT: usize = 7;
pub(crate) const ALBUM_SORT: usize = 8;
pub(crate) const ALBUM_ARTIST_SORT: usize = 9;
pub(crate) const MB_ALBUM_ID: usize = 10;
pub(crate) const MB_RECORDING_ID: usize = 11;

/// The fields' names, as the fixtures' descriptions spell them.
pub(crate) const FIELD_NAMES: [&str; RUN_FIELDS] = [
    "title",
    "artist",
    "album",
    "albumArtist",
    "genre",
    "composer",
    "titleSort",
    "artistSort",
    "albumSort",
    "albumArtistSort",
    "mbAlbumId",
    "mbRecordingId",
];

/// The four fields that hold every value as a list; the others hold one.
pub(crate) fn is_list(field: usize) -> bool {
    matches!(field, ARTIST | ALBUM_ARTIST | GENRE | COMPOSER)
}

/// The longest a field may be: a list's 1,023 bytes, a single value's 255.
/// Readers accept these and may cut longer.
pub(crate) fn field_max(field: usize) -> usize {
    if is_list(field) { LIST_MAX } else { VALUE_MAX }
}

/// The longest prefix of `b` of at most `limit` bytes that ends on a code
/// point boundary: never cut before a continuation byte.
pub(crate) fn cut_utf8(b: &[u8], limit: usize) -> &[u8] {
    if b.len() <= limit {
        return b;
    }
    let mut k = limit;
    while k > 0 && b[k] & 0xC0 == 0x80 {
        k -= 1;
    }
    &b[..k]
}

/// One field from its raw values, by 2.3.6's steps in their order: control
/// characters (U+0000-U+001F, U+007F) to spaces; empty values dropped; each
/// value cut to 255 bytes at a code point boundary; byte-identical repeats
/// dropped, the first kept; then the list's limits, 16 values and 1,023
/// bytes with the separators counted, where the first value that would pass
/// either ends the list. A single-valued field is the first value left after
/// the empty ones go, and the values after it never set TRUNCATED. Returns
/// the stored bytes and whether anything was cut (the record's TRUNCATED).
pub(crate) fn build_field<V: AsRef<[u8]>>(values: &[V], list: bool) -> (Vec<u8>, bool) {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    for v in values {
        let raw = v.as_ref();
        if raw.is_empty() {
            continue;
        }
        let spaced: Vec<u8> = raw.iter().map(|&c| if c < 0x20 || c == 0x7F { 0x20 } else { c }).collect();
        let kept = cut_utf8(&spaced, VALUE_MAX);
        if kept.len() < spaced.len() {
            truncated = true;
        }
        if kept.is_empty() {
            continue;
        }
        if !list {
            return (kept.to_vec(), truncated);
        }
        if out.iter().any(|o| o.as_slice() == kept) {
            continue;
        }
        let grown = total + usize::from(!out.is_empty()) + kept.len();
        if out.len() + 1 > LIST_VALUES || grown > LIST_MAX {
            truncated = true;
            break;
        }
        out.push(kept.to_vec());
        total = grown;
    }
    (out.join(&SEPARATOR), truncated)
}

/// A field as 2.3.6 leaves it: absent, or values that are non-empty, free of
/// control characters (a list's separators aside), at most 255 bytes each
/// and unique, a list of at most 16 values and 1,023 bytes. A writer refuses
/// a run that breaks this: it would not be what any reader of the same tags
/// writes.
pub(crate) fn field_conforms(field: usize, stored: &[u8]) -> bool {
    if stored.is_empty() {
        return true;
    }
    if stored.len() > field_max(field) {
        return false;
    }
    let values: Vec<&[u8]> = stored.split(|&c| c == SEPARATOR).collect();
    if values.len() > 1 && !is_list(field) {
        return false;
    }
    if values.len() > LIST_VALUES {
        return false;
    }
    for (i, v) in values.iter().enumerate() {
        if v.is_empty() || v.len() > VALUE_MAX || v.iter().any(|&c| c < 0x20 || c == 0x7F) {
            return false;
        }
        if values[..i].contains(v) {
            return false;
        }
    }
    true
}

/// A record's twelve fields as stored (2.3.6 applied; a list's values joined
/// with U+001F), the bytes of the run its record points to. Bytes, not
/// text: tag text is stored as decoded, and a reader carries a field
/// forward unchanged whatever it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Run {
    pub(crate) fields: [Vec<u8>; RUN_FIELDS],
}

impl Run {
    /// The run of raw values, field by field (`raw[f]` is field f's values,
    /// in the order the tag gave them), and whether a value or a list was
    /// cut: the caller sets the record's TRUNCATED.
    pub(crate) fn build<V: AsRef<[u8]>>(raw: &[Vec<V>; RUN_FIELDS]) -> (Run, bool) {
        let mut run = Run::default();
        let mut truncated = false;
        for (f, values) in raw.iter().enumerate() {
            let (stored, cut) = build_field(values, is_list(f));
            run.fields[f] = stored;
            truncated |= cut;
        }
        (run, truncated)
    }

    pub(crate) fn get(&self, field: usize) -> &[u8] {
        self.fields.get(field).map_or(&[], |v| v.as_slice())
    }

    /// A field's values: one for a single-valued field, a list's split at
    /// U+001F; none when absent.
    pub(crate) fn values(&self, field: usize) -> impl Iterator<Item = &[u8]> {
        let b = self.get(field);
        b.split(|&c| c == SEPARATOR).filter(move |_| !b.is_empty())
    }

    /// A field as text, any bad UTF-8 replaced: for display and keys only.
    pub(crate) fn text(&self, field: usize) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.get(field))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.fields.iter().all(|f| f.is_empty())
    }

    pub(crate) fn conforms(&self) -> bool {
        self.fields.iter().enumerate().all(|(f, v)| field_conforms(f, v))
    }

    /// The run's bytes: one byte n (the last non-empty field's index plus
    /// one), then n NUL-terminated fields; empty for a record with no field
    /// (its `strings` is 0). The same values always give the same run.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let Some(last) = self.fields.iter().rposition(|f| !f.is_empty()) else { return Vec::new() };
        let mut out = vec![(last + 1) as u8];
        for f in &self.fields[..=last] {
            out.extend_from_slice(f);
            out.push(0);
        }
        out
    }
}
