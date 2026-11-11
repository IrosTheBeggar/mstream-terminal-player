//! A conversion's length, read from its own bytes (the transfer plan's 2.3,
//! step 9). `/transcode` streams with no length, and a 200 that ends early
//! when the server's ffmpeg dies looks like any other (mStream's
//! `transcode.js`), so a converted file is held to the length the manifest
//! gives its source, within a second, before the card keeps it. The PC has
//! no Opus decoder and needs none: an Ogg Opus stream's length is its last
//! page's granule position less the pre-skip its `OpusHead` names (RFC 7845,
//! 4; the firmware repo's tools/opus_check reads it the same way), and an
//! MP3's is its frames counted, each of a fixed number of samples.
//!
//! What is read: an Opus file's first page and its last (the tail up to the
//! largest page there can be); an MP3's frame headers, the whole file in
//! blocks. From a file on this computer, or a card's stage file ([`At`]).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use crate::api::urls::TransferCodec;

use super::super::volume::CardFs;

/// The largest Ogg page: its header, 255 lacing values, 255 × 255 bytes.
const MAX_PAGE: usize = 27 + 255 + 255 * 255;
/// The most a length may differ from the manifest's.
pub(crate) const TOLERANCE_MS: u64 = 1_000;
/// What an MP3 walk reads at a time.
const BLOCK: usize = 256 * 1024;

/// Bytes read where they lie: a file here, or one on the card.
pub(crate) trait At {
    fn size(&self) -> u64;
    /// `len` bytes from `offset`, fewer at the end.
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>>;
}

/// A file on this computer.
pub(crate) struct FileAt {
    file: std::cell::RefCell<File>,
    size: u64,
}

impl FileAt {
    pub(crate) fn open(path: &std::path::Path) -> io::Result<FileAt> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(FileAt { file: std::cell::RefCell::new(file), size })
    }
}

impl At for FileAt {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(offset))?;
        let mut out = Vec::with_capacity(len.min(BLOCK));
        (&mut *file).take(len as u64).read_to_end(&mut out)?;
        Ok(out)
    }
}

/// A file on the card, by its card path.
pub(crate) struct CardAt<'c> {
    pub(crate) card: &'c dyn CardFs,
    pub(crate) path: &'c str,
    pub(crate) size: u64,
}

impl At for CardAt<'_> {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.card.read_at(self.path, offset, len)
    }
}

/// What a converted file's bytes say of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Heard {
    /// Its length in ms, what a player plays of it.
    pub(crate) ms: u64,
    pub(crate) channels: u8,
    /// It ends as a finished stream does: an Ogg stream's last page marks
    /// its end (EOS). An MP3 has no such mark: a cut one is short.
    pub(crate) ended: bool,
}

/// A converted file's length, by the codec it was asked in.
pub(crate) fn heard(codec: TransferCodec, at: &dyn At) -> Result<Heard, String> {
    match codec {
        TransferCodec::Opus => opus(at),
        TransferCodec::Mp3 => mp3(at),
    }
}

/// A converted file as a player would take it: finished, of the length the
/// manifest gives its source (`expected_ms`, when it gives one) within
/// [`TOLERANCE_MS`], and of no more channels than the player plays.
pub(crate) fn check(heard: &Heard, expected_ms: Option<u64>, channels: u32) -> Result<(), String> {
    if !heard.ended {
        return Err("the stream stops before its end".into());
    }
    if heard.channels as u32 > channels {
        return Err(format!("{} channels, where the player plays {channels}", heard.channels));
    }
    match expected_ms {
        Some(want) if heard.ms.abs_diff(want) > TOLERANCE_MS => {
            Err(format!("{} ms long, where the track is {want} ms", heard.ms))
        }
        _ => Ok(()),
    }
}

// ── Ogg Opus ────────────────────────────────────────────────────────────────

