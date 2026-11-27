//! GFM extended autolinks: `www.`, `http://`, `https://`, `ftp://` and email
//! addresses recognized in plain text.

use std::borrow::Cow;

use crate::ast::{Inline, Link, LinkKind};

/// Replaces the autolinks found in the text nodes of `inlines`.
pub(crate) fn extend<'a>(_src: &'a str, inlines: &mut Vec<Inline<'a>>) {
    if !inlines
        .iter()
        .any(|i| matches!(i, Inline::Text(t) if has_candidate(t)))
    {
        return;
    }
    let old = std::mem::take(inlines);
    for inline in old {
        match inline {
            Inline::Text(t) if has_candidate(&t) => split(t, inlines),
            other => inlines.push(other),
        }
    }
}

fn has_candidate(t: &str) -> bool {
    let b = t.as_bytes();
    b.iter().enumerate().any(|(i, &c)| match c {
        b'@' => true,
        b':' => b[i + 1..].starts_with(b"//"),
        b'.' => i >= 3 && &b[i - 3..i] == b"www",
        _ => false,
    })
}

fn sub<'a>(c: &Cow<'a, str>, start: usize, end: usize) -> Cow<'a, str> {
    match c {
        Cow::Borrowed(s) => Cow::Borrowed(&s[start..end]),
        Cow::Owned(s) => Cow::Owned(s[start..end].to_owned()),
    }
}

fn split<'a>(text: Cow<'a, str>, out: &mut Vec<Inline<'a>>) {
    let b = text.as_bytes();
    let mut done = 0;
    let mut i = 0;
    while i < b.len() {
        let found = match b[i] {
            b'w' => www(b, i).map(|end| (i, end, "http://", LinkKind::Autolink)),
            b'h' | b'f' => url(b, i).map(|end| (i, end, "", LinkKind::Autolink)),
            b'@' => email(b, i, done).map(|(start, end)| (start, end, "mailto:", LinkKind::Email)),
            _ => None,
        };
        let Some((start, end, prefix, kind)) = found else {
            i += 1;
            continue;
        };
        if start > done {
            out.push(Inline::Text(sub(&text, done, start)));
        }
        let label = sub(&text, start, end);
        let url = if prefix.is_empty() {
            label.clone()
        } else {
            Cow::Owned(format!("{prefix}{label}"))
        };
        out.push(Inline::Link(Link {
            url,
            title: Cow::Borrowed(""),
            kind,
            children: vec![Inline::Text(label)],
        }));
        done = end;
        i = end;
    }
    if done < b.len() {
        out.push(Inline::Text(sub(&text, done, b.len())));
    }
}

/// Whether an autolink may start at `i`: at the start of the text, or after
/// whitespace or one of `*`, `_`, `~` and `(`.
fn boundary_before(b: &[u8], i: usize) -> bool {
    i == 0
        || matches!(
            b[i - 1],
            b' ' | b'\t' | b'\n' | b'\r' | b'*' | b'_' | b'~' | b'('
        )
}

fn is_domain_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c >= 0x80
}

/// Scans a valid domain at `i`. Returns its end.
fn domain(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while j < b.len() && (is_domain_char(b[j]) || b[j] == b'.') {
        j += 1;
    }
    // A trailing period is punctuation, not part of the domain.
    let mut end = j;
    while end > i && b[end - 1] == b'.' {
        end -= 1;
    }
    let d = &b[i..end];
    let segments: Vec<&[u8]> = d.split(|&c| c == b'.').collect();
    if segments.len() < 2 || segments.iter().any(|s| s.is_empty()) {
        return None;
    }
    if segments[segments.len() - 2..]
        .iter()
        .any(|s| s.contains(&b'_'))
    {
        return None;
    }
    Some(j)
}

/// Extends a link over its path, then trims trailing punctuation.
fn path(b: &[u8], start: usize, domain_end: usize) -> usize {
    let mut end = domain_end;
    while end < b.len() && !matches!(b[end], b' ' | b'\t' | b'\n' | b'\r' | b'<') {
        end += 1;
    }
    loop {
        match b[end - 1] {
            b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~' => end -= 1,
            b')' => {
                let s = &b[start..end];
                let open = s.iter().filter(|&&c| c == b'(').count();
                let close = s.iter().filter(|&&c| c == b')').count();
                if close > open {
                    end -= 1;
                } else {
                    break;
                }
            }
            b';' => {
                let mut k = end - 1;
                while k > start && b[k - 1].is_ascii_alphanumeric() {
                    k -= 1;
                }
                if k < end - 1 && k > start && b[k - 1] == b'&' {
                    end = k - 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
        if end <= start {
            break;
        }
    }
    end
}

fn www(b: &[u8], i: usize) -> Option<usize> {
    if !b[i..].starts_with(b"www.") || !boundary_before(b, i) {
        return None;
    }
    let d = domain(b, i)?;
    let end = path(b, i, d);
    // The trimmed link must still hold a valid domain.
    domain(&b[..end], i)?;
    Some(end)
}

fn url(b: &[u8], i: usize) -> Option<usize> {
    let rest = &b[i..];
    let scheme = [&b"https://"[..], b"http://", b"ftp://"]
        .into_iter()
        .find(|s| rest.starts_with(s))?;
    if !boundary_before(b, i) {
        return None;
    }
    let host = i + scheme.len();
    let d = domain(b, host)?;
    let end = path(b, i, d);
    domain(&b[..end], host)?;
    Some(end)
}

fn email(b: &[u8], at: usize, done: usize) -> Option<(usize, usize)> {
    let mut start = at;
    while start > done
        && (b[start - 1].is_ascii_alphanumeric()
            || matches!(b[start - 1], b'.' | b'-' | b'_' | b'+'))
    {
        start -= 1;
    }
    if start == at {
        return None;
    }
    let mut end = at + 1;
    let mut periods = 0;
    while end < b.len() {
        match b[end] {
            c if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' => end += 1,
            b'.' if b
                .get(end + 1)
                .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') =>
            {
                periods += 1;
                end += 1;
            }
            _ => break,
        }
    }
    if periods == 0 || matches!(b[end - 1], b'-' | b'_') {
        return None;
    }
    Some((start, end))
}
