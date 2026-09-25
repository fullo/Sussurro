//! Per-language normalisation of plain text for speech (pure): everything
//! written with digits, symbols or abbreviations becomes words, in Italian
//! or English. The passes run in a fixed order, most specific first, so a
//! later pass never sees what an earlier one already turned into words:
//!
//! | pass | examples (EN / IT) |
//! |---|---|
//! | characters | emoji and zero-width characters dropped, curly quotes straightened, `…` → `...` |
//! | URLs, emails | `https://www.example.com/a` → "example dot com"; `anna@x.it` → "anna chiocciola x punto it" |
//! | abbreviations | `Dr.`, `Mr.`, `etc.`, `e.g.`, `vs.`, `No. 5` / `Sig.ra`, `Dott.ssa`, `ecc.`, `S.p.A.`, `n. 5` |
//! | initials | `U.S.A.` → "U S A", `John F. Kennedy` |
//! | currencies | `$1,248.50` → "one thousand two hundred forty-eight dollars and fifty cents"; `1.248,50 €` → "milleduecentoquarantotto euro e cinquanta centesimi"; `€2,5 mln` |
//! | dates | `12/31/2026`, `2026-12-31`, `March 3rd, 2025`, `24 September` / `31/12/2026`, `1° marzo`, `il 31/12` |
//! | times | `7:45 a.m.`, `6 p.m.`, `18:30` / `alle 7:45`, `alle 7.45` |
//! | phone numbers | `+44 20 7946 0958` → digit groups; `02 8394 1170`; after "call"/"numero" |
//! | signs, percentages | `-5` → "minus five"; `22%`, `10-20%` |
//! | units | `5 km`, `2,5 kg`, `20 °C`, `24h`, `100 GB`, `3 mln` |
//! | decades | `1990s` → "nineteen nineties", `'90s` / `anni '90` |
//! | ordinals | `3rd`, `19th` / `1°`, `2ª`; Roman numerals before `secolo`/`century` and after a name (`Luigi XIV`, `Henry VIII`) |
//! | flight numbers, codes | `BA 2490` → "B A two four nine zero"; `X4K9Q2` → "X four K nine Q two"; `MP3`, `H2O` |
//! | dimensions, ranges, fractions | `3x4`; `1861-1865` → "… to …" / "da … a …"; `3/4`, `24/7` |
//! | symbols | `&`, `#5`, `~5`, `+`, `=`, `@name`, leftover `€ $ £ % °` |
//! | numbers | everything left: `1.248` (IT) / `1,248` (EN), decimals, English years (`1861` → "eighteen sixty-one") |
//! | punctuation | `;` → `,`, dashes and brackets → commas, stray spaces |
//!
//! Replacements at the start of a sentence are capitalised, so the chunker
//! still sees where sentences begin. `Lang::Other` gets only the character
//! clean-up and URLs/emails reduced to their host.

use super::numbers::{
    cardinal, decimal, digits, en_plural_last, en_year, integer_str, ordinal, roman_value,
};
use super::Lang;
use regex::{Captures, Regex};
use std::sync::LazyLock;

/// Normalise one block of plain text.
pub fn normalize(text: &str, lang: Lang) -> String {
    let mut t = clean_chars(text);
    t = urls_and_emails(&t, lang);
    if lang != Lang::Other {
        t = abbreviations(&t, lang);
        t = initials(&t, lang);
        t = currency(&t, lang);
        t = numeric_dates(&t, lang);
        t = short_dates(&t, lang);
        t = month_dates(&t, lang);
        t = times(&t, lang);
        t = phones(&t, lang);
        t = signs(&t, lang);
        t = percents(&t, lang);
        t = units(&t, lang);
        t = decades(&t, lang);
        t = ordinals(&t, lang);
        t = romans(&t, lang);
        t = flight_numbers(&t, lang);
        t = dimensions(&t, lang);
        t = codes(&t, lang);
        t = ranges(&t, lang);
        t = fractions(&t, lang);
        t = symbols(&t, lang);
        t = numbers_left(&t, lang);
        t = punctuation(&t);
    }
    // A dropped emoji or link can leave a space before the full stop.
    collapse_ws(&SPACE_BEFORE_PUNCT.replace_all(&t, "$1"))
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// A number as written: digits with `.`/`,` separators.
const NUM: &str = r"\d+(?:[.,]\d+)*";

// ---- helpers ----

/// Replace the matches of `re` in `text` with what `f(captures, before,
/// after)` returns: `None` keeps the match, `Some((replacement, extra))`
/// replaces it and also swallows `extra` bytes of `after`. A replacement at
/// the start of a sentence is capitalised.
fn sub_ext(
    text: &str,
    re: &Regex,
    mut f: impl FnMut(&Captures, &str, &str) -> Option<(String, usize)>,
) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut last = 0;
    for caps in re.captures_iter(text) {
        let Some(m) = caps.get(0) else { continue };
        if m.start() < last {
            continue;
        }
        let before = &text[..m.start()];
        let after = &text[m.end()..];
        if let Some((rep, extra)) = f(&caps, before, after) {
            out.push_str(&text[last..m.start()]);
            if at_sentence_start(before) {
                out.push_str(&capitalize(&rep));
            } else {
                out.push_str(&rep);
            }
            last = m.end() + extra;
        }
    }
    out.push_str(&text[last..]);
    out
}

fn sub(
    text: &str,
    re: &Regex,
    mut f: impl FnMut(&Captures, &str, &str) -> Option<String>,
) -> String {
    sub_ext(text, re, |c, b, a| f(c, b, a).map(|s| (s, 0)))
}

fn at_sentence_start(before: &str) -> bool {
    let t = before.trim_end_matches(|c: char| c.is_whitespace() || "\"'«“‘([¡¿".contains(c));
    if t.is_empty() {
        return true;
    }
    t.len() != before.len() && matches!(t.chars().last(), Some('.' | '!' | '?' | '…'))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The last whitespace-separated word of `before`, if `before` ends with
/// whitespace (i.e. the match is a separate word).
fn prev_word(before: &str) -> Option<&str> {
    if !before.ends_with(char::is_whitespace) {
        return None;
    }
    before.split_whitespace().next_back()
}

fn prev_word_lower(before: &str) -> String {
    prev_word(before)
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .unwrap_or_default()
}

/// The last `n` words of `before`, lowercased, oldest first.
fn prev_words_lower(before: &str, n: usize) -> Vec<String> {
    let mut words: Vec<String> = before
        .split_whitespace()
        .rev()
        .take(n)
        .map(|w| w.to_lowercase())
        .collect();
    words.reverse();
    words
}

fn starts_alnum(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_alphanumeric())
}

fn ends_alnum(s: &str) -> bool {
    s.chars().next_back().is_some_and(|c| c.is_alphanumeric())
}

/// `after` begins a new sentence: end of text, or whitespace then an
/// uppercase letter (maybe after an opening quote).
fn new_sentence_follows(after: &str) -> bool {
    if after.trim().is_empty() {
        return true;
    }
    if !after.starts_with(char::is_whitespace) {
        return false;
    }
    after
        .trim_start()
        .trim_start_matches(['"', '«', '“', '(', '\''])
        .chars()
        .next()
        .is_some_and(char::is_uppercase)
}

// ---- numbers as written ----

struct Num {
    int: String,
    frac: Option<String>,
    /// Italian decimal written with a dot (`2.0`).
    dot: bool,
    grouped: bool,
}

static EN_GROUPED: LazyLock<Regex> = LazyLock::new(|| re(r"^\d{1,3}(?:,\d{3})+(?:\.\d+)?$"));
static IT_GROUPED: LazyLock<Regex> = LazyLock::new(|| re(r"^\d{1,3}(?:\.\d{3})+(?:,\d+)?$"));

fn parse_num(s: &str, lang: Lang) -> Option<Num> {
    let (thousands, dec) = if lang == Lang::English {
        (',', '.')
    } else {
        ('.', ',')
    };
    let grouped = if lang == Lang::English {
        &EN_GROUPED
    } else {
        &IT_GROUPED
    };
    if grouped.is_match(s) {
        let (int, frac) = match s.split_once(dec) {
            Some((i, f)) => (i, Some(f.to_string())),
            None => (s, None),
        };
        return Some(Num {
            int: int.replace(thousands, ""),
            frac,
            dot: false,
            grouped: true,
        });
    }
    let seps: Vec<char> = s.chars().filter(|c| *c == '.' || *c == ',').collect();
    match seps.as_slice() {
        [] => Some(Num {
            int: s.to_string(),
            frac: None,
            dot: false,
            grouped: false,
        }),
        [sep] if *sep == dec || (lang == Lang::Italian && *sep == '.') => {
            let (int, frac) = s.split_once(*sep)?;
            Some(Num {
                int: int.to_string(),
                frac: Some(frac.to_string()),
                dot: *sep != dec,
                grouped: false,
            })
        }
        _ => None,
    }
}

impl Num {
    fn words(&self, lang: Lang) -> String {
        match &self.frac {
            Some(f) => decimal(&self.int, f, lang, self.dot),
            None => integer_str(&self.int, lang),
        }
    }

    fn is_one(&self) -> bool {
        self.int == "1"
            && self
                .frac
                .as_deref()
                .is_none_or(|f| f.chars().all(|c| c == '0'))
    }

    fn int_value(&self) -> Option<u64> {
        self.int.parse().ok()
    }
}

/// A number token in words; a token that is not one number (`1,2,3` in
/// English) is read piece by piece.
fn number_words(s: &str, lang: Lang) -> String {
    if let Some(n) = parse_num(s, lang) {
        return n.words(lang);
    }
    let point = if lang == Lang::Italian {
        " punto "
    } else {
        " point "
    };
    let mut out = String::new();
    let mut run = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            run.push(c);
            continue;
        }
        if !run.is_empty() {
            out.push_str(&integer_str(&run, lang));
            run.clear();
        }
        out.push_str(if c == ',' { ", " } else { point });
    }
    if !run.is_empty() {
        out.push_str(&integer_str(&run, lang));
    }
    out
}

// ---- characters ----