/// Ogg's CRC-32: polynomial 0x04C11DB7, no reflection, from 0.
fn ogg_crc(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in bytes {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

/// One page's header, read at the start of `b`.
struct Page {
    flags: u8,
    granule: i64,
    serial: u32,
    /// The header and the body: the whole page.
    len: usize,
    /// Where the body starts.
    body: usize,
}

fn page(b: &[u8]) -> Option<Page> {
    if b.len() < 27 || &b[..4] != b"OggS" || b[4] != 0 {
        return None;
    }
    let segments = b[26] as usize;
    let lacing = b.get(27..27 + segments)?;
    let body = 27 + segments;
    let len = body + lacing.iter().map(|&l| l as usize).sum::<usize>();
    let granule = i64::from_le_bytes(b[6..14].try_into().ok()?);
    let serial = u32::from_le_bytes(b[14..18].try_into().ok()?);
    Some(Page { flags: b[5], granule, serial, len, body })
}

/// The page is whole: its CRC is the one its bytes give.
fn page_ok(b: &[u8], p: &Page) -> bool {
    if b.len() < p.len {
        return false;
    }
    let mut copy = b[..p.len].to_vec();
    let stored = u32::from_le_bytes(copy[22..26].try_into().unwrap_or_default());
    copy[22..26].fill(0);
    ogg_crc(&copy) == stored
}

/// An Ogg Opus stream: its last page's granule less the pre-skip, at 48 kHz.
fn opus(at: &dyn At) -> Result<Heard, String> {
    let size = at.size();
    let head = at.read_at(0, MAX_PAGE.min(size as usize)).map_err(|e| e.to_string())?;
    let first = page(&head).filter(|p| page_ok(&head, p)).ok_or("no Ogg page at its start")?;
    let packet = &head[first.body..first.len];
    if first.flags & 0x02 == 0 || packet.len() < 19 || &packet[..8] != b"OpusHead" {
        return Err("not an Ogg Opus stream".into());
    }
    let channels = packet[9];
    let pre_skip = u16::from_le_bytes([packet[10], packet[11]]) as i64;

    // The last page ends where the file does: found from the end back.
    let from = size.saturating_sub(MAX_PAGE as u64);
    let tail = at.read_at(from, (size - from) as usize).map_err(|e| e.to_string())?;
    let mut last = None;
    let mut k = tail.len().saturating_sub(27);
    loop {
        if tail[k..].starts_with(b"OggS")
            && let Some(p) = page(&tail[k..])
            && k + p.len == tail.len()
            && p.serial == first.serial
            && page_ok(&tail[k..], &p)
        {
            last = Some(p);
            break;
        }
        if k == 0 {
            break;
        }
        k -= 1;
    }
    let last = last.ok_or("the stream stops partway through a page")?;
    if last.granule < 0 {
        return Err("its last page ends no packet".into());
    }
    // 48 samples a millisecond: no product to overflow, whatever the page.
    let samples = (last.granule - pre_skip).max(0) as u64;
    Ok(Heard { ms: samples / 48, channels, ended: last.flags & 0x04 != 0 })
}

// ── MP3 ─────────────────────────────────────────────────────────────────────

/// One MPEG audio frame's header: its length in bytes, its samples, its
/// sample rate, its channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    len: usize,
    samples: u32,
    rate: u32,
    channels: u8,
    /// Where a Xing or Info header would start, past the side information.
    side_end: usize,
}

fn frame(h: [u8; 4]) -> Option<Frame> {
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0: 2.5, 2: 2, 3: 1
    let layer = (h[1] >> 1) & 3; // 1: III, 2: II, 3: I
    let (bitrate_index, rate_index) = ((h[2] >> 4) as usize, ((h[2] >> 2) & 3) as usize);
    if version == 1 || layer == 0 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
        return None;
    }
    const V1: [[u32; 15]; 3] = [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    ];
    const V2: [[u32; 15]; 2] = [
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ];
    let mpeg1 = version == 3;
    let l = (3 - layer) as usize; // 0: I, 1: II, 2: III
    let kbps = if mpeg1 { V1[l][bitrate_index] } else { V2[l.min(1)][bitrate_index] };
    let rates = match version {
        3 => [44_100, 48_000, 32_000],
        2 => [22_050, 24_000, 16_000],
        _ => [11_025, 12_000, 8_000],
    };
    let rate = rates[rate_index];
    let padding = ((h[2] >> 1) & 1) as usize;
    let channels = if h[3] >> 6 == 3 { 1 } else { 2 };
    let (len, samples) = match l {
        0 => ((12 * kbps * 1000 / rate) as usize * 4 + padding * 4, 384),
        1 => ((144 * kbps * 1000 / rate) as usize + padding, 1152),
        _ if mpeg1 => ((144 * kbps * 1000 / rate) as usize + padding, 1152),
        _ => ((72 * kbps * 1000 / rate) as usize + padding, 576),
    };
    let side = match (mpeg1, channels) {
        (true, 1) => 17,
        (true, _) => 32,
        (false, 1) => 9,
        (false, _) => 17,
    };
    Some(Frame { len, samples, rate, channels, side_end: 4 + side })
}

