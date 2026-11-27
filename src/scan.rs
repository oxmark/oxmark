//! Hand-written scanners for the syntax shared by the block and inline
//! parsers: link labels, destinations and titles, raw HTML and autolinks.
//!
//! Every scanner takes the subject bytes and a start position and returns
//! the position just after the construct, or `None` when it does not match.

#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c')
}

/// Skips spaces and tabs.
#[inline]
pub(crate) fn skip_spaces(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

/// Skips spaces and tabs, at most one line ending, then spaces and tabs.
pub(crate) fn spnl(b: &[u8], i: usize) -> usize {
    let mut i = skip_spaces(b, i);
    if i < b.len() && b[i] == b'\n' {
        i = skip_spaces(b, i + 1);
    }
    i
}

/// A link label `[...]` starting at `i`. Returns the end, after `]`.
pub(crate) fn link_label(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    let limit = i + 1 + 999;
    while j < b.len() && j < limit + 1 {
        match b[j] {
            b'\\' if j + 1 < b.len() => j += 2,
            b'[' => return None,
            b']' => return (j <= limit).then_some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// A link destination starting at `i`.
/// Returns `(content_start, content_end, end)`.
pub(crate) fn link_destination(b: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    if b.get(i) == Some(&b'<') {
        let mut j = i + 1;
        while j < b.len() {
            match b[j] {
                b'>' => return Some((i + 1, j, j + 1)),
                b'\\' if j + 1 < b.len() && b[j + 1].is_ascii_punctuation() => j += 2,
                b'<' | b'\n' | b'\r' => return None,
                _ => j += 1,
            }
        }
        return None;
    }
    let mut j = i;
    let mut depth = 0usize;
    while j < b.len() {
        match b[j] {
            b'\\' if j + 1 < b.len() && b[j + 1].is_ascii_punctuation() => j += 2,
            b'(' => {
                depth += 1;
                // Bound the nesting, as allowed by the spec, so that
                // unclosed destinations cannot make parsing quadratic.
                if depth > 32 {
                    return None;
                }
                j += 1;
            }
            b')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                j += 1;
            }
            c if c <= b' ' || c == 0x7f => break,
            _ => j += 1,
        }
    }
    if depth != 0 || (j == i && b.get(j) != Some(&b')')) {
        return None;
    }
    Some((i, j, j))
}

/// A link title (`"..."`, `'...'` or `(...)`) starting at `i`.
/// Returns `(content_start, content_end, end)`.
pub(crate) fn link_title(b: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    let close = match b.get(i)? {
        b'"' => b'"',
        b'\'' => b'\'',
        b'(' => b')',
        _ => return None,
    };
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' if j + 1 < b.len() => j += 2,
            c if c == close => return Some((i + 1, j, j + 1)),
            b'(' if close == b')' => return None,
            _ => j += 1,
        }
    }
    None
}

fn tag_name(b: &[u8], i: usize) -> Option<usize> {
    if !b.get(i)?.is_ascii_alphabetic() {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
        j += 1;
    }
    Some(j)
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    i
}

fn attribute_value(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        q @ (b'"' | b'\'') => {
            let len = b[i + 1..].iter().position(|&c| c == q)?;
            Some(i + 1 + len + 1)
        }
        _ => {
            let mut j = i;
            while j < b.len()
                && !matches!(b[j], b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
                && b[j] > b' '
            {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

/// An HTML open tag `<name attr="value">` starting at `i` (at `<`).
pub(crate) fn open_tag(b: &[u8], i: usize) -> Option<usize> {
    let mut j = tag_name(b, i + 1)?;
    loop {
        let k = skip_ws(b, j);
        if k == j {
            break;
        }
        if !b
            .get(k)
            .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_' || c == b':')
        {
            j = k;
            break;
        }
        let mut n = k + 1;
        while n < b.len()
            && (b[n].is_ascii_alphanumeric() || matches!(b[n], b'_' | b'.' | b':' | b'-'))
        {
            n += 1;
        }
        j = n;
        let v = skip_ws(b, n);
        if b.get(v) == Some(&b'=') {
            j = attribute_value(b, skip_ws(b, v + 1))?;
        }
    }
    if b.get(j) == Some(&b'/') {
        j += 1;
    }
    (b.get(j) == Some(&b'>')).then_some(j + 1)
}

/// An HTML closing tag `</name>` starting at `i` (at `<`).
pub(crate) fn closing_tag(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i + 1) != Some(&b'/') {
        return None;
    }
    let j = skip_ws(b, tag_name(b, i + 2)?);
    (b.get(j) == Some(&b'>')).then_some(j + 1)
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > b.len() {
        return None;
    }
    b[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Remembers which unterminated constructs were already scanned to the end of
/// the subject, so pathological inputs stay linear.
#[derive(Default)]
pub(crate) struct HtmlMemo {
    no_comment_end: bool,
    no_pi_end: bool,
    no_cdata_end: bool,
    no_decl_end: bool,
}

/// Any inline raw HTML construct starting at `i` (at `<`).
pub(crate) fn html_tag(b: &[u8], i: usize, memo: &mut HtmlMemo) -> Option<usize> {
    match *b.get(i + 1)? {
        b'/' => closing_tag(b, i),
        b'?' => {
            if memo.no_pi_end {
                return None;
            }
            let end = find(b, i + 2, b"?>");
            memo.no_pi_end = end.is_none();
            end.map(|e| e + 2)
        }
        b'!' => {
            let rest = &b[i + 2..];
            if rest.starts_with(b"--") {
                if rest[2..].starts_with(b">") {
                    return Some(i + 5);
                }
                if rest[2..].starts_with(b"->") {
                    return Some(i + 6);
                }
                if memo.no_comment_end {
                    return None;
                }
                let end = find(b, i + 4, b"-->");
                memo.no_comment_end = end.is_none();
                end.map(|e| e + 3)
            } else if rest.starts_with(b"[CDATA[") {
                if memo.no_cdata_end {
                    return None;
                }
                let end = find(b, i + 9, b"]]>");
                memo.no_cdata_end = end.is_none();
                end.map(|e| e + 3)
            } else if rest.first().is_some_and(u8::is_ascii_alphabetic) {
                if memo.no_decl_end {
                    return None;
                }
                let end = b[i + 2..].iter().position(|&c| c == b'>');
                memo.no_decl_end = end.is_none();
                end.map(|e| i + 2 + e + 1)
            } else {
                None
            }
        }
        _ => open_tag(b, i),
    }
}

/// A URI autolink `<scheme:...>` starting at `i` (at `<`).
pub(crate) fn uri_autolink(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if !b.get(j)?.is_ascii_alphabetic() {
        return None;
    }
    j += 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'.' | b'+' | b'-')) {
        j += 1;
    }
    let scheme_len = j - i - 1;
    if !(2..=32).contains(&scheme_len) || b.get(j) != Some(&b':') {
        return None;
    }
    j += 1;
    while j < b.len() {
        match b[j] {
            b'>' => return Some(j + 1),
            b'<' => return None,
            c if c <= b' ' => return None,
            _ => j += 1,
        }
    }
    None
}

#[inline]
pub(crate) fn is_email_local(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&c)
}

/// The domain part of an email address starting at `i`: labels of
/// alphanumerics and inner hyphens (at most 63 bytes) separated by dots.
/// Returns the end of the last label.
fn email_domain(b: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let start = i;
        if !b.get(i)?.is_ascii_alphanumeric() {
            return None;
        }
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
            i += 1;
        }
        if i - start > 63 || b[i - 1] == b'-' {
            return None;
        }
        if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_alphanumeric) {
            i += 1;
        } else {
            return Some(i);
        }
    }
}

/// An email autolink `<user@example.com>` starting at `i` (at `<`).
pub(crate) fn email_autolink(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < b.len() && is_email_local(b[j]) {
        j += 1;
    }
    if j == i + 1 || b.get(j) != Some(&b'@') {
        return None;
    }
    let end = email_domain(b, j + 1)?;
    (b.get(end) == Some(&b'>')).then_some(end + 1)
}
