//! The calculator and unit converter behind the search (DESIGN.md,
//! "Calculator and units"). Pure std, no network, no currencies.
//!
//! The input is whatever the user typed, so everything here is bounded: the
//! text is capped at 256 characters, the parser nests at most 64 deep, every
//! intermediate result must be finite, and nothing panics. Text that is not
//! math or a conversion returns `None` before any real work is done.

use crate::result::{Action, Kind, ResultItem, prior};

/// Longest input looked at, in characters.
const MAX_CHARS: usize = 256;
/// Deepest nesting of parentheses, unary signs and powers.
const MAX_DEPTH: u32 = 64;
/// Significant digits shown.
const SIGNIFICANT: usize = 12;

/// What kind of answer it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcKind {
    Math,
    Conversion,
}

/// A calculator answer, ready to show and copy.
#[derive(Clone, Debug, PartialEq)]
pub struct Calculation {
    /// The input, normalised, e.g. `5 km → mi`.
    pub expression: String,
    pub value: f64,
    /// The value in the locale's decimal separator, at most 12 significant
    /// digits, no trailing zeros, no grouping.
    pub display: String,
    pub kind: CalcKind,
    /// The target unit of a conversion.
    pub unit: Option<String>,
}

impl Calculation {
    /// The single result row: "= 3.10686 mi", Enter copies it.
    pub fn to_result(&self) -> ResultItem {
        let text = match &self.unit {
            Some(u) => format!("{} {}", self.display, u),
            None => self.display.clone(),
        };
        ResultItem {
            id: "calc".to_string(),
            kind: Kind::Calculator,
            title: format!("= {text}"),
            subtitle: self.expression.clone(),
            icon: "accessories-calculator".to_string(),
            score: prior(Kind::Calculator),
            action: Action::Copy { text },
        }
    }
}

/// Evaluates `input` as a calculation or a unit conversion. `decimal` is the
/// locale's decimal separator, '.' or ','. Returns `None` for anything else.
pub fn evaluate(input: &str, decimal: char) -> Option<Calculation> {
    // Four bytes per char at most: cheap reject before counting.
    if input.len() > MAX_CHARS * 4 || input.chars().take(MAX_CHARS + 1).count() > MAX_CHARS {
        return None;
    }
    let decimal = if decimal == ',' { ',' } else { '.' };
    let s = input.trim().trim_end_matches('=').trim_end();
    if s.is_empty() {
        return None;
    }
    // Math and conversions need a digit, or at least a constant or a
    // parenthesis ("ln(e)", "2*pi").
    let b = s.as_bytes();
    let has_digit = b.iter().any(u8::is_ascii_digit);
    if !has_digit && !s.contains(['π', '(']) && !b.windows(2).any(|w| w.eq_ignore_ascii_case(b"pi"))
    {
        return None;
    }
    if has_digit && let Some(c) = convert(s, decimal) {
        return Some(c);
    }
    math(s, decimal)
}

// ---------------------------------------------------------------- numbers

fn fin(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

/// Formats `v` with at most 12 significant digits (see `Calculation::display`).
fn format_number(v: f64, decimal: char) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".to_string();
    }
    let s = format!("{:.*e}", SIGNIFICANT - 1, v.abs());
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let mut digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let mut out = String::with_capacity(24);
    if v < 0.0 {
        out.push('-');
    }
    if (-6..15).contains(&exp) {
        if exp >= 0 {
            let n = exp as usize + 1;
            if digits.len() <= n {
                out.push_str(&digits);
                out.extend(std::iter::repeat_n('0', n - digits.len()));
            } else {
                out.push_str(&digits[..n]);
                out.push(decimal);
                out.push_str(&digits[n..]);
            }
        } else {
            out.push('0');
            out.push(decimal);
            out.extend(std::iter::repeat_n('0', (-exp - 1) as usize));
            out.push_str(&digits);
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push(decimal);
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.unsigned_abs()));
    }
    out
}

/// Reads an unsigned number at the start of `s` (digits, one `decimal`, an
/// optional exponent). Returns the value and the byte length used.
fn lex_number(s: &str, decimal: char) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let dec = decimal as u8;
    let mut i = 0;
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == dec {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    // Exponent only when digits follow ("1e5", "2e-3"), so "2e" stays 2 × e.
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let text = &s[..i];
    let v: f64 = if decimal == '.' {
        text.parse().ok()?
    } else {
        text.replace(decimal, ".").parse().ok()?
    };
    Some((fin(v)?, i))
}

