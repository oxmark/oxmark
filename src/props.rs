//! Comark attribute lists: `{.class #id key="value" bool :json='{...}'}`.
//!
//! A port of Comark's `searchProps`, including its leniency: a list without
//! its closing `}` runs to the end of the text, invalid keys are skipped and
//! nothing is unescaped.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::ast::{Attribute, Attributes};
use crate::block::to_cow;

/// A parsed attribute list.
pub(crate) struct Props<'a> {
    pub attrs: Attributes<'a>,
    /// The position after the list.
    pub end: usize,
    /// Whether the closing `}` was found.
    pub closed: bool,
}

struct Scanner<'s> {
    s: &'s str,
    b: &'s [u8],
    i: usize,
    /// Bracket groups already scanned: opener position to closer position,
    /// or `None` when the group does not close.
    groups: HashMap<usize, Option<usize>>,
}

impl<'s> Scanner<'s> {
    #[inline]
    fn at(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Skips a backslash and the character after it.
    fn skip_escape(&mut self) {
        self.i += 2;
        while self.i < self.b.len() && !self.s.is_char_boundary(self.i) {
            self.i += 1;
        }
    }

    fn slice(&self, start: usize) -> &'s str {
        &self.s[start.min(self.b.len())..self.i.min(self.b.len())]
    }

    /// Advances past the current character to the next one in `stop`.
    fn until(&mut self, stop: &[u8], bracket_aware: bool) {
        while self.i < self.b.len() {
            self.i += 1;
            if self.at() == Some(b'\\') {
                self.skip_escape();
            }
            if bracket_aware {
                if let Some(close) = self.at().and_then(bracket_pair) {
                    let open = self.i;
                    if self.bracket(close) {
                        continue;
                    }
                    self.i = open;
                }
            }
            if self.at().is_some_and(|c| stop.contains(&c)) {
                break;
            }
        }
    }

    /// Scans a bracket group from its opener. Returns whether it closed; if
    /// so, `self.i` is on the closer.
    ///
    /// Comark scans groups recursively. This is the same scan with an
    /// explicit stack, and each opener's outcome is remembered so a group is
    /// scanned once however many times it is reached.
    fn bracket(&mut self, close: u8) -> bool {
        let mut stack = vec![(close, self.i)];
        while self.i < self.b.len() {
            self.i += 1;
            let Some(c) = self.at() else { break };
            let (want, _) = *stack.last().unwrap();
            if matches!(c, b'"' | b'\'' | b'`') {
                self.until(&[c], false);
            } else if c == want {
                let (_, open) = stack.pop().unwrap();
                self.groups.insert(open, Some(self.i));
                if stack.is_empty() {
                    return true;
                }
            } else if let Some(inner) = bracket_pair(c) {
                match self.groups.get(&self.i) {
                    Some(&Some(end)) => self.i = end,
                    Some(None) => break,
                    None => stack.push((inner, self.i)),
                }
            }
        }
        // Every open group runs to the end of the text.
        for (_, open) in stack {
            self.groups.insert(open, None);
        }
        self.i = self.b.len();
        false
    }

    fn value(&mut self) -> usize {
        let start = self.i;
        match self.at() {
            Some(c) if bracket_pair(c).is_some() => {
                self.bracket(bracket_pair(c).unwrap());
                self.i += 1;
            }
            Some(q @ (b'"' | b'\'' | b'`')) => {
                self.until(&[q], false);
                self.i += 1;
            }
            _ => self.until(b" }", false),
        }
        start
    }
}

#[inline]
fn bracket_pair(c: u8) -> Option<u8> {
    match c {
        b'[' => Some(b']'),
        b'{' => Some(b'}'),
        b'(' => Some(b')'),
        _ => None,
    }
}

/// Whether `key` matches `^:?[a-z_][a-z0-9_-]*$` (case-insensitive).
fn valid_key(key: &str) -> bool {
    let b = key.as_bytes();
    let b = b.strip_prefix(b":").unwrap_or(b);
    match b.split_first() {
        Some((&first, rest)) => {
            (first.is_ascii_alphabetic() || first == b'_')
                && rest
                    .iter()
                    .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        }
        None => false,
    }
}

