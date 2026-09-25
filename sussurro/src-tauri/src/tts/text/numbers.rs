//! Numbers as words, Italian and English (pure): cardinals, ordinals,
//! English years, digit-by-digit reading and decimals.
//!
//! Italian follows the written norm: compounds are one word
//! (`milleduecentoquarantotto`), tens drop their final vowel before `uno`
//! and `otto` (`ventuno`, `ventotto`), `cento` before `otto`/`ottanta`
//! (`centotto`, `centottanta`), a final `tre` takes the accent (`ventitré`,
//! `milletré`) and `uno` becomes `un` before `mila`/`milioni`
//! (`ventunmila`, `un milione`). Millions and billions are separate words
//! (`un milione duecentomila`).
//!
//! English uses the American form without "and" (`one thousand two hundred
//! forty-eight`), hyphens for 21–99 (`forty-eight`) and the usual year
//! reading (`nineteen oh five`, `two thousand nine`, `twenty twenty-six`).

use super::Lang;

/// Integers at or above this are read digit by digit: nobody says them as a
/// number, and the word forms stop at billions (IT) / trillions (EN).
pub const MAX_CARDINAL: u64 = 999_999_999_999;

const IT_UNITS: [&str; 20] = [
    "zero",
    "uno",
    "due",
    "tre",
    "quattro",
    "cinque",
    "sei",
    "sette",
    "otto",
    "nove",
    "dieci",
    "undici",
    "dodici",
    "tredici",
    "quattordici",
    "quindici",
    "sedici",
    "diciassette",
    "diciotto",
    "diciannove",
];
const IT_TENS: [&str; 10] = [
    "",
    "",
    "venti",
    "trenta",
    "quaranta",
    "cinquanta",
    "sessanta",
    "settanta",
    "ottanta",
    "novanta",
];
const EN_UNITS: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const EN_TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// Cardinal number in words. `Lang::Other` gets the plain digits back.
pub fn cardinal(n: u64, lang: Lang) -> String {
    match lang {
        Lang::Italian if n <= MAX_CARDINAL => it_cardinal(n),
        Lang::English if n <= MAX_CARDINAL => en_cardinal(n),
        Lang::Italian | Lang::English => digits(&n.to_string(), lang),
        Lang::Other => n.to_string(),
    }
}

/// Ordinal in words (`third`, `terzo`/`terza`). `feminine` only matters
/// for Italian.
pub fn ordinal(n: u64, lang: Lang, feminine: bool) -> String {
    match lang {
        Lang::Italian => it_ordinal(n, feminine),
        Lang::English => en_ordinal(n),
        Lang::Other => n.to_string(),
    }
}