// ------------------------------------------------------------------- math

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok<'a> {
    Num(f64, &'a str),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
    Pct,
    Ident(&'a str),
}

fn lex(s: &str, decimal: char) -> Option<Vec<Tok<'_>>> {
    let mut toks = Vec::with_capacity(16);
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next()?;
        let next_is_digit = rest[c.len_utf8()..]
            .chars()
            .next()
            .is_some_and(|n| n.is_ascii_digit());
        if c.is_ascii_digit() || (c == decimal && next_is_digit) {
            let (v, len) = lex_number(rest, decimal)?;
            toks.push(Tok::Num(v, &rest[..len]));
            i += len;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let len = rest
                .find(|ch: char| !ch.is_ascii_alphabetic())
                .unwrap_or(rest.len());
            toks.push(Tok::Ident(&rest[..len]));
            i += len;
            continue;
        }
        let tok = match c {
            c if c.is_whitespace() => {
                i += c.len_utf8();
                continue;
            }
            '+' => Tok::Plus,
            '-' | '−' | '–' => Tok::Minus,
            '×' | '·' => Tok::Star,
            '÷' | '/' => Tok::Slash,
            '*' => {
                if rest.as_bytes().get(1) == Some(&b'*') {
                    i += 1;
                    Tok::Caret
                } else {
                    Tok::Star
                }
            }
            '^' => Tok::Caret,
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '%' => Tok::Pct,
            'π' => Tok::Ident("π"),
            _ => return None,
        };
        toks.push(tok);
        i += c.len_utf8();
        if toks.len() > MAX_CHARS {
            return None;
        }
    }
    Some(toks)
}

fn is_const(name: &str) -> bool {
    name.eq_ignore_ascii_case("pi") || name == "π" || name.eq_ignore_ascii_case("e")
}

fn is_func(name: &str) -> bool {
    const FUNCS: [&str; 12] = [
        "sqrt", "sin", "cos", "tan", "asin", "acos", "atan", "ln", "log", "abs", "exp", "floor",
    ];
    FUNCS.iter().any(|f| name.eq_ignore_ascii_case(f))
        || name.eq_ignore_ascii_case("ceil")
        || name.eq_ignore_ascii_case("round")
}

struct Parser<'a> {
    t: &'a [Tok<'a>],
    i: usize,
    depth: u32,
    /// Whether an operator, function, constant or percent was used: a bare
    /// number is not a calculation.
    calc: bool,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<Tok<'a>> {
        self.t.get(self.i).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(Tok::Plus) => {
                    self.i += 1;
                    self.calc = true;
                    v = fin(v + self.term()?)?;
                }
                Some(Tok::Minus) => {
                    self.i += 1;
                    self.calc = true;
                    v = fin(v - self.term()?)?;
                }
                _ => return Some(v),
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            match self.peek() {
                Some(Tok::Star) => {
                    self.i += 1;
                    self.calc = true;
                    v = fin(v * self.unary()?)?;
                }
                Some(Tok::Slash) => {
                    self.i += 1;
                    self.calc = true;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return None;
                    }
                    v = fin(v / d)?;
                }
                // A percent sign the postfix step left alone is a modulo.
                Some(Tok::Pct) => {
                    self.i += 1;
                    self.calc = true;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return None;
                    }
                    v = fin(v % d)?;
                }
                Some(Tok::LParen) => {
                    self.calc = true;
                    v = fin(v * self.unary()?)?;
                }
                Some(Tok::Ident(n))
                    if !n.eq_ignore_ascii_case("of")
                        && (is_func(n) || is_const(n) && !n.eq_ignore_ascii_case("e")) =>
                {
                    self.calc = true;
                    v = fin(v * self.unary()?)?;
                }
                _ => return Some(v),
            }
        }
    }

    /// Every recursion passes through here, so the depth limit lives here.
    fn unary(&mut self) -> Option<f64> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        let r = match self.peek() {
            Some(Tok::Minus) => {
                self.i += 1;
                self.unary().map(|v| -v)
            }
            Some(Tok::Plus) => {
                self.i += 1;
                self.unary()
            }
            _ => self.power(),
        };
        self.depth -= 1;
        r
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.postfix()?;
        if self.peek() == Some(Tok::Caret) {
            self.i += 1;
            self.calc = true;
            // The exponent goes through unary: right-associative, and 2^-3.
            let e = self.unary()?;
            return fin(base.powf(e));
        }
        Some(base)
    }

    fn postfix(&mut self) -> Option<f64> {
        let mut v = self.atom()?;
        while self.peek() == Some(Tok::Pct) {
            match self.t.get(self.i + 1).copied() {
                Some(Tok::Ident(n)) if n.eq_ignore_ascii_case("of") => {
                    self.i += 2;
                    self.calc = true;
                    let rhs = self.unary()?;
                    return fin(v / 100.0 * rhs);
                }
                // "10 % 3": the term loop takes it as a modulo.
                Some(Tok::Num(..) | Tok::LParen | Tok::Ident(_)) => break,
                _ => {
                    self.i += 1;
                    self.calc = true;
                    v /= 100.0;
                }
            }
        }
        Some(v)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.peek()? {
            Tok::Num(v, _) => {
                self.i += 1;
                Some(v)
            }
            Tok::LParen => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(Tok::RParen) {
                    return None;
                }
                self.i += 1;
                Some(v)
            }
            Tok::Ident(name) => {
                self.i += 1;
                if name.eq_ignore_ascii_case("pi") || name == "π" {
                    return Some(std::f64::consts::PI);
                }
                if name.eq_ignore_ascii_case("e") {
                    return Some(std::f64::consts::E);
                }
                if !is_func(name) {
                    return None;
                }
                self.calc = true;
                let x = if self.peek() == Some(Tok::LParen) {
                    self.atom()?
                } else {
                    self.unary()?
                };
                apply(&name.to_ascii_lowercase(), x)
            }
            _ => None,
        }
    }
}

