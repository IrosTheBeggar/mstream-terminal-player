//! Finding mStream servers on the local network.
//!
//! mStream advertises `_mstream._tcp` over mDNS by default, and its TXT
//! records carry everything needed to build a working base URL — scheme, port
//! and even the path prefix, so a server behind a reverse proxy resolves
//! correctly. It also advertises `iroh=1` when the Quick Connect tunnel is
//! available, which is how we can offer pairing only where it would work.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use mdns_sd::{ScopedIp, ServiceDaemon, ServiceEvent};
use reqwest::Url;

use crate::api::server_url;

const SERVICE_TYPE: &str = "_mstream._tcp.local.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    /// Friendly name from the advert, falling back to the instance label.
    /// Network-chosen, so it arrives gated: printable, one line, bounded.
    pub name: String,
    pub base_url: String,
    /// Advertised version, through the same gate as `name`.
    pub version: Option<String>,
    /// The server advertises an Iroh tunnel, so a pairing code can be used.
    pub quick_connect: bool,
}

/// Browse for servers, collecting whatever answers within `window`.
///
/// Blocking: mDNS has no "that's everyone" signal, so this simply listens for
/// a fixed period. Call it from a worker thread.
pub fn browse(window: Duration) -> Result<Vec<DiscoveredServer>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("could not start mDNS: {e}"))?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| format!("could not browse for servers: {e}"))?;

    let deadline = Instant::now() + window;
    let mut found: Vec<DiscoveredServer> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                // The same instance can resolve more than once (one event per
                // interface); keep the first.
                if !seen.insert(info.fullname.clone()) {
                    continue;
                }
                if let Some(server) = to_server(&info) {
                    found.push(server);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(found)
}

fn to_server(info: &mdns_sd::ResolvedService) -> Option<DiscoveredServer> {
    let address = pick_address(&info.addresses)?;
    let txt = |key: &str| info.get_property_val_str(key).map(str::to_string);

    // The advert's own port is authoritative when present; the SRV port is the
    // fallback.
    let port = txt("port").and_then(|p| p.parse::<u16>().ok()).unwrap_or(info.port);

    Some(DiscoveredServer {
        name: display_name(txt("name").as_deref(), &info.fullname),
        base_url: base_url(txt("scheme").as_deref(), &address, port, txt("path").as_deref())?,
        version: display_version(txt("v").as_deref()),
        quick_connect: txt("iroh").as_deref() == Some("1"),
    })
}

/// The one name a discovered row will ever draw, whoever chose it.
///
/// Both candidates — the TXT `name` and the instance label behind it — are
/// bytes a stranger on the network typed (finding #71). A responder calling
/// itself "Porch\r\n\x1b[41m ROGUE" drew two rows and a red background,
/// through `discover`'s println! and the Quick Connect list alike: a CR
/// inside a ratatui Span lands in a cell, crossterm emits it, and the frame
/// below it shifts — so the row a user reads is no longer the row their
/// cursor is on. The gate sits here, where the record is read, so every
/// screen downstream draws plain text.
fn display_name(advertised: Option<&str>, fullname: &str) -> String {
    // Wide enough for any honest name. The picker pads every row to the
    // widest name, so one long advert would push every URL off the screen.
    const CAP: usize = 40;
    let name = advertised
        .map(|n| printable(n, CAP))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| printable(&instance_label(fullname), CAP));
    if name.is_empty() {
        // Both candidates gated away to nothing; the row still needs a label.
        "(unnamed)".to_string()
    } else {
        name
    }
}

/// The advertised version, if anything printable is left of it.
fn display_version(advertised: Option<&str>) -> Option<String> {
    // "5.13.0" needs six; nothing past sixteen is still a version string.
    advertised.map(|v| printable(v, 16)).filter(|v| !v.is_empty())
}

/// `raw` minus every character that steers a terminal or a reader instead of
/// showing itself, trimmed, and cut to `cap` characters.
fn printable(raw: &str, cap: usize) -> String {
    let kept: String = raw.chars().filter(|c| !steers(*c)).collect();
    kept.trim().chars().take(cap).collect()
}

