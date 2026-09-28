//! Article links (0.12, #258, P17): the main text of a web page saved as a
//! note item (`source: url:<link>`), which the Audio tab can then read
//! aloud like any other item (#256; only with the experimental module on).
//!
//! - **Network**: the page is fetched through [`super::direct::fetch_bytes`]
//!   — the same rules as a transcribed link (#123/#216): http(s) only, no
//!   credentials, hosts on this computer or the local network refused unless
//!   the run ticks *Allow local network addresses*, every redirect hop
//!   checked and pinned to the checked addresses, never through a system
//!   proxy, the model downloads' timeouts, and a size cap
//!   ([`MAX_PAGE_BYTES`]). No request is made from here otherwise: no
//!   images, scripts, styles or other pages.
//! - **What is accepted**: an HTML (or XHTML) page. A direct audio or video
//!   link, a video platform page, or any other content type is refused with
//!   a message pointing to *Transcribe*. Out of scope: logins, paywalls and
//!   pages that build their text with JavaScript (no script ever runs) —
//!   they come back with [`TooLittleText`] instead of an empty item.
//! - **Charset**: the `Content-Type` header's `charset`, else a byte-order
//!   mark, else `<meta charset>` / `http-equiv` in the first 1024 bytes,
//!   else UTF-8 when the bytes are valid UTF-8, else windows-1252 (the
//!   HTML standard's fallback for `iso-8859-1`), via `encoding_rs`.
//! - **Extraction**: `dom_smoothie` (MIT, a Readability.js port over the
//!   `dom_query`/`html5ever` parser already in the tree), markdown output:
//!   headings, paragraphs and lists kept, links reduced to their text,
//!   images, scripts, navigation, ads and comments dropped.
//! - **Item**: a `note` (not a transcription: there is no audio to align,
//!   no speakers, no subtitles), title from the page (`og:title`, then
//!   `<title>`, then `<h1>`, else the link's host), date now, `language`
//!   from `<html lang>` when it names one, one segment per markdown block —
//!   the same shape as a note from the archive API (#251).

use super::{direct, is_web_page, names_media_file, parse_link, sniff_extension, LinkKind};
use crate::archive::{self, ItemMeta, ItemType, Segment, SegmentsFile};
use anyhow::{bail, Result};
use encoding_rs::Encoding;
use reqwest::Url;
use std::path::Path;

/// Largest page accepted: well above any article's HTML (inline styles and
/// scripts included), small enough to parse in memory.
pub const MAX_PAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Elements the extractor may walk: bounds the work on a hostile page.
pub const MAX_ELEMENTS: usize = 200_000;

/// Fewer characters of text than this after extraction: not an article.
pub const MIN_ARTICLE_CHARS: usize = 300;

/// `Accept` sent for an article.
const ACCEPT_HTML: &str = "text/html, application/xhtml+xml;q=0.9, */*;q=0.1";

/// Bytes scanned for a `<meta charset>` (the HTML standard's prescan).
const META_PRESCAN_BYTES: usize = 1024;

/// The page holds too little readable text to make an item.
#[derive(Debug)]
pub struct TooLittleText {
    pub chars: usize,
}

impl std::fmt::Display for TooLittleText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the page has too little readable text ({} characters). It may need a login, sit \
             behind a paywall or build its text with JavaScript, which Sussurro doesn't run — \
             nothing was saved",
            self.chars
        )
    }
}

impl std::error::Error for TooLittleText {}

/// What was extracted from a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Article {
    /// The page's title (may be empty: the caller falls back to the link).
    pub title: String,
    /// Markdown blocks: headings, paragraphs, lists (one block each).
    pub blocks: Vec<String>,
    /// Primary language subtag of `<html lang>` (`en`, `it`), or empty.
    pub language: String,
}

/// What the user adds to the item: a title replacing the page's (when not
/// empty), and New's default tags and categories.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    pub title: String,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
}

/// Labels trimmed, empty ones dropped, repeats (ignoring case) kept once.
fn clean_labels(labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in labels {
        let l = l.trim();
        if !l.is_empty() && !out.iter().any(|o| o.to_lowercase() == l.to_lowercase()) {
            out.push(l.to_string());
        }
    }
    out
}

/// A saved article.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Saved {
    pub id: String,
    pub title: String,
    pub paragraphs: usize,
    pub language: String,
}