fn clean_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let code = c as u32;
        match c {
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{FE0F}'
            | '\u{20E3}' => {}
            '\u{00A0}' | '\u{202F}' | '\u{2007}' | '\u{2009}' => out.push(' '),
            '’' | '‘' | '`' | '´' => out.push('\''),
            '“' | '”' | '„' => out.push('"'),
            '…' => out.push_str("..."),
            '−' => out.push('-'),
            _ if (0x1F000..=0x1FAFF).contains(&code)
                || (0x2600..=0x27BF).contains(&code)
                || (0x2190..=0x21FF).contains(&code)
                || (0x2B00..=0x2BFF).contains(&code)
                || (0xE000..=0xF8FF).contains(&code) =>
            {
                out.push(' ')
            }
            _ => out.push(c),
        }
    }
    out
}

// ---- URLs and emails ----

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b"));
static URL: LazyLock<Regex> = LazyLock::new(|| re(r#"\b(?:(?:https?|ftp)://|www\.)[^\s<>"]+"#));
static BARE_DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:com|org|net|it|eu|io|dev|app|ai|uk|de|fr|es|ch|info|gov|edu)\b(?:/[^\s]*)?",
    )
});

fn spoken_dots(s: &str, lang: Lang) -> String {
    let dot = match lang {
        Lang::Italian => " punto ",
        Lang::English => " dot ",
        Lang::Other => return s.to_string(),
    };
    s.split('.')
        .filter(|p| !p.is_empty())
        .map(|p| p.replace(['-', '_'], " "))
        .collect::<Vec<_>>()
        .join(dot)
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host).to_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// Split a URL match from the punctuation that ends the sentence after it.
fn trim_url(m: &str) -> (&str, &str) {
    let core = m.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '»']);
    (core, &m[core.len()..])
}

fn urls_and_emails(s: &str, lang: Lang) -> String {
    let t = sub(s, &EMAIL, |c, _, _| {
        let (local, domain) = c[0].split_once('@')?;
        let at = match lang {
            Lang::Italian => " chiocciola ",
            Lang::English => " at ",
            Lang::Other => "@",
        };
        Some(format!(
            "{}{at}{}",
            spoken_dots(local, lang),
            spoken_dots(&domain.to_lowercase(), lang)
        ))
    });
    let t = sub(&t, &URL, |c, _, _| {
        let (core, tail) = trim_url(&c[0]);
        let host = host_of(core);
        if host.is_empty() {
            return None;
        }
        Some(format!("{}{tail}", spoken_dots(&host, lang)))
    });
    if lang == Lang::Other {
        return t;
    }
    sub(&t, &BARE_DOMAIN, |c, before, _| {
        if before.ends_with(['@', '.', '/']) {
            return None;
        }
        let (core, tail) = trim_url(&c[0]);
        Some(format!("{}{tail}", spoken_dots(&host_of(core), lang)))
    })
}

// ---- abbreviations ----

#[derive(Clone, Copy, PartialEq)]
enum AbbrKind {
    /// Before a name: never ends a sentence.
    Title,
    /// May end a sentence: the full stop is kept when a new sentence
    /// follows.
    MayEnd,
    /// Only before a number (`No. 5`, `p. 12`).
    BeforeNumber,
}
use AbbrKind::{BeforeNumber, MayEnd, Title};

const EN_ABBR: &[(&str, &str, AbbrKind)] = &[
    ("Mr.", "Mister", Title),
    ("Mrs.", "Missus", Title),
    ("Ms.", "Miz", Title),
    ("Dr.", "Doctor", Title),
    ("Prof.", "Professor", Title),
    ("Gen.", "General", Title),
    ("Gov.", "Governor", Title),
    ("Sen.", "Senator", Title),
    ("Rep.", "Representative", Title),
    ("Capt.", "Captain", Title),
    ("Lt.", "Lieutenant", Title),
    ("Col.", "Colonel", Title),
    ("Sgt.", "Sergeant", Title),
    ("Rev.", "Reverend", Title),
    ("Hon.", "Honorable", Title),
    ("Mt.", "Mount", Title),
    ("Ft.", "Fort", Title),
    ("Jr.", "Junior", MayEnd),
    ("Sr.", "Senior", MayEnd),
    ("etc.", "et cetera", MayEnd),
    ("e.g.", "for example", MayEnd),
    ("i.e.", "that is", MayEnd),
    ("a.k.a.", "also known as", MayEnd),
    ("vs.", "versus", MayEnd),
    ("vs", "versus", MayEnd),
    ("approx.", "approximately", MayEnd),
    ("Inc.", "Incorporated", MayEnd),
    ("Ltd.", "Limited", MayEnd),
    ("Corp.", "Corporation", MayEnd),
    ("Ave.", "Avenue", MayEnd),
    ("Blvd.", "Boulevard", MayEnd),
    ("Rd.", "Road", MayEnd),
    ("Dept.", "Department", MayEnd),
    ("w/o", "without", Title),
    ("w/", "with", Title),
    ("No.", "number", BeforeNumber),
    ("Nos.", "numbers", BeforeNumber),
    ("Fig.", "figure", BeforeNumber),
    ("p.", "page", BeforeNumber),
    ("pp.", "pages", BeforeNumber),
    ("ch.", "chapter", BeforeNumber),
    ("vol.", "volume", BeforeNumber),
];

const IT_ABBR: &[(&str, &str, AbbrKind)] = &[
    ("Sig.ra", "signora", Title),
    ("Sig.na", "signorina", Title),
    ("Sigg.", "signori", Title),
    ("Sig.", "signor", Title),
    ("Dott.ssa", "dottoressa", Title),
    ("Dott.", "dottor", Title),
    ("Dr.ssa", "dottoressa", Title),
    ("Dr.", "dottor", Title),
    ("Prof.ssa", "professoressa", Title),
    ("Prof.", "professor", Title),
    ("Ing.", "ingegner", Title),
    ("Avv.", "avvocato", Title),
    ("Arch.", "architetto", Title),
    ("Geom.", "geometra", Title),
    ("Rag.", "ragionier", Title),
    ("On.", "onorevole", Title),
    ("Egr.", "egregio", Title),
    ("Gent.mo", "gentilissimo", Title),
    ("Gent.ma", "gentilissima", Title),
    ("Gent.mi", "gentilissimi", Title),
    ("Gent.", "gentile", Title),
    ("Spett.le", "spettabile", Title),
    ("Mons.", "monsignor", Title),
    ("c/o", "presso", Title),
    ("ecc.", "eccetera", MayEnd),
    ("etc.", "eccetera", MayEnd),
    ("p.es.", "per esempio", MayEnd),
    ("es.", "per esempio", MayEnd),
    ("cfr.", "confronta", MayEnd),
    ("ca.", "circa", MayEnd),
    ("S.p.A.", "esse pi a", MayEnd),
    ("SpA", "esse pi a", MayEnd),
    ("S.r.l.", "esse erre elle", MayEnd),
    ("s.r.l.", "esse erre elle", MayEnd),
    ("Srl", "esse erre elle", MayEnd),
    ("d.C.", "dopo Cristo", MayEnd),
    ("a.C.", "avanti Cristo", MayEnd),
    ("N.B.", "nota bene", MayEnd),
    ("P.S.", "post scriptum", MayEnd),
    ("vs.", "contro", MayEnd),
    ("vs", "contro", MayEnd),
    ("tel.", "telefono", MayEnd),
    ("cell.", "cellulare", MayEnd),
    ("fig.", "figura", BeforeNumber),
    ("pag.", "pagina", BeforeNumber),
    ("pagg.", "pagine", BeforeNumber),
    ("art.", "articolo", BeforeNumber),
    ("artt.", "articoli", BeforeNumber),
    ("cap.", "capitolo", BeforeNumber),
    ("vol.", "volume", BeforeNumber),
    ("nr.", "numero", BeforeNumber),
    ("n°", "numero", BeforeNumber),
    ("n.", "numero", BeforeNumber),
];