/// Removes matching quotes around a value (`"a"`, `'a'`, `` `a` ``).
fn unquote(v: &str) -> &str {
    let b = v.as_bytes();
    if b.len() >= 2
        && matches!(b[0], b'"' | b'\'' | b'`')
        && b[b.len() - 1] == b[0]
        && !v[1..v.len() - 1].contains(['\n', '\r'])
    {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// Parses the attribute list starting at `start` (a `{`) in `s`. Returns
/// `None` for `{{`, which is template syntax.
pub(crate) fn search<'a>(src: &'a str, s: &str, start: usize) -> Option<Props<'a>> {
    debug_assert_eq!(s.as_bytes().get(start), Some(&b'{'));
    if s.as_bytes().get(start + 1) == Some(&b'{') {
        return None;
    }
    let mut sc = Scanner {
        s,
        b: s.as_bytes(),
        i: start + 1,
        groups: HashMap::new(),
    };
    let mut attrs = Vec::new();
    let mut closed = false;
    let push = |attrs: &mut Vec<Attribute<'a>>, name: Cow<'a, str>, value: &str| {
        attrs.push(Attribute {
            name,
            value: to_cow(src, unquote(value)),
        });
    };
    while sc.i < sc.b.len() {
        match sc.b[sc.i] {
            b'\\' => sc.skip_escape(),
            b'}' => {
                sc.i += 1;
                closed = true;
                break;
            }
            b' ' => sc.i += 1,
            b'.' => {
                sc.i += 1;
                let from = sc.i;
                sc.until(b" #.}", true);
                push(&mut attrs, Cow::Borrowed("class"), sc.slice(from));
            }
            b'#' => {
                sc.i += 1;
                let from = sc.i;
                sc.until(b" #.}", false);
                push(&mut attrs, Cow::Borrowed("id"), sc.slice(from));
            }
            _ => {
                let from = sc.i;
                while sc.i < sc.b.len() {
                    sc.i += 1;
                    if matches!(sc.at(), Some(b' ' | b'}' | b'=')) {
                        break;
                    }
                }
                let key = sc.slice(from).trim();
                if sc.at() == Some(b'=') {
                    sc.i += 1;
                    let v = sc.value();
                    let value = sc.slice(v);
                    if valid_key(key) {
                        push(&mut attrs, to_cow(src, key), value);
                    }
                } else if valid_key(key) {
                    let name = if key.starts_with(':') {
                        to_cow(src, key)
                    } else {
                        Cow::Owned(format!(":{key}"))
                    };
                    push(&mut attrs, name, "true");
                }
            }
        }
    }
    Some(Props {
        attrs: Attributes::from_list(attrs, true),
        end: sc.i.min(sc.b.len()),
        closed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(s: &str) -> Vec<(String, String)> {
        search(s, s, 0)
            .unwrap()
            .attrs
            .iter()
            .map(|a| (a.name.to_string(), a.value.to_string()))
            .collect()
    }

    #[test]
    fn parses_comark_props() {
        assert_eq!(
            props(r#"{.a .b #id key="v w" bool :obj='{"k": "v"}' x=1}"#),
            [
                ("class", "a b"),
                ("id", "id"),
                ("key", "v w"),
                (":bool", "true"),
                (":obj", r#"{"k": "v"}"#),
                ("x", "1"),
            ]
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
        );
        assert_eq!(props("{.bg-[#000] .p-2}")[0].1, "bg-[#000] p-2");
        assert!(search("{{ x }}", "{{ x }}", 0).is_none());
        let unclosed = search("{.a", "{.a", 0).unwrap();
        assert!(!unclosed.closed);
        assert_eq!(unclosed.end, 3);
        assert_eq!(props("{1x=2 a\"b}"), Vec::<(String, String)>::new());
    }
}