/// Trig results that are zero up to rounding noise (sin pi) show as 0.
fn snap(x: f64) -> f64 {
    if x.abs() < 1e-15 { 0.0 } else { x }
}

fn apply(name: &str, x: f64) -> Option<f64> {
    let r = match name {
        "sqrt" => x.sqrt(),
        "sin" => snap(x.sin()),
        "cos" => snap(x.cos()),
        "tan" => snap(x.tan()),
        "asin" => x.asin(),
        "acos" => x.acos(),
        "atan" => x.atan(),
        "ln" => x.ln(),
        "log" => x.log10(),
        "abs" => x.abs(),
        "exp" => x.exp(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "round" => x.round(),
        _ => return None,
    };
    fin(r)
}

/// The normalised text of a token list.
fn render(toks: &[Tok<'_>]) -> String {
    let mut out = String::with_capacity(toks.len() * 3);
    let mut prev: Option<Tok<'_>> = None;
    for &t in toks {
        let after_atom = matches!(
            prev,
            Some(Tok::Num(..) | Tok::RParen | Tok::Pct | Tok::Ident(_))
        );
        let after_func = matches!(prev, Some(Tok::Ident(n)) if is_func(n));
        match t {
            Tok::Num(_, text) => {
                if after_atom {
                    out.push(' ');
                }
                out.push_str(text);
            }
            Tok::Ident(n) => {
                if after_atom {
                    out.push(' ');
                }
                out.push_str(n);
            }
            Tok::LParen => {
                if after_atom && !after_func {
                    out.push(' ');
                }
                out.push('(');
            }
            Tok::RParen => out.push(')'),
            Tok::Pct => out.push('%'),
            Tok::Plus | Tok::Minus | Tok::Star | Tok::Slash | Tok::Caret => {
                let sym = match t {
                    Tok::Plus => "+",
                    Tok::Minus => "−",
                    Tok::Star => "×",
                    Tok::Slash => "÷",
                    _ => "^",
                };
                let binary = matches!(prev, Some(Tok::Num(..) | Tok::RParen | Tok::Pct))
                    || matches!(prev, Some(Tok::Ident(n)) if !is_func(n));
                if binary {
                    out.push(' ');
                    out.push_str(sym);
                    out.push(' ');
                } else {
                    out.push_str(sym);
                }
            }
        }
        prev = Some(t);
    }
    out
}

fn math(s: &str, decimal: char) -> Option<Calculation> {
    let toks = lex(s, decimal)?;
    if toks.is_empty() {
        return None;
    }
    let mut p = Parser {
        t: &toks,
        i: 0,
        depth: 0,
        calc: false,
    };
    let value = fin(p.expr()?)?;
    if p.i != toks.len() || !p.calc {
        return None;
    }
    let value = if value == 0.0 { 0.0 } else { value };
    Some(Calculation {
        expression: render(&toks),
        value,
        display: format_number(value, decimal),
        kind: CalcKind::Math,
        unit: None,
    })
}

// ------------------------------------------------------------------ units

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dim {
    Length,
    Mass,
    Temperature,
    Volume,
    Area,
    Speed,
    Time,
    Data,
}

struct Unit {
    symbol: &'static str,
    /// Spellings that match only with exactly this case.
    exact: &'static [&'static str],
    /// Lowercase spellings, matched case-insensitively.
    names: &'static [&'static str],
    dim: Dim,
    /// base = value × factor + offset (metres, kilograms, kelvin, cubic
    /// metres, square metres, m/s, seconds, bytes).
    factor: f64,
    offset: f64,
}

const fn unit(
    symbol: &'static str,
    exact: &'static [&'static str],
    names: &'static [&'static str],
    dim: Dim,
    factor: f64,
) -> Unit {
    Unit {
        symbol,
        exact,
        names,
        dim,
        factor,
        offset: 0.0,
    }
}

use Dim::*;