/// One entry per written form: the table's own and the one with its first
/// letter's case flipped (`ecc.`/`Ecc.`), longest first.
fn abbr_table(lang: Lang) -> &'static [(String, &'static str, AbbrKind)] {
    static EN: LazyLock<Vec<(String, &'static str, AbbrKind)>> =
        LazyLock::new(|| expand_abbr(EN_ABBR));
    static IT: LazyLock<Vec<(String, &'static str, AbbrKind)>> =
        LazyLock::new(|| expand_abbr(IT_ABBR));
    if lang == Lang::Italian {
        &IT
    } else {
        &EN
    }
}

fn expand_abbr(table: &[(&str, &'static str, AbbrKind)]) -> Vec<(String, &'static str, AbbrKind)> {
    let mut out: Vec<(String, &'static str, AbbrKind)> = Vec::new();
    for (key, rep, kind) in table {
        let mut chars = key.chars();
        let first = chars.next().unwrap_or(' ');
        let flipped: String = if first.is_uppercase() {
            first.to_lowercase().chain(chars).collect()
        } else {
            first.to_uppercase().chain(chars).collect()
        };
        for form in [key.to_string(), flipped] {
            if !out.iter().any(|(k, _, _)| *k == form) {
                out.push((form, rep, *kind));
            }
        }
    }
    out.sort_by_key(|(k, _, _)| std::cmp::Reverse(k.len()));
    out
}

fn abbr_regex(lang: Lang) -> &'static Regex {
    static EN: LazyLock<Regex> = LazyLock::new(|| build_abbr_regex(Lang::English));
    static IT: LazyLock<Regex> = LazyLock::new(|| build_abbr_regex(Lang::Italian));
    if lang == Lang::Italian {
        &IT
    } else {
        &EN
    }
}

fn build_abbr_regex(lang: Lang) -> Regex {
    let alts: Vec<String> = abbr_table(lang)
        .iter()
        .map(|(k, _, _)| regex::escape(k))
        .collect();
    re(&format!("(?:{})", alts.join("|")))
}

static BEFORE_NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"^\s?\d"));

fn abbreviations(s: &str, lang: Lang) -> String {
    let table = abbr_table(lang);
    sub(s, abbr_regex(lang), |c, before, after| {
        let form = &c[0];
        // A separate word: not glued to letters on either side.
        if ends_alnum(before) || (ends_alnum(form) && starts_alnum(after)) {
            return None;
        }
        let (_, rep, kind) = table.iter().find(|(k, _, _)| k == form)?;
        match kind {
            Title => {
                // `St.`-like ambiguity is avoided by only listing titles
                // that precede a name.
                Some(format!(
                    "{rep}{}",
                    if form.ends_with('/') { " " } else { "" }
                ))
            }
            MayEnd => {
                let stop = form.ends_with('.') && new_sentence_follows(after);
                Some(format!("{rep}{}", if stop { "." } else { "" }))
            }
            BeforeNumber => BEFORE_NUMBER.is_match(after).then(|| format!("{rep} ")),
        }
    })
}

static INITIALISM: LazyLock<Regex> = LazyLock::new(|| re(r"\b(?:\p{Lu}\.){2,}"));
static INITIAL: LazyLock<Regex> = LazyLock::new(|| re(r"\b\p{Lu}\.\s"));
static ST: LazyLock<Regex> = LazyLock::new(|| re(r"\bSt\."));
static INITIAL_AHEAD: LazyLock<Regex> = LazyLock::new(|| re(r"^\p{Lu}\.\s"));

fn initials(s: &str, lang: Lang) -> String {
    let t = sub(s, &INITIALISM, |c, before, after| {
        if ends_alnum(before) || starts_alnum(after) {
            return None;
        }
        let letters: Vec<String> = c[0]
            .chars()
            .filter(|ch| *ch != '.')
            .map(String::from)
            .collect();
        let stop = if after.trim().is_empty() { "." } else { "" };
        Some(format!("{}{stop}", letters.join(" ")))
    });
    // English only: `St. Paul` is a saint, `Main St.` a street.
    let t = if lang != Lang::English {
        t
    } else {
        sub(&t, &ST, |_, _, after| {
            let next_cap = after
                .trim_start()
                .chars()
                .next()
                .is_some_and(char::is_uppercase);
            Some(if next_cap && after.starts_with(' ') {
                "Saint".to_string()
            } else if new_sentence_follows(after) {
                "Street.".to_string()
            } else {
                "Street".to_string()
            })
        })
    };
    // `John F. Kennedy`, `J. R. R. Tolkien`: an initial before a
    // capitalised word, after a capitalised word that doesn't start the
    // sentence (`Plan B. Then` keeps its full stop), or in a chain of
    // initials.
    sub(&t, &INITIAL, |c, before, after| {
        let next_cap = after.chars().next().is_some_and(char::is_uppercase);
        let chain = INITIAL_AHEAD.is_match(after);
        let prev_ok = prev_word(before).is_some_and(|w| {
            let is_initial = w.len() == 2 && w.ends_with('.');
            let capitalised = w.chars().next().is_some_and(char::is_uppercase)
                && w.trim_end_matches('.').chars().all(char::is_alphabetic);
            let stem = before.trim_end();
            let start = stem.rfind(char::is_whitespace).map_or(0, |i| i + 1);
            is_initial || (capitalised && !at_sentence_start(&stem[..start]))
        });
        (next_cap && (prev_ok || chain)).then(|| format!("{} ", &c[0][..c[0].len() - 2]))
    })
}

// ---- currencies ----

#[derive(Clone, Copy, PartialEq)]
enum Cur {
    Dollar,
    Euro,
    Pound,
    Yen,
    Franc,
}

fn cur_from(s: &str) -> Option<Cur> {
    Some(match s.to_lowercase().as_str() {
        "$" | "usd" | "dollar" | "dollars" | "dollaro" | "dollari" => Cur::Dollar,
        "€" | "eur" | "euro" | "euros" => Cur::Euro,
        "£" | "gbp" | "sterlina" | "sterline" => Cur::Pound,
        "¥" | "jpy" | "yen" => Cur::Yen,
        "chf" | "franc" | "francs" | "franco" | "franchi" => Cur::Franc,
        _ => return None,
    })
}

struct CurNames {
    one: &'static str,
    many: &'static str,
    sub: Option<(&'static str, &'static str)>,
    feminine: bool,
}

fn cur_names(cur: Cur, lang: Lang) -> CurNames {
    let n = |one, many, sub, feminine| CurNames {
        one,
        many,
        sub,
        feminine,
    };
    if lang == Lang::Italian {
        match cur {
            Cur::Dollar => n(
                "dollaro",
                "dollari",
                Some(("centesimo", "centesimi")),
                false,
            ),
            Cur::Euro => n("euro", "euro", Some(("centesimo", "centesimi")), false),
            Cur::Pound => n("sterlina", "sterline", Some(("penny", "pence")), true),
            Cur::Yen => n("yen", "yen", None, false),
            Cur::Franc => n(
                "franco svizzero",
                "franchi svizzeri",
                Some(("centesimo", "centesimi")),
                false,
            ),
        }
    } else {
        match cur {
            Cur::Dollar => n("dollar", "dollars", Some(("cent", "cents")), false),
            Cur::Euro => n("euro", "euros", Some(("cent", "cents")), false),
            Cur::Pound => n("pound", "pounds", Some(("penny", "pence")), false),
            Cur::Yen => n("yen", "yen", None, false),
            Cur::Franc => n(
                "Swiss franc",
                "Swiss francs",
                Some(("centime", "centimes")),
                false,
            ),
        }
    }
}

#[derive(Clone, Copy)]
enum Scale {
    Thousand,
    Million,
    Billion,
    Trillion,
}

static SCALE_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^\s?(million|billion|trillion|thousand|milioni|milione|miliardi|miliardo|mln|mld|mila|bn|mn|m|k|M|B|K)\b",
    )
});

fn scale_from(s: &str) -> Option<Scale> {
    Some(match s {
        "thousand" | "mila" | "k" | "K" => Scale::Thousand,
        "million" | "milioni" | "milione" | "mln" | "mn" | "m" | "M" => Scale::Million,
        "billion" | "miliardi" | "miliardo" | "mld" | "bn" | "B" => Scale::Billion,
        "trillion" => Scale::Trillion,
        _ => return None,
    })
}

/// A scale word following a money amount, and how many bytes it takes.
fn scale_after(after: &str) -> Option<(Scale, usize)> {
    let c = SCALE_AFTER.captures(after)?;
    Some((scale_from(&c[1])?, c[0].len()))
}

fn money(num: &Num, cur: Cur, scale: Option<Scale>, lang: Lang) -> String {
    let names = cur_names(cur, lang);
    let it = lang == Lang::Italian;
    let one_word = |fem: bool| {
        if !it {
            "one"
        } else if fem {
            "una"
        } else {
            "un"
        }
    };
    if let Some(scale) = scale {
        if let (Scale::Thousand, None, Some(v)) = (scale, &num.frac, num.int_value()) {
            let n = Num {
                int: (v * 1000).to_string(),
                frac: None,
                dot: false,
                grouped: false,
            };
            return money(&n, cur, None, lang);
        }
        let (one, many) = match (scale, it) {
            (Scale::Million, true) => ("milione", "milioni"),
            (Scale::Billion, true) => ("miliardo", "miliardi"),
            (Scale::Trillion, true) => ("bilione", "bilioni"),
            (Scale::Thousand, true) => ("mila", "mila"),
            (Scale::Million, false) => ("million", "million"),
            (Scale::Billion, false) => ("billion", "billion"),
            (Scale::Trillion, false) => ("trillion", "trillion"),
            (Scale::Thousand, false) => ("thousand", "thousand"),
        };
        let amount = if num.is_one() {
            format!("{} {one}", one_word(false))
        } else {
            format!("{} {many}", num.words(lang))
        };
        return if it {
            format!("{amount} di {}", names.many)
        } else {
            format!("{amount} {}", names.many)
        };
    }
    let cents = match (&num.frac, names.sub) {
        (None, _) => Some(0),
        (Some(f), _) if f.chars().all(|c| c == '0') => Some(0),
        (Some(f), Some(_)) if f.len() <= 2 => {
            let padded = format!("{f:0<2}");
            padded.parse::<u64>().ok()
        }
        _ => None,
    };
    let Some(cents) = cents else {
        return format!("{} {}", num.words(lang), names.many);
    };
    let units = num.int_value().unwrap_or(0);
    let units_words = match units {
        1 => format!("{} {}", one_word(names.feminine), names.one),
        _ => format!("{} {}", integer_str(&num.int, lang), names.many),
    };
    let Some((sub_one, sub_many)) = names.sub.filter(|_| cents > 0) else {
        return units_words;
    };
    let cents_words = match cents {
        1 => format!("{} {sub_one}", one_word(false)),
        c => format!("{} {sub_many}", cardinal(c, lang)),
    };
    if units == 0 {
        cents_words
    } else {
        let and = if it { "e" } else { "and" };
        format!("{units_words} {and} {cents_words}")
    }
}

static CUR_PREFIX: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"(?:US)?([$€£¥])\s?({NUM})")));
static CUR_PREFIX_CODE: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"\b(USD|EUR|GBP|CHF|JPY)\s?({NUM})")));
static CUR_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"({NUM})\s?([$€£¥]|USD\b|EUR\b|GBP\b|CHF\b|JPY\b)"
    ))
});
static CUR_SUFFIX_WORD: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"({NUM})\s(?i:(euros?|dollars?|dollari|dollaro|sterline|sterlina|yen|franchi|franco|francs?))\b"
    ))
});

fn currency(s: &str, lang: Lang) -> String {
    let prefix = |c: &Captures, before: &str, after: &str| {
        if ends_alnum(before) {
            return None;
        }
        let cur = cur_from(&c[1])?;
        let num = parse_num(&c[2], lang)?;
        match scale_after(after) {
            Some((scale, len)) => Some((money(&num, cur, Some(scale), lang), len)),
            None if starts_alnum(after) => None,
            None => Some((money(&num, cur, None, lang), 0)),
        }
    };
    let t = sub_ext(s, &CUR_PREFIX, prefix);
    let t = sub_ext(&t, &CUR_PREFIX_CODE, prefix);
    let suffix = |c: &Captures, before: &str, after: &str| {
        if ends_alnum(before) || starts_alnum(after) {
            return None;
        }
        let cur = cur_from(&c[2])?;
        let num = parse_num(&c[1], lang)?;
        Some(money(&num, cur, None, lang))
    };
    let t = sub(&t, &CUR_SUFFIX, suffix);
    sub(&t, &CUR_SUFFIX_WORD, suffix)
}

