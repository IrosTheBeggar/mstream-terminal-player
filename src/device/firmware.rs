//! Where the firmware comes from, and what it is. Three sources, in the
//! order the flags prefer them: a path (`--firmware`: the merged
//! `firmware.factory.bin` of a build, or a release's `*-full.bin`), a
//! release by tag (`--release`: downloaded from the firmware repo's GitHub
//! Releases and checked against that release's `SHA256SUMS`), and the
//! release this build of the player pins — tag and checksum written here,
//! bumped by hand when a player release adopts a firmware release. A
//! GitHub release's files can be replaced after the fact, so the pin's
//! checksum, not the tag, is what the player trusts.
//!
//! Every image carries its own name and version: ESP-IDF's app description
//! (`esp_app_desc_t`) sits at a fixed offset of the app, and the firmware
//! fills it with its git version and the project name. The same 256 bytes,
//! read back from a board, say what is on it (see engine::Link::app_desc).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rust_i18n::t;
use sha2::{Digest, Sha256};

use super::DeviceError;

/// The firmware's repository, and the stem its release files share.
const REPO: &str = "IrosTheBeggar/mstream-mp3-player";
const ASSET_STEM: &str = "mstream-player-core2";
/// The release this player pins: what `device flash` writes when no
/// `--firmware` or `--release` says otherwise. Bumped by hand with player
/// releases, once a firmware release has been tried on a board.
pub(crate) const PINNED_TAG: Option<&str> = Some("v0.8.0");
/// The sha256 of that release's `*-full.bin`, from its SHA256SUMS (and the
/// same as GitHub's own digest of the asset, checked when it was pinned).
/// Release assets are mutable on GitHub; this is the trust anchor, not the
/// file. v0.8.0's `-full.bin` runs the flash in QIO, as every release has
/// since v0.6.0; the release's `-dio-full.bin` (the same firmware in DIO,
/// for a Core2 that keeps restarting on QIO) goes on with `--firmware`.
pub(crate) const PINNED_FULL_SHA256: Option<&str> =
    Some("ba77f290f7159b2e40a7d0907a05b1400ed6529abe297437595259ed8d0671a3");

/// Where the merged image expects the app: ota_0 in the firmware's
/// partition table, and where the bootloader at 0x1000 sits inside it.
pub(crate) const APP_OFFSET: usize = 0x10000;
const BOOTLOADER_OFFSET: usize = 0x1000;
/// The first byte of every ESP image header.
const IMAGE_MAGIC: u8 = 0xE9;
/// A Core2 has 16 MB of flash; an image past this is not one of ours, and
/// a download past it is a lie or a mistake — stop reading either way.
const MAX_IMAGE: usize = 8 * 1024 * 1024;
const MAX_SUMS: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A piece of the image and where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    pub offset: u32,
    pub data: Vec<u8>,
}

/// An image ready to write: what it calls itself, where it came from (for
/// the page's header and the log), and its pieces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Firmware {
    pub version: String,
    pub origin: String,
    pub kind: Origin,
    pub segments: Vec<Segment>,
}

/// Where an image came from, as the one word the page's header puts
/// beside the version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A release, downloaded now.
    Release,
    /// A release, from the copy kept earlier.
    Cached,
    /// A file, or a build directory.
    File,
}

impl Firmware {
    pub fn bytes(&self) -> usize {
        self.segments.iter().map(|s| s.data.len()).sum()
    }
}

/// ESP-IDF's `esp_app_desc_t`: the fields the player reads. The firmware's
/// `tools/version.py` and `Version.cpp` fill `version` with the build's
/// git description (`v0.5.0`, `v0.5.0-3-gabc1234-dirty`) and
/// `project_name` with the firmware's name; an image of some other
/// firmware says something else, and a blank flash says nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppDesc {
    pub version: String,
    pub project: String,
    pub idf: String,
    /// The first eight hex digits of the ELF's SHA-256 — what About shows.
    pub elf8: String,
}

impl AppDesc {
    pub const MAGIC: u32 = 0xABCD_5432;
    /// Where the description sits in an app image: after the 24-byte image
    /// header and the first segment's 8-byte header.
    pub const OFFSET_IN_APP: usize = 0x20;
    pub const LEN: usize = 256;
    pub const OURS: &str = "mstream-mp3-player";

