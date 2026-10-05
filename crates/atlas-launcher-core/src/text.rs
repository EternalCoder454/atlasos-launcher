//! Text for searching and showing: folding (case and diacritics), cleaning
//! untrusted text for display, and the match quality of a query against a
//! name (docs/DESIGN.md, "Search").

use std::fmt;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Longest text the launcher shows for one field; longer is cut with "…".
pub const MAX_DISPLAY_CHARS: usize = 512;

/// Lower case without diacritics, with compatibility forms unified (NFKD,
/// combining marks dropped, then lower-cased): "Écran" and "ecran" fold alike.
pub fn fold(s: &str) -> String {
    s.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `c` is a control or bidi-formatting character that untrusted text
/// must not carry into the UI (it could reorder or hide what is shown).
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}' // zero-width and LRM/RLM
            | '\u{202A}'..='\u{202E}' // embeddings and overrides
            | '\u{2060}'..='\u{2069}' // word joiner, isolates
            | '\u{FEFF}' | '\u{061C}' | '\u{FFF9}'..='\u{FFFB}'
            | '\u{00AD}' // soft hyphen
            | '\u{034F}' // combining grapheme joiner
            | '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' // Hangul fillers
            | '\u{17B4}'..='\u{17B5}' // Khmer inherent vowels
            | '\u{180B}'..='\u{180F}' // Mongolian variation selectors
            | '\u{FE00}'..='\u{FE0F}' // variation selectors
            | '\u{E0000}'..='\u{E007F}' // tags
            | '\u{E0100}'..='\u{E01EF}' // variation selectors supplement
            | '\u{2800}' // braille blank
            | '\u{FFFC}' // object replacement
            | '\u{1D173}'..='\u{1D17A}' // musical formatting
            | '\u{2028}' | '\u{2029}') // line and paragraph separators
}

/// Longest absolute icon path, in bytes.
pub const MAX_ICON_PATH_BYTES: usize = 4096;

/// A theme icon name: `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`.
pub fn valid_icon_name(icon: &str) -> bool {
    (1..=128).contains(&icon.len())
        && icon.as_bytes()[0].is_ascii_alphanumeric()
        && icon
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// An absolute icon path: starts with `/` (so no scheme), at most 4096 bytes,
/// not `//` first, no `..` segment, no NUL or other control character.
pub fn valid_icon_path(icon: &str) -> bool {
    icon.starts_with('/')
        && !icon.starts_with("//")
        && icon.len() <= MAX_ICON_PATH_BYTES
        && !icon.chars().any(is_unsafe_char)
        && !icon.split('/').any(|seg| seg == "..")
}

/// A theme icon name or an absolute path.
pub fn valid_icon(icon: &str) -> bool {
    valid_icon_name(icon) || valid_icon_path(icon)
}

fn is_sep(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | '(' | ')' | ',' | ':' | '+')
}

/// Untrusted text made safe to show: control and bidi characters become
/// spaces, runs of whitespace one space, ends trimmed, and at most
/// [`MAX_DISPLAY_CHARS`] characters kept.
pub fn clean_display(s: &str) -> String {
    clean_display_max(s, MAX_DISPLAY_CHARS)
}

/// [`clean_display`] with another length cap (in characters, at least 1).
pub fn clean_display_max(s: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    let mut out = String::with_capacity(s.len().min(max_chars * 4));
    let mut count = 0;
    let mut pending_space = false;
    for c in s.chars() {
        let c = if is_unsafe_char(c) { ' ' } else { c };
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            if count + 1 >= max_chars {
                break;
            }
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        if count == max_chars {
            break;
        }
        out.push(c);
        count += 1;
    }
    if count == max_chars && s.chars().filter(|c| !c.is_whitespace()).count() > count {
        out.pop();
        out.push('…');
    }
    out
}

/// How well a query matched a field, strongest first. The numbers are the
/// match quality `m` of DESIGN.md.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub enum MatchClass {
    Exact,
    Prefix,
    WordPrefix,
    Acronym,
    Substring,
    Fuzzy(f32),
}

impl MatchClass {
    /// The match quality, 0..=1.
    pub fn quality(self) -> f32 {
        match self {
            MatchClass::Exact => 1.0,
            MatchClass::Prefix => 0.9,
            MatchClass::WordPrefix => 0.8,
            MatchClass::Acronym => 0.7,
            MatchClass::Substring => 0.5,
            MatchClass::Fuzzy(q) => q.clamp(0.3, 0.5),
        }
    }
}