// ---- dates ----

const EN_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const IT_MONTHS: [&str; 12] = [
    "gennaio",
    "febbraio",
    "marzo",
    "aprile",
    "maggio",
    "giugno",
    "luglio",
    "agosto",
    "settembre",
    "ottobre",
    "novembre",
    "dicembre",
];

fn en_month_index(s: &str) -> Option<usize> {
    const ABBR: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let s = s.trim_end_matches('.');
    if s == "Sept" {
        return Some(8);
    }
    EN_MONTHS
        .iter()
        .position(|m| *m == s)
        .or_else(|| ABBR.iter().position(|a| *a == s))
}

fn it_month_index(s: &str) -> Option<usize> {
    let s = s.to_lowercase();
    IT_MONTHS.iter().position(|m| *m == s)
}

/// `(day, month 1–12, year)` in words.
fn date_words(day: u64, month: usize, year: Option<u64>, lang: Lang) -> String {
    if lang == Lang::Italian {
        let d = if day == 1 {
            "primo".to_string()
        } else {
            cardinal(day, lang)
        };
        let m = IT_MONTHS[month - 1];
        match year {
            Some(y) => format!("{d} {m} {}", cardinal(y, lang)),
            None => format!("{d} {m}"),
        }
    } else {
        let m = EN_MONTHS[month - 1];
        let d = ordinal(day, lang, false);
        match year {
            Some(y) => format!("{m} {d}, {}", en_year(y)),
            None => format!("{m} {d}"),
        }
    }
}

fn full_year(y: &str) -> Option<u64> {
    let v: u64 = y.parse().ok()?;
    Some(match y.len() {
        2 if v <= 69 => 2000 + v,
        2 => 1900 + v,
        4 => v,
        _ => return None,
    })
}

static NUMERIC_DATE: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b(\d{1,4})([/.\-])(\d{1,2})([/.\-])(\d{2,4})\b"));

fn numeric_dates(s: &str, lang: Lang) -> String {
    sub(s, &NUMERIC_DATE, |c, before, after| {
        if c[2] != c[4] || ends_alnum(before) || before.ends_with(['.', ',', '/', '-']) {
            return None;
        }
        if after.starts_with(['/', '.', '-', ','])
            && after[1..].starts_with(|ch: char| ch.is_ascii_digit())
        {
            return None;
        }
        let (a, b, y) = (&c[1], &c[3], &c[5]);
        let (day, month, year) = if a.len() == 4 {
            if y.len() > 2 {
                return None;
            }
            (y.parse().ok()?, b.parse().ok()?, a.parse().ok()?)
        } else {
            if a.len() > 2 {
                return None;
            }
            let (a, b): (u64, u64) = (a.parse().ok()?, b.parse().ok()?);
            let year = full_year(y)?;
            // English: month first unless the first number can't be one.
            if lang == Lang::English && a <= 12 {
                (b, a, year)
            } else {
                (a, b, year)
            }
        };
        let month = usize::try_from(month).ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        Some(date_words(day, month, Some(year), lang))
    })
}

static SHORT_DATE: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{1,2})/(\d{1,2})\b"));

/// `31/12` without a year, only after a word that introduces a date.
fn short_dates(s: &str, lang: Lang) -> String {
    const IT_WORDS: &[&str] = &[
        "il", "al", "dal", "del", "nel", "entro", "fino", "dall'", "all'", "dell'", "nell'", "l'",
        "scadenza", "giorno", "data",
    ];
    const EN_WORDS: &[&str] = &[
        "on", "by", "from", "until", "till", "since", "due", "before", "after", "through", "date",
        "deadline",
    ];
    sub(s, &SHORT_DATE, |c, before, after| {
        if after.starts_with('/') {
            return None;
        }
        let word = before
            .rsplit(|ch: char| ch.is_whitespace())
            .find(|w| !w.is_empty())
            .unwrap_or("")
            .to_lowercase();
        let elided = before.ends_with('\'');
        let words = if lang == Lang::Italian {
            IT_WORDS
        } else {
            EN_WORDS
        };
        if !(words.contains(&word.as_str()) || (elided && lang == Lang::Italian)) {
            return None;
        }
        let (a, b): (u64, u64) = (c[1].parse().ok()?, c[2].parse().ok()?);
        let (day, month) = if lang == Lang::English && a <= 12 {
            (b, a)
        } else {
            (a, b)
        };
        let month = usize::try_from(month).ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        Some(date_words(day, month, None, lang))
    })
}

static EN_MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\b(January|February|March|April|May|June|July|August|September|October|November|December|Jan\.?|Feb\.?|Mar\.?|Apr\.?|Jun\.?|Jul\.?|Aug\.?|Sept?\.?|Oct\.?|Nov\.?|Dec\.?)\s+(\d{1,2})(?:st|nd|rd|th)?\b",
    )
});
static EN_DAY_MONTH: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\b(\d{1,2})(?:st|nd|rd|th)?\s+(?:of\s+)?(January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sept?|Oct|Nov|Dec)\b",
    )
});
static IT_DAY_MONTH: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\b(\d{1,2})\s?[°º]?\s+((?i:gennaio|febbraio|marzo|aprile|maggio|giugno|luglio|agosto|settembre|ottobre|novembre|dicembre))\b",
    )
});

/// A day number followed by something that makes it not a day
/// (`March 3:45`, `May 5%`, `June 12.5`).
fn not_a_day(after: &str) -> bool {
    after.starts_with([':', '%', '/'])
        || (after.starts_with(['.', ',']) && after[1..].starts_with(|c: char| c.is_ascii_digit()))
        || starts_alnum(after)
}

fn month_dates(s: &str, lang: Lang) -> String {
    if lang == Lang::Italian {
        return sub(s, &IT_DAY_MONTH, |c, _, _| {
            let day: u64 = c[1].parse().ok()?;
            it_month_index(&c[2])?;
            if !(1..=31).contains(&day) {
                return None;
            }
            let d = if day == 1 {
                "primo".to_string()
            } else {
                cardinal(day, lang)
            };
            Some(format!("{d} {}", &c[2]))
        });
    }
    let t = sub(s, &EN_MONTH_DAY, |c, _, after| {
        let day: u64 = c[2].parse().ok()?;
        if !(1..=31).contains(&day) || not_a_day(after) {
            return None;
        }
        let month = en_month_index(&c[1])?;
        Some(format!(
            "{} {}",
            EN_MONTHS[month],
            ordinal(day, lang, false)
        ))
    });
    sub(&t, &EN_DAY_MONTH, |c, before, _| {
        let day: u64 = c[1].parse().ok()?;
        if !(1..=31).contains(&day) {
            return None;
        }
        let month = en_month_index(&c[2])?;
        let the = if prev_word_lower(before) == "the" {
            ""
        } else {
            "the "
        };
        Some(format!(
            "{the}{} of {}",
            ordinal(day, lang, false),
            EN_MONTHS[month]
        ))
    })
}

// ---- times ----

static TIME: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{1,2}):(\d{2})(?::(\d{2}))?\b"));
static IT_DOTTED_TIME: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(alle|dalle|le|ore|verso le|entro le|fino alle|tra le|e le|delle)\s+(\d{1,2})\.(\d{2})\b",
    )
});
static BARE_HOUR: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{1,2})\b"));
static AMPM: LazyLock<Regex> = LazyLock::new(|| re(r"^\s?([AaPp])\.?\s?[Mm]\.?"));

/// `a.m.`/`p.m.` right after a time: its words and the bytes it takes (plus
/// a full stop when it also ends the sentence).
fn ampm_after(after: &str) -> Option<(&'static str, usize, bool)> {
    let c = AMPM.captures(after)?;
    let len = c[0].len();
    let rest = &after[len..];
    if starts_alnum(rest) {
        return None;
    }
    let word = if c[1].eq_ignore_ascii_case("a") {
        "ay em"
    } else {
        "pee em"
    };
    let stop = c[0].ends_with('.') && new_sentence_follows(rest);
    Some((word, len, stop))
}

fn time_words(h: u64, m: u64, sec: Option<u64>, lang: Lang, ampm: bool) -> String {
    let hour = cardinal(h, lang);
    let base = if lang == Lang::Italian {
        if m == 0 {
            hour
        } else {
            format!("{hour} e {}", cardinal(m, lang))
        }
    } else if m == 0 {
        if ampm {
            hour
        } else if h <= 12 {
            format!("{hour} o'clock")
        } else {
            format!("{hour} hundred")
        }
    } else if m < 10 {
        format!("{hour} oh {}", cardinal(m, lang))
    } else {
        format!("{hour} {}", cardinal(m, lang))
    };
    match (sec, lang) {
        (Some(s), Lang::Italian) => format!("{base} e {} secondi", cardinal(s, lang)),
        (Some(s), _) => format!("{base} and {} seconds", cardinal(s, lang)),
        (None, _) => base,
    }
}

fn times(s: &str, lang: Lang) -> String {
    let t = if lang == Lang::Italian {
        sub(s, &IT_DOTTED_TIME, |c, _, after| {
            let (h, m): (u64, u64) = (c[2].parse().ok()?, c[3].parse().ok()?);
            if h > 24 || m > 59 || after.starts_with(|ch: char| ch.is_ascii_digit()) {
                return None;
            }
            Some(format!("{} {}", &c[1], time_words(h, m, None, lang, false)))
        })
    } else {
        s.to_string()
    };
    let t = sub_ext(&t, &TIME, |c, before, after| {
        if before.ends_with(':') || after.starts_with(':') {
            return None;
        }
        let (h, m): (u64, u64) = (c[1].parse().ok()?, c[2].parse().ok()?);
        let sec: Option<u64> = c.get(3).and_then(|g| g.as_str().parse().ok());
        if h > 24 || m > 59 || sec.is_some_and(|x| x > 59) {
            return None;
        }
        let ampm = (lang == Lang::English).then(|| ampm_after(after)).flatten();
        let words = time_words(h, m, sec, lang, ampm.is_some());
        Some(match ampm {
            Some((w, len, stop)) => (format!("{words} {w}{}", if stop { "." } else { "" }), len),
            None => (words, 0),
        })
    });
    if lang != Lang::English {
        return t;
    }
    sub_ext(&t, &BARE_HOUR, |c, before, after| {
        if before.ends_with(['.', ',', ':']) && ends_alnum(before.trim_end_matches(['.', ',', ':']))
        {
            return None;
        }
        let h: u64 = c[1].parse().ok()?;
        if !(1..=12).contains(&h) {
            return None;
        }
        let (w, len, stop) = ampm_after(after)?;
        Some((
            format!("{} {w}{}", cardinal(h, lang), if stop { "." } else { "" }),
            len,
        ))
    })
}