    /// The description in `bytes` (the 256 bytes at its offset), if the
    /// magic word is there.
    pub fn parse(bytes: &[u8]) -> Option<AppDesc> {
        if bytes.len() < Self::LEN {
            return None;
        }
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != Self::MAGIC {
            return None;
        }
        let text = |from: usize, to: usize| -> String {
            let raw = &bytes[from..to];
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).trim().to_string()
        };
        Some(AppDesc {
            version: text(16, 48),
            project: text(48, 80),
            idf: text(112, 144),
            elf8: bytes[144..148].iter().map(|b| format!("{b:02x}")).collect(),
        })
    }

    /// The description in an app image (the bytes from its start).
    pub fn in_app(app: &[u8]) -> Option<AppDesc> {
        app.get(Self::OFFSET_IN_APP..Self::OFFSET_IN_APP + Self::LEN).and_then(Self::parse)
    }

    pub fn is_ours(&self) -> bool {
        self.project == Self::OURS
    }
}

/// What a file is, by its shape. A merged image has an image header at
/// 0x1000 (the bootloader) and one at 0x10000 (the app), and nothing but
/// erased bytes before the first; an app alone starts with its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// The merged `firmware.factory.bin` / `*-full.bin`: written at 0x0.
    Merged,
    /// The app alone (`firmware.bin` / `*-app.bin`): written at ota_0.
    App,
}

/// The layout and the description of an image, or nothing for a file that
/// is neither (its description missing counts as neither: every image the
/// firmware builds carries one, and the page needs the version).
pub(crate) fn classify(bytes: &[u8]) -> Option<(Layout, AppDesc)> {
    let header_at = |at: usize| bytes.get(at).copied() == Some(IMAGE_MAGIC);
    if header_at(BOOTLOADER_OFFSET) && header_at(APP_OFFSET) && bytes[0] == 0xFF {
        let desc = AppDesc::in_app(&bytes[APP_OFFSET..])?;
        return Some((Layout::Merged, desc));
    }
    if header_at(0) {
        let desc = AppDesc::in_app(bytes)?;
        return Some((Layout::App, desc));
    }
    None
}

/// Where the image comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Pinned,
    Release(String),
    Local(PathBuf),
}

impl Source {
    pub fn from_args(firmware: Option<PathBuf>, release: Option<String>) -> Source {
        match (firmware, release) {
            (Some(path), _) => Source::Local(path),
            (None, Some(tag)) => Source::Release(tag),
            (None, None) => Source::Pinned,
        }
    }

