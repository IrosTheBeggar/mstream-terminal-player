//! Formats, from the server's `supportedAudioFiles` (the ping's): the route
//! checks `outputCodec` against its `true` keys, so the menu offers nothing
//! else. m4b and m3u never appear (an audiobook container is no choice for
//! a song, m3u is a playlist); AAC lands as an .m4a, so it is offered only
//! in M4A's place. (The phone's `lib/util/ytdl_formats.dart`.)

use super::Label;

/// The menu's order.
pub(crate) const FORMAT_ORDER: [&str; 6] = ["mp3", "m4a", "opus", "ogg", "flac", "wav"];

/// The menu: [`FORMAT_ORDER`], AAC after M4A's slot only while M4A is off,
/// MP3 alone when nothing usable is known.
pub(crate) fn format_options(enabled: Option<&[String]>) -> Vec<&'static str> {
    let Some(enabled) = enabled else { return vec!["mp3"] };
    let on = |f: &str| enabled.iter().any(|e| e == f);
    let mut out = Vec::new();
    for f in FORMAT_ORDER {
        if on(f) {
            out.push(f);
        }
        if f == "m4a" && !on("m4a") && on("aac") {
            out.push("aac");
        }
    }
    if out.is_empty() { vec!["mp3"] } else { out }
}

/// MP3 when it is offered, else the first option.
pub(crate) fn default_format(enabled: Option<&[String]>) -> &'static str {
    let options = format_options(enabled);
    if options.contains(&"mp3") { "mp3" } else { options[0] }
}

/// The remembered format (`config.toml`'s `ytdl_format`) while it is still
/// offered, else the default.
pub(crate) fn effective_format(remembered: Option<&str>, enabled: Option<&[String]>) -> &'static str {
    let options = format_options(enabled);
    remembered.and_then(|r| options.iter().find(|o| **o == r).copied()).unwrap_or_else(|| default_format(enabled))
}

/// A format's name as the menu and the rows show it — never translated,
/// like the transcode codec names.
pub(crate) fn format_name(codec: &str) -> String {
    let c = codec.to_ascii_lowercase();
    match c.as_str() {
        "mp3" => "MP3".into(),
        "m4a" => "M4A".into(),
        "opus" => "Opus".into(),
        "ogg" => "OGG Vorbis".into(),
        "flac" => "FLAC".into(),
        "wav" => "WAV".into(),
        "aac" => "AAC".into(),
        _ => codec.to_uppercase(),
    }
}

/// A menu item's help line, or `None` (OGG Vorbis and AAC have none).
pub(crate) fn format_help(codec: &str) -> Option<Label> {
    let key = match codec.to_ascii_lowercase().as_str() {
        "mp3" => "gui.ytdl.format.mp3",
        "m4a" => "gui.ytdl.format.m4a",
        "opus" => "gui.ytdl.format.opus",
        "flac" => "gui.ytdl.format.flac",
        "wav" => "gui.ytdl.format.wav",
        _ => return None,
    };
    Some(Label::new(key))
}

/// The extension the finished file gets: yt-dlp writes AAC into an .m4a
/// (mStream's OUTPUT_EXT_MAP) and is asked for m4a when the codec is m4b;
/// every other codec names its own.
pub(crate) fn output_extension(codec: &str) -> String {
    let c = codec.to_ascii_lowercase();
    if c == "aac" || c == "m4b" { "m4a".into() } else { c }
}

