//! Turning what a person types into a server URL the client can use.
//!
//! People type `nas:3000`, `music.example.com`, or paste a browser address
//! bar complete with a `#!/artists` fragment. All of those name a server
//! perfectly clearly, and none of them parse as a URL:
//! `Url::parse("nas:3000")` reads `nas` as the *scheme*, which is how a typed
//! host:port used to produce an error about protocols the user never typed.
//!
//! Everything that accepts a server address — the connect screen and every
//! `--server` flag — runs it through [`normalize`] first, so exactly one
//! canonical spelling reaches the API client and, more importantly, the
//! config file. Storing what was typed instead would leave `nas:3000` and
//! `http://nas:3000` looking like two different servers, each with its own
//! half of the session.

use std::net::IpAddr;

use url::Url;

/// Canonicalise a typed server address, filling in whatever was left out.
///
/// Returns the URL without a trailing slash, matching [`Client::server`] and
/// what the config file stores. Errors are written for the person who typed
/// the thing, not for a log.
///
/// [`Client::server`]: super::Client::server
pub fn normalize(input: &str) -> Result<String, String> {
    let typed = input.trim();
    if typed.is_empty() {
        return Err("enter a server address".to_string());
    }

    // Whether a scheme was typed has to be settled before parsing, precisely
    // because the parser's answer is untrustworthy here.
    let (scheme, authority) = match typed.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest.to_string()),
        None => (scheme_for(typed).to_string(), bracket_bare_ipv6(typed)),
    };

    if !matches!(scheme.as_str(), "http" | "https") {
        return Err(format!(
            "'{scheme}' is not a protocol this player speaks — use http:// or https://"
        ));
    }
    if authority.is_empty() {
        return Err(format!("'{typed}' names a protocol but no server"));
    }

    let mut url = Url::parse(&format!("{scheme}://{authority}"))
        .map_err(|_| format!("'{typed}' is not a server address I can read"))?;

    match url.host_str() {
        Some(host) if !host.is_empty() => {}
        _ => return Err(format!("'{typed}' is missing a server name")),
    }

    // A pasted address bar carries a query and a fragment that mean something
    // to the web UI and nothing to us. Credentials in the URL likewise: mStream
    // authenticates with the username and password fields, never with these.
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);

    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// True when signing in would put a password on the wire in the clear
/// somewhere other than the local network. Plain http to a home server is
/// how most of mStream runs and is not worth a warning; plain http across
/// the internet is.
pub fn crosses_the_internet_unencrypted(url: &str) -> bool {
    match Url::parse(url) {
        Ok(url) => url.scheme() == "http" && !url.host_str().is_none_or(is_local_host),
        Err(_) => false,
    }
}

/// Which scheme to assume when none was typed. Guessing `http` for an address
/// that could be public would quietly downgrade every later request, so only
/// addresses that *cannot* be public get it.
fn scheme_for(typed: &str) -> &'static str {
    if is_local_host(host_of(typed)) { "http" } else { "https" }
}

/// The host out of a bare `user@host:port/path`, without parsing — which is
/// the whole point, since this runs before there is a parseable URL.
fn host_of(typed: &str) -> &str {
    let authority = typed.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    // More than one colon and no brackets: a bare IPv6 literal, so every
    // colon belongs to the address rather than separating a port.
    if authority.matches(':').count() > 1 {
        return authority;
    }
    authority.split(':').next().unwrap_or_default()
}

/// `::1` is a perfectly clear thing to type and not a legal URL authority.
fn bracket_bare_ipv6(typed: &str) -> String {
    let bare_v6 = !typed.starts_with('[')
        && !typed.contains('/')
        && typed.matches(':').count() > 1
        && typed.parse::<IpAddr>().is_ok();
    if bare_v6 { format!("[{typed}]") } else { typed.to_string() }
}

