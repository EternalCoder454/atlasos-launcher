//! Guards of the launcher's QML (and of the dock button's) that draws text or
//! opens things from outside. They read the QML source, so a new `Text`, link,
//! image, loader or script that skips a rule fails here, in CI, and not in a
//! review three releases later (docs/SECURITY.md, "Displayed text").
//!
//! What reaches the QML from outside: app names, comments and desktop-action
//! names from desktop files, file names and paths, KRunner matches, Settings
//! page titles, the user's own names for apps and the query. All of it is
//! cleaned in the Rust core (control and bidi characters out, capped) and must
//! be *drawn* as plain text. The rules:
//!
//! - every `Text`, `Label`, `Heading`, `TextEdit` and `TextArea` says
//!   `textFormat: Text.PlainText`; `TelamonLabel`, `TelamonTextField` and
//!   `TelamonTextArea` (plain by default) are never switched away from it;
//!   nothing asks for rich, styled, Markdown or auto-detected text;
//! - the style's own tooltip (`ToolTip.text`) is not used for data: it draws
//!   its text in a format of its own. `TelamonToolTip` is plain;
//! - no QML runs text as code or fetches (`eval`, `Function`, `Qt.include`,
//!   `XMLHttpRequest`, `WebSocket`, web views, `Qt.createQmlObject`,
//!   `setSource`), no `Loader { source }`, no `Qt.openUrlExternally`: the
//!   launcher opens things through the C++ side, which checks them;
//! - an `Image` or `AnimatedImage` is made only where `IMAGES` lists it, with
//!   the source it lists (a vetted icon path, a theme name);
//! - the checker itself is tested: snippets that must pass and must fail.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- the rules

/// Text types: the last segment of the type name (`QQC2.Label` is `Label`).
const TEXT_TYPES: &[&str] = &[
    "Text",
    "Label",
    "Heading",
    "SelectableLabel",
    "TextEdit",
    "TextArea",
];

/// Types whose instances are Telamon.Ui text: they must never be given a
/// `textFormat` that is not plain.
const PLAIN_BY_DEFAULT: &[&str] = &["TelamonLabel", "TelamonTextArea", "TelamonTextField"];

/// Files that make an image-like item (an `Image`, or a Telamon.Ui control that
/// loads one: `TelamonAvatar`, `TelamonChoiceCard`, `TelamonScreenshotCarousel`),
/// and the `source` it must have, with why that source is safe.
const IMAGES: &[(&str, &[&str], &str)] = &[(
    "apps/telamon-launcher/qml/LauncherAccountButton.qml",
    &[
        "account.actions.userIcon.length > 0 ? \"file://\" + account.actions.userIcon.split(\"/\").map(encodeURIComponent).join(\"/\") : \"\"",
    ],
    "the user's picture is a path AccountsService stores, vetted in C++ (trustedIconFile: a regular \
     file of the user or root, of sane size and dimensions) before it is a property of Actions",
)];

/// Files that make a `Kirigami.Icon` (a theme name or a file path) and the
/// `source` it must have.
const ICONS: &[(&str, &str)] = &[
    (
        "apps/telamon-launcher/qml/LauncherItemIcon.qml",
        "name.startsWith(\"/\") ? fileUrl(name) : (name.length > 0 ? name : \"application-x-executable\")",
    ),
    (
        // The dock button's own icon: `Plasmoid.icon` is the constant "atlasos".
        "plasmoid/net.eterneon.telamon.launcher.button/contents/ui/main.qml",
        "Plasmoid.icon",
    ),
];

// ------------------------------------------------------------ the QML reader

struct Source {
    /// Relative to the repository, with `/`.
    path: String,
    text: String,
    /// `text` with the inside of comments and string literals blanked
    /// (same length, same line breaks), so braces and names can be found
    /// without a string or a comment fooling the search.
    masked: String,
}

/// `text` with comments and the inside of string, template and regular
/// expression literals replaced by spaces (newlines kept).
fn mask(text: &str) -> String {
    mask_with(text, true)
}

