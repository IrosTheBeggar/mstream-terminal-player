//! Constant bitrate, read off the head of an MP3 stream: the one case in
//! which a seek can be computed from byte offsets and land where it says it
//! did (performance audit #72).
//!
//! rodio decodes MP3 through symphonia, whose default seek ("accurate")
//! never moves the reader forward: it walks every frame from where it is to
//! the target. On a local file the walk costs microseconds. On a stream it
//! reads every byte in between as the download delivers it, so a seek past
//! the download frontier waits for the whole gap on the original connection
//! — 37.8s for a 90% seek one second into a six-minute track at 2 Mbit/s.
//! A coarse seek computes a byte offset and moves the reader there instead:
//! one Range request. But symphonia's coarse offset is proportional (it
//! parses a VBR file's Xing table of contents and never uses it), so it is
//! exact on a constant-bitrate file and seconds off on a variable one — and
//! rodio reports the requested position either way, which is the clock the
//! engine times its crossfades and gapless appends by. Coarse is for CBR
//! only; this says which a stream is.

use std::io::{self, Read, Seek, SeekFrom};

/// Frames that must agree on one bitrate before a stream with no Xing,
/// Info or VBRI frame counts as constant.
const AGREEING_FRAMES: usize = 8;

/// Whether `reader` holds constant-bitrate MPEG Layer III audio, leaving it
/// rewound to the start either way (the only error is a failed rewind: the
/// decoder cannot start mid-stream). Anything the sniff cannot vouch for —
/// a Xing or VBRI frame, bitrates that differ, a first frame that is not
/// where the ID3v2 tag says it is, not MPEG at all — is a no, which keeps
/// the exact accurate seek.
pub(crate) fn is_cbr_mp3<R: Read + Seek>(reader: &mut R) -> io::Result<bool> {
    let cbr = sniff(reader).unwrap_or(false);
    reader.seek(SeekFrom::Start(0))?;
    Ok(cbr)
}

/// Reads no further than the verdict needs, since every byte of it may
/// still be on its way: the tag (the probe reads it next anyway), then one
/// frame when that frame is a Xing, Info or VBRI tag, or AGREEING_FRAMES
/// when it is audio.
fn sniff<R: Read>(reader: &mut R) -> io::Result<bool> {
    let mut head = [0u8; 10];
    reader.read_exact(&mut head)?;
    let mut bytes = Vec::new();
    if let Some(len) = id3v2_len(&head) {
        // Read through the tag rather than seeking past it: on a stream, a
        // seek past the download frontier is a Range request, and the
        // decoder's probe reads the tag from the spool straight after.
        io::copy(&mut reader.by_ref().take(len - head.len() as u64), &mut io::sink())?;
    } else if Header::parse(&head).is_some() {
        bytes.extend_from_slice(&head);
    } else {
        // Neither a tag nor a frame: FLAC, WAV, Ogg, MP4. Nothing to wait for.
        return Ok(false);
    }
    let mut first: Option<Header> = None;
    let mut at = 0;
    for _ in 0..AGREEING_FRAMES {
        fill(reader, &mut bytes, at + 4)?;
        let Some(frame) = Header::parse(&bytes[at..]) else { return Ok(false) };
        fill(reader, &mut bytes, at + frame.len)?;
        match &first {
            None => {
                if let Some(verdict) = tag_verdict(&bytes[at..], &frame) {
                    return Ok(verdict);
                }
                first = Some(frame);
            }
            // No tag. Symphonia then estimates the length from the first
            // frames' average size, and a coarse seek is exact only if every
            // frame is that size give or take the padding byte: so the
            // frames must share one bitrate.
            Some(first) if !frame.same_rate_as(first) => return Ok(false),
            Some(_) => {}
        }
        at = bytes.len();
    }
    Ok(true)
}

/// Reads on until `bytes` holds `len` of them.
fn fill<R: Read>(reader: &mut R, bytes: &mut Vec<u8>, len: usize) -> io::Result<()> {
    let have = bytes.len();
    if have < len {
        bytes.resize(len, 0);
        reader.read_exact(&mut bytes[have..])?;
    }
    Ok(())
}

/// The whole ID3v2 tag's length, header (and footer) included, when `head`
/// starts one.
fn id3v2_len(head: &[u8; 10]) -> Option<u64> {
    let syncsafe = &head[6..10];
    if &head[..3] != b"ID3" || head[3] == 0xFF || syncsafe.iter().any(|&b| b & 0x80 != 0) {
        return None;
    }
    let size = syncsafe.iter().fold(0u64, |acc, &b| acc << 7 | u64::from(b));
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + size + footer)
}

