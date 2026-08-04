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
    pub name: String,
    pub base_url: String,
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

    let name = txt("name")
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| instance_label(&info.fullname));

    Some(DiscoveredServer {
        name,
        base_url: base_url(txt("scheme").as_deref(), &address, port, txt("path").as_deref())?,
        version: txt("v"),
        quick_connect: txt("iroh").as_deref() == Some("1"),
    })
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
}
