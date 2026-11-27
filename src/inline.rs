//! Inline parsing (second phase).
//!
//! A port of the CommonMark reference algorithm: inlines are first built as a
//! doubly linked list in an arena so that emphasis and link processing can
//! move nodes around cheaply, then converted to the public tree.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::ast::{Inline, Link, LinkKind};
use crate::block::{lift, to_cow};
use crate::scan;
use crate::text::{
    char_at, char_before, is_punctuation, is_whitespace, normalize_label, parse_entity, unescape,
};
use crate::Options;

/// A link reference definition.
#[derive(Debug, Clone)]
pub(crate) struct LinkDef<'a> {
    pub url: Cow<'a, str>,
    pub title: Cow<'a, str>,
}

/// Link reference definitions, by normalized label.
pub(crate) type RefMap<'a> = HashMap<String, LinkDef<'a>>;

const NONE: usize = usize::MAX;

/// Maximum nesting of inline containers (emphasis, links, images). Deeper
/// constructs are left unparsed, which keeps recursion bounded.
const MAX_INLINE_DEPTH: u32 = 100;

/// Parses one link reference definition at the start of `text`.
/// Returns the number of bytes consumed, or 0 when there is none.
pub(crate) fn parse_reference<'a>(src: &'a str, text: &str, refs: &mut RefMap<'a>) -> usize {
    let b = text.as_bytes();
    let Some(label_end) = scan::link_label(b, 0) else {
        return 0;
    };
    if b.get(label_end) != Some(&b':') {
        return 0;
    }
    let p = scan::spnl(b, label_end + 1);
    let Some((ds, de, after_dest)) = scan::link_destination(b, p) else {
        return 0;
    };
    let at_line_end = |mut i: usize| -> Option<usize> {
        i = scan::skip_spaces(b, i);
        match b.get(i) {
            None => Some(i),
            Some(b'\n') => Some(i + 1),
            _ => None,
        }
    };
    let before_title = after_dest;
    let p = scan::spnl(b, after_dest);
    let mut title = None;
    let mut end = None;
    if p != before_title {
        if let Some((ts, te, after_title)) = scan::link_title(b, p) {
            if let Some(e) = at_line_end(after_title) {
                title = Some((ts, te));
                end = Some(e);
            }
        }
    }
    let end = match end {
        Some(e) => e,
        None => match at_line_end(before_title) {
            Some(e) => e,
            None => return 0,
        },
    };
    let label = normalize_label(&text[1..label_end - 1]);
    if label.is_empty() {
        return 0;
    }
    refs.entry(label).or_insert_with(|| LinkDef {
        url: lift(src, unescape(&text[ds..de])),
        title: title.map_or(Cow::Borrowed(""), |(ts, te)| {
            lift(src, unescape(&text[ts..te]))
        }),
    });
    end
}

