//! Character classes, entity decoding and backslash unescaping.

use std::borrow::Cow;

use crate::tables::{ENTITIES, PUNCTUATION, WHITESPACE};

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Unicode punctuation as defined by CommonMark: general categories P and S.
#[inline]
pub(crate) fn is_punctuation(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_punctuation()
    } else {
        in_ranges(PUNCTUATION, c)
    }
}

/// Unicode whitespace as defined by CommonMark: category Zs, tab, line feed,
/// form feed and carriage return.
#[inline]
pub(crate) fn is_whitespace(c: char) -> bool {
    if c.is_ascii() {
        matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r')
    } else {
        in_ranges(WHITESPACE, c)
    }
}

#[inline]
pub(crate) fn is_space_or_tab(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Looks up an HTML5 named character reference, without `&` and `;`.
pub(crate) fn lookup_entity(name: &str) -> Option<&'static str> {
    ENTITIES
        .binary_search_by(|&(n, _)| n.cmp(name))
        .ok()
        .map(|i| ENTITIES[i].1)
}

/// Parses an entity or numeric character reference at the start of `s`
/// (which starts with `&`). Returns the decoded text and the length matched.
pub(crate) fn parse_entity(s: &str) -> Option<(Cow<'static, str>, usize)> {
    let b = s.as_bytes();
    debug_assert_eq!(b.first(), Some(&b'&'));
    if b.get(1) == Some(&b'#') {
        let (hex, digits_start) = match b.get(2) {
            Some(b'x' | b'X') => (true, 3),
            _ => (false, 2),
        };
        let max = if hex { 6 } else { 7 };
        let mut i = digits_start;
        let mut value: u32 = 0;
        while i < b.len() && i - digits_start < max {
            let d = match (hex, b[i]) {
                (_, c @ b'0'..=b'9') => (c - b'0') as u32,
                (true, c @ b'a'..=b'f') => (c - b'a' + 10) as u32,
                (true, c @ b'A'..=b'F') => (c - b'A' + 10) as u32,
                _ => break,
            };
            value = value * if hex { 16 } else { 10 } + d;
            i += 1;
        }
        if i == digits_start || b.get(i) != Some(&b';') {
            return None;
        }
        let c = match char::from_u32(value) {
            Some(c) if value != 0 => c,
            _ => '\u{FFFD}',
        };
        Some((Cow::Owned(c.to_string()), i + 1))
    } else {
        let mut i = 1;
        while i < b.len() && i <= 32 && b[i].is_ascii_alphanumeric() {
            i += 1;
        }
        if i < 3 || b.get(i) != Some(&b';') || !b[1].is_ascii_alphabetic() {
            return None;
        }
        lookup_entity(&s[1..i]).map(|v| (Cow::Borrowed(v), i + 1))
    }
}

/// Resolves backslash escapes and entities, as in link destinations, titles
/// and info strings.
pub(crate) fn unescape(s: &str) -> Cow<'_, str> {
    let b = s.as_bytes();
    let Some(first) = b.iter().position(|&c| c == b'\\' || c == b'&') else {
        return Cow::Borrowed(s);
    };
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut i = first;
    while i < b.len() {
        match b[i] {
            b'\\' if i + 1 < b.len() && b[i + 1].is_ascii_punctuation() => {
                out.push_str(&s[last..i]);
                last = i + 1;
                i += 2;
            }
            b'&' => {
                if let Some((text, len)) = parse_entity(&s[i..]) {
                    out.push_str(&s[last..i]);
                    out.push_str(&text);
                    i += len;
                    last = i;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    if last == 0 && out.is_empty() {
        return Cow::Borrowed(s);
    }
    out.push_str(&s[last..]);
    Cow::Owned(out)
}

/// Normalizes a link label (without its brackets) for reference matching:
/// trims, collapses internal whitespace and applies Unicode case folding.
pub(crate) fn normalize_label(label: &str) -> String {
    let mut collapsed = String::with_capacity(label.len());
    for word in label
        .split([' ', '\t', '\n', '\r'])
        .filter(|w| !w.is_empty())
    {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    if collapsed.is_ascii() {
        collapsed.make_ascii_uppercase();
        collapsed
    } else {
        // Lowercasing then uppercasing approximates full case folding
        // (for example `ẞ` and `SS` both become `SS`).
        collapsed.to_lowercase().to_uppercase()
    }
}

/// Decodes the UTF-8 character ending just before byte `i` of `s`.
#[inline]
pub(crate) fn char_before(s: &str, i: usize) -> Option<char> {
    s[..i].chars().next_back()
}

/// Decodes the UTF-8 character starting at byte `i` of `s`.
#[inline]
pub(crate) fn char_at(s: &str, i: usize) -> Option<char> {
    s[i..].chars().next()
}