    /// The image, read or downloaded. `progress` hears a download's bytes
    /// so far and, when the server said, its total.
    pub fn resolve(&self, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Firmware, DeviceError> {
        match self {
            Source::Local(path) => local(path),
            Source::Release(tag) => release(tag, None, progress),
            Source::Pinned => match (PINNED_TAG, PINNED_FULL_SHA256) {
                (Some(tag), Some(sha)) => release(tag, Some(sha), progress),
                _ => Err(DeviceError::Firmware(t!("dev.fw_no_pin").to_string())),
            },
        }
    }
}

/// A file, or a build directory's merged image.
fn local(path: &Path) -> Result<Firmware, DeviceError> {
    let file = if path.is_dir() { path.join("firmware.factory.bin") } else { path.to_path_buf() };
    let shown = file.display().to_string();
    let bytes = std::fs::read(&file)
        .map_err(|e| DeviceError::Firmware(t!("dev.fw_read", path = shown, err = e).to_string()))?;
    if bytes.len() > MAX_IMAGE {
        return Err(DeviceError::Firmware(t!("dev.fw_too_big", what = shown).to_string()));
    }
    let (layout, desc) = vetted(&bytes, &shown)?;
    Ok(Firmware {
        version: desc.version,
        origin: t!("dev.origin_file", path = shown).to_string(),
        kind: Origin::File,
        segments: vec![segment(layout, bytes)],
    })
}

/// The image's one segment: the merged image at 0x0, an app at ota_0.
fn segment(layout: Layout, data: Vec<u8>) -> Segment {
    let offset = match layout {
        Layout::Merged => 0,
        Layout::App => APP_OFFSET as u32,
    };
    Segment { offset, data }
}

/// `bytes` as one of our images, or the reason it is not.
fn vetted(bytes: &[u8], what: &str) -> Result<(Layout, AppDesc), DeviceError> {
    let Some((layout, desc)) = classify(bytes) else {
        return Err(DeviceError::Firmware(t!("dev.fw_not_image", path = what).to_string()));
    };
    if !desc.is_ours() {
        return Err(DeviceError::Firmware(
            t!("dev.fw_not_ours", what = what, name = desc.project).to_string(),
        ));
    }
    Ok((layout, desc))
}

/// `mstream-player-core2-v0.7.0-full.bin`: the release's merged image. The
/// firmware's packager replaces what a file name cannot carry (a `+` in a
/// dev build's version) with `_`; a release tag has none, but the rule is
/// the same here so a `--release` of such a build still resolves.
pub(crate) fn full_asset_name(tag: &str) -> String {
    let file_version: String = tag
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    format!("{ASSET_STEM}-{file_version}-full.bin")
}

/// Where a release's files are. `MSTREAM_FIRMWARE_BASE` points the player
/// at a mirror (or a test's local server) instead of GitHub: the files are
/// expected right under it, by their release names.
fn asset_url(tag: &str, name: &str) -> String {
    match std::env::var("MSTREAM_FIRMWARE_BASE").ok().filter(|b| !b.is_empty()) {
        Some(base) => format!("{}/{name}", base.trim_end_matches('/')),
        None => format!("https://github.com/{REPO}/releases/download/{tag}/{name}"),
    }
}

/// A release's merged image: the copy downloaded earlier when its checksum
/// still holds, else a fresh download, checked, then kept. `expected` is
/// the pin's checksum; without one the release's own SHA256SUMS is fetched
/// first, which trusts the release as it stands today.
fn release(
    tag: &str,
    expected: Option<&str>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Firmware, DeviceError> {
    let name = full_asset_name(tag);
    let expected = match expected {
        Some(sha) => sha.to_ascii_lowercase(),
        None => {
            let sums = fetch(&asset_url(tag, "SHA256SUMS"), MAX_SUMS, &mut |_, _| {}, "SHA256SUMS")?;
            let sums = String::from_utf8_lossy(&sums);
            parse_sums(&sums)
                .into_iter()
                .find(|(_, file)| *file == name)
                .map(|(sha, _)| sha)
                .ok_or_else(|| DeviceError::Firmware(t!("dev.fw_sums_missing", name = name).to_string()))?
        }
    };

    let cache = crate::config::firmware_dir().map(|dir| dir.join(tag).join(&name));
    if let Some(bytes) = cache.as_ref().and_then(|path| std::fs::read(path).ok())
        && sha256_hex(&bytes) == expected
    {
        let (layout, desc) = vetted(&bytes, &name)?;
        return Ok(Firmware {
            version: desc.version,
            origin: t!("dev.origin_cached", tag = tag).to_string(),
            kind: Origin::Cached,
            segments: vec![segment(layout, bytes)],
        });
    }

    let bytes = fetch(&asset_url(tag, &name), MAX_IMAGE, progress, &name)?;
    if sha256_hex(&bytes) != expected {
        return Err(DeviceError::Firmware(t!("dev.fw_checksum", name = name).to_string()));
    }
    let (layout, desc) = vetted(&bytes, &name)?;
    if let Some(path) = cache {
        // Kept for next time; a cache that cannot be written only costs
        // the next download, so nothing here fails on it.
        let _ = path.parent().map(std::fs::create_dir_all);
        let part = path.with_extension("part");
        if std::fs::write(&part, &bytes).is_ok() {
            let _ = std::fs::rename(&part, &path);
        }
    }
    Ok(Firmware {
        version: desc.version,
        origin: t!("dev.origin_release", tag = tag).to_string(),
        kind: Origin::Release,
        segments: vec![segment(layout, bytes)],
    })
}

/// `(sha256, file name)` per line of a `sha256sum` listing — two spaces,
/// or a space and an asterisk for a binary-mode entry.
pub(crate) fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (sha, rest) = line.split_once(' ')?;
            let name = rest.trim_start().trim_start_matches('*').trim();
            (sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (sha.to_ascii_lowercase(), name.to_string()))
        })
        .collect()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// One GET, the body capped at `cap` bytes, `progress` told as it lands.