/// Addresses that can only exist on the local network. Used both to pick a
/// scheme and to decide whether plaintext deserves a warning — one definition
/// so the two can never disagree.
pub fn is_local_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);

    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    // mDNS names resolve on the LAN and nowhere else.
    if host.ends_with(".local") {
        return true;
    }

    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => {
            // is_unique_local and is_unicast_link_local are still unstable,
            // so test the prefixes directly: fc00::/7 and fe80::/10.
            ip.is_loopback()
                || ip.octets()[0] & 0xfe == 0xfc
                || ip.segments()[0] & 0xffc0 == 0xfe80
        }
        // A name with no dots ("nas", "attic") cannot be a public domain.
        Err(_) => !host.contains('.'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_a_missing_scheme_by_where_the_server_lives() {
        // Public names get https: guessing http would downgrade silently.
        assert_eq!(normalize("demo.mstream.io").unwrap(), "https://demo.mstream.io");
        assert_eq!(normalize("music.example.com:8443").unwrap(), "https://music.example.com:8443");

        // Anything that can only be on this network gets http, which is how
        // an mStream server on a LAN is actually served.
        assert_eq!(normalize("localhost:3000").unwrap(), "http://localhost:3000");
        assert_eq!(normalize("192.168.1.71:3999").unwrap(), "http://192.168.1.71:3999");
        assert_eq!(normalize("10.0.0.5").unwrap(), "http://10.0.0.5");
        assert_eq!(normalize("172.16.4.1:3000").unwrap(), "http://172.16.4.1:3000");
        assert_eq!(normalize("127.0.0.1:3000").unwrap(), "http://127.0.0.1:3000");
        assert_eq!(normalize("attic.local:3000").unwrap(), "http://attic.local:3000");
        assert_eq!(normalize("nas:3000").unwrap(), "http://nas:3000");
    }

    #[test]
    fn a_typed_scheme_is_always_obeyed() {
        // Even where the guess would have differed: an explicit choice is not
        // ours to second-guess.
        assert_eq!(normalize("http://demo.mstream.io").unwrap(), "http://demo.mstream.io");
        assert_eq!(normalize("https://192.168.1.71:3999").unwrap(), "https://192.168.1.71:3999");
        assert_eq!(normalize("HTTPS://Demo.MStream.IO").unwrap(), "https://demo.mstream.io");
    }

    #[test]
    fn keeps_the_subpath_and_drops_the_noise() {
        // Reverse-proxy subpaths are load-bearing; trailing slashes are not.
        assert_eq!(normalize("host/mstream").unwrap(), "http://host/mstream");
        assert_eq!(normalize("https://ex.com/mstream/").unwrap(), "https://ex.com/mstream");
        // Pasted from a browser, fragment and all.
        assert_eq!(normalize("https://ex.com/#!/artists").unwrap(), "https://ex.com");
        assert_eq!(normalize("https://ex.com/?tab=1").unwrap(), "https://ex.com");
        // Credentials in the URL are not how anything here authenticates.
        assert_eq!(normalize("http://bob:pw@nas:3000").unwrap(), "http://nas:3000");
        assert_eq!(normalize("  https://ex.com  ").unwrap(), "https://ex.com");
    }

    #[test]
    fn ipv6_survives_being_typed_bare() {
        assert_eq!(normalize("::1").unwrap(), "http://[::1]");
        assert_eq!(normalize("[::1]:3000").unwrap(), "http://[::1]:3000");
        assert_eq!(normalize("[fe80::1]:3000").unwrap(), "http://[fe80::1]:3000");
        assert_eq!(normalize("[2606:4700::1111]").unwrap(), "https://[2606:4700::1111]");
    }

    #[test]
    fn what_cannot_be_read_is_said_plainly() {
        // No library-speak: never "relative URL without a base".
        for bad in ["", "   ", "ftp://host", "http://", "https:// ", "http://:3000"] {
            let err = normalize(bad).unwrap_err();
            assert!(!err.contains("relative URL"), "{bad:?} leaked parser-speak: {err}");
            assert!(!err.is_empty());
        }
        assert!(normalize("ftp://host").unwrap_err().contains("http://"));
        assert_eq!(normalize("").unwrap_err(), "enter a server address");
    }

    #[test]
    fn normalizing_twice_changes_nothing() {
        for input in ["demo.mstream.io", "nas:3000", "https://ex.com/mstream/", "::1"] {
            let once = normalize(input).unwrap();
            assert_eq!(normalize(&once).unwrap(), once, "not idempotent: {input}");
        }
    }

    #[test]
    fn plaintext_is_only_alarming_off_the_local_network() {
        assert!(crosses_the_internet_unencrypted("http://music.example.com"));
        assert!(crosses_the_internet_unencrypted("http://93.184.216.34"));

        assert!(!crosses_the_internet_unencrypted("https://music.example.com"));
        assert!(!crosses_the_internet_unencrypted("http://localhost:3000"));
        assert!(!crosses_the_internet_unencrypted("http://127.0.0.1:3000"));
        // The common mStream setup: a box on the LAN, over http. Warning here
        // would be noise on every single sign-in.
        assert!(!crosses_the_internet_unencrypted("http://192.168.1.71:3999"));
        assert!(!crosses_the_internet_unencrypted("http://attic.local:3000"));
        assert!(!crosses_the_internet_unencrypted("http://[::1]:3000"));
        assert!(!crosses_the_internet_unencrypted("not a url"));
    }
}