/// `One_More_Time.mp3`, or `""` without a stem.
pub(crate) fn file_name_for(stem: &str, codec: &str) -> String {
    if stem.is_empty() { String::new() } else { format!("{stem}.{}", output_extension(codec)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The server's default map (src/state/config.js), `true` keys only.
    fn server_default() -> Vec<String> {
        list(&["mp3", "flac", "wav", "ogg", "aac", "m4a", "m4b", "opus"])
    }

    #[test]
    fn the_menu_keeps_its_order_puts_aac_in_m4as_place_and_falls_back_to_mp3() {
        let options = |v: &[&str]| format_options(Some(&list(v)));
        assert_eq!(format_options(Some(&server_default())), FORMAT_ORDER);
        assert_eq!(options(&["wav", "opus", "mp3"]), ["mp3", "opus", "wav"]);
        assert_eq!(options(&["flac", "aac", "mp3"]), ["mp3", "aac", "flac"]);
        assert_eq!(options(&["aac", "m4a"]), ["m4a"]);
        assert_eq!(options(&["aac"]), ["aac"]);
        assert_eq!(options(&["m4b", "mp3", "m3u", "wma"]), ["mp3"]);
        for bad in [&[][..], &["m4b", "m3u"][..]] {
            assert_eq!(options(bad), ["mp3"], "{bad:?}");
        }
        assert_eq!(format_options(None), ["mp3"]);
    }

    #[test]
    fn the_pings_map_gives_its_true_keys_and_a_list_its_strings() {
        // As the ping reads `supportedAudioFiles` (crate::api::types).
        let enabled = |supported: serde_json::Value| {
            let ping: crate::api::types::Ping = serde_json::from_value(serde_json::json!({ "supportedAudioFiles": supported })).unwrap();
            ping.supported_formats.map(|mut f| {
                f.sort();
                f
            })
        };
        let map = serde_json::json!({ "mp3": true, "flac": true, "wav": false, "ogg": true, "aac": true, "m4a": false, "m4b": true, "opus": true, "m3u": false });
        let on = enabled(map).expect("a map");
        assert_eq!(on, ["aac", "flac", "m4b", "mp3", "ogg", "opus"]);
        assert_eq!(format_options(Some(&on)), ["mp3", "aac", "opus", "ogg", "flac"]);
        assert_eq!(enabled(serde_json::json!(["mp3", 7, "flac"])), Some(list(&["flac", "mp3"])));
        assert_eq!(enabled(serde_json::json!({ "mp3": 1 })), Some(Vec::new()), "only true counts");
        for bad in [serde_json::Value::Null, serde_json::json!("mp3"), serde_json::json!(7), serde_json::json!(true)] {
            assert_eq!(enabled(bad.clone()), None, "{bad}");
        }
    }

    #[test]
    fn the_default_is_mp3_when_offered_and_a_remembered_one_holds_while_offered() {
        let d = |v: &[&str]| default_format(Some(&list(v)));
        assert_eq!(default_format(Some(&server_default())), "mp3");
        assert_eq!(d(&["wav", "mp3"]), "mp3");
        assert_eq!(d(&["wav", "flac", "opus"]), "opus");
        assert_eq!(d(&["aac", "wav"]), "aac");
        assert_eq!(default_format(None), "mp3");
        assert_eq!(d(&["m3u"]), "mp3");
        let e = |r: Option<&str>, v: &[&str]| effective_format(r, Some(&list(v)));
        assert_eq!(effective_format(Some("opus"), Some(&server_default())), "opus");
        assert_eq!(e(Some("aac"), &["aac", "mp3"]), "aac");
        assert_eq!(e(Some("opus"), &["mp3", "flac"]), "mp3");
        assert_eq!(e(Some("opus"), &["wav", "flac"]), "flac");
        assert_eq!(effective_format(Some("aac"), Some(&server_default())), "mp3", "aac while m4a is on is not offered");
        assert_eq!(effective_format(Some("m4b"), Some(&server_default())), "mp3");
        assert_eq!(effective_format(None, Some(&server_default())), "mp3");
        assert_eq!(e(None, &["ogg", "wav"]), "ogg");
        assert_eq!(effective_format(None, None), "mp3");
    }

    #[test]
    fn names_are_not_translated_and_help_words_are_keys() {
        let names: Vec<String> = ["mp3", "m4a", "opus", "ogg", "flac", "wav", "aac", "WMA"].into_iter().map(format_name).collect();
        assert_eq!(names, ["MP3", "M4A", "Opus", "OGG Vorbis", "FLAC", "WAV", "AAC", "WMA"]);
        let keys: Vec<&str> = ["mp3", "m4a", "opus", "flac", "wav"].into_iter().map(|c| format_help(c).expect("help").key).collect();
        assert_eq!(keys, ["gui.ytdl.format.mp3", "gui.ytdl.format.m4a", "gui.ytdl.format.opus", "gui.ytdl.format.flac", "gui.ytdl.format.wav"]);
        assert_eq!(format_help("ogg"), None);
        assert_eq!(format_help("aac"), None);
    }

    #[test]
    fn the_extension_follows_the_format_as_the_server_names_the_file() {
        assert_eq!(output_extension("aac"), "m4a");
        assert_eq!(output_extension("m4b"), "m4a");
        for codec in ["mp3", "m4a", "opus", "ogg", "flac", "wav"] {
            assert_eq!(output_extension(codec), codec);
        }
        assert_eq!(file_name_for("One_More_Time", "mp3"), "One_More_Time.mp3");
        assert_eq!(file_name_for("Gorillaz_-_Feel_Good_Inc._Official_Video_HD", "aac"), "Gorillaz_-_Feel_Good_Inc._Official_Video_HD.m4a");
        assert_eq!(file_name_for("", "mp3"), "");
    }
}