/// The shared runtime drives reqwest; this is called from the worker
/// thread, never from inside the runtime.
fn fetch(
    url: &str,
    cap: usize,
    progress: &mut dyn FnMut(u64, Option<u64>),
    what: &str,
) -> Result<Vec<u8>, DeviceError> {
    let failed = |err: String| DeviceError::Firmware(t!("dev.fw_download", what = what, err = err).to_string());
    let result = crate::runtime::block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = client
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let total = response.content_length();
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > cap {
                return Err(t!("dev.fw_too_big", what = what).to_string());
            }
            progress(bytes.len() as u64, total);
        }
        Ok::<Vec<u8>, String>(bytes)
    });
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(err)) | Err(err) => Err(failed(err)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The first 0x120 bytes of a real core2 `firmware.bin` (the image
    /// header, the first segment's header, the app description) — a build
    /// of 2026-09-30, version `v0.5.0-dev+ac93229-dirty`.
    pub(crate) const APP_HEAD: &[u8] = include_bytes!("../../test/golden/core2/app-head.bin");

    /// A description as the firmware writes one, for `version` and
    /// `project` — the rest as a real build fills it.
    pub(crate) fn desc_bytes(version: &str, project: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; AppDesc::LEN];
        bytes[..4].copy_from_slice(&AppDesc::MAGIC.to_le_bytes());
        bytes[16..16 + version.len()].copy_from_slice(version.as_bytes());
        bytes[48..48 + project.len()].copy_from_slice(project.as_bytes());
        bytes[112..118].copy_from_slice(b"v5.5.5");
        bytes[144..176].copy_from_slice(&[0xA1; 32]);
        bytes
    }

    /// A merged image as pioarduino makes one: erased bytes, a header at
    /// 0x1000, a header at 0x10000 followed by `desc`.
    pub(crate) fn merged_bytes(desc: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF; APP_OFFSET + AppDesc::OFFSET_IN_APP + AppDesc::LEN + 64];
        bytes[BOOTLOADER_OFFSET] = IMAGE_MAGIC;
        bytes[APP_OFFSET] = IMAGE_MAGIC;
        let at = APP_OFFSET + AppDesc::OFFSET_IN_APP;
        bytes[at..at + desc.len()].copy_from_slice(desc);
        bytes
    }

    #[test]
    fn the_real_builds_description_reads_back() {
        let desc = AppDesc::in_app(APP_HEAD).expect("a description at 0x20");
        assert_eq!(desc.version, "v0.5.0-dev+ac93229-dirty");
        assert_eq!(desc.project, "mstream-mp3-player");
        assert_eq!(desc.idf, "v5.5.5");
        assert_eq!(desc.elf8, "a12e6cfd");
        assert!(desc.is_ours());
    }

    #[test]
    fn a_description_without_the_magic_word_is_nothing() {
        let mut bytes = desc_bytes("v0.5.0", "mstream-mp3-player");
        assert!(AppDesc::parse(&bytes).is_some());
        bytes[0] ^= 1;
        assert!(AppDesc::parse(&bytes).is_none());
        assert!(AppDesc::parse(&bytes[..100]).is_none(), "too short to be one");
        let other = AppDesc::parse(&desc_bytes("3.3.12", "arduino-lib-builder")).unwrap();
        assert!(!other.is_ours(), "the Arduino core's own description: other firmware");
    }

    #[test]
    fn a_file_is_a_merged_image_an_app_or_neither() {
        let desc = desc_bytes("v0.5.0", "mstream-mp3-player");
        let merged = merged_bytes(&desc);
        assert!(matches!(classify(&merged), Some((Layout::Merged, d)) if d.version == "v0.5.0"));
        let mut app = vec![0u8; AppDesc::OFFSET_IN_APP];
        app[0] = IMAGE_MAGIC;
        app.extend_from_slice(&desc);
        assert!(matches!(classify(&app), Some((Layout::App, d)) if d.version == "v0.5.0"));
        assert!(classify(APP_HEAD).is_some_and(|(layout, _)| layout == Layout::App));
        assert!(classify(b"not an image at all").is_none());
        let mut headers_only = vec![0xFF; APP_OFFSET + 0x200];
        headers_only[BOOTLOADER_OFFSET] = IMAGE_MAGIC;
        headers_only[APP_OFFSET] = IMAGE_MAGIC;
        assert!(classify(&headers_only).is_none(), "a merged image without a description is not ours");
    }

    #[test]
    fn a_local_path_is_a_file_or_a_build_directory() {
        let _en = crate::setup::tests::in_locale("en");
        let dir = std::env::temp_dir().join(format!("mstream-player-fw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let merged = merged_bytes(&desc_bytes("v0.5.0-3-gabc1234", "mstream-mp3-player"));
        std::fs::write(dir.join("firmware.factory.bin"), &merged).unwrap();
        let from_dir = Source::Local(dir.clone()).resolve(&mut |_, _| {}).unwrap();
        assert_eq!(from_dir.version, "v0.5.0-3-gabc1234");
        assert_eq!(from_dir.segments.len(), 1);
        assert_eq!(from_dir.segments[0].offset, 0, "the merged image goes at 0x0");
        assert_eq!(from_dir.bytes(), merged.len());
        assert!(from_dir.origin.contains("firmware.factory.bin"));

        let other = dir.join("other.bin");
        std::fs::write(&other, merged_bytes(&desc_bytes("1.0", "someone-elses"))).unwrap();
        let err = Source::Local(other).resolve(&mut |_, _| {}).unwrap_err();
        assert!(err.text().contains("someone-elses"), "{}", err.text());

        let junk = dir.join("junk.bin");
        std::fs::write(&junk, b"hello").unwrap();
        let err = Source::Local(junk).resolve(&mut |_, _| {}).unwrap_err();
        assert!(err.text().contains("not a Core2 firmware image"), "{}", err.text());

        let missing = Source::Local(dir.join("nope.bin")).resolve(&mut |_, _| {}).unwrap_err();
        assert!(missing.text().contains("nope.bin"), "{}", missing.text());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pin_names_a_release_and_its_checksum_or_says_it_is_empty() {
        let _en = crate::setup::tests::in_locale("en");
        match (PINNED_TAG, PINNED_FULL_SHA256) {
            (Some(tag), Some(sha)) => {
                assert!(tag.starts_with('v') && tag[1..].split('.').count() == 3, "a release tag: {tag}");
                assert_eq!(full_asset_name(tag), format!("mstream-player-core2-{tag}-full.bin"));
                assert!(sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{sha}");
            }
            (None, None) => {
                let err = Source::Pinned.resolve(&mut |_, _| {}).unwrap_err();
                assert!(err.text().contains("--firmware"), "{}", err.text());
            }
            other => panic!("a tag without its checksum, or the other way round: {other:?}"),
        }
        assert_eq!(
            Source::from_args(Some(PathBuf::from("x.bin")), Some("v9".into())),
            Source::Local(PathBuf::from("x.bin")),
            "a path outranks a tag"
        );
        assert_eq!(Source::from_args(None, Some("v9".into())), Source::Release("v9".into()));
        assert_eq!(Source::from_args(None, None), Source::Pinned);
    }

    #[test]
    fn the_release_names_and_the_sums_listing() {
        assert_eq!(full_asset_name("v0.5.0"), "mstream-player-core2-v0.5.0-full.bin");
        assert_eq!(
            full_asset_name("v0.5.0-dev+abc1234"),
            "mstream-player-core2-v0.5.0-dev_abc1234-full.bin",
            "the packager's `+` → `_`"
        );
        let text = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789ABCDEF  mstream-player-core2-v0.5.0-full.bin
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *SHA256SUMS-binary-mode
not a line
0123  too-short.bin
";
        let sums = parse_sums(text);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[0].1, "mstream-player-core2-v0.5.0-full.bin");
        assert!(sums[0].0.ends_with("abcdef"), "lowercased");
        assert_eq!(sums[1].1, "SHA256SUMS-binary-mode");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn a_mirror_env_points_the_download_elsewhere() {
        // The variable is process-global; set it, check, clear it.
        unsafe { std::env::set_var("MSTREAM_FIRMWARE_BASE", "http://127.0.0.1:1/fw/") };
        assert_eq!(asset_url("v0.5.0", "SHA256SUMS"), "http://127.0.0.1:1/fw/SHA256SUMS");
        unsafe { std::env::remove_var("MSTREAM_FIRMWARE_BASE") };
        assert_eq!(
            asset_url("v0.5.0", "x.bin"),
            "https://github.com/IrosTheBeggar/mstream-mp3-player/releases/download/v0.5.0/x.bin"
        );
    }
}