/// How far [`Prepared::match_depth`] looks, cheapest first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Depth {
    /// Exact, prefix and word prefix.
    Words,
    /// Plus acronym and substring.
    Strict,
    /// Plus typo-tolerant matching.
    Fuzzy,
}

/// Fuzzy matching runs only on names of at most this many characters.
const MAX_FUZZY_CHARS: usize = 128;

/// A field prepared once for matching many queries: its folded text, where
/// each word starts and its acronym.
#[derive(Clone, Default)]
pub struct Prepared {
    pub folded: String,
    /// Characters in `folded`.
    chars: usize,
    /// Byte offsets into `folded` where words start.
    word_starts: Vec<usize>,
    /// First letter of each word ("Visual Studio Code" → "vsc").
    acronym: String,
}

/// Redacted: lengths only, never the text.
impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("chars", &self.chars)
            .field("words", &self.word_starts.len())
            .finish_non_exhaustive()
    }
}

impl Prepared {
    pub fn new(text: &str) -> Self {
        // Word starts come from the original text (camelCase needs its case),
        // so fold char by char. Combining marks fold to nothing and are
        // invisible to the word logic ("e\u{301}cran" is one word).
        let mut folded = String::with_capacity(text.len());
        let mut word_starts = Vec::new();
        let mut acronym = String::new();
        let mut prev: Option<char> = None;
        for c in text.chars() {
            if is_combining_mark(c) {
                continue;
            }
            let starts_word = !is_sep(c)
                && match prev {
                    None => true,
                    Some(p) => {
                        is_sep(p)
                            || (p.is_lowercase() && c.is_uppercase())
                            || p.is_alphabetic() != c.is_alphabetic()
                    }
                };
            let start = folded.len();
            let mut one = [0; 4];
            let mut first = None;
            for f in c
                .encode_utf8(&mut one)
                .chars()
                .nfkd()
                .filter(|c| !is_combining_mark(*c))
                .flat_map(char::to_lowercase)
            {
                first.get_or_insert(f);
                folded.push(f);
            }
            if starts_word && let Some(f) = first {
                word_starts.push(start);
                acronym.push(f);
            }
            prev = Some(c);
        }
        Prepared {
            chars: folded.chars().count(),
            folded,
            word_starts,
            acronym,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    fn words(&self) -> impl Iterator<Item = &str> {
        self.word_starts.iter().map(move |&s| {
            let rest = &self.folded[s..];
            let end = rest.find(is_sep).unwrap_or(rest.len());
            &rest[..end]
        })
    }

    /// The best match of a folded query, or None, with typo tolerance. Use
    /// it on names only; other fields use [`Prepared::matches_strict`].
    pub fn matches(&self, query: &Query) -> Option<MatchClass> {
        self.match_depth(query, Depth::Fuzzy)
    }

    /// Like [`Prepared::matches`] but never fuzzy: exact, prefix, word prefix,
    /// acronym or substring.
    pub fn matches_strict(&self, query: &Query) -> Option<MatchClass> {
        self.match_depth(query, Depth::Strict)
    }

    /// Exact, prefix or word prefix only (the cheapest check).
    fn matches_words(&self, query: &Query) -> Option<MatchClass> {
        self.match_depth(query, Depth::Words)
    }

    /// Multi-word queries match as a word prefix when every query word
    /// starts some word of the field.
    fn match_depth(&self, query: &Query, depth: Depth) -> Option<MatchClass> {
        let q = query.folded.as_str();
        if q.is_empty() || self.folded.is_empty() {
            return None;
        }
        if self.folded == q {
            return Some(MatchClass::Exact);
        }
        if self.folded.starts_with(q) {
            return Some(MatchClass::Prefix);
        }
        if query.words.len() > 1 {
            if query
                .words
                .iter()
                .all(|w| self.words().any(|fw| fw.starts_with(w.as_str())))
            {
                return Some(MatchClass::WordPrefix);
            }
        } else if self.words().skip(1).any(|w| w.starts_with(q)) {
            return Some(MatchClass::WordPrefix);
        }
        if depth == Depth::Words {
            return None;
        }
        let compact: std::borrow::Cow<'_, str> = if query.words.len() > 1 {
            q.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .into()
        } else {
            q.into()
        };
        if compact.chars().count() >= 2 && self.acronym.starts_with(&*compact) {
            return Some(MatchClass::Acronym);
        }
        if self.folded.contains(q) {
            return Some(MatchClass::Substring);
        }
        if depth == Depth::Fuzzy && self.chars <= MAX_FUZZY_CHARS {
            return self.fuzzy(&compact);
        }
        None
    }

    /// Typo-tolerant matching for queries of 3 or more characters: either one
    /// typo (substitution, transposition, extra or missing letter) against a
    /// word prefix, or the query's letters in order starting at a word start,
    /// scored by how tightly they sit.
    fn fuzzy(&self, q: &str) -> Option<MatchClass> {
        if q.len() < 3 {
            return None;
        }
        let qc: Vec<char> = q.chars().take(MAX_QUERY_CHARS).collect();
        if qc.len() < 3 {
            return None;
        }
        if qc.len() >= 4 {
            let mut wc: Vec<char> = Vec::with_capacity(qc.len() + 1);
            for w in self.words() {
                wc.clear();
                wc.extend(w.chars().take(qc.len() + 1));
                for len in [qc.len() - 1, qc.len(), qc.len() + 1] {
                    if len <= wc.len() && damerau_at_most_one(&qc, &wc[..len]) {
                        return Some(MatchClass::Fuzzy(0.48));
                    }
                }
            }
        }
        // Subsequence from a word start: score by span.
        let mut best: Option<usize> = None;
        for &s in &self.word_starts {
            let tail = &self.folded[s..];
            if !tail.starts_with(qc[0]) {
                continue;
            }
            let mut it = tail.chars();
            let mut span = 0;
            let mut ok = true;
            'q: for &c in &qc {
                loop {
                    match it.next() {
                        Some(x) => {
                            span += 1;
                            if x == c {
                                continue 'q;
                            }
                        }
                        None => {
                            ok = false;
                            break 'q;
                        }
                    }
                }
            }
            if ok {
                best = Some(best.map_or(span, |b| b.min(span)));
            }
        }
        best.map(|span| {
            let tight = qc.len() as f32 / span as f32; // 1.0 when contiguous
            MatchClass::Fuzzy(0.3 + 0.2 * tight)
        })
    }
}

