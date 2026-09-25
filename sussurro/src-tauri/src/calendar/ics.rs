//! iCalendar (RFC 5545) syntax: content lines and components. A small
//! strict reader — no crate — because only a handful of properties matter
//! here and every one of them is checked by the caller.
//!
//! - Lines end in CRLF or LF; a line starting with a space or a tab
//!   continues the previous one (folding, §3.1).
//! - `NAME;PARAM=a,"b;c":value` — parameter values may be quoted (and then
//!   hold `;`, `:` and `,`); names are case-insensitive.
//! - Components nest with `BEGIN:X` / `END:X`. A mismatched `END`, content
//!   outside `VCALENDAR` or a file that stops inside a component is an
//!   error; a single unreadable line inside a component is skipped and
//!   counted, so one odd line doesn't hide a whole calendar.

use anyhow::{bail, Result};

/// Longest unfolded content line accepted (a long description is fine).
pub const MAX_LINE: usize = 1024 * 1024;
/// Deepest component nesting accepted (real files use 3).
pub const MAX_DEPTH: usize = 16;

/// One content line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Upper-case name, e.g. `DTSTART`.
    pub name: String,
    /// Upper-case parameter names with their (unquoted) values.
    pub params: Vec<(String, Vec<String>)>,
    /// The raw value (text escapes not yet undone, see [`unescape`]).
    pub value: String,
}

impl Property {
    /// First value of parameter `name` (upper case).
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.first())
            .map(String::as_str)
    }
}

/// A component (`VCALENDAR`, `VEVENT`, `VTIMEZONE`, `STANDARD`…).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Component {
    /// Upper-case name.
    pub name: String,
    pub props: Vec<Property>,
    pub children: Vec<Component>,
}

impl Component {
    /// First property called `name`.
    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.props.iter().find(|p| p.name == name)
    }

    /// Every property called `name`.
    pub fn props_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.props.iter().filter(move |p| p.name == name)
    }
}

/// The calendars of a file (usually one `VCALENDAR`) and how many content
/// lines could not be read.
#[derive(Debug, Default)]
pub struct Parsed {
    pub calendars: Vec<Component>,
    pub skipped_lines: usize,
}

/// Undo folding: logical lines with their 1-based starting line number.
fn unfold(text: &str) -> Result<Vec<(usize, String)>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if let Some((_, last)) = out.last_mut() {
                if last.len() + rest.len() > MAX_LINE {
                    bail!("line {} is too long", i + 1);
                }
                last.push_str(rest);
                continue;
            }
        }
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > MAX_LINE {
            bail!("line {} is too long", i + 1);
        }
        out.push((i + 1, line.to_string()));
    }
    Ok(out)
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// Parse one unfolded content line; `None` when it isn't one.
pub fn parse_line(line: &str) -> Option<Property> {
    let name_end = line.find([';', ':'])?;
    let name = &line[..name_end];
    if name.is_empty() || !name.chars().all(is_name_char) {
        return None;
    }
    let mut params = Vec::new();
    let mut rest = &line[name_end..];
    while let Some(r) = rest.strip_prefix(';') {
        let eq = r.find('=')?;
        let pname = &r[..eq];
        if pname.is_empty() || !pname.chars().all(is_name_char) {
            return None;
        }
        let mut r = &r[eq + 1..];
        let mut values = Vec::new();
        loop {
            if let Some(q) = r.strip_prefix('"') {
                let close = q.find('"')?;
                values.push(q[..close].to_string());
                r = &q[close + 1..];
            } else {
                let end = r.find([',', ';', ':']).unwrap_or(r.len());
                values.push(r[..end].to_string());
                r = &r[end..];
            }
            match r.strip_prefix(',') {
                Some(more) => r = more,
                None => break,
            }
        }
        params.push((pname.to_ascii_uppercase(), values));
        rest = r;
    }
    let value = rest.strip_prefix(':')?;
    Some(Property {
        name: name.to_ascii_uppercase(),
        params,
        value: value.to_string(),
    })
}