// ---- what may be fetched ---------------------------------------------------

/// Refuse, before any request, a link that is plainly not an article: a
/// video platform or a link naming an audio/video file. Pure (no DNS).
pub fn check_link(input: &str) -> Result<Url> {
    let link = parse_link(input)?;
    if link.kind == LinkKind::Platform {
        bail!(
            "{} is a video site: use Transcribe to turn the video's audio into text",
            link.url.host_str().unwrap_or("this link")
        );
    }
    if names_media_file(&link.url) {
        bail!("the link names an audio or video file: use Transcribe instead");
    }
    Ok(link.url)
}

/// Whether the response is a page to extract; else why not. Pure.
pub fn check_content(content_type: Option<&str>, head: &[u8]) -> Result<()> {
    let media_type = content_type
        .map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|m| !m.is_empty());
    let is_media = |m: &str| m.starts_with("audio/") || m.starts_with("video/");
    if media_type.as_deref().is_some_and(is_media) || sniff_extension(head).is_some() {
        bail!("the link opens an audio or video file, not an article: use Transcribe instead");
    }
    match media_type.as_deref() {
        Some("text/html" | "application/xhtml+xml") => Ok(()),
        // No type, or the generic one: the bytes decide.
        None | Some("application/octet-stream") if is_web_page(None, head) => Ok(()),
        Some(m) => {
            bail!("the link opens a {m} document, not a web page — only HTML articles can be saved")
        }
        None => bail!("the link doesn't open a web page — only HTML articles can be saved"),
    }
}

// ---- charset -----------------------------------------------------------------

/// The `charset=` of a `Content-Type` value, if encoding_rs knows it. Pure.
fn charset_param(content_type: &str) -> Option<&'static Encoding> {
    content_type.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| Encoding::for_label(v.trim().trim_matches(['"', '\'']).as_bytes()))
            .flatten()
    })
}

/// `<meta charset="…">` or `<meta http-equiv="Content-Type" content="…;
/// charset=…">` in the first [`META_PRESCAN_BYTES`]. Pure.
fn meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    let head = &bytes[..bytes.len().min(META_PRESCAN_BYTES)];
    // ASCII-compatible prescan: the markup itself is ASCII in every charset
    // a page may declare this way.
    let text: String = head
        .iter()
        .map(|&b| {
            if b.is_ascii() {
                b.to_ascii_lowercase() as char
            } else {
                ' '
            }
        })
        .collect();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("<meta") {
        rest = &rest[i + 5..];
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        let Some(j) = tag.find("charset") else {
            continue;
        };
        let value = tag[j + 7..].trim_start();
        let Some(value) = value.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start().trim_start_matches(['"', '\'']);
        let end = value
            .find(|c: char| c == '"' || c == '\'' || c == ';' || c == '/' || c.is_whitespace())
            .unwrap_or(value.len());
        let enc = Encoding::for_label(&value.as_bytes()[..end])?;
        // A page can't really be UTF-16 if its markup reads as ASCII.
        return Some(
            if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE {
                encoding_rs::UTF_8
            } else {
                enc
            },
        );
    }
    None
}

/// The page's text: header charset, then BOM, then `<meta>`, then UTF-8 if
/// valid, else windows-1252. Malformed bytes become U+FFFD. Pure.
pub fn decode_html(bytes: &[u8], content_type: Option<&str>) -> String {
    let declared = content_type.and_then(charset_param);
    let (enc, bom_len) = match Encoding::for_bom(bytes) {
        // A BOM wins over a `<meta>` (and, as in browsers, over the header).
        Some((enc, n)) => (enc, n),
        None => (
            declared.or_else(|| meta_charset(bytes)).unwrap_or_else(|| {
                if std::str::from_utf8(bytes).is_ok() {
                    encoding_rs::UTF_8
                } else {
                    encoding_rs::WINDOWS_1252
                }
            }),
            0,
        ),
    };
    let (text, _) = enc.decode_without_bom_handling(&bytes[bom_len..]);
    text.into_owned()
}

// ---- extraction --------------------------------------------------------------

