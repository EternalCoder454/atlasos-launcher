//! The web-search row: the engines the user can choose and the https URL a
//! query opens (docs/DESIGN.md, "Search"). Nothing here touches the network;
//! the browser opens the URL.

use crate::result::{Action, Kind, ResultItem};
use crate::text::{Query, clean_display_max};

/// Most bytes of query text put into a URL.
pub const MAX_QUERY_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Engine {
    #[default]
    DuckDuckGo,
    Google,
    Bing,
    Startpage,
    Ecosia,
    Brave,
    Kagi,
    Qwant,
}

impl Engine {
    /// The engine named in `launcher.conf` (case-insensitive); unknown names
    /// give DuckDuckGo.
    pub fn from_config(name: &str) -> Engine {
        match name.trim().to_ascii_lowercase().as_str() {
            "google" => Engine::Google,
            "bing" => Engine::Bing,
            "startpage" => Engine::Startpage,
            "ecosia" => Engine::Ecosia,
            "brave" => Engine::Brave,
            "kagi" => Engine::Kagi,
            "qwant" => Engine::Qwant,
            _ => Engine::DuckDuckGo,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Engine::DuckDuckGo => "DuckDuckGo",
            Engine::Google => "Google",
            Engine::Bing => "Bing",
            Engine::Startpage => "Startpage",
            Engine::Ecosia => "Ecosia",
            Engine::Brave => "Brave Search",
            Engine::Kagi => "Kagi",
            Engine::Qwant => "Qwant",
        }
    }

    /// The URL prefix, ending where the encoded query goes.
    fn prefix(self) -> &'static str {
        match self {
            Engine::DuckDuckGo => "https://duckduckgo.com/?q=",
            Engine::Google => "https://www.google.com/search?q=",
            Engine::Bing => "https://www.bing.com/search?q=",
            Engine::Startpage => "https://www.startpage.com/do/search?q=",
            Engine::Ecosia => "https://www.ecosia.org/search?q=",
            Engine::Brave => "https://search.brave.com/search?q=",
            Engine::Kagi => "https://kagi.com/search?q=",
            Engine::Qwant => "https://www.qwant.com/?q=",
        }
    }

    /// An https URL that searches for `query`: at most [`MAX_QUERY_BYTES`]
    /// bytes of it (cut at a character boundary), percent-encoded.
    pub fn search_url(self, query: &str) -> String {
        let mut url = String::from(self.prefix());
        url.push_str(&percent_encode(truncate_bytes(query, MAX_QUERY_BYTES)));
        url
    }
}

fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// RFC 3986 percent-encoding: unreserved bytes (`A-Z a-z 0-9 - . _ ~`) stay,
/// every other UTF-8 byte becomes `%XX`.
pub fn percent_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
    out
}

/// The "Search the web" row for a query; None when the query is empty. Its
/// score is 0 (prior of Web): it sorts last unless nothing else matches.
pub fn web_row(engine: Engine, q: &Query) -> Option<ResultItem> {
    let text = q.raw.trim();
    if text.is_empty() {
        return None;
    }
    Some(ResultItem {
        id: "web".to_owned(),
        kind: Kind::Web,
        title: format!("Search the web for “{}”", clean_display_max(text, 128)),
        subtitle: engine.display_name().to_owned(),
        icon: "internet-web-browser".to_owned(),
        score: 0.0,
        action: Action::OpenWeb {
            url: engine.search_url(text),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_rfc3986() {
        assert_eq!(percent_encode("a b&c/é"), "a%20b%26c%2F%C3%A9");
        assert_eq!(percent_encode("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(percent_encode("+%#?=\n\0"), "%2B%25%23%3F%3D%0A%00");
    }

    #[test]
    fn engines_from_config() {
        assert_eq!(Engine::from_config("Google"), Engine::Google);
        assert_eq!(Engine::from_config(" kagi "), Engine::Kagi);
        assert_eq!(Engine::from_config("nope"), Engine::DuckDuckGo);
        assert_eq!(Engine::from_config(""), Engine::DuckDuckGo);
    }

    #[test]
    fn every_url_is_https() {
        for e in [
            Engine::DuckDuckGo,
            Engine::Google,
            Engine::Bing,
            Engine::Startpage,
            Engine::Ecosia,
            Engine::Brave,
            Engine::Kagi,
            Engine::Qwant,
        ] {
            let u = e.search_url("a b&c/é");
            assert!(u.starts_with("https://"), "{u}");
            assert!(u.ends_with("q=a%20b%26c%2F%C3%A9"), "{u}");
            assert!(!e.display_name().is_empty());
        }
    }

    #[test]
    fn caps_query_length() {
        let long = "é".repeat(2000);
        let u = Engine::Google.search_url(&long);
        let encoded = &u[Engine::Google.prefix().len()..];
        assert_eq!(encoded.len(), 512 * 6);
        let odd = "a".repeat(1023) + "é";
        let u = Engine::Google.search_url(&odd);
        assert_eq!(u.len() - Engine::Google.prefix().len(), 1023);
    }

    #[test]
    fn row() {
        assert!(web_row(Engine::Google, &Query::new("   ", 1)).is_none());
        assert!(web_row(Engine::Google, &Query::new("", 1)).is_none());
        let r = web_row(Engine::Brave, &Query::new("  rust lang ", 1)).unwrap();
        assert_eq!(r.title, "Search the web for “rust lang”");
        assert_eq!(r.subtitle, "Brave Search");
        assert_eq!(r.id, "web");
        assert_eq!(r.kind, Kind::Web);
        assert_eq!(r.score, 0.0);
        assert_eq!(
            r.action,
            Action::OpenWeb {
                url: "https://search.brave.com/search?q=rust%20lang".into()
            }
        );
    }

    #[test]
    fn fuzz_no_panic() {
        let mut s = 0x1234_5678_u64;
        for _ in 0..500 {
            let mut t = String::new();
            for _ in 0..(s % 40) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                t.push(char::from_u32(((s >> 33) % 0x3000) as u32).unwrap_or('?'));
            }
            let _ = web_row(Engine::from_config(&t), &Query::new(&t, 0));
        }
    }
}