// ---- phone numbers ----

static PHONE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:\+\d{1,3}|\b\d{2,5})(?:[ .\-]\d{2,5}){1,5}\b"));

fn phones(s: &str, lang: Lang) -> String {
    const EN_WORDS: &[&str] = &[
        "call",
        "phone",
        "tel",
        "telephone",
        "number",
        "fax",
        "mobile",
        "dial",
        "text",
        "whatsapp",
        "at",
    ];
    const IT_WORDS: &[&str] = &[
        "numero",
        "telefono",
        "tel",
        "chiama",
        "chiamate",
        "chiamare",
        "cellulare",
        "cell",
        "fax",
        "whatsapp",
        "contattare",
        "contatta",
    ];
    sub(s, &PHONE, |c, before, after| {
        let m = &c[0];
        if ends_alnum(before) || after.starts_with(|ch: char| ch.is_ascii_digit()) {
            return None;
        }
        let groups: Vec<&str> = m.split([' ', '.', '-']).filter(|g| !g.is_empty()).collect();
        let total: usize = groups
            .iter()
            .map(|g| g.chars().filter(char::is_ascii_digit).count())
            .sum();
        let words = if lang == Lang::Italian {
            IT_WORDS
        } else {
            EN_WORDS
        };
        let keyword = words.contains(&prev_word_lower(before).as_str());
        let ok = if m.starts_with('+') {
            total >= 7
        } else if m.starts_with('0') {
            total >= 6 && groups.len() >= 2
        } else {
            keyword && total >= 7 && groups.len() >= 2
        };
        if !ok {
            return None;
        }
        let plus = if lang == Lang::Italian {
            "più"
        } else {
            "plus"
        };
        let spoken: Vec<String> = groups
            .iter()
            .map(|g| match g.strip_prefix('+') {
                Some(rest) => format!("{plus} {}", digits(rest, lang)),
                None => digits(g, lang),
            })
            .collect();
        Some(spoken.join(", "))
    })
}

// ---- signs, percentages ----

static SIGN: LazyLock<Regex> = LazyLock::new(|| re(r"-(\d)"));

fn signs(s: &str, lang: Lang) -> String {
    let minus = if lang == Lang::Italian {
        "meno"
    } else {
        "minus"
    };
    sub(s, &SIGN, |c, before, _| {
        let ok = before.is_empty() || before.ends_with([' ', '(', '[', '\t']);
        ok.then(|| format!("{minus} {}", &c[1]))
    })
}

static PERCENT: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"({NUM})(?:\s?[-–]\s?({NUM}))?\s?%")));

fn percents(s: &str, lang: Lang) -> String {
    let word = if lang == Lang::Italian {
        "per cento"
    } else {
        "percent"
    };
    sub(s, &PERCENT, |c, before, _| {
        if ends_alnum(before) {
            return None;
        }
        let a = number_words(&c[1], lang);
        let tra_il = matches!(
            prev_words_lower(before, 2).as_slice(),
            [x, y] if (x == "tra" || x == "fra") && y == "il"
        );
        Some(match c.get(2) {
            // "tra il 10-20%" → "tra il dieci e il venti per cento".
            Some(b) if lang == Lang::Italian && tra_il => {
                format!("{a} e il {} {word}", number_words(b.as_str(), lang))
            }
            Some(b) if lang == Lang::Italian => {
                format!("da {a} a {} {word}", number_words(b.as_str(), lang))
            }
            Some(b) => format!("{a} to {} {word}", number_words(b.as_str(), lang)),
            None => format!("{a} {word}"),
        })
    })
}

// ---- units ----

/// `(symbol, English one, English many, Italian one, Italian many)`; an
/// empty form means the unit is not read in that language.
#[rustfmt::skip]
const UNITS: &[(&str, &str, &str, &str, &str)] = &[
    ("km/h", "kilometer per hour", "kilometers per hour", "un chilometro orario", "chilometri orari"),
    ("km²", "square kilometer", "square kilometers", "un chilometro quadrato", "chilometri quadrati"),
    ("m²", "square meter", "square meters", "un metro quadrato", "metri quadrati"),
    ("m2", "square meter", "square meters", "un metro quadrato", "metri quadrati"),
    ("mq", "", "", "un metro quadrato", "metri quadrati"),
    ("m³", "cubic meter", "cubic meters", "un metro cubo", "metri cubi"),
    ("km", "kilometer", "kilometers", "un chilometro", "chilometri"),
    ("cm", "centimeter", "centimeters", "un centimetro", "centimetri"),
    ("mm", "millimeter", "millimeters", "un millimetro", "millimetri"),
    ("m", "meter", "meters", "un metro", "metri"),
    ("kg", "kilogram", "kilograms", "un chilogrammo", "chilogrammi"),
    ("mg", "milligram", "milligrams", "un milligrammo", "milligrammi"),
    ("g", "gram", "grams", "un grammo", "grammi"),
    ("ml", "milliliter", "milliliters", "un millilitro", "millilitri"),
    ("cl", "centiliter", "centiliters", "un centilitro", "centilitri"),
    ("l", "liter", "liters", "un litro", "litri"),
    ("L", "liter", "liters", "un litro", "litri"),
    ("kWh", "kilowatt hour", "kilowatt hours", "un chilowattora", "chilowattora"),
    ("MWh", "megawatt hour", "megawatt hours", "un megawattora", "megawattora"),
    ("kW", "kilowatt", "kilowatts", "un chilowatt", "chilowatt"),
    ("MW", "megawatt", "megawatts", "un megawatt", "megawatt"),
    ("W", "watt", "watts", "un watt", "watt"),
    ("V", "volt", "volts", "un volt", "volt"),
    ("mAh", "milliamp hour", "milliamp hours", "un milliampere ora", "milliampere ora"),
    ("GHz", "gigahertz", "gigahertz", "un gigahertz", "gigahertz"),
    ("MHz", "megahertz", "megahertz", "un megahertz", "megahertz"),
    ("kHz", "kilohertz", "kilohertz", "un kilohertz", "kilohertz"),
    ("Hz", "hertz", "hertz", "un hertz", "hertz"),
    ("TB", "terabyte", "terabytes", "un terabyte", "terabyte"),
    ("GB", "gigabyte", "gigabytes", "un gigabyte", "gigabyte"),
    ("MB", "megabyte", "megabytes", "un megabyte", "megabyte"),
    ("KB", "kilobyte", "kilobytes", "un kilobyte", "kilobyte"),
    ("kB", "kilobyte", "kilobytes", "un kilobyte", "kilobyte"),
    ("Gbps", "gigabit per second", "gigabits per second", "un gigabit al secondo", "gigabit al secondo"),
    ("Mbps", "megabit per second", "megabits per second", "un megabit al secondo", "megabit al secondo"),
    ("ms", "millisecond", "milliseconds", "un millisecondo", "millisecondi"),
    ("min", "minute", "minutes", "un minuto", "minuti"),
    ("sec", "second", "seconds", "un secondo", "secondi"),
    ("h", "hour", "hours", "un'ora", "ore"),
    ("°C", "degree Celsius", "degrees Celsius", "un grado Celsius", "gradi Celsius"),
    ("°F", "degree Fahrenheit", "degrees Fahrenheit", "un grado Fahrenheit", "gradi Fahrenheit"),
    ("°", "degree", "degrees", "", ""),
    ("mph", "mile per hour", "miles per hour", "un miglio orario", "miglia orarie"),
    ("px", "pixel", "pixels", "un pixel", "pixel"),
    ("ft", "foot", "feet", "", ""),
    ("lbs", "pound", "pounds", "", ""),
    ("lb", "pound", "pounds", "", ""),
    ("oz", "ounce", "ounces", "", ""),
    ("bn", "billion", "billion", "", ""),
    ("mln", "", "", "un milione", "milioni"),
    ("mld", "", "", "un miliardo", "miliardi"),
];

static UNIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    let mut keys: Vec<&str> = UNITS.iter().map(|u| u.0).collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.chars().count()));
    let alts: Vec<String> = keys.iter().map(|k| regex::escape(k)).collect();
    re(&format!(r"({NUM})\s?({})", alts.join("|")))
});

fn units(s: &str, lang: Lang) -> String {
    sub(s, &UNIT_RE, |c, before, after| {
        if ends_alnum(before) || starts_alnum(after) || after.starts_with(['\'', '²', '³']) {
            return None;
        }
        let unit = UNITS.iter().find(|u| u.0 == &c[2])?;
        let (one, many) = if lang == Lang::Italian {
            (unit.3, unit.4)
        } else {
            (unit.1, unit.2)
        };
        if many.is_empty() {
            return None;
        }
        let num = parse_num(&c[1], lang)?;
        Some(if num.is_one() && num.frac.is_none() {
            if lang == Lang::Italian {
                one.to_string()
            } else {
                format!("one {one}")
            }
        } else {
            format!("{} {many}", num.words(lang))
        })
    })
}

// ---- decades, ordinals, Roman numerals ----

static EN_DECADE: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{3}0)s\b"));
static SHORT_DECADE: LazyLock<Regex> = LazyLock::new(|| re(r"'(\d0)s\b"));
static SHORT_YEAR: LazyLock<Regex> = LazyLock::new(|| re(r"'(\d{2})\b"));

