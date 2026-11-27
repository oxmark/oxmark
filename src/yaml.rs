//! A small YAML reader for component props and frontmatter.
//!
//! It covers what Markdown documents use: block mappings and sequences,
//! flow collections, plain, quoted and block scalars, and comments. Scalars
//! resolve like js-yaml's JSON schema, which Comark uses. Anything else is
//! an error, and the caller keeps the text as it is.

use std::fmt::Write;

/// A YAML value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    /// A number, already written as JavaScript would.
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Writes the value as JSON, like `JSON.stringify`.
    pub(crate) fn write_json(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(n),
            Value::String(s) => write_json_string(s, out),
            Value::Array(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_json(out);
                }
                out.push(']');
            }
            Value::Object(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(k, out);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }

    pub(crate) fn to_json(&self) -> String {
        let mut s = String::new();
        self.write_json(&mut s);
        s
    }
}

pub(crate) fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Nesting limit, against hostile input.
const MAX_DEPTH: usize = 64;

#[derive(Debug)]
pub(crate) struct Error;

type Result<T> = std::result::Result<T, Error>;

struct Line<'s> {
    indent: usize,
    /// The text after the indentation, without a trailing comment.
    text: &'s str,
    /// The text after the indentation, as written.
    raw: &'s str,
}

/// Parses a YAML document. An empty document gives `None`.
pub(crate) fn parse(src: &str) -> Result<Option<Value>> {
    let mut lines = Vec::new();
    for raw in src.lines() {
        let content = raw.trim_start_matches(' ');
        if content.starts_with('\t') {
            return Err(Error);
        }
        let indent = raw.len() - content.len();
        let text = strip_comment(content).trim_end();
        if text.is_empty() {
            // Blank and comment lines matter only inside block scalars.
            lines.push(Line {
                indent: usize::MAX,
                text: "",
                raw: content,
            });
            continue;
        }
        if text == "---" || text == "..." {
            if lines.iter().any(|l: &Line| l.indent != usize::MAX) {
                return Err(Error);
            }
            continue;
        }
        lines.push(Line {
            indent,
            text,
            raw: content,
        });
    }
    let mut p = Parser { lines, i: 0 };
    p.skip_blank();
    if p.i >= p.lines.len() {
        return Ok(None);
    }
    let indent = p.lines[p.i].indent;
    let v = p.block(indent, 0)?;
    p.skip_blank();
    if p.i < p.lines.len() {
        return Err(Error);
    }
    Ok(Some(v))
}

/// Removes a `# comment` that follows whitespace, outside quotes.
fn strip_comment(s: &str) -> &str {
    let b = s.as_bytes();
    let mut quote = 0u8;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' && quote == b'"' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else if c == b'#' && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') {
            return &s[..i];
        } else if (c == b'"' || c == b'\'')
            && (i == 0 || matches!(b[i - 1], b' ' | b'[' | b'{' | b',' | b':' | b'-'))
        {
            quote = c;
        }
        i += 1;
    }
    s
}

struct Parser<'s> {
    lines: Vec<Line<'s>>,
    i: usize,
}