/// `mask`, optionally keeping the strings (only comments and regular
/// expressions blanked).
fn mask_with(text: &str, strings: bool) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut i = 0;
    // The last byte that is not white space or part of a comment: a `/` after
    // an operator or a bracket starts a regular expression, after a name it
    // divides.
    let mut prev = b'\n';
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let start = i;
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                blank(&mut out, start, i);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                blank(&mut out, start, i);
                continue;
            }
            q @ (b'"' | b'\'' | b'`') => {
                let start = i + 1;
                i += 1;
                while i < b.len() && b[i] != q && !(b[i] == b'\n' && q != b'`') {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let end = i.min(b.len());
                if strings {
                    blank(&mut out, start, end);
                }
                i = end + 1;
                prev = q;
                continue;
            }
            b'/' if regex_may_start(&out, i, prev) => {
                // A regular expression literal.
                let start = i + 1;
                i += 1;
                let mut class = false;
                while i < b.len() && b[i] != b'\n' && (class || b[i] != b'/') {
                    match b[i] {
                        b'\\' => i += 1,
                        b'[' => class = true,
                        b']' => class = false,
                        _ => {}
                    }
                    i += 1;
                }
                let end = i.min(b.len());
                blank(&mut out, start, end);
                i = end + 1;
                prev = b'/';
                continue;
            }
            c if !c.is_ascii_whitespace() => prev = c,
            _ => {}
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

/// Whether a `/` at `at` starts a regular expression: after an operator, an
/// opening bracket, a separator, `=>`, or a keyword such as `return`; after a
/// name, a number or a closing bracket it divides. `out` is the source so far
/// with comments and strings blanked.
fn regex_may_start(out: &[u8], at: usize, prev: u8) -> bool {
    if b"(,=:[!&|?{};\n+-*<>%~^".contains(&prev) {
        return true;
    }
    if !(prev.is_ascii_alphanumeric() || prev == b'_') {
        return false;
    }
    let mut end = at;
    while end > 0 && out[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && (out[start - 1].is_ascii_alphanumeric() || out[start - 1] == b'_') {
        start -= 1;
    }
    matches!(
        &out[start..end],
        b"return"
            | b"typeof"
            | b"case"
            | b"in"
            | b"of"
            | b"delete"
            | b"void"
            | b"throw"
            | b"new"
            | b"else"
            | b"do"
    )
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

fn is_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.'
}

/// One `Type { ... }`: the name as written, the line, and the byte range of
/// the braces' inside in the source.
struct Element {
    name: String,
    line: usize,
    open: usize,
    close: usize,
}

impl Element {
    /// The last segment of the name: `QQC2.Label` is `Label`.
    fn base(&self) -> &str {
        self.name.rsplit('.').next().unwrap()
    }
}

/// Every `Name {` of the masked source whose last segment starts with a
/// capital: an object of that type (or an `enum`, which no rule asks about).
fn elements(src: &Source) -> Vec<Element> {
    let b = src.masked.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !(b[i].is_ascii_alphabetic() || b[i] == b'_') || (i > 0 && is_name(b[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && is_name(b[i]) {
            i += 1;
        }
        let name = &src.masked[start..i];
        let mut j = i;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let last = name.rsplit('.').next().unwrap();
        if b.get(j) == Some(&b'{') && last.as_bytes()[0].is_ascii_uppercase() {
            let mut depth = 0i32;
            let mut k = j;
            let mut close = b.len();
            while k < b.len() {
                match b[k] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            close = k;
                            break;
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            out.push(Element {
                name: name.to_string(),
                line: line_of(&src.text, start),
                open: j + 1,
                close,
            });
        }
    }
    out
}

/// The values of `prop:` (or `prop =`) that belong to the element itself, not
/// to an element or a function inside it.
fn own_values<'a>(src: &'a Source, e: &Element, prop: &str) -> Vec<&'a str> {
    own_spans(src, e, prop)
        .into_iter()
        .map(|(from, to)| src.masked[from..to].trim())
        .collect()
}

/// The byte ranges of the values of `prop:` that belong to the element itself.
fn own_spans(src: &Source, e: &Element, prop: &str) -> Vec<(usize, usize)> {
    let b = src.masked.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = e.open;
    while i < e.close {
        match b[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            c if depth == 0
                && (c.is_ascii_alphabetic() || c == b'_')
                && (i == 0 || !is_name(b[i - 1])) =>
            {
                let start = i;
                while i < e.close && is_name(b[i]) {
                    i += 1;
                }
                if &src.masked[start..i] == prop {
                    let mut j = i;
                    while j < e.close && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < e.close && b[j] == b':' {
                        j += 1;
                        let from = j;
                        while j < e.close && !matches!(b[j], b'\n' | b';' | b'}') {
                            j += 1;
                        }
                        out.push((from, j));
                    }
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Where the name `word` is called in the masked source: `word`, white space,
/// `(`, and not the tail of a longer name (`evaluate(` is another name; a `.`
/// before it is a method call of the same name and counts). Offsets of the
/// word and of the first byte after the `(`.
fn calls(masked: &str, word: &str) -> Vec<(usize, usize)> {
    let b = masked.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(p) = masked[at..].find(word) {
        let p = at + p;
        at = p + word.len();
        let before_ok = p == 0 || !(b[p - 1].is_ascii_alphanumeric() || b[p - 1] == b'_');
        let mut j = at;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if before_ok && j < b.len() && b[j] == b'(' {
            out.push((p, j + 1));
        }
    }
    out
}

fn note(out: &mut Vec<String>, src: &Source, p: usize, what: &str) {
    out.push(format!("{}:{}: {what}", src.path, line_of(&src.text, p)));
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("qml" | "js")
        ) {
            out.push(path);
        }
    }
}

/// The QML of the app and of the dock button's plasmoid.
fn sources() -> Vec<Source> {
    let root = repo();
    let mut files = Vec::new();
    for dir in ["apps/telamon-launcher/qml", "plasmoid"] {
        walk(&root.join(dir), &mut files);
    }
    files.sort();
    let out: Vec<Source> = files
        .iter()
        .map(|p| {
            let text = fs::read_to_string(p).unwrap();
            Source {
                path: p
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                masked: mask(&text),
                text,
            }
        })
        .collect();
    assert!(out.len() >= 14, "found only {} QML files", out.len());
    out
}

// ---------------------------------------------------------------- the checks

/// Every violation of the plain-text rules in `src`.
fn text_findings(src: &Source) -> Vec<String> {
    let mut out = Vec::new();
    for e in elements(src) {
        if TEXT_TYPES.contains(&e.base()) {
            let formats = own_values(src, &e, "textFormat");
            if !formats
                .iter()
                .any(|f| f.ends_with(".PlainText") && !f.contains('?'))
            {
                out.push(format!(
                    "{}:{}: `{}` without `textFormat: Text.PlainText` draws <b>, <a href> and <img> of \
                     its text as markup",
                    src.path, e.line, e.name
                ));
            }
        }
        if PLAIN_BY_DEFAULT.contains(&e.base()) {
            for f in own_values(src, &e, "textFormat") {
                if !f.ends_with(".PlainText") {
                    out.push(format!(
                        "{}:{}: `{}` is plain text unless told otherwise; this one is `{f}`",
                        src.path, e.line, e.name
                    ));
                }
            }
        }
    }
    // Any other `textFormat` that is not plain, in any form (a binding, an
    // assignment in a handler).
    let m = &src.masked;
    let mut at = 0;
    while let Some(p) = m[at..].find("textFormat") {
        let p = at + p;
        at = p + "textFormat".len();
        let rest = m[at..].trim_start();
        if rest.starts_with(':') || (rest.starts_with('=') && !rest.starts_with("==")) {
            let value: String = rest[1..]
                .lines()
                .next()
                .unwrap_or("")
                .split([';', '}'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !value.ends_with(".PlainText") || value.contains('?') {
                out.push(format!(
                    "{}:{}: textFormat is `{value}`: text from outside is plain",
                    src.path,
                    line_of(&src.text, p)
                ));
            }
        }
    }
    if let Some(p) = m.find("ToolTip.text") {
        out.push(format!(
            "{}:{}: ToolTip.text: the style draws that tooltip in a format of its own; use \
             TelamonToolTip (plain)",
            src.path,
            line_of(&src.text, p)
        ));
    }
    for token in [
        "RichText",
        "StyledText",
        "MarkdownText",
        "AutoText",
        "Text.Markdown",
        "TextEdit.Markdown",
    ] {
        if let Some(p) = m.find(token) {
            out.push(format!(
                "{}:{}: {token}: nothing here draws markup",
                src.path,
                line_of(&src.text, p)
            ));
        }
    }
    out
}

/// Violations of the code, link and network rules in `src`.
fn code_findings(src: &Source) -> Vec<String> {
    let mut out = Vec::new();
    let m = &src.masked;
    for bad in [
        "new Function",
        "Qt.include",
        "XMLHttpRequest",
        "WebSocket",
        "WebView",
        "WebEngine",
        "importScripts",
        "setSource",
        "openUrlExternally",
        "Qt.openUrl",
        "Qt.createQmlObject",
        "Qt.createComponent",
        "QtQuick.Window.Process",
        "Qt.labs.platform",
    ] {
        let mut at = 0;
        while let Some(p) = m[at..].find(bad) {
            let p = at + p;
            at = p + bad.len();
            note(
                &mut out,
                src,
                p,
                &format!(
                    "{bad}: the launcher's QML neither runs text as code, fetches, nor opens links itself"
                ),
            );
        }
    }
    // The text of `openUrlExternally` in any spelling (Qt["openUrlExternally"]
    // is blanked by the masking above).
    let code = mask_with(&src.text, false);
    if let Some(p) = code.find("openUrlExternally") {
        note(
            &mut out,
            src,
            p,
            "openUrlExternally: the C++ side opens URLs, after checking them",
        );
    }
    for word in ["eval", "Function", "fetch"] {
        for (p, _) in calls(m, word) {
            note(
                &mut out,
                src,
                p,
                &format!("{word}(): the launcher's QML neither runs text as code nor fetches"),
            );
        }
    }
    if let Some(p) = m.find('`') {
        note(
            &mut out,
            src,
            p,
            "a template literal: use string concatenation, so the lint reads all the code",
        );
    }
    // A Loader runs a component of the app, never a file named by data.
    for e in elements(src) {
        if e.base() == "Loader" && !own_values(src, &e, "source").is_empty() {
            out.push(format!(
                "{}:{}: Loader with `source`: use `sourceComponent`; a source is a file to run",
                src.path, e.line
            ));
        }
    }
    out
}

/// Violations of the image rules: an image-like item is made only where
/// `IMAGES` says, with the source it lists; a `Kirigami.Icon` only where
/// `ICONS` says, with its source.
fn image_findings(
    src: &Source,
    images: &[(&str, &[&str], &str)],
    icons: &[(&str, &str)],
) -> Vec<String> {
    let mut out = Vec::new();
    for e in elements(src) {
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        match e.base() {
            "Image"
            | "AnimatedImage"
            | "BorderImage"
            | "AnimatedSprite"
            | "TelamonAvatar"
            | "TelamonChoiceCard"
            | "TelamonScreenshotCarousel" => {
                let sources: Vec<String> = own_raw(src, &e, "source")
                    .into_iter()
                    .map(|s| norm(&s))
                    .collect();
                match images.iter().find(|(f, _, _)| *f == src.path) {
                    None => out.push(format!(
                        "{}:{}: `{}` here: IMAGES lists the files that may make one, with the source it has",
                        src.path, e.line, e.name
                    )),
                    Some((_, allowed, _)) => {
                        for s in &sources {
                            if !allowed.iter().any(|a| norm(a) == *s) {
                                out.push(format!(
                                    "{}:{}: `{}` source `{s}` is not one IMAGES lists",
                                    src.path, e.line, e.name
                                ));
                            }
                        }
                        if sources.is_empty() {
                            out.push(format!("{}:{}: `{}` without a source", src.path, e.line, e.name));
                        }
                    }
                }
            }
            "Icon" if e.name.contains("Kirigami") || e.name == "Icon" => {
                let sources: Vec<String> = own_raw(src, &e, "source")
                    .into_iter()
                    .map(|s| norm(&s))
                    .collect();
                match icons.iter().find(|(f, _)| *f == src.path) {
                    None => {
                        // An icon with a literal theme name is fine anywhere.
                        for s in &sources {
                            if !is_theme_literal(s) {
                                out.push(format!(
                                    "{}:{}: `{}` with a source that is not a theme name literal: use LauncherItemIcon",
                                    src.path, e.line, e.name
                                ));
                            }
                        }
                    }
                    Some((_, allowed)) => {
                        for s in &sources {
                            if norm(allowed) != *s {
                                out.push(format!(
                                    "{}:{}: `{}` source `{s}` is not the one ICONS lists",
                                    src.path, e.line, e.name
                                ));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The values of `prop:` of the element itself, as written (strings kept).
fn own_raw(src: &Source, e: &Element, prop: &str) -> Vec<String> {
    own_spans(src, e, prop)
        .into_iter()
        .map(|(from, to)| src.text[from..to].trim().to_owned())
        .collect()
}

/// A value that is one plain string literal: a theme icon name.
fn is_theme_literal(value: &str) -> bool {
    let t = value.trim();
    let Some(q) = t.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
        return false;
    };
    t.len() >= 2 && t.ends_with(q) && !t[1..t.len() - 1].contains([q, '\\', '+'])
}

fn assert_clean(findings: Vec<String>) {
    assert!(findings.is_empty(), "\n{}", findings.join("\n"));
}

// ------------------------------------------------------------ the real files

#[test]
fn drawn_text_is_plain() {
    let all = sources();
    let mut f = Vec::new();
    for src in &all {
        f.extend(text_findings(src));
    }
    assert_clean(f);
}

#[test]
fn no_qml_runs_text_as_code_fetches_or_opens_links() {
    let all = sources();
    let mut f = Vec::new();
    for src in &all {
        f.extend(code_findings(src));
    }
    assert_clean(f);
}

#[test]
fn images_and_icons_come_from_checked_sources() {
    let all = sources();
    let mut f = Vec::new();
    for src in &all {
        f.extend(image_findings(src, IMAGES, ICONS));
    }
    assert_clean(f);
    // The lists are not stale: each file is there and still says its source.
    for (file, allowed, why) in IMAGES {
        let src = all.iter().find(|s| s.path == *file).expect(file);
        assert!(
            elements(src).iter().any(|e| e.base() == "TelamonAvatar"),
            "{file} no longer makes an avatar"
        );
        assert!(why.len() > 20 && !allowed.is_empty());
    }
    for (file, _) in ICONS {
        assert!(all.iter().any(|s| s.path == *file), "{file}");
    }
}

#[test]
fn the_user_picture_is_a_vetted_path() {
    // The account button's picture comes from Actions::userIcon, which C++
    // sets only from trustedIconFile (docs/SECURITY.md).
    let cpp = fs::read_to_string(repo().join("apps/telamon-launcher/cpp/actions.cpp")).unwrap();
    assert!(cpp.contains("bool trustedIconFile("));
    assert!(cpp.contains("if (trustedIconFile(icon)) {\n                m_userIcon = icon;"));
}

#[test]
fn the_dock_button_talks_to_the_launcher_only() {
    let all = sources();
    let plasmoid = all
        .iter()
        .find(|s| s.path.starts_with("plasmoid/") && s.path.ends_with("main.qml"))
        .expect("the plasmoid");
    // One bus name, one path, one interface, and the methods the interface has.
    let t = &plasmoid.text;
    assert!(t.contains("\"net.eterneon.telamon.launcher\""));
    for member in ["ToggleStart", "SetDockAnchor", "ImportPins"] {
        assert!(t.contains(&format!("\"{member}\"")), "{member}");
    }
    let calls_made = plasmoid.masked.matches("DBus.SessionBus.asyncCall").count();
    assert_eq!(
        calls_made, 1,
        "every call goes through the one `call` helper"
    );
    for other in [
        "DBus.SystemBus",
        "Plasmoid.configuration.Run",
        "executeCommand",
        "Executable",
    ] {
        assert!(!plasmoid.masked.contains(other), "{other}");
    }
}

// ------------------------------------------------- the checker checks itself

fn snippet(path: &str, text: &str) -> Source {
    Source {
        path: path.to_owned(),
        text: text.to_owned(),
        masked: mask(text),
    }
}

fn text_of(text: &str) -> Vec<String> {
    text_findings(&snippet("t.qml", text))
}

fn code_of(text: &str) -> Vec<String> {
    code_findings(&snippet("t.qml", text))
}

#[test]
fn checker_accepts_plain_text() {
    assert!(text_of("Item { Text { text: model.title; textFormat: Text.PlainText } }").is_empty());
    assert!(text_of("Item { QQC2.Label { textFormat: Text.PlainText\n text: x } }").is_empty());
    assert!(text_of("Item { TelamonLabel { text: model.title } }").is_empty());
    assert!(text_of("Item { TelamonLabel { textFormat: Text.PlainText } }").is_empty());
    // Strings and comments do not count.
    assert!(text_of("Item { TelamonLabel { text: \"textFormat: Text.RichText\" } }").is_empty());
    assert!(text_of("// Text { }\nItem { }").is_empty());
}

#[test]
fn checker_rejects_markup_text() {
    for bad in [
        "Item { Text { text: x } }",
        "Item { Label { text: x; textFormat: Text.AutoText } }",
        "Item { Text { textFormat: Text.RichText } }",
        "Item { Text { textFormat: Text.StyledText } }",
        "Item { Text { textFormat: Text.MarkdownText } }",
        "Item { TextEdit { text: x } }",
        "Item { TelamonLabel { textFormat: Text.RichText } }",
        "Item { TelamonTextField { textFormat: Text.StyledText } }",
        "Item { TelamonLabel { text: x; Component.onCompleted: textFormat = Text.RichText } }",
        "Item { Text { textFormat: cond ? Text.PlainText : Text.RichText } }",
        "Item { Rectangle { QQC2.ToolTip.text: model.title } }",
    ] {
        assert!(!text_of(bad).is_empty(), "{bad}");
    }
    // Not fooled by a PlainText in a child or a string.
    assert!(!text_of("Item { Text { text: \"textFormat: Text.PlainText\"; Item { textFormat: Text.PlainText } } }").is_empty());
}

#[test]
fn checker_code_and_network() {
    for bad in [
        "Item { Component.onCompleted: eval(x) }",
        "Item { Component.onCompleted: new Function(x) }",
        "Item { Component.onCompleted: Qt.include(x) }",
        "Item { Component.onCompleted: { var r = new XMLHttpRequest() } }",
        "Item { Component.onCompleted: Qt.openUrlExternally(x) }",
        "Item { Component.onCompleted: Qt[\"openUrlExternally\"](x) }",
        "Item { Component.onCompleted: Qt.createQmlObject(x, this) }",
        "Item { Loader { source: model.path } }",
        "Item { Component.onCompleted: loader.setSource(x) }",
        "Item { Component.onCompleted: fetch(u) }",
        "Item { property string s: `${x}` }",
    ] {
        assert!(!code_of(bad).is_empty(), "{bad}");
    }
    assert!(code_of("Item { Loader { sourceComponent: Component { Item { } } } }").is_empty());
    assert!(code_of("Item { Component.onCompleted: Qt.callLater(f) }").is_empty());
    // A name that only contains a bad one's letters, or a comment, is fine.
    assert!(code_of("Item { property int evaluate: 1; property int prefetch: 2 }").is_empty());
    assert!(code_of("// eval(x)\nItem { }").is_empty());
}

#[test]
fn checker_images() {
    let ok = snippet(
        "f.qml",
        "Item { Image { source: a.b } Kirigami.Icon { source: \"folder\" } TelamonAvatar { source: a.b } }",
    );
    assert!(image_findings(&ok, &[("f.qml", &["a.b"], "why")], &[]).is_empty());
    // An image where none is listed, or with another source.
    assert!(!image_findings(&ok, &[], &[]).is_empty());
    assert!(!image_findings(&ok, &[("f.qml", &["c.d"], "why")], &[]).is_empty());
    // An icon whose source comes from data.
    let data = snippet("g.qml", "Item { Kirigami.Icon { source: model.icon } }");
    assert!(!image_findings(&data, &[], &[]).is_empty());
    assert!(image_findings(&data, &[], &[("g.qml", "model.icon")]).is_empty());
    assert!(!image_findings(&data, &[], &[("g.qml", "other")]).is_empty());
    // A theme name literal is fine anywhere.
    let lit = snippet("h.qml", "Item { Kirigami.Icon { source: 'go-next' } }");
    assert!(image_findings(&lit, &[], &[]).is_empty());
}

#[test]
fn checker_finds_the_real_files() {
    let all = sources();
    assert!(all.iter().any(|s| s.path.ends_with("Panel.qml")));
    assert!(
        all.iter()
            .any(|s| s.path.ends_with("LauncherResultRow.qml"))
    );
    assert!(all.iter().any(|s| s.path.starts_with("plasmoid/")));
    // It sees the elements it is meant to judge.
    let row = all
        .iter()
        .find(|s| s.path.ends_with("LauncherResultRow.qml"))
        .unwrap();
    let labels = elements(row)
        .iter()
        .filter(|e| e.base() == "TelamonLabel")
        .count();
    assert!(labels >= 3, "{labels}");
}