fn decades(s: &str, lang: Lang) -> String {
    let mut t = s.to_string();
    if lang == Lang::English {
        t = sub(&t, &EN_DECADE, |c, _, _| {
            let n: u64 = c[1].parse().ok()?;
            Some(en_plural_last(&en_year(n)))
        });
        t = sub(&t, &SHORT_DECADE, |c, before, _| {
            if ends_alnum(before) {
                return None;
            }
            let n: u64 = c[1].parse().ok()?;
            Some(en_plural_last(&cardinal(n, lang)))
        });
    }
    sub(&t, &SHORT_YEAR, |c, before, after| {
        // `l'80%` or `dell'86` style elisions stay for the other passes;
        // a quote closing after the number is not a year either.
        if ends_alnum(before) || after.starts_with('\'') {
            return None;
        }
        let n: u64 = c[1].parse().ok()?;
        Some(cardinal(n, lang))
    })
}

static EN_ORDINAL: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d+)(?:st|nd|rd|th)\b"));
static IT_ORDINAL: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d+)\s?([°ºª])"));

fn ordinals(s: &str, lang: Lang) -> String {
    if lang == Lang::English {
        return sub(s, &EN_ORDINAL, |c, _, _| {
            Some(ordinal(c[1].parse().ok()?, lang, false))
        });
    }
    sub(s, &IT_ORDINAL, |c, _, _| {
        let n: u64 = c[1].parse().ok()?;
        Some(ordinal(n, lang, &c[2] == "ª"))
    })
}

static ROMAN: LazyLock<Regex> = LazyLock::new(|| re(r"\b([IVXLCDM]{2,})\b"));
static CENTURY_AFTER: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s+(?i:(secolo|secoli|sec\.|century|centuries))"));

/// Words after which an English Roman numeral is a plain number
/// ("Chapter II" = "Chapter two", "World War II").
const EN_CARDINAL_CONTEXT: &[&str] = &[
    "Chapter", "Part", "Volume", "Act", "Scene", "War", "Phase", "Section", "Book", "Type",
    "Stage", "Level", "Class", "Grade", "Appendix", "Article", "Title", "Episode", "Season",
    "Round", "Vol", "Page", "Table", "Figure", "Step", "Unit",
];
/// Italian nouns that take a feminine ordinal ("Parte seconda").
const IT_FEMININE: &[&str] = &[
    "Parte",
    "Fase",
    "Sezione",
    "Scena",
    "Classe",
    "Stagione",
    "Guerra",
    "Serie",
    "Puntata",
    "Legislatura",
    "Repubblica",
    "Tavola",
    "Appendice",
];

fn romans(s: &str, lang: Lang) -> String {
    sub_ext(s, &ROMAN, |c, before, after| {
        if ends_alnum(before) {
            return None;
        }
        let n = roman_value(&c[1])?;
        if let Some(m) = CENTURY_AFTER.captures(after) {
            if n > 30 {
                return None;
            }
            let ord = ordinal(n, lang, false);
            if m[1].eq_ignore_ascii_case("sec.") {
                return Some((format!("{ord} secolo"), m[0].len()));
            }
            return Some((ord, 0));
        }
        // After a name: Luigi XIV, Henry VIII, Giovanni Paolo II.
        let prev = prev_word(before)?;
        let is_name = prev.chars().next().is_some_and(char::is_uppercase)
            && prev.chars().all(char::is_alphabetic)
            && prev.chars().skip(1).any(char::is_lowercase);
        if !is_name || n > 39 {
            return None;
        }
        if lang == Lang::Italian {
            let fem = IT_FEMININE.contains(&prev) || prev.ends_with('a');
            Some((ordinal(n, lang, fem), 0))
        } else if EN_CARDINAL_CONTEXT.contains(&prev) {
            Some((cardinal(n, lang), 0))
        } else {
            Some((format!("the {}", ordinal(n, lang, false)), 0))
        }
    })
}

// ---- flight numbers, codes ----

/// Airline code, a space, the flight number (`BA 2490`); glued forms
/// (`BA2490`) are left to [`codes`], which can't tell them from `FY2025`.
static FLIGHT: LazyLock<Regex> = LazyLock::new(|| re(r"\b([A-Z]{2})\s(\d{3,4})\b"));
const NOT_AIRLINES: &[&str] = &["AD", "BC", "AC", "DC", "PM", "AM", "NO", "OK", "TV", "PC"];

fn spell_letters(s: &str) -> String {
    s.chars().map(String::from).collect::<Vec<_>>().join(" ")
}

fn flight_numbers(s: &str, lang: Lang) -> String {
    sub(s, &FLIGHT, |c, _, after| {
        if NOT_AIRLINES.contains(&&c[1])
            || (after.starts_with(['.', ',', ':'])
                && after[1..].starts_with(|ch: char| ch.is_ascii_digit()))
        {
            return None;
        }
        Some(format!("{} {}", spell_letters(&c[1]), digits(&c[2], lang)))
    })
}

static DIMENSIONS: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"\b({NUM})\s?[x×]\s?({NUM})\b")));

fn dimensions(s: &str, lang: Lang) -> String {
    let by = if lang == Lang::Italian { "per" } else { "by" };
    sub(s, &DIMENSIONS, |c, _, _| {
        Some(format!(
            "{} {by} {}",
            number_words(&c[1], lang),
            number_words(&c[2], lang)
        ))
    })
}

static CODE: LazyLock<Regex> = LazyLock::new(|| re(r"\b[\p{L}\p{N}]*\p{N}[\p{L}\p{N}]*\b"));

/// Mixed letters and digits: booking codes are spelled (`X4K9Q2`), product
/// names keep their word and read their number (`MP3`, `COVID19`).
fn codes(s: &str, lang: Lang) -> String {
    sub(s, &CODE, |c, _, _| {
        let tok = &c[0];
        if !tok.chars().any(char::is_alphabetic) {
            return None;
        }
        let mut runs: Vec<(bool, String)> = Vec::new();
        for ch in tok.chars() {
            let is_digit = ch.is_ascii_digit();
            match runs.last_mut() {
                Some((d, run)) if *d == is_digit => run.push(ch),
                _ => runs.push((is_digit, ch.to_string())),
            }
        }
        let digit_runs = runs.iter().filter(|(d, _)| *d).count();
        let letter_runs = runs.len() - digit_runs;
        let spelled = digit_runs >= 2 || letter_runs >= 2;
        let words: Vec<String> = runs
            .iter()
            .map(|(is_digit, run)| {
                if !is_digit {
                    return if spelled && run.chars().count() <= 3 {
                        spell_letters(run)
                    } else {
                        run.clone()
                    };
                }
                let year =
                    run.len() == 4 && run.parse::<u64>().is_ok_and(|v| (1100..=2099).contains(&v));
                if spelled || (run.len() > 3 && !year) {
                    digits(run, lang)
                } else if year && lang == Lang::English {
                    en_year(run.parse().unwrap_or(0))
                } else {
                    integer_str(run, lang)
                }
            })
            .collect();
        Some(words.join(" "))
    })
}

// ---- ranges, fractions ----

static RANGE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"\b({NUM})\s?[-–]\s?({NUM})\b")));

fn ranges(s: &str, lang: Lang) -> String {
    sub(s, &RANGE, |c, before, after| {
        if before.ends_with(['-', '–', '/']) || after.starts_with(['-', '–', '/']) {
            return None;
        }
        Some(if lang == Lang::Italian {
            let intro = matches!(
                prev_word_lower(before).as_str(),
                "da" | "dal" | "dalle" | "dai" | "dallo" | "dalla" | "tra" | "fra"
            );
            if intro {
                format!("{} a {}", &c[1], &c[2])
            } else {
                format!("da {} a {}", &c[1], &c[2])
            }
        } else {
            format!("{} to {}", &c[1], &c[2])
        })
    })
}

static FRACTION: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{1,3})/(\d{1,3})\b"));

fn fractions(s: &str, lang: Lang) -> String {
    sub(s, &FRACTION, |c, _, after| {
        if after.starts_with('/') {
            return None;
        }
        let (n, d): (u64, u64) = (c[1].parse().ok()?, c[2].parse().ok()?);
        if d == 0 {
            return None;
        }
        let it = lang == Lang::Italian;
        if (n, d) == (24, 7) {
            return Some(if it {
                "ventiquattro su sette".to_string()
            } else {
                "twenty-four seven".to_string()
            });
        }
        if d > 10 {
            let over = if it { "su" } else { "over" };
            return Some(format!(
                "{} {over} {}",
                cardinal(n, lang),
                cardinal(d, lang)
            ));
        }
        Some(if it {
            let den = match d {
                2 => "mezzo".to_string(),
                _ => ordinal(d, lang, false),
            };
            if n == 1 {
                format!("un {den}")
            } else {
                let plural = match d {
                    2 => "mezzi".to_string(),
                    _ => format!("{}i", &den[..den.len() - 1]),
                };
                format!("{} {plural}", cardinal(n, lang))
            }
        } else {
            let den = match d {
                2 => "half".to_string(),
                4 => "quarter".to_string(),
                _ => ordinal(d, lang, false),
            };
            if n == 1 {
                format!("one {den}")
            } else {
                let plural = match d {
                    2 => "halves".to_string(),
                    _ => format!("{den}s"),
                };
                format!("{} {plural}", cardinal(n, lang))
            }
        })
    })
}

// ---- symbols ----

static HASH_NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"#\s?(\d)"));
static ABOUT: LazyLock<Regex> = LazyLock::new(|| re(r"[~≈]\s?(\d)"));
static MENTION: LazyLock<Regex> = LazyLock::new(|| re(r"@(\w)"));
static SLASH_WORDS: LazyLock<Regex> = LazyLock::new(|| re(r"(\p{L})/(\p{L})"));
static ARROWS: LazyLock<Regex> = LazyLock::new(|| re(r"\s*(?:->|=>|<-|<=>)\s*"));
static LEFTOVER_MARKS: LazyLock<Regex> = LazyLock::new(|| re(r"[*_#|\\^`~{}<>]"));