impl<'s> Parser<'s> {
    fn skip_blank(&mut self) {
        while self.i < self.lines.len() && self.lines[self.i].indent == usize::MAX {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<&Line<'s>> {
        self.skip_blank();
        self.lines.get(self.i)
    }

    /// Parses the block node whose first line is at `indent`.
    fn block(&mut self, indent: usize, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error);
        }
        let line = self.peek().ok_or(Error)?;
        if is_seq_item(line.text) {
            self.sequence(indent, depth)
        } else if split_key(line.text).is_some() {
            self.mapping(indent, depth)
        } else {
            // A multi-line plain or quoted scalar.
            let mut text = String::from(line.text);
            self.i += 1;
            while let Some(next) = self.peek() {
                if next.indent < indent {
                    break;
                }
                text.push(' ');
                text.push_str(next.text);
                self.i += 1;
            }
            scalar(&text, depth)
        }
    }

    fn sequence(&mut self, indent: usize, depth: usize) -> Result<Value> {
        let mut items = Vec::new();
        while let Some(line) = self.peek() {
            if line.indent != indent || !is_seq_item(line.text) {
                if line.indent >= indent {
                    return Err(Error);
                }
                break;
            }
            let rest = line.text[1..].trim_start_matches(' ');
            let offset = line.text.len() - rest.len();
            if rest.is_empty() {
                self.i += 1;
                items.push(self.nested(indent, depth)?);
            } else {
                // The item content starts a new block at its own column.
                let raw_offset = line.raw.len() - line.raw[offset..].len();
                let l = &mut self.lines[self.i];
                l.indent += raw_offset;
                l.text = rest;
                l.raw = &l.raw[raw_offset..];
                let item_indent = l.indent;
                items.push(self.block_or_scalar(item_indent, depth)?);
            }
        }
        Ok(Value::Array(items))
    }

    fn mapping(&mut self, indent: usize, depth: usize) -> Result<Value> {
        let mut entries: Vec<(String, Value)> = Vec::new();
        while let Some(line) = self.peek() {
            if line.indent != indent {
                if line.indent > indent {
                    return Err(Error);
                }
                break;
            }
            let Some((key, rest)) = split_key(line.text) else {
                return Err(Error);
            };
            let key = match scalar(key, depth)? {
                Value::String(s) => s,
                Value::Null => "null".to_owned(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n,
                _ => return Err(Error),
            };
            if entries.iter().any(|(k, _)| *k == key) {
                return Err(Error);
            }
            self.i += 1;
            let value = if rest.is_empty() {
                // A sequence may sit at the same indentation as its key.
                match self.peek() {
                    Some(l) if l.indent == indent && is_seq_item(l.text) => {
                        self.sequence(indent, depth + 1)?
                    }
                    _ => self.nested(indent, depth)?,
                }
            } else if let Some(style) = block_scalar_header(rest) {
                self.block_scalar(indent, style)?
            } else {
                self.inline_value(rest, indent, depth)?
            };
            entries.push((key, value));
        }
        Ok(Value::Object(entries))
    }

    /// A value that may continue on more indented lines.
    fn inline_value(&mut self, first: &str, indent: usize, depth: usize) -> Result<Value> {
        let mut text = String::from(first);
        while let Some(next) = self.peek() {
            if next.indent <= indent {
                break;
            }
            text.push(' ');
            text.push_str(next.text);
            self.i += 1;
        }
        scalar(&text, depth)
    }

    /// The content of a sequence item or mapping value on its own line.
    fn block_or_scalar(&mut self, indent: usize, depth: usize) -> Result<Value> {
        let text = self.lines[self.i].text;
        if is_seq_item(text) || split_key(text).is_some() {
            return self.block(indent, depth + 1);
        }
        if let Some(style) = block_scalar_header(text) {
            self.i += 1;
            return self.block_scalar(indent.saturating_sub(1), style);
        }
        self.i += 1;
        self.inline_value(text, indent.saturating_sub(1), depth)
    }

    /// The more indented block after a `key:` or `-` line, or null.
    fn nested(&mut self, indent: usize, depth: usize) -> Result<Value> {
        match self.peek() {
            Some(l) if l.indent > indent => {
                let i = l.indent;
                self.block(i, depth + 1)
            }
            _ => Ok(Value::Null),
        }
    }

    fn block_scalar(&mut self, indent: usize, (literal, chomp): (bool, u8)) -> Result<Value> {
        let mut lines: Vec<String> = Vec::new();
        let mut block_indent = None;
        while self.i < self.lines.len() {
            let l = &self.lines[self.i];
            if l.indent == usize::MAX {
                lines.push(String::new());
                self.i += 1;
                continue;
            }
            if l.indent <= indent {
                break;
            }
            let bi = *block_indent.get_or_insert(l.indent);
            if l.indent < bi {
                break;
            }
            // Keep the extra indentation of the line.
            lines.push(format!("{}{}", " ".repeat(l.indent - bi), l.raw));
            self.i += 1;
        }
        // Trailing blank lines are handled by the chomping indicator.
        let mut trailing = 0;
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
            trailing += 1;
        }
        let mut out = String::new();
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                let folded =
                    !literal && !l.is_empty() && !lines[i - 1].is_empty() && !l.starts_with(' ');
                out.push(if folded { ' ' } else { '\n' });
            }
            out.push_str(l);
        }
        match chomp {
            b'-' => {}
            b'+' => {
                out.push('\n');
                for _ in 0..trailing {
                    out.push('\n');
                }
            }
            _ => {
                if !lines.is_empty() {
                    out.push('\n');
                }
            }
        }
        Ok(Value::String(out))
    }
}