/// Every digit as its own word, separated by spaces (`zero due`).
/// Non-digit characters are dropped.
pub fn digits(s: &str, lang: Lang) -> String {
    let table = match lang {
        Lang::Italian => &IT_UNITS,
        Lang::English => &EN_UNITS,
        Lang::Other => return s.to_string(),
    };
    s.chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| table[d as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// A number written with digits only (`"0042"`, `"1248"`): leading zeros or
/// more digits than [`MAX_CARDINAL`] allows are read digit by digit.
pub fn integer_str(s: &str, lang: Lang) -> String {
    if s.len() > 1 && s.starts_with('0') {
        return digits(s, lang);
    }
    match s.parse::<u64>() {
        Ok(n) if n <= MAX_CARDINAL => cardinal(n, lang),
        _ => digits(s, lang),
    }
}

/// `int` + decimal separator + `frac` (digits as written). English reads
/// the fraction digit by digit (`three point one four`); Italian reads a
/// short fraction as a number (`tre virgola quattordici`) unless it starts
/// with zero (`zero virgola zero cinque`). `dot` = the separator was a dot
/// in Italian text (versions such as `2.0`: `due punto zero`).
pub fn decimal(int: &str, frac: &str, lang: Lang, dot: bool) -> String {
    let whole = integer_str(int, lang);
    match lang {
        Lang::English => format!("{whole} point {}", digits(frac, lang)),
        Lang::Italian => {
            let sep = if dot { "punto" } else { "virgola" };
            let f = if frac.starts_with('0') || frac.len() > 3 {
                digits(frac, lang)
            } else {
                integer_str(frac, lang)
            };
            format!("{whole} {sep} {f}")
        }
        Lang::Other => format!("{int}.{frac}"),
    }
}

// ---- Italian ----

fn it_below_100(n: u64) -> String {
    if n < 20 {
        return IT_UNITS[n as usize].to_string();
    }
    let tens = IT_TENS[(n / 10) as usize];
    let unit = n % 10;
    match unit {
        0 => tens.to_string(),
        1 | 8 => format!("{}{}", &tens[..tens.len() - 1], IT_UNITS[unit as usize]),
        _ => format!("{tens}{}", IT_UNITS[unit as usize]),
    }
}

fn it_below_1000(n: u64) -> String {
    let h = n / 100;
    let rest = n % 100;
    let hundreds = match h {
        0 => String::new(),
        1 => "cento".to_string(),
        _ => format!("{}cento", IT_UNITS[h as usize]),
    };
    if rest == 0 {
        return hundreds;
    }
    let tail = it_below_100(rest);
    if hundreds.is_empty() {
        return tail;
    }
    // cento + otto / ottanta → centotto, centottanta.
    if tail.starts_with("ott") {
        format!("{}{tail}", &hundreds[..hundreds.len() - 1])
    } else {
        format!("{hundreds}{tail}")
    }
}

/// The last group (< 1000) of a number: a final `tre` of a compound takes
/// the accent (`ventitré`, `milletré`), a bare `tre` does not.
fn it_accent(word: String, group: u64, compound: bool) -> String {
    let accent = group % 10 == 3 && group % 100 != 13 && (group > 3 || compound);
    if accent && word.ends_with("tre") {
        format!("{}tré", &word[..word.len() - 3])
    } else {
        word
    }
}

/// `uno` → `un` before `mila`, `milione`, `miliardo`.
fn it_apocope(word: String) -> String {
    match word.strip_suffix("uno") {
        Some(stem) => format!("{stem}un"),
        None => word,
    }
}

fn it_cardinal(n: u64) -> String {
    if n == 0 {
        return "zero".to_string();
    }
    let billions = n / 1_000_000_000;
    let millions = (n / 1_000_000) % 1000;
    let thousands = (n / 1000) % 1000;
    let rest = n % 1000;
    let mut parts: Vec<String> = Vec::new();
    for (value, one, many) in [
        (billions, "un miliardo", "miliardi"),
        (millions, "un milione", "milioni"),
    ] {
        match value {
            0 => {}
            1 => parts.push(one.to_string()),
            v => parts.push(format!("{} {many}", it_apocope(it_below_1000(v)))),
        }
    }
    let mut low = String::new();
    match thousands {
        0 => {}
        1 => low.push_str("mille"),
        t => {
            // No accent inside a compound: ventitremila.
            low.push_str(&it_apocope(it_below_1000(t)));
            low.push_str("mila");
        }
    }
    if rest > 0 {
        let compound = n >= 1000;
        low.push_str(&it_accent(it_below_1000(rest), rest, compound));
    }
    if !low.is_empty() {
        parts.push(low);
    }
    parts.join(" ")
}

const IT_ORDINALS: [&str; 11] = [
    "", "primo", "secondo", "terzo", "quarto", "quinto", "sesto", "settimo", "ottavo", "nono",
    "decimo",
];

fn it_ordinal(n: u64, feminine: bool) -> String {
    let masc = if (1..=10).contains(&n) {
        IT_ORDINALS[n as usize].to_string()
    } else if n == 0 {
        "zeresimo".to_string()
    } else {
        let card = it_cardinal(n).replace(' ', "");
        if let Some(stem) = card.strip_suffix("tré") {
            format!("{stem}treesimo")
        } else if card.ends_with("sei") {
            format!("{card}esimo")
        } else if card.ends_with("mila") || card.ends_with("milioni") || card.ends_with("miliardi")
        {
            // duemila → duemillesimo, due milioni → duemilionesimo.
            let stem = card
                .strip_suffix("mila")
                .map(|s| format!("{s}mill"))
                .or_else(|| card.strip_suffix("milioni").map(|s| format!("{s}milion")))
                .or_else(|| card.strip_suffix("miliardi").map(|s| format!("{s}miliard")))
                .unwrap_or_default();
            format!("{stem}esimo")
        } else {
            let mut stem = card;
            stem.pop();
            format!("{stem}esimo")
        }
    };
    if feminine {
        format!("{}a", &masc[..masc.len() - 1])
    } else {
        masc
    }
}

// ---- English ----

fn en_below_100(n: u64) -> String {
    if n < 20 {
        return EN_UNITS[n as usize].to_string();
    }
    let tens = EN_TENS[(n / 10) as usize];
    match n % 10 {
        0 => tens.to_string(),
        u => format!("{tens}-{}", EN_UNITS[u as usize]),
    }
}

fn en_below_1000(n: u64) -> String {
    let h = n / 100;
    let rest = n % 100;
    match (h, rest) {
        (0, r) => en_below_100(r),
        (h, 0) => format!("{} hundred", EN_UNITS[h as usize]),
        (h, r) => format!("{} hundred {}", EN_UNITS[h as usize], en_below_100(r)),
    }
}

fn en_cardinal(n: u64) -> String {
    if n == 0 {
        return "zero".to_string();
    }
    let mut parts = Vec::new();
    for (scale, name) in [
        (1_000_000_000_000u64, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1000, "thousand"),
    ] {
        let v = (n / scale) % 1000;
        if v > 0 {
            parts.push(format!("{} {name}", en_below_1000(v)));
        }
    }
    let rest = n % 1000;
    if rest > 0 {
        parts.push(en_below_1000(rest));
    }
    parts.join(" ")
}

fn en_ordinal_word(word: &str) -> String {
    match word {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    }
}

fn en_ordinal(n: u64) -> String {
    let card = en_cardinal(n);
    // Only the last word (or the part after the last hyphen) changes.
    let cut = card.rfind([' ', '-']).map(|i| i + 1).unwrap_or(0);
    format!("{}{}", &card[..cut], en_ordinal_word(&card[cut..]))
}

/// English year reading: `nineteen oh five`, `eighteen sixty-one`,
/// `nineteen hundred`, `two thousand nine`, `twenty twenty-six`. Outside
/// 1100–2099 (and for 2000–2009) the plain cardinal.
pub fn en_year(n: u64) -> String {
    if !(1100..=2099).contains(&n) || (2000..=2009).contains(&n) {
        return en_cardinal(n);
    }
    let hi = en_below_100(n / 100);
    match n % 100 {
        0 => format!("{hi} hundred"),
        lo @ 1..=9 => format!("{hi} oh {}", EN_UNITS[lo as usize]),
        lo => format!("{hi} {}", en_below_100(lo)),
    }
}

/// English plural of a number's last word, for decades: `nineties`,
/// `hundreds`, `tens`.
pub fn en_plural_last(words: &str) -> String {
    let cut = words.rfind([' ', '-']).map(|i| i + 1).unwrap_or(0);
    let last = &words[cut..];
    let plural = match last.strip_suffix('y') {
        Some(stem) => format!("{stem}ies"),
        None => format!("{last}s"),
    };
    format!("{}{plural}", &words[..cut])
}

/// Value of a Roman numeral written in canonical form (`XIX` → 19);
/// `None` for anything that is not one (`IIII`, `VX`, `MIXED`).
pub fn roman_value(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let val = |c: char| match c {
        'I' => Some(1),
        'V' => Some(5),
        'X' => Some(10),
        'L' => Some(50),
        'C' => Some(100),
        'D' => Some(500),
        'M' => Some(1000),
        _ => None,
    };
    let vals: Vec<u64> = s.chars().map(val).collect::<Option<_>>()?;
    let mut total = 0u64;
    for (i, v) in vals.iter().enumerate() {
        if vals.get(i + 1).is_some_and(|next| next > v) {
            total = total.checked_sub(*v)?;
        } else {
            total += v;
        }
    }
    (to_roman(total) == s).then_some(total)
}

fn to_roman(mut n: u64) -> String {
    const TABLE: [(u64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (v, s) in TABLE {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const IT: Lang = Lang::Italian;
    const EN: Lang = Lang::English;

    #[test]
    fn italian_cardinals() {
        let cases = [
            (0, "zero"),
            (1, "uno"),
            (3, "tre"),
            (13, "tredici"),
            (17, "diciassette"),
            (21, "ventuno"),
            (23, "ventitré"),
            (28, "ventotto"),
            (40, "quaranta"),
            (99, "novantanove"),
            (100, "cento"),
            (101, "centouno"),
            (103, "centotré"),
            (108, "centotto"),
            (180, "centottanta"),
            (213, "duecentotredici"),
            (1000, "mille"),
            (1003, "milletré"),
            (1248, "milleduecentoquarantotto"),
            (1774, "millesettecentosettantaquattro"),
            (1861, "milleottocentosessantuno"),
            (2000, "duemila"),
            (2025, "duemilaventicinque"),
            (2026, "duemilaventisei"),
            (2028, "duemilaventotto"),
            (3003, "tremilatré"),
            (21_000, "ventunmila"),
            (23_000, "ventitremila"),
            (100_000, "centomila"),
            (1_000_000, "un milione"),
            (1_200_000, "un milione duecentomila"),
            (2_000_000, "due milioni"),
            (21_000_000, "ventun milioni"),
            (1_000_000_000, "un miliardo"),
            (3_500_000_000, "tre miliardi cinquecento milioni"),
        ];
        for (n, want) in cases {
            assert_eq!(cardinal(n, IT), want, "{n}");
        }
    }

    #[test]
    fn english_cardinals() {
        let cases = [
            (0, "zero"),
            (7, "seven"),
            (15, "fifteen"),
            (22, "twenty-two"),
            (40, "forty"),
            (100, "one hundred"),
            (105, "one hundred five"),
            (1248, "one thousand two hundred forty-eight"),
            (2490, "two thousand four hundred ninety"),
            (1_000_000, "one million"),
            (3_500_021, "three million five hundred thousand twenty-one"),
            (2_000_000_000, "two billion"),
        ];
        for (n, want) in cases {
            assert_eq!(cardinal(n, EN), want, "{n}");
        }
    }

    #[test]
    fn huge_numbers_are_read_digit_by_digit() {
        assert_eq!(
            cardinal(1_000_000_000_000, EN),
            "one zero zero zero zero zero zero zero zero zero zero zero zero"
        );
        assert_eq!(integer_str("0042", IT), "zero zero quattro due");
        assert_eq!(integer_str("42", IT), "quarantadue");
    }

    #[test]
    fn italian_ordinals() {
        let cases = [
            (1, false, "primo"),
            (1, true, "prima"),
            (3, false, "terzo"),
            (10, true, "decima"),
            (11, false, "undicesimo"),
            (19, false, "diciannovesimo"),
            (20, false, "ventesimo"),
            (21, false, "ventunesimo"),
            (23, false, "ventitreesimo"),
            (26, false, "ventiseiesimo"),
            (28, false, "ventottesimo"),
            (100, false, "centesimo"),
            (1000, false, "millesimo"),
            (2000, false, "duemillesimo"),
        ];
        for (n, fem, want) in cases {
            assert_eq!(ordinal(n, IT, fem), want, "{n}");
        }
    }

    #[test]
    fn english_ordinals() {
        let cases = [
            (1, "first"),
            (2, "second"),
            (3, "third"),
            (5, "fifth"),
            (8, "eighth"),
            (9, "ninth"),
            (12, "twelfth"),
            (17, "seventeenth"),
            (19, "nineteenth"),
            (20, "twentieth"),
            (21, "twenty-first"),
            (24, "twenty-fourth"),
            (31, "thirty-first"),
            (100, "one hundredth"),
            (103, "one hundred third"),
        ];
        for (n, want) in cases {
            assert_eq!(ordinal(n, EN, false), want, "{n}");
        }
    }

    #[test]
    fn english_years() {
        let cases = [
            (1066, "one thousand sixty-six"),
            (1861, "eighteen sixty-one"),
            (1900, "nineteen hundred"),
            (1905, "nineteen oh five"),
            (1999, "nineteen ninety-nine"),
            (2000, "two thousand"),
            (2009, "two thousand nine"),
            (2010, "twenty ten"),
            (2025, "twenty twenty-five"),
            (2028, "twenty twenty-eight"),
            (2100, "two thousand one hundred"),
        ];
        for (n, want) in cases {
            assert_eq!(en_year(n), want, "{n}");
        }
        assert_eq!(en_plural_last(&en_year(1990)), "nineteen nineties");
        assert_eq!(en_plural_last(&en_year(1900)), "nineteen hundreds");
    }

    #[test]
    fn decimals() {
        assert_eq!(decimal("3", "14", EN, false), "three point one four");
        assert_eq!(decimal("3", "14", IT, false), "tre virgola quattordici");
        assert_eq!(decimal("0", "05", IT, false), "zero virgola zero cinque");
        assert_eq!(decimal("2", "0", IT, true), "due punto zero");
        assert_eq!(
            decimal("1", "2345", IT, false),
            "uno virgola due tre quattro cinque"
        );
    }

    #[test]
    fn digits_and_other_language() {
        assert_eq!(digits("2490", EN), "two four nine zero");
        assert_eq!(digits("02", IT), "zero due");
        assert_eq!(cardinal(42, Lang::Other), "42");
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(roman_value("XIX"), Some(19));
        assert_eq!(roman_value("XXIII"), Some(23));
        assert_eq!(roman_value("MCMXC"), Some(1990));
        assert_eq!(roman_value("IIII"), None);
        assert_eq!(roman_value("VX"), None);
        assert_eq!(roman_value("ABC"), None);
    }
}