/// An MP3's length: its frames counted from the first past an ID3v2 tag,
/// a leading Xing or Info frame (no audio) left out, up to the last whole
/// frame before the end or a tag at the end.
fn mp3(at: &dyn At) -> Result<Heard, String> {
    let size = at.size();
    let read = |offset: u64, len: usize| at.read_at(offset, len).map_err(|e| e.to_string());
    let mut pos = 0u64;
    let id3 = read(0, 10)?;
    if id3.len() == 10 && &id3[..3] == b"ID3" {
        let synchsafe = id3[6..10].iter().fold(0u64, |v, &b| v << 7 | (b & 0x7F) as u64);
        let footer = if id3[5] & 0x10 != 0 { 10 } else { 0 };
        pos = 10 + synchsafe + footer;
    }
    let mut block: Vec<u8> = Vec::new();
    let mut block_at = pos;
    let mut seconds = 0f64;
    let mut channels = 0u8;
    let mut frames = 0u64;
    loop {
        if pos + 4 > size {
            break;
        }
        if pos < block_at || pos + 4 > block_at + block.len() as u64 {
            block = read(pos, BLOCK)?;
            block_at = pos;
            if block.len() < 4 {
                // Shorter than its size said: the frames so far.
                break;
            }
        }
        let at_block = (pos - block_at) as usize;
        let head = [block[at_block], block[at_block + 1], block[at_block + 2], block[at_block + 3]];
        let Some(f) = frame(head) else {
            // A tag at the end (ID3v1, APE), or bytes that are no frame:
            // the frames counted so far are the length.
            break;
        };
        if pos + f.len as u64 > size {
            break;
        }
        let tagged = frames == 0 && {
            let start = at_block + f.side_end;
            let tag = |b: &[u8]| b.get(start..start + 4).is_some_and(|t| t == b"Xing" || t == b"Info");
            if block.len() >= start + 4 { tag(&block) } else { tag(&read(pos, f.side_end + 4)?) }
        };
        if !tagged {
            seconds += f.samples as f64 / f.rate as f64;
            channels = channels.max(f.channels);
        }
        frames += 1;
        pos += f.len as u64;
    }
    if seconds == 0.0 {
        return Err("no MPEG audio frame in it".into());
    }
    Ok(Heard { ms: (seconds * 1000.0).round() as u64, channels, ended: true })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bytes in memory as a file.
    pub(crate) struct Mem(pub(crate) Vec<u8>);

    impl At for Mem {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }

        fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            let start = (offset as usize).min(self.0.len());
            Ok(self.0[start..(start + len).min(self.0.len())].to_vec())
        }
    }

    /// One Ogg page of `packets` (each under 255 bytes), CRC and all.
    fn ogg_page(flags: u8, granule: i64, seq: u32, packets: &[&[u8]]) -> Vec<u8> {
        let mut out = b"OggS\0".to_vec();
        out.push(flags);
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&0x5EED_u32.to_le_bytes());
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(packets.len() as u8);
        for p in packets {
            out.push(p.len() as u8);
        }
        for p in packets {
            out.extend_from_slice(p);
        }
        let crc = ogg_crc(&out);
        out[22..26].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// An Ogg Opus stream of `ms` (as the granule counts it) on `channels`,
    /// with a pre-skip of 312: OpusHead, OpusTags, then an audio page per
    /// second, the last one marked EOS when `ended`. lofty reads it.
    pub(crate) fn opus_bytes(ms: u64, channels: u8, ended: bool) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, channels]);
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&[0, 0, 0]);
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&5u32.to_le_bytes());
        tags.extend_from_slice(b"tests");
        tags.extend_from_slice(&0u32.to_le_bytes());
        let mut out = ogg_page(0x02, 0, 0, &[&head]);
        out.extend(ogg_page(0, 0, 1, &[&tags]));
        let samples = ms * 48 + 312;
        let pages = ms.div_ceil(1000).max(1);
        for n in 1..=pages {
            let granule = (samples * n / pages) as i64;
            let flags = if n == pages && ended { 0x04 } else { 0 };
            // A silent 20 ms frame (TOC 0xF8, code 0), as a tool would pad one.
            out.extend(ogg_page(flags, granule, 1 + n as u32, &[&[0xF8, 0xFF, 0xFE]]));
        }
        out
    }

    /// An MPEG-1 Layer III stream at 128 kbit/s, 44.1 kHz, joint stereo:
    /// `frames` frames of silence-shaped bytes, a LAME-style Info frame
    /// first when `info`, an ID3v2 tag before them all.
    pub(crate) fn mp3_bytes(frames: usize, info: bool) -> Vec<u8> {
        let mut out = b"ID3\x04\0\0\0\0\0\x0A".to_vec();
        out.extend_from_slice(&[0; 10]);
        let header = [0xFF, 0xFB, 0x90, 0x40];
        let len = 144 * 128_000 / 44_100;
        for n in 0..frames + info as usize {
            let mut f = header.to_vec();
            f.resize(len, 0);
            if info && n == 0 {
                f[4 + 32..4 + 36].copy_from_slice(b"Info");
            }
            out.extend(f);
        }
        out
    }

    #[test]
    fn an_opus_streams_length_is_its_last_granule_less_the_pre_skip() {
        let stream = opus_bytes(181_500, 2, true);
        let got = opus(&Mem(stream.clone())).unwrap();
        assert_eq!(got, Heard { ms: 181_500, channels: 2, ended: true });
        assert!(check(&got, Some(181_000), 2).is_ok());
        assert!(check(&got, Some(182_400), 2).is_ok());
        let off = check(&got, Some(183_000), 2).unwrap_err();
        assert!(off.contains("181500 ms"), "{off}");
        assert!(check(&got, None, 2).is_ok(), "no length to hold it to");
        assert!(check(&got, Some(181_500), 1).unwrap_err().contains("2 channels"));

        // lofty reads the same stream.
        let reading = crate::device::card::tags::read_bytes(&stream, "opus");
        assert_eq!(reading.rec.duration_ms as u64 / 100, 1_815);

        // Cut: mid-page there is no last page; at a page's end, no EOS.
        let cut = Mem(stream[..stream.len() - 2].to_vec());
        assert!(opus(&cut).unwrap_err().contains("partway"), "a page cut short");
        let unended = opus(&Mem(opus_bytes(90_000, 2, false))).unwrap();
        assert!(!unended.ended);
        assert!(check(&unended, Some(90_000), 2).unwrap_err().contains("before its end"));

        // A page whose CRC fails is no page; nor is another stream's.
        let mut torn = stream.clone();
        let n = torn.len();
        torn[n - 1] ^= 0xFF;
        assert!(opus(&Mem(torn)).is_err());
        let mp3 = Mem(mp3_bytes(10, false));
        assert!(opus(&mp3).unwrap_err().contains("Ogg"), "not Ogg at all");

        // A whole last page whose granule is as large as one can be: a
        // length no track has, never an overflow.
        let mut huge = opus_bytes(1_000, 2, false);
        huge.extend(ogg_page(0x04, i64::MAX, 9, &[&[0xF8, 0xFF, 0xFE]]));
        let got = opus(&Mem(huge)).unwrap();
        assert_eq!(got.ms, (i64::MAX as u64 - 312) / 48);
        assert!(check(&got, Some(181_500), 2).is_err());
    }

    /// Bytes that are fewer than the size first said: a file cut since.
    struct Shrunk(Vec<u8>, u64);

    impl At for Shrunk {
        fn size(&self) -> u64 {
            self.1
        }

        fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            Mem(self.0.clone()).read_at(offset, len)
        }
    }

    #[test]
    fn a_file_shorter_than_its_size_said_ends_the_walk() {
        let bytes = mp3_bytes(100, false);
        let size = bytes.len() as u64 + 1_000;
        assert_eq!(mp3(&Shrunk(bytes, size)).unwrap().ms, 2_612);
        let stream = opus_bytes(2_000, 2, true);
        let size = stream.len() as u64 + 1_000;
        assert_eq!(opus(&Shrunk(stream, size)).unwrap().ms, 2_000, "its tail as it is");
        assert!(opus(&Shrunk(Vec::new(), 1 << 20)).is_err());
    }

    #[test]
    fn an_mp3s_length_is_its_frames_counted() {
        // 100 frames of 1152 samples at 44.1 kHz: 2.612 s.
        let got = mp3(&Mem(mp3_bytes(100, false))).unwrap();
        assert_eq!(got, Heard { ms: 2_612, channels: 2, ended: true });
        // A leading Info frame is no audio.
        assert_eq!(mp3(&Mem(mp3_bytes(100, true))).unwrap().ms, 2_612);
        // A cut last frame isn't counted, and a tag at the end ends the walk.
        let mut cut = mp3_bytes(100, false);
        cut.truncate(cut.len() - 10);
        assert_eq!(mp3(&Mem(cut)).unwrap().ms, 2_586);
        let mut tagged = mp3_bytes(100, false);
        tagged.extend_from_slice(b"TAG");
        tagged.resize(tagged.len() + 125, b' ');
        assert_eq!(mp3(&Mem(tagged)).unwrap().ms, 2_612);
        assert!(mp3(&Mem(b"not audio at all".to_vec())).is_err());

        // A walk across blocks: longer than one block.
        let long = mp3_bytes(1_000, false);
        assert!(long.len() > BLOCK);
        assert_eq!(mp3(&Mem(long)).unwrap().ms, 26_122);

        // The parity corpus's constant-bitrate file, as lofty reads it.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/tags/cbr.mp3");
        let bytes = std::fs::read(&path).unwrap();
        let walked = mp3(&Mem(bytes.clone())).unwrap().ms;
        let lofty = crate::device::card::tags::read_bytes(&bytes, "mp3").rec.duration_ms as u64;
        assert!(walked.abs_diff(lofty) <= 30, "{walked} against lofty's {lofty}");
    }
}