/// Characters that act on the terminal rather than appear in it: the C0/C1
/// controls (CR and LF split a row; ESC and the C1 CSI open ANSI sequences
/// that recolour, wipe or move things), plus the bidi embedding, override
/// and isolate marks, which draw nothing but visually reorder everything
/// after them — in a picker, the difference between the row a user reads
/// and the row they select.
fn steers(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Where an advert says its server lives, or `None` for one we won't offer.
///
/// This is the only place a stranger gets to choose part of a URL the player
/// will later post a password to, and everything here is about holding that
/// choice down to the part they're entitled to. A TXT record is publishable by
/// anyone on the network and can say anything: `scheme` used to be
/// interpolated straight into `format!("{scheme}://{address}:{port}")`, so a
/// responder advertising `scheme=https://evil.example/#` produced a base URL
/// whose real host was the attacker's, with the LAN address the user
/// recognised tucked after a '#' that `Url::join` then quietly dropped
/// (finding #29). Quick Connect showed an ordinary login form, and the
/// username and password went to evil.example over TLS with no warning to see.
///
/// So the scheme is an allowlist — one we don't speak is no use to us even
/// when it's honest — and the path is assembled by the parser rather than by
/// `format!`, because `extend` percent-encodes whatever would end the path and
/// start a host, a query or a fragment.
fn base_url(scheme: Option<&str>, address: &str, port: u16, path: Option<&str>) -> Option<String> {
    let scheme = match scheme.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("http") {
        s if s.eq_ignore_ascii_case("http") => "http",
        s if s.eq_ignore_ascii_case("https") => "https",
        _ => return None,
    };

    // The authority is entirely ours: an address picked out of the advert's A
    // records and a port that survived parse::<u16>.
    let mut base = Url::parse(&format!("{scheme}://{address}:{port}")).ok()?;
    if let Some(path) = path {
        base.path_segments_mut().ok()?.extend(path.split('/').filter(|s| !s.is_empty()));
    }

    // normalize is the canonicaliser here, not the guard — offered
    // "https://evil.example" it would hand it back without a murmur. It runs
    // so a discovered server is spelled the same way a typed one is, which is
    // what keeps the config file from holding two entries for one machine.
    server_url::normalize(base.as_str()).ok()
}

/// How much we want to route to a given IPv4 address. Lower is better.
///
/// A machine running mStream often advertises several addresses — a laptop
/// with WSL, Docker or Hyper-V publishes its virtual adapters right alongside
/// the Wi-Fi address that other devices can actually reach. Picking the wrong
/// one yields a URL that works on the server's own machine and nowhere else,
/// so the ordinary LAN ranges come first and the container-ish 172.16/12 range
/// last.
fn rank_v4(addr: &std::net::Ipv4Addr) -> Option<u8> {
    let [a, b, ..] = addr.octets();
    match (a, b) {
        // Link-local: an address that failed to get a lease. Never useful.
        (169, 254) => None,
        (127, _) => None,
        (192, 168) => Some(0),
        (10, _) => Some(1),
        // Routable, so it might be a real address on a hosted server.
        _ if !addr.is_private() => Some(2),
        // 172.16/12 — private, but where WSL, Docker and Hyper-V live.
        _ => Some(3),
    }
}

/// Prefer IPv4 — it makes for a readable URL and avoids the scope-id quoting
/// that link-local IPv6 needs. Ties break on the address itself so the same
/// advert always resolves to the same URL.
fn pick_address(addresses: &HashSet<ScopedIp>) -> Option<String> {
    let mut v4: Vec<(u8, std::net::Ipv4Addr)> = Vec::new();
    let mut v6: Vec<std::net::Ipv6Addr> = Vec::new();
    for address in addresses {
        match address {
            ScopedIp::V4(candidate) => {
                if let Some(rank) = rank_v4(candidate.addr()) {
                    v4.push((rank, *candidate.addr()));
                }
            }
            ScopedIp::V6(candidate) => v6.push(*candidate.addr()),
            // The enum is non-exhaustive; anything new is simply not a form
            // we know how to put in a URL.
            _ => {}
        }
    }

    v4.sort();
    if let Some((_, best)) = v4.first() {
        return Some(best.to_string());
    }
    v6.sort();
    v6.first().map(|addr| format!("[{addr}]"))
}

/// `"Living Room._mstream._tcp.local."` → `"Living Room"`.
fn instance_label(fullname: &str) -> String {
    fullname
        .split_once("._mstream.")
        .map(|(label, _)| label.to_string())
        .unwrap_or_else(|| fullname.to_string())
}

/// Diagnostic: list what's on the network right now.
pub fn print_found(seconds: f64) -> i32 {
    match browse(Duration::from_secs_f64(seconds)) {
        Ok(servers) if servers.is_empty() => {
            println!("(no mStream servers found on this network)");
            0
        }
        Ok(servers) => {
            for server in servers {
                println!(
                    "{:<24} {:<32} {}{}",
                    server.name,
                    server.base_url,
                    server.version.map(|v| format!("v{v}")).unwrap_or_default(),
                    if server.quick_connect { "  [quick connect]" } else { "" },
                );
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::IpAddr;

    fn addresses(ips: &[&str]) -> HashSet<ScopedIp> {
        ips.iter().map(|ip| ScopedIp::from(ip.parse::<IpAddr>().unwrap())).collect()
    }

    #[test]
    fn prefers_the_real_lan_over_a_virtual_adapter() {
        // Exactly what this machine advertises: a WSL adapter on 172.28 and
        // the Wi-Fi address other devices can actually reach. Picking the
        // former gives a URL that only works on the server's own machine.
        let picked = pick_address(&addresses(&["172.28.0.1", "192.168.1.71"]));
        assert_eq!(picked.as_deref(), Some("192.168.1.71"));
    }

    #[test]
    fn skips_link_local_and_loopback() {
        assert_eq!(
            pick_address(&addresses(&["169.254.6.29", "10.0.0.5"])).as_deref(),
            Some("10.0.0.5")
        );
        assert_eq!(pick_address(&addresses(&["169.254.6.29", "127.0.0.1"])), None);
    }

    #[test]
    fn is_deterministic_across_runs() {
        // The addresses arrive in a HashSet, so without an explicit order the
        // same advert could resolve to a different URL each time.
        let ips = ["10.1.2.3", "10.0.0.9", "192.168.4.4", "172.20.0.1"];
        let first = pick_address(&addresses(&ips));
        for _ in 0..20 {
            assert_eq!(pick_address(&addresses(&ips)), first);
        }
        assert_eq!(first.as_deref(), Some("192.168.4.4"));
    }

    #[test]
    fn falls_back_to_ipv6_in_brackets() {
        // Bracketed so it can go straight into a URL.
        assert_eq!(pick_address(&addresses(&["fe80::1"])).as_deref(), Some("[fe80::1]"));
    }

    #[test]
    fn an_honest_advert_still_becomes_the_url_it_always_did() {
        assert_eq!(base_url(None, "192.168.1.71", 3000, None).unwrap(), "http://192.168.1.71:3000");
        assert_eq!(
            base_url(Some("https"), "192.168.1.71", 3999, None).unwrap(),
            "https://192.168.1.71:3999"
        );
        // A reverse-proxy subpath is load-bearing, however it's spelled.
        assert_eq!(
            base_url(None, "10.0.0.5", 3000, Some("/mstream/")).unwrap(),
            "http://10.0.0.5:3000/mstream"
        );
        assert_eq!(
            base_url(None, "10.0.0.5", 3000, Some("music/mstream")).unwrap(),
            "http://10.0.0.5:3000/music/mstream"
        );
        assert_eq!(
            base_url(Some("HTTPS"), "[fe80::1]", 3000, None).unwrap(),
            "https://[fe80::1]:3000"
        );
        // Malformed rather than hostile: an empty scheme meant nothing at all,
        // and the default is what the record would have carried anyway.
        assert_eq!(base_url(Some("  "), "10.0.0.5", 3000, None).unwrap(), "http://10.0.0.5:3000");
    }

    #[test]
    fn a_scheme_we_do_not_speak_takes_the_whole_advert_with_it() {
        // The first two are finding #29 itself: interpolated into the old
        // format! they moved the host and hid the move behind a fragment.
        for hostile in [
            "https://evil.example/#",
            "http://evil.example/?",
            "https://user@evil.example",
            "javascript",
            "file",
            "ftp",
        ] {
            assert_eq!(
                base_url(Some(hostile), "192.168.1.71", 3000, None),
                None,
                "{hostile:?} was accepted as a scheme"
            );
        }
    }

    #[test]
    fn a_hostile_path_is_still_only_a_path() {
        // Whatever the record spells, the host we end up talking to is the one
        // picked off the network: a subpath is the whole of what an advert is
        // allowed to choose.
        for hostile in [
            "#@evil.example",
            "?x=1#@evil.example",
            "..",
            "../../../..",
            "x/../../..",
            "\\\\evil.example",
            "/@evil.example/",
            "a#b?c",
            "mstream#@evil.example",
        ] {
            let built = base_url(None, "192.168.1.71", 3000, Some(hostile))
                .unwrap_or_else(|| panic!("{hostile:?} was rejected outright"));
            let parsed = Url::parse(&built).unwrap();
            let moved = format!("{hostile:?} moved the host: {built}");
            assert_eq!(parsed.host_str(), Some("192.168.1.71"), "{moved}");
            assert_eq!(parsed.port(), Some(3000), "{hostile:?} moved the port: {built}");
            assert!(parsed.username().is_empty(), "{hostile:?} grew credentials: {built}");
            assert!(
                parsed.query().is_none() && parsed.fragment().is_none(),
                "{hostile:?} grew a tail the client would drop: {built}"
            );
        }
    }

    #[test]
    fn strips_the_service_suffix_from_an_instance_name() {
        assert_eq!(instance_label("Living Room._mstream._tcp.local."), "Living Room");
        // Anything unexpected is passed through rather than mangled.
        assert_eq!(instance_label("odd-name"), "odd-name");
    }

    #[test]
    fn the_rogue_porch_advert_draws_as_one_plain_row() {
        // Finding #71's live proof, byte for byte: CR LF to split the row,
        // ESC [41m to paint it red. What survives is text on one line.
        let name =
            display_name(Some("Porch\r\n\x1b[41m  ROGUE  \x1b[0m"), "x._mstream._tcp.local.");
        assert_eq!(name, "Porch[41m  ROGUE  [0m");
    }

    #[test]
    fn nothing_that_steers_a_terminal_survives_the_gate() {
        for hostile in [
            "two\rrows",
            "two\nrows",
            "wipe\x1b[2J",
            "c1 csi \u{9b}31m",
            "bell \x07",
            "tab\tstop",
            "reorder \u{202E}me",
            "isolate \u{2066}me\u{2069}",
        ] {
            let name = display_name(Some(hostile), "x._mstream._tcp.local.");
            assert!(
                name.chars().all(|c| !c.is_control()),
                "{hostile:?} kept a control character: {name:?}"
            );
            assert!(
                !name.contains(|c| ('\u{202A}'..='\u{202E}').contains(&c)
                    || ('\u{2066}'..='\u{2069}').contains(&c)),
                "{hostile:?} kept a bidi mark: {name:?}"
            );
        }
    }

    #[test]
    fn an_honest_advert_is_untouched() {
        assert_eq!(display_name(Some("Living Room"), "x._mstream._tcp.local."), "Living Room");
        assert_eq!(display_name(Some("Büro 🎵"), "x._mstream._tcp.local."), "Büro 🎵");
        assert_eq!(display_version(Some("5.13.0")), Some("5.13.0".to_string()));
        assert_eq!(display_version(None), None);
    }

    #[test]
    fn a_name_with_nothing_printable_falls_back_like_a_blank_one_always_has() {
        // Whitespace-only names have always fallen back to the instance
        // label; a name that is all ESC and CR is the same advert in gloves.
        assert_eq!(display_name(Some("   "), "Porch._mstream._tcp.local."), "Porch");
        assert_eq!(display_name(Some("\x1b\r\n"), "Porch._mstream._tcp.local."), "Porch");
        // Outer whitespace goes too: it only skews the column.
        assert_eq!(display_name(Some("  Porch  "), "x._mstream._tcp.local."), "Porch");
    }

    #[test]
    fn the_instance_label_is_a_strangers_bytes_too() {
        // No TXT name, and the mDNS instance itself is hostile: the fallback
        // goes through the same gate.
        assert_eq!(display_name(None, "Rogue\x1b[2J._mstream._tcp.local."), "Rogue[2J");
        // Every candidate gated away: the row still needs a label to click.
        assert_eq!(display_name(Some("\x1b"), "\u{7}._mstream._tcp.local."), "(unnamed)");
    }

    #[test]
    fn an_endless_name_cannot_push_the_urls_off_the_screen() {
        // The picker pads every row to the widest name; a 300-character name
        // would carry every URL past the right edge.
        let name = display_name(Some(&"x".repeat(300)), "y._mstream._tcp.local.");
        assert_eq!(name.chars().count(), 40);
        assert_eq!(display_version(Some(&"9".repeat(300))).unwrap().chars().count(), 16);
    }
}