fn is_seq_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// Parses `|`, `|-`, `>+`... Returns (literal, chomping).
fn block_scalar_header(text: &str) -> Option<(bool, u8)> {
    let b = text.as_bytes();
    let literal = match b.first() {
        Some(b'|') => true,
        Some(b'>') => false,
        _ => return None,
    };
    match &b[1..] {
        [] => Some((literal, 0)),
        [c @ (b'-' | b'+')] => Some((literal, *c)),
        _ => None,
    }
}

/// Splits `key: value` at the first `: ` (or final `:`) outside quotes and
/// brackets.
fn split_key(text: &str) -> Option<(&str, &str)> {
    let b = text.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'"' | b'\'')) {
        let q = b[0];
        i = 1;
        while i < b.len() && b[i] != q {
            if b[i] == b'\\' && q == b'"' {
                i += 1;
            }
            i += 1;
        }
        i += 1;
    } else if matches!(
        b.first(),
        Some(b'[' | b'{' | b'-' | b'?' | b'|' | b'>' | b'!' | b'&' | b'*')
    ) && !(b[0] == b'-' && b.get(1).is_some_and(|c| *c != b' '))
    {
        return None;
    }
    while i < b.len() {
        if b[i] == b':' && (i + 1 == b.len() || b[i + 1] == b' ') {
            let key = text[..i].trim_end();
            if key.is_empty() {
                return None;
            }
            return Some((key, text[i + 1..].trim_start()));
        }
        i += 1;
    }
    None
}

/// Parses an inline scalar or flow collection.
fn scalar(text: &str, depth: usize) -> Result<Value> {
    let text = text.trim();
    let mut p = Flow {
        b: text.as_bytes(),
        s: text,
        i: 0,
    };
    let v = p.value(depth, false)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(Error);
    }
    Ok(v)
}

struct Flow<'s> {
    s: &'s str,
    b: &'s [u8],
    i: usize,
}

impl Flow<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n') {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize, in_flow: bool) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error);
        }
        self.ws();
        match self.b.get(self.i) {
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b']') {
                        self.i += 1;
                        return Ok(Value::Array(items));
                    }
                    items.push(self.value(depth + 1, true)?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return Err(Error),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut entries = Vec::new();
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        return Ok(Value::Object(entries));
                    }
                    let key = match self.value(depth + 1, true)? {
                        Value::String(s) => s,
                        Value::Number(n) => n,
                        Value::Bool(b) => b.to_string(),
                        Value::Null => "null".to_owned(),
                        _ => return Err(Error),
                    };
                    self.ws();
                    let value = if self.b.get(self.i) == Some(&b':') {
                        self.i += 1;
                        self.value(depth + 1, true)?
                    } else {
                        Value::Null
                    };
                    entries.push((key, value));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {}
                        _ => return Err(Error),
                    }
                }
            }
            Some(b'"') => self.double_quoted().map(Value::String),
            Some(b'\'') => self.single_quoted().map(Value::String),
            _ => {
                let start = self.i;
                while self.i < self.b.len() {
                    let c = self.b[self.i];
                    if in_flow && matches!(c, b',' | b']' | b'}') {
                        break;
                    }
                    if c == b':'
                        && (self.i + 1 == self.b.len()
                            || matches!(self.b[self.i + 1], b' ' | b',' | b']' | b'}') && in_flow)
                    {
                        break;
                    }
                    self.i += 1;
                }
                let plain = self.s[start..self.i].trim();
                if !in_flow && plain.contains(": ") {
                    return Err(Error);
                }
                Ok(resolve(plain))
            }
        }
    }

    fn single_quoted(&mut self) -> Result<String> {
        self.i += 1;
        let mut out = String::new();
        let start = self.i;
        let mut from = start;
        while self.i < self.b.len() {
            if self.b[self.i] == b'\'' {
                out.push_str(&self.s[from..self.i]);
                if self.b.get(self.i + 1) == Some(&b'\'') {
                    out.push('\'');
                    self.i += 2;
                    from = self.i;
                    continue;
                }
                self.i += 1;
                return Ok(out);
            }
            self.i += 1;
        }
        Err(Error)
    }

    fn double_quoted(&mut self) -> Result<String> {
        self.i += 1;
        let mut out = String::new();
        let mut from = self.i;
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'"' => {
                    out.push_str(&self.s[from..self.i]);
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(&self.s[from..self.i]);
                    let c = *self.b.get(self.i + 1).ok_or(Error)?;
                    self.i += 2;
                    match c {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'0' => out.push('\0'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'"' | b'\\' | b'/' | b' ' => out.push(c as char),
                        b'u' | b'x' | b'U' => {
                            let n = match c {
                                b'x' => 2,
                                b'u' => 4,
                                _ => 8,
                            };
                            let hex = self.s.get(self.i..self.i + n).ok_or(Error)?;
                            let code = u32::from_str_radix(hex, 16).map_err(|_| Error)?;
                            out.push(char::from_u32(code).ok_or(Error)?);
                            self.i += n;
                        }
                        _ => return Err(Error),
                    }
                    from = self.i;
                }
                _ => self.i += 1,
            }
        }
        Err(Error)
    }
}