/// What a first frame says about the stream when it is a tag rather than
/// audio: Some(true) for constant, Some(false) for variable, None for no
/// tag at all.
fn tag_verdict(frame: &[u8], header: &Header) -> Option<bool> {
    // The Xing/Info tag sits right after the side information, which a tag
    // frame leaves zeroed — the same test symphonia applies, so the two of
    // us agree on which frames are tags. LAME writes "Info" for CBR and
    // "Xing" for VBR and ABR. An Info tag must carry its frame count (flag
    // bit 0): that is the length symphonia's coarse seek divides by, and
    // without it the seek is refused outright.
    let tag_at = 4 + header.side_info_len();
    let quiet = frame.get(4..tag_at).is_some_and(|side| side.iter().all(|&b| b == 0));
    match frame.get(tag_at..tag_at + 8) {
        Some(&[b'I', b'n', b'f', b'o', _, _, _, flags]) if quiet => return Some(flags & 1 != 0),
        Some(&[b'X', b'i', b'n', b'g', ..]) if quiet => return Some(false),
        _ => {}
    }
    (frame.get(36..40) == Some(b"VBRI")).then_some(false)
}

/// The parts of an MPEG audio frame header the sniff needs.
#[derive(Debug, PartialEq)]
struct Header {
    mpeg1: bool,
    mono: bool,
    sample_rate: u32,
    bitrate: u32,
    /// The whole frame, header included.
    len: usize,
}