#[derive(Debug)]
enum Kind<'a> {
    Root,
    Text(Cow<'a, str>),
    SoftBreak,
    HardBreak,
    Code(Cow<'a, str>),
    Html(Cow<'a, str>),
    Emphasis,
    Strong,
    Strikethrough,
    Link(Box<(Cow<'a, str>, Cow<'a, str>, LinkKind)>),
    Image(Box<(Cow<'a, str>, Cow<'a, str>)>),
    /// A footnote reference; its children are the label text, dropped.
    FootnoteRef(Cow<'a, str>, u32, u32),
    /// A container that would nest too deeply; its children are spliced
    /// into its parent when building the tree.
    Group,
}

#[derive(Debug)]
struct Node<'a> {
    kind: Kind<'a>,
    /// Nesting depth of the subtree (0 for leaves).
    depth: u32,
    parent: usize,
    prev: usize,
    next: usize,
    first: usize,
    last: usize,
}

#[derive(Debug)]
struct Delim {
    ch: u8,
    num: usize,
    orig: usize,
    node: usize,
    prev: usize,
    next: usize,
    can_open: bool,
    can_close: bool,
}

#[derive(Debug)]
struct Bracket {
    node: usize,
    prev_delim: usize,
    index: usize,
    image: bool,
    active: bool,
    bracket_after: bool,
}

/// Bytes that may start something other than plain text.
static SPECIAL: [bool; 256] = {
    let mut t = [false; 256];
    let chars = b"\n`[]\\!<&*_~";
    let mut i = 0;
    while i < chars.len() {
        t[chars[i] as usize] = true;
        i += 1;
    }
    t
};

struct Parser<'a, 's, 'r> {
    src: &'a str,
    subject: &'s str,
    b: &'s [u8],
    pos: usize,
    refs: &'r RefMap<'a>,
    footnotes: Footnotes<'a>,
    opts: &'r Options,
    nodes: Vec<Node<'a>>,
    delims: Vec<Delim>,
    delim_top: usize,
    brackets: Vec<Bracket>,
    html_memo: scan::HtmlMemo,
    /// Start of the last backtick run of each length, computed on the first
    /// code span so unmatched openers are detected in constant time.
    tick_runs: Option<HashMap<usize, usize>>,
    /// Bit 1 (2) is set when no closing `"` (`'`) exists for a link title.
    no_title_end: u8,
    /// Whether the subject may contain an extended autolink.
    autolink_candidates: bool,
}

/// Parses inline content.
pub(crate) fn parse<'a>(
    src: &'a str,
    subject: &str,
    refs: &RefMap<'a>,
    opts: &Options,
    scratch: &mut Scratch<'a>,
) -> Vec<Inline<'a>> {
    let mut nodes = std::mem::take(&mut scratch.nodes);
    nodes.clear();
    let mut delims = std::mem::take(&mut scratch.delims);
    delims.clear();
    let mut brackets = std::mem::take(&mut scratch.brackets);
    brackets.clear();
    let footnotes = std::mem::take(&mut scratch.footnotes);
    let mut p = Parser {
        src,
        subject,
        b: subject.as_bytes(),
        pos: 0,
        refs,
        footnotes,
        opts,
        nodes,
        delims,
        delim_top: NONE,
        brackets,
        html_memo: scan::HtmlMemo::default(),
        tick_runs: None,
        no_title_end: 0,
        autolink_candidates: opts.autolinks
            && (subject.contains('@') || subject.contains("://") || subject.contains("www.")),
    };
    p.nodes.push(Node {
        kind: Kind::Root,
        depth: 0,
        parent: NONE,
        prev: NONE,
        next: NONE,
        first: NONE,
        last: NONE,
    });
    p.run();
    let out = p.build(0, false);
    scratch.nodes = p.nodes;
    scratch.delims = p.delims;
    scratch.brackets = p.brackets;
    scratch.footnotes = p.footnotes;
    out
}

/// State kept between the inline parses of a document: reused buffers and
/// footnote numbering.
#[derive(Default)]
pub(crate) struct Scratch<'a> {
    nodes: Vec<Node<'a>>,
    delims: Vec<Delim>,
    brackets: Vec<Bracket>,
    pub(crate) footnotes: Footnotes<'a>,
}

/// Footnote definitions and the order in which they are referenced.
#[derive(Default)]
pub(crate) struct Footnotes<'a> {
    /// Content of each definition, by label.
    pub(crate) defs: HashMap<String, Cow<'a, str>>,
    /// Referenced labels in order, with their reference count.
    pub(crate) used: Vec<(Cow<'a, str>, u32)>,
    /// Position of each referenced label in `used`.
    index: HashMap<String, usize>,
}

impl<'a> Footnotes<'a> {
    /// Records a reference. Returns `(index, occurrence)`, both from 1.
    fn reference(&mut self, label: Cow<'a, str>) -> (u32, u32) {
        let pos = match self.index.get(label.as_ref()) {
            Some(&p) => p,
            None => {
                self.index.insert(label.to_string(), self.used.len());
                self.used.push((label, 0));
                self.used.len() - 1
            }
        };
        self.used[pos].1 += 1;
        (pos as u32 + 1, self.used[pos].1)
    }
}