fn symbols(s: &str, lang: Lang) -> String {
    let it = lang == Lang::Italian;
    let mut t = HASH_NUMBER
        .replace_all(s, if it { "numero $1" } else { "number $1" })
        .into_owned();
    t = ABOUT
        .replace_all(&t, if it { "circa $1" } else { "about $1" })
        .into_owned();
    t = sub(&t, &MENTION, |c, before, _| {
        (before.is_empty() || before.ends_with(char::is_whitespace)).then(|| c[1].to_string())
    });
    t = ARROWS.replace_all(&t, ", ").into_owned();
    t = SLASH_WORDS.replace_all(&t, "$1 $2").into_owned();
    let pairs: &[(&str, &str)] = if it {
        &[
            (" = ", " uguale a "),
            (" + ", " più "),
            (" < ", " minore di "),
            (" > ", " maggiore di "),
            ("&", " e "),
            ("+", " più "),
            ("%", " per cento"),
            ("‰", " per mille"),
            ("€", " euro"),
            ("$", " dollari"),
            ("£", " sterline"),
            ("¥", " yen"),
            ("°", " "),
            ("@", " chiocciola "),
        ]
    } else {
        &[
            (" = ", " equals "),
            (" + ", " plus "),
            (" < ", " less than "),
            (" > ", " greater than "),
            ("&", " and "),
            ("+", " plus "),
            ("%", " percent"),
            ("‰", " per mille"),
            ("€", " euros"),
            ("$", " dollars"),
            ("£", " pounds"),
            ("¥", " yen"),
            ("°", " degrees"),
            ("@", " at "),
        ]
    };
    for (from, to) in pairs {
        t = t.replace(from, to);
    }
    LEFTOVER_MARKS.replace_all(&t, " ").into_owned()
}

// ---- numbers left ----

static NUMBER: LazyLock<Regex> = LazyLock::new(|| re(NUM));

fn numbers_left(s: &str, lang: Lang) -> String {
    sub(s, &NUMBER, |c, _, _| {
        let tok = &c[0];
        if lang == Lang::English {
            if let Some(n) = parse_num(tok, lang) {
                let v = n.int_value().unwrap_or(0);
                if n.frac.is_none() && !n.grouped && n.int.len() == 4 && (1100..=2099).contains(&v)
                {
                    return Some(en_year(v));
                }
            }
        }
        Some(number_words(tok, lang))
    })
}

// ---- punctuation ----

static DASH: LazyLock<Regex> = LazyLock::new(|| re(r"\s*—\s*|\s+[-–]{1,2}\s+|\s*--\s*"));
static BRACKET: LazyLock<Regex> = LazyLock::new(|| re(r"\s*[()\[\]]\s*"));
static SPACE_BEFORE_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"\s+([,.!?:])"));
static COMMA_RUN: LazyLock<Regex> = LazyLock::new(|| re(r",(?:\s*,)+"));
static COMMA_THEN_STOP: LazyLock<Regex> = LazyLock::new(|| re(r",\s*([.!?:])"));
static STOP_THEN_COMMA: LazyLock<Regex> = LazyLock::new(|| re(r"([.!?:])\s*,"));
static COMMA_NO_SPACE: LazyLock<Regex> = LazyLock::new(|| re(r",(\p{L})"));