/// `[text](target)` → `text`, `![alt](src)` → nothing, and the `<` `>`
/// autolink brackets dropped. Pure.
fn strip_links(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let image = chars[i] == '!' && chars.get(i + 1) == Some(&'[');
        if chars[i] == '[' || image {
            let open = if image { i + 1 } else { i };
            if let Some((text_end, target_end)) = link_span(&chars, open) {
                if !image {
                    let inner: String = chars[open + 1..text_end].iter().collect();
                    out.push_str(&strip_links(&inner));
                }
                i = target_end + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// For `[` at `open`: the index of its `]` and of the `)` closing the
/// `(target)` right after it, brackets nested. None when it isn't a link.
fn link_span(chars: &[char], open: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut close = None;
    for (k, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut depth = 0usize;
    for (k, &c) in chars.iter().enumerate().skip(close + 1) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((close, k));
                }
            }
            _ => {}
        }
    }
    None
}

/// `\.` → `.`: the extractor escapes markdown punctuation, which would
/// otherwise reach the transcript, the search index and read-aloud. Pure.
fn unescape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if next.is_ascii_punctuation() {
                    out.push(next);
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// The number of an ordered-list line (`1. text`), if it is one. Pure.
fn ordered_item(line: &str) -> Option<&str> {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    (digits > 0 && line[digits..].starts_with(". ")).then(|| &line[digits + 2..])
}

/// Markdown from the extractor → blocks: split on blank lines, escapes
/// removed, links to their text, images and empty lines dropped, spaces
/// inside a line collapsed, ordered lists numbered 1, 2, 3 (the extractor
/// writes `1.` on every item), one block per list item, a block that is
/// only markup (`---`, `-`) dropped. Pure.
pub fn markdown_blocks(markdown: &str) -> Vec<String> {
    let text = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks = Vec::new();
    for raw in crate::api::archive_write::paragraphs(&text) {
        let lines: Vec<String> = raw
            .lines()
            .map(|l| {
                let l = strip_links(l);
                let l = unescape(&l);
                // Keep the list/heading indentation marker, collapse the rest.
                let indent: String = l.chars().take_while(|c| *c == ' ').collect();
                let body = l.split_whitespace().collect::<Vec<_>>().join(" ");
                if body.is_empty() {
                    String::new()
                } else {
                    format!("{indent}{body}")
                }
            })
            .filter(|l| !l.is_empty())
            .collect();
        let mut n = 0;
        let lines: Vec<String> = lines
            .into_iter()
            .map(|l| match ordered_item(&l) {
                Some(rest) => {
                    n += 1;
                    format!("{n}. {rest}")
                }
                None => {
                    // An indented line belongs to the item above; anything
                    // else ends the list.
                    if !l.starts_with(' ') {
                        n = 0;
                    }
                    l
                }
            })
            .collect();
        // A list becomes one block per item (its indented lines with it):
        // one line each in the transcript, one segment each to edit.
        let is_item = |l: &str| {
            l.starts_with("- ")
                || l.starts_with("* ")
                || l.starts_with("+ ")
                || ordered_item(l).is_some()
        };
        let pieces: Vec<String> = if lines.first().is_some_and(|l| is_item(l)) {
            let mut pieces: Vec<String> = Vec::new();
            for l in lines {
                match pieces.last_mut() {
                    Some(p) if !is_item(&l) => {
                        p.push('\n');
                        p.push_str(&l);
                    }
                    _ => pieces.push(l),
                }
            }
            pieces
        } else {
            vec![lines.join("\n")]
        };
        blocks.extend(
            pieces
                .into_iter()
                .filter(|b| b.chars().any(char::is_alphanumeric)),
        );
    }
    blocks
}

/// Characters of readable text in `blocks` (letters and digits). Pure.
fn text_chars(blocks: &[String]) -> usize {
    blocks
        .iter()
        .flat_map(|b| b.chars())
        .filter(|c| c.is_alphanumeric())
        .count()
}

/// The primary subtag of a `lang` attribute (`en-GB` → `en`), or empty
/// when it isn't one. Pure.
fn primary_language(lang: Option<&str>) -> String {
    let Some(lang) = lang else {
        return String::new();
    };
    let primary = lang.trim().split(['-', '_']).next().unwrap_or("");
    if (2..=3).contains(&primary.len()) && primary.chars().all(|c| c.is_ascii_alphabetic()) {
        primary.to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// Extract the main text of `html`. Refuses a page with less than
/// [`MIN_ARTICLE_CHARS`] of text ([`TooLittleText`]). Pure.
pub fn extract(html: &str) -> Result<Article> {
    use dom_smoothie::{Config, Readability, ReadabilityError, TextMode};
    let cfg = Config {
        text_mode: TextMode::Markdown,
        max_elements_to_parse: MAX_ELEMENTS,
        ..Default::default()
    };
    let mut r = Readability::new(html, None, Some(cfg))?;
    let parsed = match r.parse() {
        Ok(a) => a,
        Err(ReadabilityError::GrabFailed) => return Err(TooLittleText { chars: 0 }.into()),
        Err(ReadabilityError::TooManyElements(..)) => {
            bail!("the page is too large to read (more than {MAX_ELEMENTS} elements)")
        }
        Err(e) => return Err(e.into()),
    };
    let blocks = markdown_blocks(&parsed.text_content);
    let chars = text_chars(&blocks);
    if chars < MIN_ARTICLE_CHARS {
        return Err(TooLittleText { chars }.into());
    }
    Ok(Article {
        title: parsed
            .title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        blocks,
        language: primary_language(parsed.lang.as_deref()),
    })
}

// ---- fetch + item ----------------------------------------------------------

/// Fetch `url` with the link rules (see the module docs) and extract it.
pub fn fetch_article(url: &Url, allow_local: bool) -> Result<Article> {
    let page = direct::fetch_bytes(url, allow_local, ACCEPT_HTML, MAX_PAGE_BYTES)?;
    check_content(page.content_type.as_deref(), &page.bytes)?;
    let html = decode_html(&page.bytes, page.content_type.as_deref());
    extract(&html)
}

/// The note's frontmatter and segments (pure): one segment per block, the
/// user's title first, then the page's, then the link's host.
pub fn build_item(
    article: &Article,
    url: &Url,
    extras: &Extras,
    date: &str,
) -> (ItemMeta, SegmentsFile) {
    let segments = article
        .blocks
        .iter()
        .enumerate()
        .map(|(i, block)| {
            // No audio: times only space the blocks apart, so the
            // transcript renders one paragraph per segment (as #251).
            let at = i as u64 * archive::render::PARAGRAPH_GAP_MS;
            Segment {
                id: i as u32,
                start_ms: at,
                end_ms: at,
                raw: block.clone(),
                text: block.clone(),
                ..Default::default()
            }
        })
        .collect();
    let title = [extras.title.trim(), article.title.trim()]
        .into_iter()
        .find(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| super::title_from_url(url));
    let meta = ItemMeta {
        item_type: ItemType::Note,
        title,
        date: date.to_string(),
        source: super::source_label(url),
        language: article.language.clone(),
        tags: clean_labels(&extras.tags),
        categories: clean_labels(&extras.categories),
        ..Default::default()
    };
    (
        meta,
        SegmentsFile {
            segments,
            ..Default::default()
        },
    )
}

/// Check, fetch, extract and save the article at `input` as a new note in
/// `archive_dir`, indexed at once (`index` = the search index file).
pub fn save_article(
    archive_dir: &Path,
    index: &Path,
    input: &str,
    allow_local: bool,
    extras: &Extras,
) -> Result<Saved> {
    let url = check_link(input)?;
    let article = fetch_article(&url, allow_local)?;
    let date = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let (meta, segments) = build_item(&article, &url, extras, &date);
    let id = archive::create_item(archive_dir, &meta, &segments)?;
    // Best effort: a failed index update is caught up by the next sync.
    if let Err(e) = archive::Index::open(archive_dir, index).and_then(|mut idx| idx.index_item(&id))
    {
        eprintln!("archive index: update failed ({e:#})");
    }
    Ok(Saved {
        id,
        title: meta.title,
        paragraphs: segments.segments.len(),
        language: meta.language,
    })
}

#[cfg(test)]
mod tests {
    use super::super::direct::tests::serve;
    use super::*;

    const EN: &str = include_str!("testdata/article-en.html");
    const IT: &str = include_str!("testdata/article-it.html");
    const SHORT: &str = include_str!("testdata/article-short.html");
    const LATIN1: &[u8] = include_bytes!("testdata/article-latin1.html");

    fn all(a: &Article) -> String {
        a.blocks.join("\n\n")
    }

    #[test]
    fn english_article_keeps_the_text_and_drops_the_noise() {
        let a = extract(EN).unwrap();
        let text = all(&a);
        assert_eq!(a.title, "Why lighthouses blink in patterns");
        assert_eq!(a.language, "en");
        assert!(text.contains("Every lighthouse along a coast"), "{text}");
        // Markdown escapes don't reach the item.
        assert!(
            text.contains("rhythm of light and darkness. Sailors"),
            "{text}"
        );
        assert!(
            text.contains("same so that old charts remain useful"),
            "{text}"
        );
        // Headings and lists survive as markdown.
        assert!(
            a.blocks
                .iter()
                .any(|b| b.starts_with('#') && b.contains("How the patterns are made")),
            "{text}"
        );
        assert!(text.contains("- Fixed: a steady light"), "{text}");
        // A link keeps its text, never its target.
        assert!(text.contains("about the Fresnel lens and how"), "{text}");
        assert!(
            !text.contains("example.org") && !text.contains("]("),
            "{text}"
        );
        for noise in [
            "TRACKING-SCRIPT-TEXT",
            "SCRIPT-WRITTEN-TEXT",
            "BUY CHEAP BOATS",
            "COMMENT-TEXT",
            "Subscribe now",
            "Ten boats you must see",
            "Cookie settings",
            "font-family",
        ] {
            assert!(!text.contains(noise), "{noise} kept:\n{text}");
        }
    }

    #[test]
    fn italian_article_keeps_accents_lists_and_title() {
        let a = extract(IT).unwrap();
        let text = all(&a);
        assert_eq!(a.language, "it");
        assert!(a.title.starts_with("Il pane di casa"), "{}", a.title);
        assert!(
            text.contains("la fretta è la vera nemica del pane"),
            "{text}"
        );
        assert!(text.contains("2. Dare la forma della pagnotta"), "{text}");
        assert!(!text.contains('\\'), "{text}");
        for noise in [
            "TESTO-DELLO-SCRIPT",
            "PUBBLICITÀ",
            "COMMENTO-UTENTE",
            "Tutti i diritti",
        ] {
            assert!(!text.contains(noise), "{noise} kept:\n{text}");
        }
    }

    #[test]
    fn a_page_with_too_little_text_is_refused() {
        let e = extract(SHORT).unwrap_err();
        assert!(e.is::<TooLittleText>(), "{e}");
        assert!(e.to_string().contains("JavaScript"), "{e}");
        let e = extract("").unwrap_err();
        assert!(e.is::<TooLittleText>(), "{e}");
    }

    #[test]
    fn charsets_come_from_the_header_the_bom_or_the_meta_tag() {
        // <meta charset="iso-8859-1">, no header: windows-1252 decoding.
        let html = decode_html(LATIN1, None);
        assert!(html.contains("Café crème à la française"), "{html}");
        let a = extract(&html).unwrap();
        assert_eq!(a.language, "fr");
        assert!(all(&a).contains("fraîchement moulu"), "{}", all(&a));

        // The header wins over the meta tag.
        let utf8 = "<meta charset=\"iso-8859-1\"><p>è</p>".as_bytes();
        assert!(decode_html(utf8, Some("text/html; charset=UTF-8")).contains("<p>è</p>"));
        assert!(decode_html(utf8, None).contains("<p>Ã¨</p>"));
        // A BOM wins over both.
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(utf8);
        assert!(decode_html(&bom, Some("text/html; charset=iso-8859-1")).contains("<p>è</p>"));
        // http-equiv form, quoted header value.
        let eq = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\"><p>\xe8</p>";
        assert!(decode_html(eq, None).contains("<p>è</p>"));
        assert!(
            decode_html(b"<p>\xe8</p>", Some("text/html; charset=\"latin1\"")).contains("<p>è</p>")
        );
        // Nothing declared: UTF-8 when valid, else windows-1252.
        assert!(decode_html("<p>è</p>".as_bytes(), None).contains("<p>è</p>"));
        assert!(decode_html(b"<p>\xe8</p>", None).contains("<p>è</p>"));
        // An unknown label falls through to the next rule.
        assert!(decode_html("<p>è</p>".as_bytes(), Some("text/html; charset=bogus")).contains("è"));
    }

    #[test]
    fn only_html_pages_are_accepted() {
        assert!(check_content(Some("text/html; charset=utf-8"), b"").is_ok());
        assert!(check_content(Some("application/xhtml+xml"), b"").is_ok());
        assert!(check_content(None, b"<!DOCTYPE html><p>").is_ok());
        assert!(check_content(Some("application/octet-stream"), b"<html>").is_ok());
        let e = check_content(Some("audio/mpeg"), b"ID3")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Transcribe"), "{e}");
        // Media mislabelled as HTML is caught by its bytes.
        let e = check_content(Some("text/html"), b"RIFF\0\0\0\0WAVEfmt ")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Transcribe"), "{e}");
        let e = check_content(Some("application/pdf"), b"%PDF-1.7")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("application/pdf") && e.contains("only HTML"),
            "{e}"
        );
        assert!(check_content(None, b"plain words").is_err());
    }

    #[test]
    fn media_and_video_site_links_go_to_transcribe() {
        for link in [
            "https://www.youtube.com/watch?v=abc",
            "https://example.com/podcast/episode.mp3",
        ] {
            let e = check_link(link).unwrap_err().to_string();
            assert!(e.contains("Transcribe"), "{link}: {e}");
        }
        assert!(check_link("ftp://example.com/a").is_err());
        assert!(check_link("https://user:pw@example.com/a").is_err());
        assert!(check_link("https://example.com/news/2026/lighthouses").is_ok());
    }

    #[test]
    fn markdown_blocks_strip_links_and_images() {
        let md = "# Title\n\nSee [the *docs*](https://x.test/a_(b)) and ![logo](/l.png) now.\n\n\
                  ![](only-image.png)\n\n---\n\n- one\n- [two](/t)\n\n  spaced   out  ";
        assert_eq!(
            markdown_blocks(md),
            vec![
                "# Title",
                "See the *docs* and now.",
                "- one",
                "- two",
                "spaced out"
            ]
        );
        // Escapes go, ordered lists are numbered.
        assert_eq!(
            markdown_blocks(
                "End\\. A \\*star\\* and C:\\\\dir \\x\n\n1. one\n1. two\n  more\n1. three"
            ),
            vec![
                "End. A *star* and C:\\dir \\x",
                "1. one",
                "2. two\n  more",
                "3. three"
            ]
        );
        // Brackets that aren't links stay.
        assert_eq!(markdown_blocks("a [note] here"), vec!["a [note] here"]);
    }

    #[test]
    fn primary_language_takes_the_first_subtag() {
        assert_eq!(primary_language(Some("en-GB")), "en");
        assert_eq!(primary_language(Some(" IT ")), "it");
        assert_eq!(primary_language(Some("x")), "");
        assert_eq!(primary_language(Some("en!")), "");
        assert_eq!(primary_language(None), "");
    }

    fn html_route(body: &str) -> super::super::direct::tests::Route {
        (
            200,
            vec![("Content-Type", "text/html; charset=utf-8".into())],
            body.as_bytes().to_vec(),
        )
    }

    #[test]
    fn the_article_path_uses_the_checked_client() {
        let srv = serve(vec![
            ("/story", html_route(EN)),
            ("/go", (302, vec![("Location", "/story".into())], vec![])),
            (
                "/audio",
                (
                    200,
                    vec![("Content-Type", "audio/mpeg".into())],
                    b"ID3\x04".to_vec(),
                ),
            ),
            ("/login", html_route(SHORT)),
        ]);
        let url = |p: &str| Url::parse(&format!("{}{p}", srv.base)).unwrap();

        // 127.0.0.1 is this computer: refused without the tick.
        let e = fetch_article(&url("/go"), false).unwrap_err().to_string();
        assert!(e.contains("Allow local network addresses"), "{e}");

        // With it, the redirect is followed and the page extracted.
        let a = fetch_article(&url("/go"), true).unwrap();
        assert_eq!(a.title, "Why lighthouses blink in patterns");

        // (Each redirect hop going through the same address rules is
        // covered by `direct`'s own tests: this path adds no client.)

        let e = fetch_article(&url("/audio"), true).unwrap_err().to_string();
        assert!(e.contains("Transcribe"), "{e}");
        let e = fetch_article(&url("/login"), true).unwrap_err();
        assert!(e.is::<TooLittleText>(), "{e}");
        let e = fetch_article(&url("/missing"), true)
            .unwrap_err()
            .to_string();
        assert!(e.contains("404"), "{e}");
    }

    #[test]
    fn a_saved_article_is_an_indexed_note_with_one_segment_per_block() {
        let srv = serve(vec![("/news/lighthouses", html_route(EN))]);
        let dir = tempfile::tempdir().unwrap();
        let archive_dir = dir.path().join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let index = dir.path().join("index.sqlite");
        let link = format!("{}/news/lighthouses#top", srv.base);

        let saved = save_article(&archive_dir, &index, &link, true, &Extras::default()).unwrap();
        assert_eq!(saved.title, "Why lighthouses blink in patterns");
        assert_eq!(saved.language, "en");

        let item = archive::read_item(&archive_dir, &saved.id).unwrap();
        assert_eq!(item.meta.item_type, ItemType::Note);
        // The fragment is not part of the source.
        assert_eq!(
            item.meta.source,
            format!("url:{}/news/lighthouses", srv.base)
        );
        assert_eq!(item.meta.language, "en");
        assert!(!item.meta.date.is_empty());
        let segs = &item.segments.segments;
        assert_eq!(segs.len(), saved.paragraphs);
        assert!(segs.len() >= 5, "{segs:?}");
        for (i, s) in segs.iter().enumerate() {
            assert_eq!(s.id, i as u32);
            assert_eq!(s.raw, s.text);
            assert_eq!(s.start_ms, i as u64 * archive::render::PARAGRAPH_GAP_MS);
        }
        // The transcript holds the blocks as paragraphs, and it is searchable.
        assert!(
            item.body.contains("Every lighthouse along a coast"),
            "{}",
            item.body
        );
        let hits = archive::with_index(&archive_dir, &index, |idx| {
            idx.search("characteristic", &Default::default())
        })
        .unwrap();
        assert!(hits.iter().any(|h| h.id == saved.id), "{hits:?}");

        // A title from the user wins; a failed page leaves nothing behind.
        let extras = Extras {
            title: "  My title ".into(),
            tags: vec!["read".into(), " Read ".into(), " ".into(), "later".into()],
            categories: vec!["web".into()],
        };
        let saved2 = save_article(&archive_dir, &index, &link, true, &extras).unwrap();
        assert_eq!(saved2.title, "My title");
        let meta2 = archive::read_item(&archive_dir, &saved2.id).unwrap().meta;
        assert_eq!(meta2.tags, vec!["read", "later"]);
        assert_eq!(meta2.categories, vec!["web"]);
        let before = std::fs::read_dir(&archive_dir).unwrap().count();
        let nope = format!("{}/nope", srv.base);
        assert!(save_article(&archive_dir, &index, &nope, true, &Extras::default()).is_err());
        assert_eq!(std::fs::read_dir(&archive_dir).unwrap().count(), before);
    }

    #[test]
    fn the_title_falls_back_to_the_host() {
        let a = Article {
            title: " ".into(),
            blocks: vec!["text".into()],
            language: String::new(),
        };
        let url = Url::parse("https://www.example.com/").unwrap();
        let (meta, segs) = build_item(&a, &url, &Extras::default(), "2026-09-28T10:00:00+02:00");
        assert_eq!(meta.title, "example.com");
        assert_eq!(meta.source, "url:https://www.example.com/");
        assert_eq!(meta.date, "2026-09-28T10:00:00+02:00");
        assert_eq!(segs.segments.len(), 1);
    }

    /// A real page through the real network rules. Needs the network:
    /// `SUSSURRO_LIVE_ARTICLE=<url> cargo test live_article -- --ignored`.
    #[test]
    #[ignore]
    fn live_article_extraction() {
        let link = std::env::var("SUSSURRO_LIVE_ARTICLE")
            .unwrap_or_else(|_| "https://en.wikipedia.org/wiki/Lighthouse".into());
        let a = fetch_article(&check_link(&link).unwrap(), false).unwrap();
        eprintln!("{} ({}): {} blocks", a.title, a.language, a.blocks.len());
        assert!(!a.title.is_empty());
        assert!(text_chars(&a.blocks) >= MIN_ARTICLE_CHARS);
    }
}
