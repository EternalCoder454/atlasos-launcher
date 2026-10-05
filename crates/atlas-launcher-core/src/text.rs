//! Text for searching and showing: folding (case and diacritics), cleaning
//! untrusted text for display, and the match quality of a query against a
//! name (docs/DESIGN.md, "Search").

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
fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}' // zero-width and LRM/RLM
            | '\u{202A}'..='\u{202E}' // embeddings and overrides
            | '\u{2060}'..='\u{2069}' // word joiner, isolates
            | '\u{FEFF}' | '\u{061C}' | '\u{FFF9}'..='\u{FFFB}')
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

/// A field prepared once for matching many queries: its folded text, where
/// each word starts and its acronym.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub folded: String,
    /// Byte offsets into `folded` where words start.
    word_starts: Vec<usize>,
    /// First letter of each word ("Visual Studio Code" → "vsc").
    acronym: String,
}

impl Prepared {
    pub fn new(text: &str) -> Self {
        // Word starts come from the original text (camelCase needs its case),
        // so fold word by word.
        let mut folded = String::with_capacity(text.len());
        let mut word_starts = Vec::new();
        let mut acronym = String::new();
        let mut prev: Option<char> = None;
        for c in text.chars() {
            let sep = c.is_whitespace()
                || matches!(c, '-' | '_' | '.' | '/' | '(' | ')' | ',' | ':' | '+');
            let starts_word = !sep
                && match prev {
                    None => true,
                    Some(p) => {
                        let p_sep = p.is_whitespace()
                            || matches!(p, '-' | '_' | '.' | '/' | '(' | ')' | ',' | ':' | '+');
                        p_sep
                            || (p.is_lowercase() && c.is_uppercase())
                            || (p.is_alphabetic() != c.is_alphabetic() && !p_sep)
                    }
                };
            let start = folded.len();
            let f = fold(c.encode_utf8(&mut [0; 4]));
            if starts_word && !f.is_empty() {
                word_starts.push(start);
                acronym.push_str(
                    f.chars()
                        .next()
                        .map(|c| c.to_string())
                        .as_deref()
                        .unwrap_or(""),
                );
            }
            folded.push_str(&f);
            prev = Some(c);
        }
        Prepared {
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
            let end = rest
                .find(|c: char| {
                    c.is_whitespace()
                        || matches!(c, '-' | '_' | '.' | '/' | '(' | ')' | ',' | ':' | '+')
                })
                .unwrap_or(rest.len());
            &rest[..end]
        })
    }

    /// The best match of a folded query, or None. Multi-word queries match
    /// as a word prefix when every query word starts some word of the field.
    pub fn matches(&self, query: &Query) -> Option<MatchClass> {
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
        let compact: String = q.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.chars().count() >= 2 && self.acronym.starts_with(&compact) {
            return Some(MatchClass::Acronym);
        }
        if self.folded.contains(q) {
            return Some(MatchClass::Substring);
        }
        self.fuzzy(&compact)
    }

    /// Typo-tolerant matching for queries of 3 or more characters: either one
    /// typo (substitution, transposition, extra or missing letter) against a
    /// word prefix, or the query's letters in order starting at a word start,
    /// scored by how tightly they sit.
    fn fuzzy(&self, q: &str) -> Option<MatchClass> {
        let qc: Vec<char> = q.chars().collect();
        if qc.len() < 3 {
            return None;
        }
        if qc.len() >= 4 {
            for w in self.words() {
                let wc: Vec<char> = w.chars().collect();
                for len in [qc.len().saturating_sub(1), qc.len(), qc.len() + 1] {
                    if len == 0 || len > wc.len() {
                        continue;
                    }
                    if damerau_at_most_one(&qc, &wc[..len]) {
                        return Some(MatchClass::Fuzzy(0.48));
                    }
                }
            }
        }
        // Subsequence from a word start: score by span.
        let fc: Vec<char> = self.folded.chars().collect();
        let starts: Vec<usize> = self
            .word_starts
            .iter()
            .map(|&b| self.folded[..b].chars().count())
            .collect();
        let mut best: Option<usize> = None;
        for &s in &starts {
            if fc.get(s) != Some(&qc[0]) {
                continue;
            }
            let mut i = s;
            let mut ok = true;
            for &c in &qc {
                match fc[i..].iter().position(|&x| x == c) {
                    Some(p) => i += p + 1,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                let span = i - s;
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
#[derive(Clone, Debug, Default)]
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

impl Query {
    pub fn new(raw: &str, serial: u64) -> Self {
        let raw: String = raw
            .chars()
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
        let m = match s.matches(q) {
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
            d.matches(q),
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
    fn fields_weighted() {
        let q = Query::new("browser", 0);
        let name = Prepared::new("Firefox");
        let kw = [Prepared::new("Web Browser")];
        assert_eq!(score_fields(&q, &name, &kw, None), Some(0.65));
        let desc = Prepared::new("browser for the web");
        assert_eq!(score_fields(&q, &name, &[], Some(&desc)), Some(0.3));
    }
}