/// Resolves a plain scalar with js-yaml's JSON schema types.
fn resolve(s: &str) -> Value {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" | "-.inf" | "-.Inf" | "-.INF"
        | ".nan" | ".NaN" | ".NAN" => return Value::Number("null".to_owned()),
        _ => {}
    }
    let b = s.as_bytes();
    let (sign, digits) = match b.first() {
        Some(b'-') => ("-", &s[1..]),
        Some(b'+') => ("", &s[1..]),
        _ => ("", s),
    };
    let radix = |prefix: &str, radix: u32| -> Option<Value> {
        let body = digits.strip_prefix(prefix)?;
        let n = i128::from_str_radix(body, radix).ok()?;
        Some(Value::Number(format_number(
            if sign == "-" { -n } else { n } as f64,
        )))
    };
    if let Some(v) = radix("0x", 16)
        .or_else(|| radix("0o", 8))
        .or_else(|| radix("0b", 2))
    {
        return v;
    }
    let db = digits.as_bytes();
    if !db.is_empty() && db.iter().all(u8::is_ascii_digit) {
        let n: f64 = s.parse().unwrap_or(f64::NAN);
        return Value::Number(format_number(n));
    }
    if is_float(db) {
        if let Ok(n) = s.parse::<f64>() {
            return Value::Number(format_number(n));
        }
    }
    Value::String(s.to_owned())
}

/// `[-+]?(\.[0-9]+|[0-9][0-9]*)(\.[0-9]*)?([eE][-+]?[0-9]+)?`, without the sign.
fn is_float(b: &[u8]) -> bool {
    let mut i = 0;
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - s
    };
    if b.first() == Some(&b'.') {
        i = 1;
        if digits(&mut i) == 0 {
            return false;
        }
    } else {
        if digits(&mut i) == 0 {
            return false;
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i);
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

/// Formats a number like JavaScript's `Number.prototype.toString`.
fn format_number(n: f64) -> String {
    if !n.is_finite() {
        return "null".to_owned();
    }
    if n == 0.0 {
        return "0".to_owned();
    }
    let abs = n.abs();
    if (1e-7..1e21).contains(&abs) {
        let s = format!("{n}");
        return s;
    }
    // Exponent form: shortest digits, then `e+N` / `e-N`.
    let s = format!("{n:e}");
    match s.split_once('e') {
        Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(src: &str) -> String {
        parse(src)
            .unwrap()
            .map_or("undefined".into(), |v| v.to_json())
    }

    #[test]
    fn reads_markdown_yaml() {
        assert_eq!(
            json("title: My Component\ncount: 42\nenabled: true\nitems:\n  - First item\n  - Second item\nconfig:\n  theme: dark # comment\n  mode: 'auto'\n"),
            r#"{"title":"My Component","count":42,"enabled":true,"items":["First item","Second item"],"config":{"theme":"dark","mode":"auto"}}"#
        );
        assert_eq!(
            json("a:\n- x\n- y: 1\n  z: 2\n"),
            r#"{"a":["x",{"y":1,"z":2}]}"#
        );
        assert_eq!(
            json("a: [1, b, {c: d}]\nb: \"q\\n\"\nc: 1.50\nd: ~\n"),
            r#"{"a":[1,"b",{"c":"d"}],"b":"q\n","c":1.5,"d":null}"#
        );
        assert_eq!(
            json("text: |\n  line 1\n  line 2\nnext: >-\n  a\n  b\n"),
            r#"{"text":"line 1\nline 2\n","next":"a b"}"#
        );
        assert_eq!(json(""), "undefined");
        assert!(parse("a: b: c").is_err());
        assert!(parse("a: 1\na: 2").is_err());
    }
}