fn punctuation(s: &str) -> String {
    let mut t = s.replace(';', ",");
    t = DASH.replace_all(&t, ", ").into_owned();
    t = BRACKET.replace_all(&t, ", ").into_owned();
    t = SPACE_BEFORE_PUNCT.replace_all(&t, "$1").into_owned();
    t = COMMA_RUN.replace_all(&t, ",").into_owned();
    t = COMMA_THEN_STOP.replace_all(&t, "$1").into_owned();
    t = STOP_THEN_COMMA.replace_all(&t, "$1").into_owned();
    t = COMMA_NO_SPACE.replace_all(&t, ", $1").into_owned();
    let t = t.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    let t = t.trim_end();
    match t.strip_suffix(',') {
        Some(stem) => format!("{stem}."),
        None => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IT: Lang = Lang::Italian;
    const EN: Lang = Lang::English;

    fn en(s: &str) -> String {
        normalize(s, EN)
    }
    fn it(s: &str) -> String {
        normalize(s, IT)
    }

    #[test]
    fn plain_numbers() {
        assert_eq!(
            en("I have 3 cats and 1,248 fish."),
            "I have three cats and one thousand two hundred forty-eight fish."
        );
        assert_eq!(
            it("Ho 3 gatti e 1.248 pesci."),
            "Ho tre gatti e milleduecentoquarantotto pesci."
        );
        assert_eq!(en("Pi is 3.14."), "Pi is three point one four.");
        assert_eq!(
            it("Pi greco è 3,14."),
            "Pi greco è tre virgola quattordici."
        );
        assert_eq!(
            it("La versione 2.0 è uscita."),
            "La versione due punto zero è uscita."
        );
        assert_eq!(en("Scores 1,2,3."), "Scores one, two, three.");
        assert_eq!(en("Agent 007 is here."), "Agent zero zero seven is here.");
    }

    #[test]
    fn a_number_starting_a_sentence_is_capitalised() {
        assert_eq!(
            en("It ended. 12 people came."),
            "It ended. Twelve people came."
        );
        assert_eq!(it("3 persone."), "Tre persone.");
    }

    #[test]
    fn english_years() {
        assert_eq!(
            en("Built in 1861, rebuilt in 1905 and 2009."),
            "Built in eighteen sixty-one, rebuilt in nineteen oh five and two thousand nine."
        );
        assert_eq!(
            en("By 2028 we expect 2,028 units."),
            "By twenty twenty-eight we expect two thousand twenty-eight units."
        );
        assert_eq!(
            en("Flight 2490 left."),
            "Flight two thousand four hundred ninety left."
        );
        assert_eq!(it("Nel 1861."), "Nel milleottocentosessantuno.");
    }

    #[test]
    fn decades() {
        assert_eq!(
            en("In the 1990s and the '80s."),
            "In the nineteen nineties and the eighties."
        );
        assert_eq!(it("Gli anni '90."), "Gli anni novanta.");
    }

    #[test]
    fn negative_numbers() {
        assert_eq!(en("It was -5 outside."), "It was minus five outside.");
        assert_eq!(it("Erano -5 gradi."), "Erano meno cinque gradi.");
    }

    #[test]
    fn percentages() {
        assert_eq!(
            en("Tax is 22% and growth 3.5%."),
            "Tax is twenty-two percent and growth three point five percent."
        );
        assert_eq!(
            it("IVA al 22%, crescita del 2,5 %."),
            "IVA al ventidue per cento, crescita del due virgola cinque per cento."
        );
        assert_eq!(
            en("Between 10-20% of users."),
            "Between ten to twenty percent of users."
        );
        assert_eq!(it("Tra il 10-20%."), "Tra il dieci e il venti per cento.");
        assert_eq!(it("Il 10-20%."), "Il da dieci a venti per cento.");
    }

    #[test]
    fn currencies_english() {
        assert_eq!(
            en("The quote comes to $1,248.50."),
            "The quote comes to one thousand two hundred forty-eight dollars and fifty cents."
        );
        assert_eq!(
            en("It costs $1 or €20 or £5.99."),
            "It costs one dollar or twenty euros or five pounds and ninety-nine pence."
        );
        assert_eq!(en("Only $0.50."), "Only fifty cents.");
        assert_eq!(
            en("A $2.5 million deal and $3bn more."),
            "A two point five million dollars deal and three billion dollars more."
        );
        assert_eq!(
            en("Pay 30 EUR or USD 12.05."),
            "Pay thirty euros or twelve dollars and five cents."
        );
        assert_eq!(en("¥500 today."), "Five hundred yen today.");
        assert_eq!(en("It was $5k."), "It was five thousand dollars.");
    }

    #[test]
    fn currencies_italian() {
        assert_eq!(
            it("Il preventivo è di 1.248,50 euro."),
            "Il preventivo è di milleduecentoquarantotto euro e cinquanta centesimi."
        );
        assert_eq!(
            it("Costa 1 € o 1 sterlina."),
            "Costa un euro o una sterlina."
        );
        assert_eq!(it("Costa € 0,50."), "Costa cinquanta centesimi.");
        assert_eq!(
            it("Un affare da €2,5 mln e 3 milioni di euro."),
            "Un affare da due virgola cinque milioni di euro e tre milioni di euro."
        );
        assert_eq!(
            it("Sono 10 $ e 1,05 dollari."),
            "Sono dieci dollari e un dollaro e cinque centesimi."
        );
        assert_eq!(it("Costa 20 CHF."), "Costa venti franchi svizzeri.");
    }

    #[test]
    fn numeric_dates() {
        assert_eq!(
            en("It expires on 12/31/2026."),
            "It expires on December thirty-first, twenty twenty-six."
        );
        assert_eq!(
            en("It expires on 31/12/2026."),
            "It expires on December thirty-first, twenty twenty-six."
        );
        assert_eq!(
            en("Released 2026-09-25."),
            "Released September twenty-fifth, twenty twenty-six."
        );
        assert_eq!(
            it("Scade il 31/12/2026."),
            "Scade il trentuno dicembre duemilaventisei."
        );
        assert_eq!(
            it("Dal 1.3.2025 al 01-04-25."),
            "Dal primo marzo duemilaventicinque al primo aprile duemilaventicinque."
        );
        assert_eq!(it("Entro il 31/12."), "Entro il trentuno dicembre.");
        assert_eq!(en("Due by 3/15."), "Due by March fifteenth.");
        assert_eq!(
            en("Invalid 13/13/2026."),
            "Invalid thirteen/thirteen/twenty twenty-six."
        );
    }

    #[test]
    fn month_name_dates() {
        assert_eq!(
            en("From Monday, March 3rd, 2025 to Thursday, April 17th."),
            "From Monday, March third, twenty twenty-five to Thursday, April seventeenth."
        );
        assert_eq!(
            en("Due by January 15. London, September 24."),
            "Due by January fifteenth. London, September twenty-fourth."
        );
        assert_eq!(
            en("On 24 September 2025 and the 3rd of May."),
            "On the twenty-fourth of September twenty twenty-five and the third of May."
        );
        assert_eq!(
            en("Sept. 5 and Dec 1st."),
            "September fifth and December first."
        );
        assert_eq!(en("In May 2025."), "In May twenty twenty-five.");
        assert_eq!(
            it("Lunedì 3 marzo 2025, il 1° maggio e l'8 Marzo."),
            "Lunedì tre marzo duemilaventicinque, il primo maggio e l'otto Marzo."
        );
        assert_eq!(it("Roma, 24 settembre."), "Roma, ventiquattro settembre.");
        assert_eq!(it("Il 1 gennaio."), "Il primo gennaio.");
    }

    #[test]
    fn times() {
        assert_eq!(
            en("It departs at 7:45 a.m. from Terminal 5."),
            "It departs at seven forty-five ay em from Terminal five."
        );
        assert_eq!(en("Call before 6 p.m."), "Call before six pee em.");
        assert_eq!(en("At 6 pm. Then home."), "At six pee em. Then home.");
        assert_eq!(
            en("At 18:30, 7:05 and 9:00."),
            "At eighteen thirty, seven oh five and nine o'clock."
        );
        assert_eq!(en("At 21:00 sharp."), "At twenty-one hundred sharp.");
        assert_eq!(en("Lap 1:02:03."), "Lap one oh two and three seconds.");
        assert_eq!(
            it("Parte alle 7:45 e arriva alle 13.30, entro le 18."),
            "Parte alle sette e quarantacinque e arriva alle tredici e trenta, entro le diciotto."
        );
        assert_eq!(
            it("Alle 9:00 e alle 10:05."),
            "Alle nove e alle dieci e cinque."
        );
        assert_eq!(en("At 5 amazing places."), "At five amazing places.");
    }

    #[test]
    fn phone_numbers() {
        assert_eq!(
            en("Call +44 20 7946 0958 now."),
            "Call plus four four, two zero, seven nine four six, zero nine five eight now."
        );
        assert_eq!(it("Chiamate il numero 02 8394 1170 entro le 18."), "Chiamate il numero zero due, otto tre nove quattro, uno uno sette zero entro le diciotto.");
        assert_eq!(
            it("Tel. 06-1234-5678."),
            "Telefono zero sei, uno due tre quattro, cinque sei sette otto."
        );
        assert_eq!(
            en("Phone 555 123 4567."),
            "Phone five five five, one two three, four five six seven."
        );
        assert_eq!(
            en("Between 2020 2021 events."),
            "Between twenty twenty twenty twenty-one events."
        );
    }

    #[test]
    fn units() {
        assert_eq!(en("40 km and 1 km and 2.5 kg at 20 °C."), "Forty kilometers and one kilometer and two point five kilograms at twenty degrees Celsius.");
        assert_eq!(it("Sono 5 km, 1 km, 2,5 kg a 20°C in 24h."), "Sono cinque chilometri, un chilometro, due virgola cinque chilogrammi a venti gradi Celsius in ventiquattro ore.");
        assert_eq!(en("A 100 GB disk at 3 GHz, 5km away, 60 mph."), "A one hundred gigabytes disk at three gigahertz, five kilometers away, sixty miles per hour.");
        assert_eq!(
            it("Un appartamento di 80 mq, 3 mln di persone."),
            "Un appartamento di ottanta metri quadrati, tre milioni di persone."
        );
        assert_eq!(
            it("Nel 2025 l'azienda."),
            "Nel duemilaventicinque l'azienda."
        );
        assert_eq!(en("It is 30° outside."), "It is thirty degrees outside.");
    }

    #[test]
    fn ordinals() {
        assert_eq!(
            en("The 19th century and the 1st, 2nd, 3rd, 22nd."),
            "The nineteenth century and the first, second, third, twenty-second."
        );
        assert_eq!(
            it("Il 3° posto e la 2ª classificata."),
            "Il terzo posto e la seconda classificata."
        );
    }

    #[test]
    fn roman_numerals_in_context() {
        assert_eq!(it("Era il XIX secolo."), "Era il diciannovesimo secolo.");
        assert_eq!(
            it("Nel XV sec. e con Luigi XIV ed Elisabetta II."),
            "Nel quindicesimo secolo e con Luigi quattordicesimo ed Elisabetta seconda."
        );
        assert_eq!(it("Papa Giovanni XXIII."), "Papa Giovanni ventitreesimo.");
        assert_eq!(
            en("Henry VIII and World War II and the XIX century."),
            "Henry the eighth and World War two and the nineteenth century."
        );
        assert_eq!(en("A CD and a DVD and MIX."), "A CD and a DVD and MIX.");
        assert_eq!(en("I went."), "I went.");
    }

    #[test]
    fn flight_numbers_and_codes() {
        assert_eq!(
            en("Flight BA 2490, code X4K9Q2."),
            "Flight B A two four nine zero, code X four K nine Q two."
        );
        assert_eq!(
            it("Il volo AZ 1774, codice X4K9Q2."),
            "Il volo A Z uno sette sette quattro, codice X quattro K nove Q due."
        );
        assert_eq!(
            en("An MP3 of COVID19 in 3D, B2B, H2O and FY2025."),
            "An MP three of COVID nineteen in three D, B two B, H two O and FY twenty twenty-five."
        );
        assert_eq!(it("In 4K e 5G."), "In quattro K e cinque G.");
    }

    #[test]
    fn ranges_dimensions_fractions() {
        assert_eq!(
            en("From 1861-1865."),
            "From eighteen sixty-one to eighteen sixty-five."
        );
        assert_eq!(it("Orario 9-18."), "Orario da nove a diciotto.");
        assert_eq!(it("Dalle 9 - 18."), "Dalle nove a diciotto.");
        assert_eq!(en("A 3x4 room."), "A three by four room.");
        assert_eq!(it("Una stanza 3 × 4."), "Una stanza tre per quattro.");
        assert_eq!(en("Add 1/2 cup and 3/4 of it, 2/3, 5/8, 7/24 or 24/7."), "Add one half cup and three quarters of it, two thirds, five eighths, seven over twenty-four or twenty-four seven.");
        assert_eq!(
            it("Aggiungi 1/2, 2/3, 3/4 e 24/7."),
            "Aggiungi un mezzo, due terzi, tre quarti e ventiquattro su sette."
        );
    }

    #[test]
    fn abbreviations_english() {
        assert_eq!(
            en("Dr. Smith met Mrs. Jones, Mr. Lee etc. Then they left."),
            "Doctor Smith met Missus Jones, Mister Lee et cetera. Then they left."
        );
        assert_eq!(
            en("Fruit, e.g. apples, i.e. food, vs. meat."),
            "Fruit, for example apples, that is food, versus meat."
        );
        assert_eq!(
            en("See No. 5 and p. 12. No. The end."),
            "See number five and page twelve. No. The end."
        );
        assert_eq!(
            en("St. Paul lives on Main St. in Acme Inc. premises."),
            "Saint Paul lives on Main Street in Acme Incorporated premises."
        );
        assert_eq!(en("It ends with Acme Ltd."), "It ends with Acme Limited.");
    }

    #[test]
    fn abbreviations_italian() {
        assert_eq!(
            it("Il Sig. Rossi e la Sig.ra Bianchi, la Dott.ssa Verdi e il Prof. Neri."),
            "Il signor Rossi e la signora Bianchi, la dottoressa Verdi e il professor Neri."
        );
        assert_eq!(
            it("Mele, pere ecc. Poi basta."),
            "Mele, pere eccetera. Poi basta."
        );
        assert_eq!(
            it("Vedi pag. 12, art. 3 e n. 5, cfr. sopra."),
            "Vedi pagina dodici, articolo tre e numero cinque, confronta sopra."
        );
        assert_eq!(
            it("Acme S.p.A. e Beta S.r.l. dal 50 a.C."),
            "Acme esse pi a e Beta esse erre elle dal cinquanta avanti Cristo."
        );
        assert_eq!(it("Ecc. finale."), "Eccetera finale.");
    }

    #[test]
    fn initials() {
        assert_eq!(
            en("The U.S.A. and John F. Kennedy and J. R. R. Tolkien."),
            "The U S A and John F Kennedy and J R R Tolkien."
        );
        assert_eq!(en("Plan B. Then plan C."), "Plan B. Then plan C.");
    }

    #[test]
    fn urls_and_emails() {
        assert_eq!(
            en("See https://www.example.com/path?q=1, or write to anna.rossi@example.co.uk."),
            "See example dot com, or write to anna dot rossi at example dot co dot uk."
        );
        assert_eq!(
            it("Visita www.comune.roma.it oppure scrivi a info@sussurro.it."),
            "Visita comune punto roma punto it oppure scrivi a info chiocciola sussurro punto it."
        );
        assert_eq!(
            en("Go to github.com/fullo now."),
            "Go to github dot com now."
        );
        assert_eq!(
            normalize("Voir https://www.exemple.fr/a.", Lang::Other),
            "Voir exemple.fr."
        );
    }

    #[test]
    fn symbols() {
        assert_eq!(en("Tom & Jerry, #5, ~10 people, C++ and 2 + 2 = 4, @anna and/or me."), "Tom and Jerry, number five, about ten people, C plus plus and two plus two equals four, anna and or me.");
        assert_eq!(
            it("Tom & Jerry, #5, ~10 persone."),
            "Tom e Jerry, numero cinque, circa dieci persone."
        );
    }

    #[test]
    fn punctuation_and_brackets() {
        assert_eq!(
            en("One; two — three (four) five - six."),
            "One, two, three, four, five, six."
        );
        assert_eq!(en("End (see above)"), "End, see above.");
        assert_eq!(en("Wait... what?"), "Wait... what?");
    }

    #[test]
    fn emoji_and_invisible_characters_are_dropped() {
        assert_eq!(en("Great 🎉 work\u{200B}!"), "Great work!");
        assert_eq!(it("Bravo 👍🏽 davvero ❤️."), "Bravo davvero.");
    }

    #[test]
    fn other_languages_pass_through() {
        let s = "Le 3 mars 2025, 22 % des gens.";
        assert_eq!(normalize(s, Lang::Other), s);
    }

    #[test]
    fn curly_quotes_are_straightened() {
        assert_eq!(
            en("“It’s a first step,” she said."),
            "\"It's a first step,\" she said."
        );
        assert_eq!(it("«È un primo passo»."), "«È un primo passo».");
    }
}
