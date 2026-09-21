//! What this build can decode — asked of the decoder, not remembered.
//!
//! A client choosing what to send the engine has so far had to know the
//! answer from somewhere else: mStream's web remote carries a hard-coded
//! "Opus is not supported" because nothing here could tell it. `GET
//! /version` now does. The codec list is read out of symphonia's own
//! registry — the one rodio decodes through (one symphonia in the tree, see
//! Cargo.toml) — so it cannot drift from the build: the day a symphonia
//! release grows an Opus decoder, "opus" appears in the answer without
//! anyone remembering to add it, and the test below says so.

use symphonia::core::codecs::{
    CODEC_TYPE_AAC, CODEC_TYPE_ADPCM_MS, CODEC_TYPE_ALAC, CODEC_TYPE_FLAC, CODEC_TYPE_MP1,
    CODEC_TYPE_MP2, CODEC_TYPE_MP3, CODEC_TYPE_OPUS, CODEC_TYPE_PCM_S16LE, CODEC_TYPE_VORBIS,
    CodecType,
};

/// Every codec a client might ask about, under the name it goes by in a
/// file's metadata. One representative stands in for the families (PCM and
/// ADPCM register a dozen variants each, together or not at all).
const KNOWN: &[(&str, CodecType)] = &[
    ("aac", CODEC_TYPE_AAC),
    ("adpcm", CODEC_TYPE_ADPCM_MS),
    ("alac", CODEC_TYPE_ALAC),
    ("flac", CODEC_TYPE_FLAC),
    ("mp1", CODEC_TYPE_MP1),
    ("mp2", CODEC_TYPE_MP2),
    ("mp3", CODEC_TYPE_MP3),
    ("opus", CODEC_TYPE_OPUS),
    ("pcm", CODEC_TYPE_PCM_S16LE),
    ("vorbis", CODEC_TYPE_VORBIS),
];

/// The codecs with a decoder registered in this build.
pub fn codecs() -> Vec<&'static str> {
    let registry = symphonia::default::get_codecs();
    KNOWN
        .iter()
        .filter(|(_, codec)| registry.get_codec(*codec).is_some())
        .map(|(name, _)| *name)
        .collect()
}

/// The containers symphonia was built to read. Its probe keeps no list to
/// ask, so this one is remembered: it is the `all` feature set in
/// Cargo.toml — six format crates, plus the native readers that ride along
/// with the FLAC, MPEG-audio and AAC (ADTS) codecs. A container says where
/// the audio sits, not what it is: "ogg" here means Ogg Vorbis plays, and
/// an Ogg Opus file still fails on its codec.
pub const CONTAINERS: &[&str] =
    &["adts", "aiff", "caf", "flac", "isomp4", "mkv", "mpa", "ogg", "wav"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codec_list_is_what_the_decoder_says_it_is() {
        let have = codecs();
        // What a music library is actually made of.
        for expected in ["aac", "alac", "flac", "mp3", "pcm", "vorbis"] {
            assert!(have.contains(&expected), "{expected} should decode in this build: {have:?}");
        }
        // The reason the list exists. When this fails, symphonia has an
        // Opus decoder: delete mStream's hard-coded refusal, update
        // PLAN.md (finding #14) and the README, then flip this assert.
        assert!(!have.contains(&"opus"), "symphonia grew an Opus decoder — see the comment");
    }
}