/// Parse a whole file into its `VCALENDAR` components.
pub fn parse(text: &str) -> Result<Parsed> {
    let mut parsed = Parsed::default();
    let mut stack: Vec<Component> = Vec::new();
    for (n, line) in unfold(text)? {
        let Some(prop) = parse_line(&line) else {
            if stack.is_empty() {
                bail!("line {n} is not calendar data");
            }
            parsed.skipped_lines += 1;
            continue;
        };
        match prop.name.as_str() {
            "BEGIN" => {
                let name = prop.value.trim().to_ascii_uppercase();
                if stack.is_empty() && name != "VCALENDAR" {
                    bail!("line {n}: {name} outside a VCALENDAR");
                }
                if stack.len() >= MAX_DEPTH {
                    bail!("line {n}: components nested too deeply");
                }
                stack.push(Component {
                    name,
                    ..Default::default()
                });
            }
            "END" => {
                let name = prop.value.trim().to_ascii_uppercase();
                let Some(done) = stack.pop() else {
                    bail!("line {n}: END:{name} without a BEGIN");
                };
                if done.name != name {
                    bail!("line {n}: END:{name} closes BEGIN:{}", done.name);
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => parsed.calendars.push(done),
                }
            }
            _ => match stack.last_mut() {
                Some(c) => c.props.push(prop),
                None => bail!("line {n}: {} outside a VCALENDAR", prop.name),
            },
        }
    }
    if let Some(open) = stack.last() {
        bail!("the file ends inside {} (is it cut short?)", open.name);
    }
    if parsed.calendars.is_empty() {
        bail!("no VCALENDAR in the file");
    }
    Ok(parsed)
}

/// Undo TEXT escaping (§3.3.11): `\\`, `\;`, `\,`, `\n` / `\N`.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_lines_are_joined_with_crlf_or_lf() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Weekly sy\r\n nc with\r\n\t the team\r\nDESCRIPTION:a\n  b\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let p = parse(text).unwrap();
        let ev = &p.calendars[0].children[0];
        assert_eq!(
            ev.prop("SUMMARY").unwrap().value,
            "Weekly sync with the team"
        );
        // Only the first whitespace character is the fold marker.
        assert_eq!(ev.prop("DESCRIPTION").unwrap().value, "a b");
    }

    #[test]
    fn parameters_with_quotes_and_lists() {
        let p = parse_line(
            r#"ATTENDEE;CN="Rossi, Anna";ROLE=REQ-PARTICIPANT;DELEGATED-FROM="mailto:a@x.it","mailto:b@x.it":mailto:anna@example.com"#,
        )
        .unwrap();
        assert_eq!(p.name, "ATTENDEE");
        assert_eq!(p.param("CN"), Some("Rossi, Anna"));
        assert_eq!(p.param("ROLE"), Some("REQ-PARTICIPANT"));
        assert_eq!(p.params[2].1, ["mailto:a@x.it", "mailto:b@x.it"]);
        assert_eq!(p.value, "mailto:anna@example.com");
        // The value may contain colons; lower-case names are fine.
        let p = parse_line("dtstart;tzid=Europe/Rome:20260925T100000").unwrap();
        assert_eq!(
            (p.name.as_str(), p.param("TZID")),
            ("DTSTART", Some("Europe/Rome"))
        );
        let p = parse_line("URL:https://example.com/a:b").unwrap();
        assert_eq!(p.value, "https://example.com/a:b");
        for bad in [
            "no colon here",
            ":value",
            "BAD NAME:x",
            r#"X;CN="open:x"#,
            "X;=a:b",
        ] {
            assert!(parse_line(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn text_escapes() {
        assert_eq!(
            unescape(r"Q3\, review\; notes\nline 2 \\ end"),
            "Q3, review; notes\nline 2 \\ end"
        );
    }

    #[test]
    fn malformed_structure_is_an_error() {
        for (text, want) in [
            ("hello world", "not calendar data"),
            ("<!DOCTYPE html><html>", "not calendar data"),
            ("BEGIN:VEVENT\nEND:VEVENT\n", "outside a VCALENDAR"),
            (
                "BEGIN:VCALENDAR\nBEGIN:VEVENT\nEND:VCALENDAR\n",
                "closes BEGIN:VEVENT",
            ),
            ("BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:x\n", "cut short"),
            ("END:VCALENDAR\n", "without a BEGIN"),
            ("", "no VCALENDAR"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(want), "{text:?}: {e}");
        }
        // A junk line inside a component is skipped and counted.
        let p = parse("BEGIN:VCALENDAR\nthis is junk\nVERSION:2.0\nEND:VCALENDAR\n").unwrap();
        assert_eq!(p.skipped_lines, 1);
        assert_eq!(p.calendars[0].prop("VERSION").unwrap().value, "2.0");
    }

    #[test]
    fn a_bom_and_blank_lines_are_ignored() {
        let p = parse("\u{feff}BEGIN:VCALENDAR\r\n\r\nEND:VCALENDAR").unwrap();
        assert_eq!(p.calendars.len(), 1);
    }
}