/// Whether two strings are at most one edit apart (Damerau: substitution,
/// adjacent transposition, insertion or deletion).
fn damerau_at_most_one(a: &[char], b: &[char]) -> bool {
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    let mut i = 0;
    while i < la.min(lb) && a[i] == b[i] {
        i += 1;
    }
    if i == la.min(lb) {
        return true; // equal, or one extra at the end
    }
    if la == lb {
        // substitution or transposition
        a[i + 1..] == b[i + 1..]
            || (i + 1 < la && a[i] == b[i + 1] && a[i + 1] == b[i] && a[i + 2..] == b[i + 2..])
    } else if la > lb {
        a[i + 1..] == b[i..]
    } else {
        a[i..] == b[i + 1..]
    }
}

/// Longest query the launcher searches for, in characters; the rest is cut.
pub const MAX_QUERY_CHARS: usize = 256;

/// One query, folded once for every provider.
#[derive(Clone, Default)]
pub struct Query {
    /// What the user typed, cleaned (control and bidi characters removed,
    /// at most [`MAX_QUERY_CHARS`]), not trimmed.
    pub raw: String,
    /// `raw` trimmed and folded, inner whitespace collapsed.
    pub folded: String,
    /// The folded words.
    pub words: Vec<String>,
    /// Increases with every keystroke; results carry it so stale ones are
    /// dropped.
    pub serial: u64,
}

/// Redacted: what the user typed never reaches a log.
impl fmt::Debug for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("serial", &self.serial)
            .field("chars", &self.raw.chars().count())
            .finish_non_exhaustive()
    }
}

impl Query {
    pub fn new(raw: &str, serial: u64) -> Self {
        let raw: String = raw
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|c| !is_unsafe_char(*c) || *c == ' ')
            .take(MAX_QUERY_CHARS)
            .collect();
        let words: Vec<String> = fold(&raw).split_whitespace().map(str::to_owned).collect();
        Query {
            folded: words.join(" "),
            raw,
            words,
            serial,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }
}