/// Parses a single-line footnote definition `[^label]: text` at the start of
/// `text`. Returns the number of bytes consumed, or 0.
pub(crate) fn parse_footnote_def<'a>(
    src: &'a str,
    text: &str,
    defs: &mut HashMap<String, Cow<'a, str>>,
) -> usize {
    let b = text.as_bytes();
    if !text.starts_with("[^") {
        return 0;
    }
    let mut i = 2;
    while i < b.len() && !matches!(b[i], b']' | b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    if i == 2 || !text[i..].starts_with("]:") {
        return 0;
    }
    let label = &text[2..i];
    let start = i + 2;
    let end = text[start..].find('\n').map_or(b.len(), |p| start + p);
    let content = text[start..end].trim_matches([' ', '\t']);
    // Like Comark, a later definition replaces an earlier one.
    defs.insert(label.to_owned(), to_cow(src, content));
    (end + 1).min(b.len())
}

impl<'a, 's, 'r> Parser<'a, 's, 'r> {
    fn run(&mut self) {
        let len = self.b.len();
        while self.pos < len {
            let c = self.b[self.pos];
            let handled = match c {
                b'\n' => self.newline(),
                b'\\' => self.backslash(),
                b'`' => self.backticks(),
                b'*' | b'_' => self.delim(c),
                b'~' if self.opts.strikethrough => self.delim(c),
                b'[' => {
                    let node = self.add_text(self.pos, self.pos + 1);
                    self.add_bracket(node, self.pos, false);
                    self.pos += 1;
                    true
                }
                b'!' => self.bang(),
                b']' => self.close_bracket(),
                b'<' => self.autolink_or_html(),
                b'&' => self.entity(),
                _ => self.string(),
            };
            if !handled {
                self.add_text(self.pos, self.pos + 1);
                self.pos += 1;
            }
        }
        self.process_emphasis(NONE);
    }

    // ----- arena -----

    fn new_node(&mut self, kind: Kind<'a>) -> usize {
        self.nodes.push(Node {
            kind,
            depth: 0,
            parent: NONE,
            prev: NONE,
            next: NONE,
            first: NONE,
            last: NONE,
        });
        self.nodes.len() - 1
    }

    fn append_child(&mut self, parent: usize, child: usize) {
        let last = self.nodes[parent].last;
        self.nodes[child].parent = parent;
        self.nodes[child].prev = last;
        self.nodes[child].next = NONE;
        if last == NONE {
            self.nodes[parent].first = child;
        } else {
            self.nodes[last].next = child;
        }
        self.nodes[parent].last = child;
    }

    fn append(&mut self, kind: Kind<'a>) -> usize {
        let n = self.new_node(kind);
        self.append_child(0, n);
        n
    }

    fn unlink(&mut self, n: usize) {
        let Node {
            parent, prev, next, ..
        } = self.nodes[n];
        if prev != NONE {
            self.nodes[prev].next = next;
        } else if parent != NONE {
            self.nodes[parent].first = next;
        }
        if next != NONE {
            self.nodes[next].prev = prev;
        } else if parent != NONE {
            self.nodes[parent].last = prev;
        }
        self.nodes[n].parent = NONE;
        self.nodes[n].prev = NONE;
        self.nodes[n].next = NONE;
    }

    fn insert_after(&mut self, n: usize, sibling: usize) {
        let next = self.nodes[n].next;
        let parent = self.nodes[n].parent;
        self.nodes[sibling].next = next;
        self.nodes[sibling].prev = n;
        self.nodes[sibling].parent = parent;
        self.nodes[n].next = sibling;
        if next != NONE {
            self.nodes[next].prev = sibling;
        } else if parent != NONE {
            self.nodes[parent].last = sibling;
        }
    }

    /// Moves the siblings following `from` into `container`.
    fn adopt_following(&mut self, from: usize, until: usize, container: usize) {
        let mut tmp = self.nodes[from].next;
        let mut depth = 0;
        while tmp != NONE && tmp != until {
            let next = self.nodes[tmp].next;
            depth = depth.max(self.nodes[tmp].depth);
            self.unlink(tmp);
            self.append_child(container, tmp);
            tmp = next;
        }
        if depth >= MAX_INLINE_DEPTH {
            // Too deep: keep the children but drop this level of nesting.
            self.nodes[container].kind = Kind::Group;
            self.nodes[container].depth = depth;
        } else {
            self.nodes[container].depth = depth + 1;
        }
    }

    #[inline]
    fn cow(&self, start: usize, end: usize) -> Cow<'a, str> {
        to_cow(self.src, &self.subject[start..end])
    }

    fn add_text(&mut self, start: usize, end: usize) -> usize {
        let t = self.cow(start, end);
        self.append(Kind::Text(t))
    }

    // ----- inline constructs -----

    fn string(&mut self) -> bool {
        let start = self.pos;
        let mut end = start + 1;
        while end < self.b.len() && !SPECIAL[self.b[end] as usize] {
            end += 1;
        }
        self.add_text(start, end);
        self.pos = end;
        true
    }

    fn skip_line_start_spaces(&mut self) {
        while self.pos < self.b.len() && matches!(self.b[self.pos], b' ' | b'\t') {
            self.pos += 1;
        }
    }

    fn newline(&mut self) -> bool {
        self.pos += 1;
        let last = self.nodes[0].last;
        let mut hard = false;
        if last != NONE {
            if let Kind::Text(t) = &mut self.nodes[last].kind {
                let trimmed = t.trim_end_matches(' ');
                let removed = t.len() - trimmed.len();
                if removed > 0 {
                    hard = removed >= 2;
                    trim_cow_end(t, removed);
                }
            }
        }
        self.append(if hard {
            Kind::HardBreak
        } else {
            Kind::SoftBreak
        });
        self.skip_line_start_spaces();
        true
    }

    fn backslash(&mut self) -> bool {
        let next = self.b.get(self.pos + 1).copied();
        match next {
            Some(b'\n') => {
                self.pos += 2;
                self.append(Kind::HardBreak);
                self.skip_line_start_spaces();
            }
            Some(c) if c.is_ascii_punctuation() => {
                self.add_text(self.pos + 1, self.pos + 2);
                self.pos += 2;
            }
            _ => {
                self.add_text(self.pos, self.pos + 1);
                self.pos += 1;
            }
        }
        true
    }

    fn backticks(&mut self) -> bool {
        let start = self.pos;
        let b = self.b;
        let mut after = start;
        while after < b.len() && b[after] == b'`' {
            after += 1;
        }
        let n = after - start;
        let runs = self.tick_runs.get_or_insert_with(|| {
            let mut runs = HashMap::new();
            let mut i = 0;
            while let Some(off) = b[i..].iter().position(|&c| c == b'`') {
                let run_start = i + off;
                let mut run_end = run_start;
                while run_end < b.len() && b[run_end] == b'`' {
                    run_end += 1;
                }
                runs.insert(run_end - run_start, run_start);
                i = run_end;
            }
            runs
        });
        if runs.get(&n).is_some_and(|&last| last > start) {
            let mut i = after;
            while let Some(off) = b[i..].iter().position(|&c| c == b'`') {
                let run_start = i + off;
                let mut run_end = run_start;
                while run_end < b.len() && b[run_end] == b'`' {
                    run_end += 1;
                }
                if run_end - run_start == n {
                    let raw = &self.subject[after..run_start];
                    let content = if raw.contains('\n') {
                        Cow::Owned(raw.replace('\n', " "))
                    } else {
                        to_cow(self.src, raw)
                    };
                    let cb = content.as_bytes();
                    let content = if cb.len() >= 2
                        && cb[0] == b' '
                        && cb[cb.len() - 1] == b' '
                        && cb.iter().any(|&c| c != b' ')
                    {
                        let l = cb.len();
                        match content {
                            Cow::Borrowed(s) => Cow::Borrowed(&s[1..l - 1]),
                            Cow::Owned(s) => Cow::Owned(s[1..l - 1].to_owned()),
                        }
                    } else {
                        content
                    };
                    self.append(Kind::Code(content));
                    self.pos = run_end;
                    return true;
                }
                i = run_end;
            }
        }
        self.add_text(start, after);
        self.pos = after;
        true
    }

    fn delim(&mut self, c: u8) -> bool {
        let start = self.pos;
        let b = self.b;
        let mut end = start;
        while end < b.len() && b[end] == c {
            end += 1;
        }
        let n = end - start;
        let before = if start == 0 {
            '\n'
        } else {
            char_before(self.subject, start).unwrap_or('\n')
        };
        let after = char_at(self.subject, end).unwrap_or('\n');
        let after_ws = is_whitespace(after);
        let after_punct = is_punctuation(after);
        let before_ws = is_whitespace(before);
        let before_punct = is_punctuation(before);
        let left = !after_ws && (!after_punct || before_ws || before_punct);
        let right = !before_ws && (!before_punct || after_ws || after_punct);
        let (can_open, can_close) = if c == b'_' {
            (
                left && (!right || before_punct),
                right && (!left || after_punct),
            )
        } else {
            (left, right)
        };
        let node = self.add_text(start, end);
        self.pos = end;
        if (can_open || can_close) && !(c == b'~' && n > 2) {
            let d = self.delims.len();
            self.delims.push(Delim {
                ch: c,
                num: n,
                orig: n,
                node,
                prev: self.delim_top,
                next: NONE,
                can_open,
                can_close,
            });
            if self.delim_top != NONE {
                self.delims[self.delim_top].next = d;
            }
            self.delim_top = d;
        }
        true
    }

    fn remove_delim(&mut self, d: usize) {
        let Delim { prev, next, .. } = self.delims[d];
        if prev != NONE {
            self.delims[prev].next = next;
        }
        if next != NONE {
            self.delims[next].prev = prev;
        } else {
            self.delim_top = prev;
        }
    }

    fn process_emphasis(&mut self, bottom: usize) {
        let mut openers_bottom = [bottom; 16];
        let mut closer = self.delim_top;
        while closer != NONE && self.delims[closer].prev != bottom {
            closer = self.delims[closer].prev;
        }
        while closer != NONE {
            let cd = &self.delims[closer];
            if !cd.can_close {
                closer = cd.next;
                continue;
            }
            let ch = cd.ch;
            let bottom_index = match ch {
                b'_' => usize::from(cd.can_open) * 3 + cd.orig % 3,
                b'*' => 6 + usize::from(cd.can_open) * 3 + cd.orig % 3,
                _ => 12 + cd.num.min(2),
            };
            let mut opener = cd.prev;
            let mut found = false;
            while opener != NONE && opener != bottom && opener != openers_bottom[bottom_index] {
                let od = &self.delims[opener];
                let cd = &self.delims[closer];
                if od.ch == ch && od.can_open {
                    if ch == b'~' {
                        if od.num == cd.num {
                            found = true;
                            break;
                        }
                    } else {
                        let odd_match = (cd.can_open || od.can_close)
                            && !cd.orig.is_multiple_of(3)
                            && (od.orig + cd.orig).is_multiple_of(3);
                        if !odd_match {
                            found = true;
                            break;
                        }
                    }
                }
                opener = od.prev;
            }
            let old_closer = closer;
            if found {
                let use_delims = if ch == b'~' {
                    self.delims[closer].num
                } else if self.delims[closer].num >= 2 && self.delims[opener].num >= 2 {
                    2
                } else {
                    1
                };
                let opener_node = self.delims[opener].node;
                let closer_node = self.delims[closer].node;
                self.delims[opener].num -= use_delims;
                self.delims[closer].num -= use_delims;
                for n in [opener_node, closer_node] {
                    if let Kind::Text(t) = &mut self.nodes[n].kind {
                        trim_cow_end(t, use_delims);
                    }
                }
                let kind = match (ch, use_delims) {
                    (b'~', _) => Kind::Strikethrough,
                    (_, 1) => Kind::Emphasis,
                    _ => Kind::Strong,
                };
                let emph = self.new_node(kind);
                self.adopt_following(opener_node, closer_node, emph);
                self.insert_after(opener_node, emph);
                // Remove the delimiters between the opener and the closer.
                if self.delims[opener].next != closer {
                    self.delims[opener].next = closer;
                    self.delims[closer].prev = opener;
                }
                if self.delims[opener].num == 0 {
                    self.unlink(opener_node);
                    self.remove_delim(opener);
                }
                if self.delims[closer].num == 0 {
                    self.unlink(closer_node);
                    let next = self.delims[closer].next;
                    self.remove_delim(closer);
                    closer = next;
                }
            } else {
                closer = self.delims[closer].next;
                openers_bottom[bottom_index] = self.delims[old_closer].prev;
                if !self.delims[old_closer].can_open {
                    self.remove_delim(old_closer);
                }
            }
        }
        while self.delim_top != NONE && self.delim_top != bottom {
            self.remove_delim(self.delim_top);
        }
    }

    fn add_bracket(&mut self, node: usize, index: usize, image: bool) {
        if let Some(top) = self.brackets.last_mut() {
            top.bracket_after = true;
        }
        self.brackets.push(Bracket {
            node,
            prev_delim: self.delim_top,
            index,
            image,
            active: true,
            bracket_after: false,
        });
    }

    fn bang(&mut self) -> bool {
        if self.b.get(self.pos + 1) == Some(&b'[') {
            let node = self.add_text(self.pos, self.pos + 2);
            self.add_bracket(node, self.pos + 1, true);
            self.pos += 2;
        } else {
            self.add_text(self.pos, self.pos + 1);
            self.pos += 1;
        }
        true
    }

    /// Tries `(destination "title")` at `pos` (at `(`).
    fn inline_link(&mut self, pos: usize) -> Option<(Cow<'a, str>, Cow<'a, str>, usize)> {
        let b = self.b;
        let p = scan::spnl(b, pos + 1);
        let (ds, de, after_dest) = scan::link_destination(b, p)?;
        let p = scan::spnl(b, after_dest);
        let mut title = None;
        let mut end = p;
        if p > after_dest {
            let quote = match b.get(p) {
                Some(b'"') => 1,
                Some(b'\'') => 2,
                _ => 0,
            };
            if self.no_title_end & quote == 0 {
                match scan::link_title(b, p) {
                    Some((ts, te, after_title)) => {
                        title = Some((ts, te));
                        end = scan::spnl(b, after_title);
                    }
                    // An unterminated quoted title runs to the end of the
                    // subject; remember it to stay linear.
                    None => self.no_title_end |= quote,
                }
            }
        }
        if b.get(end) != Some(&b')') {
            return None;
        }
        let url = lift(self.src, unescape(&self.subject[ds..de]));
        let title = title.map_or(Cow::Borrowed(""), |(ts, te)| {
            lift(self.src, unescape(&self.subject[ts..te]))
        });
        Some((url, title, end + 1))
    }

    fn close_bracket(&mut self) -> bool {
        self.pos += 1;
        let start = self.pos;
        let Some(opener) = self.brackets.last() else {
            self.add_text(start - 1, start);
            return true;
        };
        if !opener.active {
            self.add_text(start - 1, start);
            self.brackets.pop();
            return true;
        }
        let image = opener.image;
        let mut target = None;
        if self.b.get(start) == Some(&b'(') {
            if let Some((url, title, end)) = self.inline_link(start) {
                target = Some((url, title));
                self.pos = end;
            }
        }
        if target.is_none() && self.opts.footnotes && !image && self.footnote_ref(start) {
            return true;
        }
        if target.is_none() {
            let opener = self.brackets.last().unwrap();
            let label = match scan::link_label(self.b, start) {
                Some(end) if end - start > 2 => {
                    self.pos = end;
                    Some(&self.subject[start + 1..end - 1])
                }
                Some(end) => {
                    self.pos = end;
                    (!opener.bracket_after).then(|| &self.subject[opener.index + 1..start - 1])
                }
                None => (!opener.bracket_after).then(|| &self.subject[opener.index + 1..start - 1]),
            };
            if let Some(label) = label.filter(|l| l.len() <= 4 * 999) {
                if let Some(def) = self.refs.get(&normalize_label(label)) {
                    target = Some((def.url.clone(), def.title.clone()));
                }
            }
        }
        let Some((url, title)) = target else {
            self.brackets.pop();
            self.pos = start;
            self.add_text(start - 1, start);
            return true;
        };
        let opener = self.brackets.pop().unwrap();
        let kind = if image {
            Kind::Image(Box::new((url, title)))
        } else {
            Kind::Link(Box::new((url, title, LinkKind::Normal)))
        };
        let link = self.new_node(kind);
        self.adopt_following(opener.node, NONE, link);
        self.append_child(0, link);
        self.process_emphasis(opener.prev_delim);
        self.unlink(opener.node);
        if !image {
            for b in &mut self.brackets {
                if !b.image {
                    b.active = false;
                }
            }
        }
        true
    }

    /// Tries a footnote reference `[^label]` ending just before `start`.
    fn footnote_ref(&mut self, start: usize) -> bool {
        let opener = self.brackets.last().unwrap();
        let inner = &self.subject[opener.index + 1..start - 1];
        let Some(label) = inner.strip_prefix('^') else {
            return false;
        };
        if label.is_empty()
            || label
                .bytes()
                .any(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'[' | b']'))
            || !self.footnotes.defs.contains_key(label)
        {
            return false;
        }
        let label = to_cow(self.src, label);
        let (index, occurrence) = self.footnotes.reference(label.clone());
        let opener = self.brackets.pop().unwrap();
        let node = self.new_node(Kind::FootnoteRef(label, index, occurrence));
        self.adopt_following(opener.node, NONE, node);
        self.append_child(0, node);
        self.process_emphasis(opener.prev_delim);
        self.unlink(opener.node);
        self.pos = start;
        true
    }

    fn autolink_or_html(&mut self) -> bool {
        let start = self.pos;
        if let Some(end) = scan::uri_autolink(self.b, start) {
            let url = self.cow(start + 1, end - 1);
            self.autolink(url.clone(), url, LinkKind::Autolink);
            self.pos = end;
            return true;
        }
        if let Some(end) = scan::email_autolink(self.b, start) {
            let addr = self.cow(start + 1, end - 1);
            let url = Cow::Owned(format!("mailto:{addr}"));
            self.autolink(url, addr, LinkKind::Email);
            self.pos = end;
            return true;
        }
        if let Some(end) = scan::html_tag(self.b, start, &mut self.html_memo) {
            let html = self.cow(start, end);
            self.append(Kind::Html(html));
            self.pos = end;
            return true;
        }
        false
    }

    fn autolink(&mut self, url: Cow<'a, str>, text: Cow<'a, str>, kind: LinkKind) {
        let link = self.append(Kind::Link(Box::new((url, Cow::Borrowed(""), kind))));
        let t = self.new_node(Kind::Text(text));
        self.append_child(link, t);
    }

    fn entity(&mut self) -> bool {
        match parse_entity(&self.subject[self.pos..]) {
            Some((text, len)) => {
                self.append(Kind::Text(text));
                self.pos += len;
                true
            }
            None => false,
        }
    }

    // ----- conversion -----

    fn build(&mut self, parent: usize, in_link: bool) -> Vec<Inline<'a>> {
        let mut out: Vec<Inline<'a>> = Vec::new();
        let mut c = self.nodes[parent].first;
        // Continuations of the groups being flattened.
        let mut groups = Vec::new();
        loop {
            if c == NONE {
                match groups.pop() {
                    Some(next) => {
                        c = next;
                        continue;
                    }
                    None => break,
                }
            }
            let next = self.nodes[c].next;
            let kind = std::mem::replace(&mut self.nodes[c].kind, Kind::Root);
            let inline = match kind {
                Kind::Root => unreachable!(),
                Kind::Group => {
                    groups.push(next);
                    c = self.nodes[c].first;
                    continue;
                }
                Kind::Text(t) => {
                    if t.is_empty() {
                        c = next;
                        continue;
                    }
                    if let Some(Inline::Text(prev)) = out.last_mut() {
                        merge_text(self.src, prev, t);
                        c = next;
                        continue;
                    }
                    Inline::Text(t)
                }
                Kind::SoftBreak => Inline::SoftBreak,
                Kind::HardBreak => Inline::HardBreak,
                Kind::Code(t) => Inline::Code(t),
                Kind::Html(t) => Inline::Html(t),
                Kind::Emphasis => Inline::Emphasis(self.build(c, in_link)),
                Kind::Strong => Inline::Strong(self.build(c, in_link)),
                Kind::Strikethrough => Inline::Strikethrough(self.build(c, in_link)),
                Kind::FootnoteRef(label, index, occurrence) => {
                    Inline::FootnoteReference(crate::ast::FootnoteReference {
                        label,
                        index,
                        occurrence,
                    })
                }
                Kind::Link(link) => Inline::Link(Link {
                    url: link.0,
                    title: link.1,
                    kind: link.2,
                    children: self.build(c, true),
                }),
                Kind::Image(image) => Inline::Image(Link {
                    url: image.0,
                    title: image.1,
                    kind: LinkKind::Normal,
                    children: self.build(c, in_link),
                }),
            };
            out.push(inline);
            c = next;
        }
        if self.autolink_candidates && !in_link {
            crate::autolink::extend(self.src, &mut out);
        }
        out
    }
}

/// Removes the last `n` bytes of a text.
fn trim_cow_end(t: &mut Cow<'_, str>, n: usize) {
    let len = t.len() - n;
    match t {
        Cow::Borrowed(s) => *s = &s[..len],
        Cow::Owned(s) => s.truncate(len),
    }
}

/// Appends `next` to `prev`, staying borrowed when both are adjacent slices
/// of the source.
fn merge_text<'a>(src: &'a str, prev: &mut Cow<'a, str>, next: Cow<'a, str>) {
    if let (Cow::Borrowed(a), Cow::Borrowed(b)) = (&*prev, &next) {
        if a.as_ptr() as usize + a.len() == b.as_ptr() as usize {
            if let Some(start) = crate::block::reborrow(src, a) {
                let off = start.as_ptr() as usize - src.as_ptr() as usize;
                if let Some(joined) = src.get(off..off + a.len() + b.len()) {
                    *prev = Cow::Borrowed(joined);
                    return;
                }
            }
        }
    }
    prev.to_mut().push_str(&next);
}