static UNITS: &[Unit] = &[
    // Length.
    unit(
        "mm",
        &[],
        &[
            "mm",
            "millimeter",
            "millimeters",
            "millimetre",
            "millimetres",
        ],
        Length,
        0.001,
    ),
    unit(
        "cm",
        &[],
        &[
            "cm",
            "centimeter",
            "centimeters",
            "centimetre",
            "centimetres",
        ],
        Length,
        0.01,
    ),
    unit(
        "m",
        &[],
        &["m", "meter", "meters", "metre", "metres"],
        Length,
        1.0,
    ),
    unit(
        "km",
        &[],
        &["km", "kilometer", "kilometers", "kilometre", "kilometres"],
        Length,
        1000.0,
    ),
    unit("in", &[], &["in", "inch", "inches"], Length, 0.0254),
    unit("ft", &[], &["ft", "foot", "feet"], Length, 0.3048),
    unit("yd", &[], &["yd", "yard", "yards"], Length, 0.9144),
    unit("mi", &[], &["mi", "mile", "miles"], Length, 1609.344),
    unit(
        "nmi",
        &[],
        &["nmi", "nauticalmile", "nauticalmiles"],
        Length,
        1852.0,
    ),
    // Mass.
    unit("mg", &[], &["mg", "milligram", "milligrams"], Mass, 1e-6),
    unit("g", &[], &["g", "gram", "grams"], Mass, 0.001),
    unit(
        "kg",
        &[],
        &["kg", "kilogram", "kilograms", "kilo", "kilos"],
        Mass,
        1.0,
    ),
    unit("t", &[], &["t", "tonne", "tonnes"], Mass, 1000.0),
    unit(
        "oz",
        &[],
        &["oz", "ounce", "ounces"],
        Mass,
        0.028_349_523_125,
    ),
    unit(
        "lb",
        &[],
        &["lb", "lbs", "pound", "pounds"],
        Mass,
        0.453_592_37,
    ),
    unit("st", &[], &["st", "stone", "stones"], Mass, 6.350_293_18),
    // Temperature.
    Unit {
        symbol: "°C",
        exact: &[],
        names: &["c", "°c", "celsius", "degc"],
        dim: Temperature,
        factor: 1.0,
        offset: 273.15,
    },
    Unit {
        symbol: "°F",
        exact: &[],
        names: &["f", "°f", "fahrenheit", "degf"],
        dim: Temperature,
        factor: 5.0 / 9.0,
        offset: 273.15 - 32.0 * 5.0 / 9.0,
    },
    unit("K", &[], &["k", "kelvin", "kelvins"], Temperature, 1.0),
    // Volume.
    unit(
        "ml",
        &[],
        &[
            "ml",
            "milliliter",
            "milliliters",
            "millilitre",
            "millilitres",
        ],
        Volume,
        1e-6,
    ),
    unit(
        "l",
        &[],
        &["l", "liter", "liters", "litre", "litres"],
        Volume,
        0.001,
    ),
    unit(
        "gal",
        &[],
        &["gal", "gallon", "gallons"],
        Volume,
        0.003_785_411_784,
    ),
    unit(
        "qt",
        &[],
        &["qt", "quart", "quarts"],
        Volume,
        0.000_946_352_946,
    ),
    unit(
        "pt",
        &[],
        &["pt", "pint", "pints"],
        Volume,
        0.000_473_176_473,
    ),
    unit("cup", &[], &["cup", "cups"], Volume, 0.000_236_588_236_5),
    unit(
        "fl oz",
        &[],
        &["floz", "fl oz", "fluidounce", "fluidounces"],
        Volume,
        0.000_029_573_529_562_5,
    ),
    unit(
        "m³",
        &[],
        &["m3", "m³", "cubicmeter", "cubicmeters"],
        Volume,
        1.0,
    ),
    // Area.
    unit(
        "m²",
        &[],
        &["m2", "m²", "sqm", "squaremeter", "squaremeters"],
        Area,
        1.0,
    ),
    unit(
        "km²",
        &[],
        &["km2", "km²", "sqkm", "squarekilometer", "squarekilometers"],
        Area,
        1e6,
    ),
    unit("ha", &[], &["ha", "hectare", "hectares"], Area, 10_000.0),
    unit("acre", &[], &["acre", "acres"], Area, 4_046.856_422_4),
    unit(
        "ft²",
        &[],
        &["ft2", "ft²", "sqft", "squarefoot", "squarefeet"],
        Area,
        0.092_903_04,
    ),
    unit(
        "in²",
        &[],
        &["in2", "in²", "sqin", "squareinch", "squareinches"],
        Area,
        0.000_645_16,
    ),
    // Speed.
    unit("m/s", &[], &["m/s", "mps"], Speed, 1.0),
    unit(
        "km/h",
        &[],
        &["km/h", "kmh", "kph", "kmph"],
        Speed,
        1.0 / 3.6,
    ),
    unit("mph", &[], &["mph", "mi/h"], Speed, 0.447_04),
    unit(
        "knot",
        &[],
        &["knot", "knots", "kn", "kt"],
        Speed,
        1852.0 / 3600.0,
    ),
    // Time.
    unit(
        "ms",
        &[],
        &["ms", "millisecond", "milliseconds"],
        Time,
        0.001,
    ),
    unit(
        "s",
        &[],
        &["s", "sec", "secs", "second", "seconds"],
        Time,
        1.0,
    ),
    unit(
        "min",
        &[],
        &["min", "mins", "minute", "minutes"],
        Time,
        60.0,
    ),
    unit("h", &[], &["h", "hr", "hrs", "hour", "hours"], Time, 3600.0),
    unit("day", &[], &["d", "day", "days"], Time, 86_400.0),
    unit("week", &[], &["w", "week", "weeks"], Time, 604_800.0),
    unit(
        "year",
        &[],
        &["y", "yr", "yrs", "year", "years"],
        Time,
        31_557_600.0,
    ),
    // Data: the case-sensitive spellings come first, so "MB" is megabytes and
    // "Mb" megabits, while all-lowercase ("mb", "kb") means bytes.
    unit("B", &["B"], &["byte", "bytes"], Data, 1.0),
    unit("kB", &["kB"], &["kb", "kilobyte", "kilobytes"], Data, 1e3),
    unit("MB", &["MB"], &["mb", "megabyte", "megabytes"], Data, 1e6),
    unit("GB", &["GB"], &["gb", "gigabyte", "gigabytes"], Data, 1e9),
    unit("TB", &["TB"], &["tb", "terabyte", "terabytes"], Data, 1e12),
    unit(
        "KiB",
        &["KiB"],
        &["kib", "kibibyte", "kibibytes"],
        Data,
        1024.0,
    ),
    unit(
        "MiB",
        &["MiB"],
        &["mib", "mebibyte", "mebibytes"],
        Data,
        1_048_576.0,
    ),
    unit(
        "GiB",
        &["GiB"],
        &["gib", "gibibyte", "gibibytes"],
        Data,
        1_073_741_824.0,
    ),
    unit(
        "TiB",
        &["TiB"],
        &["tib", "tebibyte", "tebibytes"],
        Data,
        1_099_511_627_776.0,
    ),
    unit("bit", &["b"], &["bit", "bits"], Data, 0.125),
    unit("kb", &["Kb", "kbit"], &["kilobit", "kilobits"], Data, 125.0),
    unit("Mb", &["Mb"], &["megabit", "megabits"], Data, 125_000.0),
    unit("Gb", &["Gb"], &["gigabit", "gigabits"], Data, 125_000_000.0),
];