/// The match quality of the best of an item's fields, weighted as DESIGN.md
/// says: the name in full; generic name and keywords up to 0.65 (exact 0.7);
/// the description up to 0.3 (and only for queries of 3 or more characters).
pub fn score_fields(
    q: &Query,
    name: &Prepared,
    secondary: &[Prepared],
    description: Option<&Prepared>,
) -> Option<f32> {
    let mut best = name.matches(q).map(MatchClass::quality);
    for s in secondary {
        let m = match s.matches_strict(q) {
            Some(MatchClass::Exact) => 0.7,
            Some(MatchClass::Prefix | MatchClass::WordPrefix) => 0.65,
            Some(MatchClass::Acronym) => 0.5,
            Some(MatchClass::Substring) => 0.4,
            Some(MatchClass::Fuzzy(_)) | None => continue,
        };
        best = Some(best.map_or(m, |b: f32| b.max(m)));
    }
    if let Some(d) = description
        && q.folded.chars().count() >= 3
        && matches!(
            d.matches_words(q),
            Some(MatchClass::Exact | MatchClass::Prefix | MatchClass::WordPrefix)
        )
    {
        best = Some(best.map_or(0.3, |b: f32| b.max(0.3)));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(field: &str, q: &str) -> Option<MatchClass> {
        Prepared::new(field).matches(&Query::new(q, 0))
    }

    #[test]
    fn unsafe_chars_extended() {
        for c in [
            '\u{2800}',
            '\u{FFFC}',
            '\u{E0100}',
            '\u{E01EF}',
            '\u{1D173}',
            '\u{1D17A}',
        ] {
            assert!(is_unsafe_char(c), "{c:?}");
        }
        assert!(!is_unsafe_char('a'));
    }

    #[test]
    fn icon_name_and_path_edges() {
        assert!(valid_icon_name("firefox"));
        assert!(valid_icon_name("a-b_c.d"));
        assert!(!valid_icon_name("-x"));
        assert!(!valid_icon_name(".hidden"));
        assert!(!valid_icon_name("_x"));
        assert!(valid_icon_path("/usr/share/a.png"));
        assert!(!valid_icon_path("//host/a.png"));
        assert!(!valid_icon_path("///a.png"));
    }

    #[test]
    fn query_tabs_become_spaces() {
        let q = Query::new("a\tb", 0);
        assert_eq!(q.raw, "a b");
        assert_eq!(q.folded, "a b");
        assert_eq!(Query::new("a\nb", 0).raw, "ab");
    }

    #[test]
    fn prepared_debug_is_redacted() {
        let d = format!("{:?}", Prepared::new("Secret Name"));
        assert!(!d.contains("ecret") && d.contains("chars"));
    }

    #[test]
    fn folds_case_and_diacritics() {
        assert_eq!(fold("Écran Ünïcödé"), "ecran unicode");
        assert_eq!(fold("ﬁle"), "file");
    }

    #[test]
    fn match_classes_in_order() {
        assert_eq!(m("Firefox", "firefox"), Some(MatchClass::Exact));
        assert_eq!(m("Firefox", "fire"), Some(MatchClass::Prefix));
        assert_eq!(
            m("Visual Studio Code", "studio"),
            Some(MatchClass::WordPrefix)
        );
        assert_eq!(
            m("Visual Studio Code", "vis code"),
            Some(MatchClass::WordPrefix)
        );
        assert_eq!(m("Visual Studio Code", "vsc"), Some(MatchClass::Acronym));
        assert_eq!(m("KeePassXC", "pass"), Some(MatchClass::WordPrefix)); // camelCase
        assert_eq!(m("Thunderbird", "derbi"), Some(MatchClass::Substring));
        assert!(matches!(m("Firefox", "frefox"), Some(MatchClass::Fuzzy(_)))); // missing letter
        assert!(matches!(
            m("Firefox", "fierfox"),
            Some(MatchClass::Fuzzy(_))
        )); // transposition
        assert!(matches!(
            m("System Monitor", "symon"),
            Some(MatchClass::Fuzzy(_))
        )); // subsequence
        assert_eq!(m("Firefox", "xyz"), None);
        assert_eq!(m("Firefox", ""), None);
    }

    #[test]
    fn diacritics_match() {
        assert_eq!(m("Écran", "ecran"), Some(MatchClass::Exact));
        assert_eq!(m("Paramètres", "parametres"), Some(MatchClass::Exact));
    }

    #[test]
    fn qualities_are_ordered() {
        let order = [
            MatchClass::Exact,
            MatchClass::Prefix,
            MatchClass::WordPrefix,
            MatchClass::Acronym,
            MatchClass::Substring,
            MatchClass::Fuzzy(0.5),
        ];
        for pair in order.windows(2) {
            assert!(pair[0].quality() >= pair[1].quality(), "{pair:?}");
        }
    }

    #[test]
    fn clean_display_strips_unsafe() {
        assert_eq!(clean_display("a\u{202E}b\u{0007}c\n\n d "), "a b c d");
        assert_eq!(clean_display_max("abcdef", 4), "abc…");
        assert_eq!(clean_display_max("abcd", 4), "abcd");
        assert_eq!(clean_display(""), "");
    }

    #[test]
    fn query_is_cleaned_and_capped() {
        let q = Query::new("  Fire\u{202E}Fox  ", 3);
        assert_eq!(q.folded, "firefox");
        assert_eq!(q.serial, 3);
        let long = "a".repeat(1000);
        assert_eq!(Query::new(&long, 0).raw.chars().count(), MAX_QUERY_CHARS);
    }

    #[test]
    fn combining_marks_make_no_word_start() {
        let p = Prepared::new("e\u{301}cran");
        assert_eq!(p.folded, "ecran");
        assert_eq!(p.word_starts, vec![0]);
        assert_eq!(p.acronym, "e");
        let p = Prepared::new("Cafe\u{301} Noir");
        assert_eq!(p.word_starts.len(), 2);
        assert_eq!(m("e\u{301}cran", "cran"), Some(MatchClass::Substring));
    }

    #[test]
    fn strict_has_no_fuzzy_and_fuzzy_is_name_only() {
        let q = Query::new("frefox", 0);
        let p = Prepared::new("Firefox");
        assert!(matches!(p.matches(&q), Some(MatchClass::Fuzzy(_))));
        assert_eq!(p.matches_strict(&q), None);
        // A secondary field or description never matches by typo.
        assert_eq!(
            score_fields(
                &q,
                &Prepared::new("Other"),
                std::slice::from_ref(&p),
                Some(&p)
            ),
            None
        );
        // A name longer than 128 characters is not fuzzy-matched.
        let long = Prepared::new(&format!("{} firefox", "a".repeat(130)));
        assert_eq!(long.matches(&q), None);
        let short = Prepared::new(&format!("{} firefox", "a".repeat(100)));
        assert!(short.matches(&q).is_some());
    }

    #[test]
    fn unsafe_chars_are_stripped() {
        for c in [
            '\u{AD}',
            '\u{34F}',
            '\u{115F}',
            '\u{1160}',
            '\u{17B4}',
            '\u{17B5}',
            '\u{180B}',
            '\u{180F}',
            '\u{3164}',
            '\u{FFA0}',
            '\u{FE00}',
            '\u{FE0F}',
            '\u{E0000}',
            '\u{E007F}',
            '\u{2028}',
            '\u{2029}',
            '\u{202E}',
            '\u{7}',
        ] {
            assert!(is_unsafe_char(c), "{c:?}");
            assert_eq!(clean_display(&format!("a{c}b")), "a b", "{c:?}");
        }
        assert!(!is_unsafe_char('a') && !is_unsafe_char('é'));
    }

    #[test]
    fn icon_checks() {
        assert!(valid_icon_name("folder-open.v2_x"));
        assert!(!valid_icon_name("") && !valid_icon_name("a b") && !valid_icon_name("../x"));
        assert!(!valid_icon_name(&"a".repeat(129)));
        assert!(valid_icon_path("/usr/share/icons/x.svg"));
        assert!(!valid_icon_path("usr/x") && !valid_icon_path("/a/../b"));
        assert!(!valid_icon_path("/a\0b") && !valid_icon_path("file:///x"));
        assert!(!valid_icon_path(&format!(
            "/{}",
            "a".repeat(MAX_ICON_PATH_BYTES)
        )));
        assert!(valid_icon("x") && valid_icon("/x") && !valid_icon("data:image/png"));
    }

    #[test]
    fn query_debug_is_redacted() {
        let q = Query::new("secret words", 7);
        let d = format!("{q:?}");
        assert!(!d.contains("secret"), "{d}");
        assert!(d.contains('7') && d.contains("12"), "{d}");
    }

    #[test]
    fn fields_weighted() {
        let q = Query::new("browser", 0);
        let name = Prepared::new("Firefox");
        let kw = [Prepared::new("Web Browser")];
        assert_eq!(score_fields(&q, &name, &kw, None), Some(0.65));
        let desc = Prepared::new("browser for the web");
        assert_eq!(score_fields(&q, &name, &[], Some(&desc)), Some(0.3));
    }
}