impl Header {
    /// A Layer III frame header at the start of `bytes`, or None. Free
    /// format and the reserved values are refused, as symphonia refuses
    /// them.
    fn parse(bytes: &[u8]) -> Option<Header> {
        const MPEG1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
        const MPEG2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
        let word = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?);
        let version = (word >> 19) & 3; // 0 = MPEG 2.5, 1 reserved, 2 = MPEG 2, 3 = MPEG 1
        let layer = (word >> 17) & 3; // 1 = Layer III
        let bitrate_index = ((word >> 12) & 0xF) as usize;
        let rate_index = ((word >> 10) & 3) as usize;
        if word >> 21 != 0x7FF
            || version == 1
            || layer != 1
            || bitrate_index == 0
            || bitrate_index == 15
            || rate_index == 3
        {
            return None;
        }
        let mpeg1 = version == 3;
        let bitrate = if mpeg1 { MPEG1 } else { MPEG2 }[bitrate_index] * 1000;
        let sample_rate = [44_100, 48_000, 32_000][rate_index] >> [2, 0, 1, 0][version as usize];
        let padding = (word >> 9) & 1;
        let len = (if mpeg1 { 144 } else { 72 } * bitrate / sample_rate + padding) as usize;
        Some(Header { mpeg1, mono: (word >> 6) & 3 == 3, sample_rate, bitrate, len })
    }

    fn side_info_len(&self) -> usize {
        match (self.mpeg1, self.mono) {
            (true, true) => 17,
            (true, false) => 32,
            (false, true) => 9,
            (false, false) => 17,
        }
    }

    fn same_rate_as(&self, other: &Header) -> bool {
        self.mpeg1 == other.mpeg1
            && self.mono == other.mono
            && self.sample_rate == other.sample_rate
            && self.bitrate == other.bitrate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// One MPEG-1 Layer III frame at 44.1 kHz, joint stereo, silent.
    fn frame(kbps: u32, padded: bool) -> Vec<u8> {
        let index = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320]
            .iter()
            .position(|&k| k == kbps)
            .unwrap() as u8;
        let len = (144 * kbps * 1000 / 44_100) as usize + usize::from(padded);
        let mut bytes = vec![0u8; len];
        bytes[..4].copy_from_slice(&[0xFF, 0xFB, index << 4 | u8::from(padded) << 1, 0x40]);
        bytes
    }

    /// A tag frame: side information zeroed, `id` at byte `at`, and flags
    /// saying a frame count follows.
    fn tagged(id: &[u8; 4], at: usize) -> Vec<u8> {
        let mut bytes = frame(128, false);
        bytes[at..at + 4].copy_from_slice(id);
        bytes[at + 7] = 1;
        bytes[at + 8..at + 12].copy_from_slice(&12_000u32.to_be_bytes());
        bytes
    }

    fn stream(first: Vec<u8>, rates: &[u32]) -> Vec<u8> {
        let mut bytes = first;
        for (i, &kbps) in rates.iter().enumerate() {
            bytes.extend(frame(kbps, i % 3 == 0));
        }
        bytes
    }

    fn with_id3(tag_body: usize, flags: u8, audio: Vec<u8>) -> Vec<u8> {
        let size = tag_body as u32;
        let mut bytes = b"ID3\x04\x00".to_vec();
        bytes.push(flags);
        bytes.extend([size >> 21, size >> 14, size >> 7, size].map(|b| (b & 0x7F) as u8));
        bytes.extend(vec![0xAAu8; tag_body]);
        if flags & 0x10 != 0 {
            bytes.extend(b"3DI\x04\x00\x10\x00\x00\x00\x00");
        }
        bytes.extend(audio);
        bytes
    }

    fn cbr(bytes: Vec<u8>) -> bool {
        let mut reader = Cursor::new(bytes);
        let cbr = is_cbr_mp3(&mut reader).unwrap();
        assert_eq!(reader.position(), 0, "always rewound for the decoder");
        cbr
    }

    #[test]
    fn an_info_frame_is_constant_bitrate_and_a_xing_or_vbri_frame_is_not() {
        let many = [192; 40];
        assert!(cbr(stream(tagged(b"Info", 36), &many)), "LAME's CBR tag");
        assert!(!cbr(stream(tagged(b"Xing", 36), &many)), "LAME's VBR tag");
        assert!(!cbr(stream(tagged(b"VBRI", 36), &many)), "Fraunhofer's VBR tag");
        // An Info tag without its frame count leaves symphonia nothing to
        // divide by, and its coarse seek refuses.
        let mut uncounted = tagged(b"Info", 36);
        uncounted[43] = 0;
        assert!(!cbr(stream(uncounted, &many)));
    }

    #[test]
    fn a_tag_word_over_live_side_information_is_audio_not_a_tag() {
        // Symphonia only treats the frame as a tag when the side
        // information is zeroed; otherwise it is audio, and the frames
        // themselves have to make the case.
        let mut first = tagged(b"Info", 36);
        first[10] = 0x5A;
        assert!(!cbr(stream(first, &[128, 320, 128, 320, 128, 320, 128, 320])));
    }

    #[test]
    fn untagged_frames_count_as_constant_only_when_they_agree() {
        assert!(cbr(stream(frame(192, false), &[192; 20])));
        assert!(!cbr(stream(frame(192, false), &[192, 192, 256, 192, 192, 192, 192, 192])));
        // Too few frames to judge: no.
        assert!(!cbr(stream(frame(192, false), &[192, 192])));
    }

    #[test]
    fn the_id3v2_tag_is_read_through_and_its_footer_counted() {
        let audio = stream(tagged(b"Info", 36), &[128; 30]);
        assert!(cbr(with_id3(36_000, 0, audio.clone())), "a cover-sized tag");
        assert!(cbr(with_id3(300, 0x10, audio.clone())), "a footer is ten bytes more");
        // A first frame that is not where the tag says it ends: whatever
        // symphonia makes of the junk, the sniff will not vouch for it.
        let mut shifted = vec![0u8; 3];
        shifted.extend(audio);
        assert!(!cbr(with_id3(300, 0, shifted)));
    }

    #[test]
    fn the_sniff_reads_no_further_than_its_verdict_needs() {
        // Every byte the sniff reads may still be on its way over the
        // network, so a tag frame settles it at once, and audio frames are
        // read only until eight agree.
        struct Counted(Cursor<Vec<u8>>, u64);
        impl Read for Counted {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.0.read(buf)?;
                self.1 += n as u64;
                Ok(n)
            }
        }
        impl Seek for Counted {
            fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
                self.0.seek(to)
            }
        }
        let read = |bytes: Vec<u8>| {
            let mut reader = Counted(Cursor::new(bytes), 0);
            is_cbr_mp3(&mut reader).unwrap();
            reader.1
        };
        let tag = with_id3(36_000, 0, stream(tagged(b"Info", 36), &[128; 200]));
        assert_eq!(read(tag), 36_010 + 417, "the tag and one frame");
        let untagged = stream(frame(192, false), &[192; 200]);
        assert!(read(untagged) < 8 * 628, "eight frames");
        assert_eq!(read(b"fLaC".repeat(1000)), 10, "a glance");
    }

    #[test]
    fn other_formats_and_short_files_are_not_mp3() {
        assert!(!cbr(b"fLaC\x00\x00\x00\x22".repeat(100)));
        assert!(!cbr(b"RIFF\x24\x00\x00\x00WAVEfmt ".repeat(100)));
        // ADTS AAC shares the sync word but not the layer.
        assert!(!cbr([0xFF, 0xF1, 0x50, 0x80].repeat(1000)));
        assert!(!cbr(b"ID3".to_vec()));
        assert!(!cbr(Vec::new()));
    }

    #[test]
    fn mpeg2_frames_measure_at_half_the_slots() {
        // MPEG-2 Layer III at 22.05 kHz, 64 kbps: 72 * 64000 / 22050 = 208.
        let header = Header::parse(&[0xFF, 0xF3, 0x80, 0x40]).unwrap();
        assert_eq!((header.mpeg1, header.sample_rate, header.bitrate), (false, 22_050, 64_000));
        assert_eq!(header.len, 208);
        assert_eq!(header.side_info_len(), 17);
        // MPEG-2.5 at 8 kHz.
        assert_eq!(Header::parse(&[0xFF, 0xE3, 0x88, 0xC0]).unwrap().sample_rate, 8_000);
    }
}