fn lookup(name: &str) -> Option<&'static Unit> {
    let name = name.trim();
    if name.is_empty() || name.len() > 24 {
        return None;
    }
    if let Some(u) = UNITS.iter().find(|u| u.exact.contains(&name)) {
        return Some(u);
    }
    let lower = name.to_lowercase();
    UNITS.iter().find(|u| u.names.contains(&lower.as_str()))
}

fn convert(s: &str, decimal: char) -> Option<Calculation> {
    let first = s.chars().next()?;
    let (neg, body) = match first {
        '-' | '−' => (true, &s[first.len_utf8()..]),
        '+' => (false, &s[1..]),
        _ => (false, s),
    };
    let body = body.trim_start();
    let (n, len) = lex_number(body, decimal)?;
    let n = if neg { -n } else { n };
    let rest = body[len..].trim_start();
    let c = rest.chars().next()?;
    if !(c.is_alphabetic() || c == '°') {
        return None;
    }
    let (from, to) = if let Some(p) = rest.find(['→', '=']) {
        let sep_len = rest[p..].chars().next()?.len_utf8();
        (
            rest[..p].trim().to_string(),
            rest[p + sep_len..].trim().to_string(),
        )
    } else {
        let words: Vec<&str> = rest.split_whitespace().collect();
        let idx = (1..words.len()).find(|&i| {
            ["in", "to", "as"]
                .iter()
                .any(|w| words[i].eq_ignore_ascii_case(w))
        })?;
        (words[..idx].join(" "), words[idx + 1..].join(" "))
    };
    let from = lookup(&from)?;
    let to = lookup(&to)?;
    if from.dim != to.dim {
        return None;
    }
    let base = n * from.factor + from.offset;
    let value = fin((base - to.offset) / to.factor)?;
    let value = if value == 0.0 { 0.0 } else { value };
    Some(Calculation {
        expression: format!(
            "{} {} → {}",
            format_number(n, decimal),
            from.symbol,
            to.symbol
        ),
        value,
        display: format_number(value, decimal),
        kind: CalcKind::Conversion,
        unit: Some(to.symbol.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(input: &str) -> Option<String> {
        evaluate(input, '.').map(|c| c.display)
    }

    fn near(input: &str, want: f64) {
        let c = evaluate(input, '.').unwrap_or_else(|| panic!("no result for {input:?}"));
        assert!(
            (c.value - want).abs() <= want.abs() * 1e-9 + 1e-12,
            "{input:?} = {} (wanted {want})",
            c.value
        );
    }

    #[test]
    fn operators() {
        assert_eq!(d("2+3").as_deref(), Some("5"));
        assert_eq!(d("7 - 10").as_deref(), Some("-3"));
        assert_eq!(d("6*7").as_deref(), Some("42"));
        assert_eq!(d("7/2").as_deref(), Some("3.5"));
        assert_eq!(d("6 × 7").as_deref(), Some("42"));
        assert_eq!(d("9 ÷ 3").as_deref(), Some("3"));
        assert_eq!(d("9 − 3").as_deref(), Some("6"));
        assert_eq!(d("2^10").as_deref(), Some("1024"));
        assert_eq!(d("2**10").as_deref(), Some("1024"));
        assert_eq!(d("10 % 3").as_deref(), Some("1"));
        assert_eq!(d("50%").as_deref(), Some("0.5"));
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(d("2+3*4").as_deref(), Some("14"));
        assert_eq!(d("(2+3)*4").as_deref(), Some("20"));
        assert_eq!(d("2^3^2").as_deref(), Some("512"));
        assert_eq!(d("-2^2").as_deref(), Some("-4"));
        assert_eq!(d("2^-1").as_deref(), Some("0.5"));
        assert_eq!(d("10-4-3").as_deref(), Some("3"));
        assert_eq!(d("100/10/5").as_deref(), Some("2"));
        assert_eq!(d("--3+1").as_deref(), Some("4"));
        assert_eq!(d("-(2+3)").as_deref(), Some("-5"));
    }

    #[test]
    fn percent_of() {
        assert_eq!(d("15% of 80").as_deref(), Some("12"));
        assert_eq!(d("15 % of 80 + 1").as_deref(), Some("13"));
    }

    #[test]
    fn functions() {
        assert_eq!(d("sqrt(16)").as_deref(), Some("4"));
        assert_eq!(d("sqrt 16 + 1").as_deref(), Some("5"));
        assert_eq!(d("sin(0)+1").as_deref(), Some("1"));
        assert_eq!(d("cos(pi)").as_deref(), Some("-1"));
        assert_eq!(d("sin(pi)+0").as_deref(), Some("0"));
        near("tan(pi/4)", 1.0);
        near("asin(1)", std::f64::consts::FRAC_PI_2);
        near("acos(1)+1", 1.0);
        near("atan(1)", std::f64::consts::FRAC_PI_4);
        near("ln(e)", 1.0);
        assert_eq!(d("log(1000)").as_deref(), Some("3"));
        assert_eq!(d("abs(-4.5)").as_deref(), Some("4.5"));
        near("exp(1)", std::f64::consts::E);
        assert_eq!(d("floor(2.7)").as_deref(), Some("2"));
        assert_eq!(d("ceil(2.1)").as_deref(), Some("3"));
        assert_eq!(d("round(2.5)").as_deref(), Some("3"));
        assert_eq!(d("SQRT(9)").as_deref(), Some("3"));
    }

    #[test]
    fn constants_and_implicit_multiplication() {
        assert_eq!(d("pi").as_deref(), None);
        assert_eq!(d("π").as_deref(), None);
        assert_eq!(d("pi+0").as_deref(), Some("3.14159265359"));
        assert_eq!(d("2pi").as_deref(), Some("6.28318530718"));
        assert_eq!(d("2π").as_deref(), Some("6.28318530718"));
        assert_eq!(d("2*e").as_deref(), Some("5.43656365692"));
        assert_eq!(d("2(3+4)").as_deref(), Some("14"));
        assert_eq!(d("1e3 + 1").as_deref(), Some("1001"));
    }

    #[test]
    fn not_calculations() {
        for s in [
            "firefox", "2", "-2", "(2)", "1.2.3", "v1.0", "c++", "", "   ", "e", "pi", "1 2",
            "2 +", "(2+3", "2+3)", "foo(3)+1", "5 apples", "3d", "1+", "sqrt", "2e", "== 3",
        ] {
            assert_eq!(d(s), None, "{s:?}");
        }
    }

    #[test]
    fn never_shows_nan_inf_or_overflow() {
        for s in [
            "1/0",
            "0/0",
            "5 % 0",
            "sqrt(-1)",
            "ln(0)",
            "ln(-1)",
            "log(0)",
            "asin(2)",
            "acos(2)",
            "10^400",
            "(-8)^0.5",
            "exp(1000)",
            "1e999+1",
            "9^9^9",
            "1/0+1",
        ] {
            assert_eq!(d(s), None, "{s:?}");
        }
    }

    #[test]
    fn display_format() {
        assert_eq!(d("0.1+0.2").as_deref(), Some("0.3"));
        assert_eq!(d("1/3").as_deref(), Some("0.333333333333"));
        assert_eq!(d("2/3").as_deref(), Some("0.666666666667"));
        assert_eq!(d("1000*1000").as_deref(), Some("1000000"));
        assert_eq!(d("123456789*1000").as_deref(), Some("123456789000"));
        assert_eq!(d("123*10^18").as_deref(), Some("1.23e+20"));
        assert_eq!(d("1/10^8").as_deref(), Some("1e-08"));
        assert_eq!(d("1/10^6").as_deref(), Some("0.000001"));
        assert_eq!(d("1-1").as_deref(), Some("0"));
        assert_eq!(d("0*-1").as_deref(), Some("0"));
        assert_eq!(d("-1/4").as_deref(), Some("-0.25"));
        assert_eq!(
            d("0.999999999999999+0.000000000000002").as_deref(),
            Some("1")
        );
    }

    #[test]
    fn expression_is_normalised() {
        let c = evaluate("2*3-1", '.').unwrap();
        assert_eq!(c.expression, "2 × 3 − 1");
        assert_eq!(c.kind, CalcKind::Math);
        assert_eq!(c.unit, None);
        let c = evaluate("sqrt(4)*-2", '.').unwrap();
        assert_eq!(c.expression, "sqrt(4) × −2");
        let c = evaluate("5km in mi", '.').unwrap();
        assert_eq!(c.expression, "5 km → mi");
    }

    #[test]
    fn trailing_equals_is_ignored() {
        assert_eq!(d("2+2=").as_deref(), Some("4"));
        assert_eq!(d("2+2 =").as_deref(), Some("4"));
    }

    #[test]
    fn length_conversions() {
        near("5 km in mi", 3.106_855_961);
        near("1 mi to km", 1.609_344);
        near("12 in to ft", 1.0);
        near("1 yd to ft", 3.0);
        near("100 cm = m", 1.0);
        near("1 m as mm", 1000.0);
        near("1 nmi to m", 1852.0);
        near("3 feet in inches", 36.0);
        near("2 miles to meters", 3218.688);
        near("5km→mi", 3.106_855_961);
        near("5 in in cm", 12.7);
    }

    #[test]
    fn mass_conversions() {
        near("1 kg to lb", 2.204_622_622);
        near("16 oz in lb", 1.0);
        near("14 lb to st", 1.0);
        near("1 t to kg", 1000.0);
        near("500 mg to g", 0.5);
        near("2 pounds in kilograms", 0.907_184_74);
        near("1 KG in g", 1000.0);
    }

    #[test]
    fn temperature_conversions() {
        near("100 c to f", 212.0);
        near("100 f to c", 37.777_777_777_8);
        near("0 °C in K", 273.15);
        near("32 °F → °C", 0.0);
        near("-40 f to c", -40.0);
        near("300 k to celsius", 26.85);
        near("98.6 fahrenheit to celsius", 37.0);
        assert_eq!(d("-40 c in f").as_deref(), Some("-40"));
        assert_eq!(d("0 c in f").as_deref(), Some("32"));
    }

    #[test]
    fn volume_conversions() {
        near("1 gal to l", 3.785_411_784);
        near("4 qt in gal", 1.0);
        near("2 pt to qt", 1.0);
        near("1 l to ml", 1000.0);
        near("1 m3 to l", 1000.0);
        near("16 cup to gal", 1.0);
        near("128 floz to gal", 1.0);
    }

    #[test]
    fn area_conversions() {
        near("1 ha to m2", 10_000.0);
        near("1 km2 to ha", 100.0);
        near("1 acre in m2", 4046.8564224);
        near("1 m2 to ft2", 10.763_910_417);
        near("1 ft2 to in2", 144.0);
    }

    #[test]
    fn speed_conversions() {
        near("36 km/h to m/s", 10.0);
        near("60 mph in kph", 96.560_64);
        near("10 knot to km/h", 18.52);
        near("1 m/s in kmh", 3.6);
    }

    #[test]
    fn time_conversions() {
        near("1 h to min", 60.0);
        near("2 day in h", 48.0);
        near("1 week to day", 7.0);
        near("1 year to day", 365.25);
        near("1500 ms to s", 1.5);
        near("90 min in hours", 1.5);
    }

    #[test]
    fn data_conversions() {
        near("1 GB to MB", 1000.0);
        near("1 GiB to MiB", 1024.0);
        near("1 KiB to B", 1024.0);
        near("1 kB to B", 1000.0);
        near("1 TB to GB", 1000.0);
        near("1 TiB to GiB", 1024.0);
        near("1 B to bit", 8.0);
        near("8 b to B", 1.0);
        near("1 MB to Mb", 8.0);
        near("1 GB to GiB", 0.931_322_574_6);
        near("2 bytes in bits", 16.0);
        near("1 mb to kb", 1000.0);
    }

    #[test]
    fn mismatched_dimensions_and_unknowns() {
        for s in [
            "5 km in kg",
            "5 c in m",
            "1 gb to s",
            "5 km in",
            "5 km in foo",
            "5 foo in km",
            "5 km",
            "5 km in mi mi",
            "5 in",
            "5 usd to eur",
            "5 km in in in",
        ] {
            assert_eq!(d(s), None, "{s:?}");
        }
    }

    #[test]
    fn conversion_result_row() {
        let c = evaluate("5 km in mi", '.').unwrap();
        assert_eq!(c.kind, CalcKind::Conversion);
        assert_eq!(c.unit.as_deref(), Some("mi"));
        assert_eq!(c.display, "3.10685596119");
        let r = c.to_result();
        assert_eq!(r.id, "calc");
        assert_eq!(r.kind, Kind::Calculator);
        assert_eq!(r.title, "= 3.10685596119 mi");
        assert_eq!(r.subtitle, "5 km → mi");
        assert_eq!(r.icon, "accessories-calculator");
        assert_eq!(r.score, prior(Kind::Calculator));
        assert_eq!(
            r.action,
            Action::Copy {
                text: "3.10685596119 mi".to_string()
            }
        );
    }

    #[test]
    fn math_result_row() {
        let r = evaluate("2+3", '.').unwrap().to_result();
        assert_eq!(r.title, "= 5");
        assert_eq!(r.subtitle, "2 + 3");
        assert_eq!(
            r.action,
            Action::Copy {
                text: "5".to_string()
            }
        );
    }

    #[test]
    fn comma_decimal() {
        let c = evaluate("2,5 + 1", ',').unwrap();
        assert_eq!(c.display, "3,5");
        assert_eq!(c.expression, "2,5 + 1");
        assert_eq!(evaluate("1/4", ',').unwrap().display, "0,25");
        assert_eq!(
            evaluate("5,5 km in mi", ',').unwrap().unit.as_deref(),
            Some("mi")
        );
        assert_eq!(evaluate("1,5 km in m", ',').unwrap().display, "1500");
        assert_eq!(evaluate(",5*2", ',').unwrap().display, "1");
        assert_eq!(evaluate("1/3", ',').unwrap().display, "0,333333333333");
        assert_eq!(evaluate("123*10^18", ',').unwrap().display, "1,23e+20");
    }

    #[test]
    fn comma_is_never_thousands() {
        assert_eq!(evaluate("1,000 + 1", '.'), None);
        assert_eq!(evaluate("1,000 + 1", ',').unwrap().display, "2");
        assert_eq!(evaluate("1.5 + 1", ','), None);
        assert_eq!(evaluate("1,2,3 + 1", ','), None);
    }

    #[test]
    fn dot_decimal() {
        assert_eq!(d("2.5 + 1").as_deref(), Some("3.5"));
        assert_eq!(d(".5*2").as_deref(), Some("1"));
        assert_eq!(d("1.5 km in m").as_deref(), Some("1500"));
    }

    #[test]
    fn depth_limit() {
        let ok = format!("{}1{}+1", "(".repeat(30), ")".repeat(30));
        assert_eq!(d(&ok).as_deref(), Some("2"));
        let deep = format!("{}1{}+1", "(".repeat(100), ")".repeat(100));
        assert_eq!(d(&deep), None);
        let minus = format!("{}1+1", "-".repeat(100));
        assert_eq!(d(&minus), None);
        let pow = format!("1{}", "^1".repeat(100));
        assert_eq!(d(&pow), None);
        let funcs = format!("{}1", "sqrt ".repeat(100));
        assert_eq!(d(&funcs), None);
    }

    #[test]
    fn length_limit() {
        let long = vec!["1"; 128].join("+");
        assert_eq!(long.chars().count(), 255);
        assert_eq!(d(&long).as_deref(), Some("128"));
        let longer = format!("{long}+1");
        assert_eq!(longer.chars().count(), 257);
        assert_eq!(d(&longer), None);
        let huge = "1+".repeat(100_000);
        assert_eq!(d(&huge), None);
        let wide = "é".repeat(300);
        assert_eq!(d(&wide), None);
    }

    #[test]
    fn unicode_and_odd_input_do_not_panic() {
        for s in [
            "°",
            "5 ° to c",
            "5°c in f",
            "π π",
            "2π3",
            "∞+1",
            "1+\u{0}",
            "５+１",
            "5 → ",
            "→",
            "=",
            "5 = = 3",
            "%",
            "% of",
            "5% of",
            "5 % of 3 %",
            "((",
            "()",
            "2 ** ",
            "2 **",
        ] {
            let _ = evaluate(s, '.');
            let _ = evaluate(s, ',');
        }
        assert_eq!(d("5°c in f").as_deref(), Some("41"));
    }

    #[test]
    fn fuzz_no_panic() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        // Random bytes.
        for _ in 0..3000 {
            let len = (next() % 80) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let s = String::from_utf8_lossy(&bytes);
            let _ = evaluate(&s, '.');
            let _ = evaluate(&s, ',');
        }
        // Token soup, which reaches deeper into the parser and converter.
        let parts = [
            "1", "2.5", "0", "9", ",", ".", "+", "-", "*", "/", "^", "**", "%", "(", ")", " ",
            "of", "pi", "π", "e", "sqrt", "sin", "ln", "km", "mi", "in", "to", "as", "→", "=", "c",
            "f", "B", "b", "MB", "m/s", "°", "−", "×", "÷", "1e5", "1e", "x",
        ];
        for _ in 0..5000 {
            let n = (next() % 14) as usize;
            let mut s = String::new();
            for _ in 0..n {
                s.push_str(parts[(next() % parts.len() as u64) as usize]);
                if next() % 3 == 0 {
                    s.push(' ');
                }
            }
            if let Some(c) = evaluate(&s, '.') {
                assert!(c.value.is_finite(), "{s:?}");
                assert!(!c.display.is_empty() && !c.display.contains("NaN"), "{s:?}");
            }
            if let Some(c) = evaluate(&s, ',') {
                assert!(c.value.is_finite(), "{s:?}");
            }
        }
    }

    #[test]
    fn typical_input_is_fast() {
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = evaluate("firefox", '.');
            let _ = evaluate("12.5*(3+4)^2/7", '.');
            let _ = evaluate("5 km in mi", '.');
        }
        // 3000 calls; a generous bound that still catches accidental
        // quadratic work (debug build).
        assert!(start.elapsed() < std::time::Duration::from_millis(600));
    }
}
