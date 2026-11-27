//! Block structure parsing (first phase).
//!
//! The algorithm follows the CommonMark reference implementations: each line
//! is matched against the open blocks, then new block starts are tried, and
//! the rest of the line is added to the innermost block that accepts text.
//! Inline content is parsed in a second phase, once every link reference
//! definition is known.

use std::borrow::Cow;
use std::cell::RefCell;

use crate::ast::{self, Alignment, Block, Inline};
use crate::inline::{self, RefMap};
use crate::text::{is_space_or_tab, unescape};
use crate::Options;

const CODE_INDENT: usize = 4;

/// Maximum nesting of container blocks. Deeper block quotes and list items
/// are parsed as text, which keeps recursion bounded on hostile input.
pub(crate) const MAX_BLOCK_DEPTH: u16 = 100;

/// Maximum number of empty cells added to complete short table rows, to
/// avoid output amplification. Further rows end the table.
const MAX_AUTOCOMPLETED_CELLS: usize = 1 << 16;
const NONE: usize = usize::MAX;

/// Returns `sub` as a slice of `src` when it points into `src`.
#[inline]
pub(crate) fn reborrow<'a>(src: &'a str, sub: &str) -> Option<&'a str> {
    let start = (sub.as_ptr() as usize).wrapping_sub(src.as_ptr() as usize);
    if start <= src.len() && sub.len() <= src.len() - start {
        src.get(start..start + sub.len())
    } else {
        None
    }
}

/// Borrows `sub` from `src` when possible, copies it otherwise.
#[inline]
pub(crate) fn to_cow<'a>(src: &'a str, sub: &str) -> Cow<'a, str> {
    match reborrow(src, sub) {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(sub.to_owned()),
    }
}

/// Re-borrows a `Cow` computed from a slice of `src` (or from a temporary).
#[inline]
pub(crate) fn lift<'a>(src: &'a str, c: Cow<'_, str>) -> Cow<'a, str> {
    match c {
        Cow::Borrowed(s) => to_cow(src, s),
        Cow::Owned(s) => Cow::Owned(s),
    }
}

/// The accumulated text of a block. Lines that are contiguous in the source
/// are kept as a single borrowed range; anything else switches to an owned
/// buffer.
#[derive(Debug, Default)]
struct Content {
    start: usize,
    end: usize,
    has: bool,
    /// The last borrowed line has no `\n` in the source (end of input or
    /// `\r\n` ending); a virtual `\n` follows `end`.
    virtual_nl: bool,
    owned: Option<String>,
}

impl Content {
    /// Appends `spaces` spaces, `text` (a slice of `src`) and a line ending.
    fn push_line(&mut self, src: &str, text: &str, spaces: usize) {
        if self.owned.is_none() && spaces == 0 {
            if let Some(s) = reborrow(src, text) {
                let abs = s.as_ptr() as usize - src.as_ptr() as usize;
                if !self.has || (abs == self.end && !self.virtual_nl) {
                    if !self.has {
                        self.start = abs;
                        self.has = true;
                    }
                    let end = abs + s.len();
                    if src.as_bytes().get(end) == Some(&b'\n') {
                        self.end = end + 1;
                        self.virtual_nl = false;
                    } else {
                        self.end = end;
                        self.virtual_nl = true;
                    }
                    return;
                }
            }
        }
        let buf = self.make_owned(src);
        for _ in 0..spaces {
            buf.push(' ');
        }
        buf.push_str(text);
        buf.push('\n');
    }

    fn make_owned(&mut self, src: &str) -> &mut String {
        if self.owned.is_none() {
            let mut s = String::new();
            if self.has {
                s.push_str(&src[self.start..self.end]);
                if self.virtual_nl {
                    s.push('\n');
                }
            }
            self.has = true;
            self.virtual_nl = false;
            self.owned = Some(s);
        }
        self.owned.as_mut().unwrap()
    }

    /// The text, without the virtual final line ending if there is one.
    fn text<'s>(&'s self, src: &'s str) -> &'s str {
        match &self.owned {
            Some(s) => s,
            None if self.has => &src[self.start..self.end],
            None => "",
        }
    }

    /// Drops the first `n` bytes.
    fn consume(&mut self, n: usize) {
        match &mut self.owned {
            Some(s) => {
                s.drain(..n);
            }
            None => self.start += n,
        }
    }

    /// Keeps only the first `n` bytes; the result ends with a line ending.
    fn truncate(&mut self, n: usize) {
        match &mut self.owned {
            Some(s) => s.truncate(n),
            None => {
                self.end = self.start + n;
                self.virtual_nl = false;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListData {
    /// The bullet character, or 0 for an ordered list.
    bullet: u8,
    start: u32,
    delimiter: u8,
    marker_offset: usize,
    padding: usize,
    tight: bool,
}

#[derive(Debug, Clone, Copy)]
struct Fence {
    fenced: bool,
    ch: u8,
    len: usize,
    offset: usize,
}

#[derive(Debug)]
enum Kind {
    Document,
    BlockQuote,
    List(ListData),
    Item(ListData),
    Paragraph,
    Heading(u8),
    ThematicBreak,
    CodeBlock(Fence),
    HtmlBlock(u8),
    Table(Box<TableHead>),
}

#[derive(Debug)]
struct TableHead {
    alignments: Vec<Alignment>,
    header: Content,
    autocompleted: usize,
}

#[derive(Debug)]
struct Node {
    kind: Kind,
    depth: u16,
    parent: usize,
    children: Vec<usize>,
    open: bool,
    start_line: usize,
    end_line: usize,
    content: Content,
}

enum Continue {
    Matched,
    Failed,
    Consumed,
}

enum Start {
    None,
    Container,
    Leaf,
}

pub(crate) struct BlockParser<'a, 'o> {
    src: &'a str,
    opts: &'o Options,
    nodes: Vec<Node>,
    refs: RefMap<'a>,
    tip: usize,
    old_tip: usize,
    last_matched: usize,
    all_closed: bool,
    line: &'a str,
    line_number: usize,
    offset: usize,
    column: usize,
    next_nonspace: usize,
    next_nonspace_column: usize,
    indent: usize,
    indented: bool,
    blank: bool,
    partially_consumed_tab: bool,
    /// Set by a block start that used the whole line without adding text.
    line_consumed: bool,
    /// Buffers reused by every inline parse.
    scratch: RefCell<inline::Scratch<'a>>,
}

impl<'a, 'o> BlockParser<'a, 'o> {
    pub(crate) fn new(src: &'a str, opts: &'o Options) -> Self {
        let doc = Node {
            kind: Kind::Document,
            depth: 0,
            parent: NONE,
            children: Vec::new(),
            open: true,
            start_line: 1,
            end_line: 0,
            content: Content::default(),
        };
        let mut nodes = Vec::with_capacity(src.len() / 32 + 4);
        nodes.push(doc);
        BlockParser {
            src,
            opts,
            nodes,
            refs: RefMap::default(),
            tip: 0,
            old_tip: 0,
            last_matched: 0,
            all_closed: true,
            line: "",
            line_number: 0,
            offset: 0,
            column: 0,
            next_nonspace: 0,
            next_nonspace_column: 0,
            indent: 0,
            indented: false,
            blank: false,
            partially_consumed_tab: false,
            line_consumed: false,
            scratch: RefCell::default(),
        }
    }

    pub(crate) fn parse(mut self) -> ast::Document<'a> {
        let src = self.src;
        let b = src.as_bytes();
        let mut start = 0;
        let has_cr = src.contains('\r');
        while start < b.len() {
            let end = if has_cr {
                memchr2(b'\n', b'\r', &b[start..])
            } else {
                find_byte(b'\n', &b[start..])
            }
            .map_or(b.len(), |p| start + p);
            self.incorporate_line(&src[start..end]);
            start = end;
            if start < b.len() {
                start += if b[start] == b'\r' && b.get(start + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            }
        }
        while self.tip != NONE {
            self.finalize(self.tip, self.line_number);
        }
        let children = self.convert_children(0);
        ast::Document { children }
    }

    // ----- line state -----

    #[inline]
    fn byte(&self, i: usize) -> Option<u8> {
        self.line.as_bytes().get(i).copied()
    }

    fn find_next_nonspace(&mut self) {
        let b = self.line.as_bytes();
        let mut i = self.offset;
        let mut cols = self.column;
        while i < b.len() {
            match b[i] {
                b' ' => cols += 1,
                b'\t' => cols += 4 - cols % 4,
                _ => break,
            }
            i += 1;
        }
        self.blank = i >= b.len();
        self.next_nonspace = i;
        self.next_nonspace_column = cols;
        self.indent = cols - self.column;
        self.indented = self.indent >= CODE_INDENT;
    }

    fn advance_next_nonspace(&mut self) {
        self.offset = self.next_nonspace;
        self.column = self.next_nonspace_column;
        self.partially_consumed_tab = false;
    }

    fn advance_offset(&mut self, mut count: usize, columns: bool) {
        let b = self.line.as_bytes();
        while count > 0 && self.offset < b.len() {
            if b[self.offset] == b'\t' {
                let to_tab = 4 - self.column % 4;
                if columns {
                    self.partially_consumed_tab = to_tab > count;
                    let n = to_tab.min(count);
                    self.column += n;
                    if !self.partially_consumed_tab {
                        self.offset += 1;
                    }
                    count -= n;
                } else {
                    self.partially_consumed_tab = false;
                    self.column += to_tab;
                    self.offset += 1;
                    count -= 1;
                }
            } else {
                self.partially_consumed_tab = false;
                self.offset += 1;
                self.column += 1;
                count -= 1;
            }
        }
    }

    // ----- tree -----

    fn add_line(&mut self) {
        let mut spaces = 0;
        if self.partially_consumed_tab {
            self.offset += 1;
            spaces = 4 - self.column % 4;
        }
        let text = &self.line[self.offset..];
        let tip = self.tip;
        self.nodes[tip].content.push_line(self.src, text, spaces);
    }

    fn can_contain(parent: &Kind, child: &Kind) -> bool {
        match parent {
            Kind::Document | Kind::BlockQuote | Kind::Item(_) => !matches!(child, Kind::Item(_)),
            Kind::List(_) => matches!(child, Kind::Item(_)),
            _ => false,
        }
    }

    fn add_child(&mut self, kind: Kind) -> usize {
        while !Self::can_contain(&self.nodes[self.tip].kind, &kind) {
            self.finalize(self.tip, self.line_number - 1);
        }
        let idx = self.nodes.len();
        let depth = self.nodes[self.tip].depth + 1;
        self.nodes.push(Node {
            kind,
            depth,
            parent: self.tip,
            children: Vec::new(),
            open: true,
            start_line: self.line_number,
            end_line: 0,
            content: Content::default(),
        });
        self.nodes[self.tip].children.push(idx);
        self.tip = idx;
        idx
    }

    fn unlink(&mut self, idx: usize) {
        let parent = self.nodes[idx].parent;
        let children = &mut self.nodes[parent].children;
        if let Some(p) = children.iter().rposition(|&c| c == idx) {
            children.remove(p);
        }
    }

    fn close_unmatched_blocks(&mut self) {
        if !self.all_closed {
            while self.old_tip != self.last_matched {
                let parent = self.nodes[self.old_tip].parent;
                self.finalize(self.old_tip, self.line_number - 1);
                self.old_tip = parent;
            }
            self.all_closed = true;
        }
    }

    fn finalize(&mut self, idx: usize, line_number: usize) {
        let above = self.nodes[idx].parent;
        self.nodes[idx].open = false;
        self.nodes[idx].end_line = line_number;
        match self.nodes[idx].kind {
            Kind::Paragraph => {
                self.parse_reference_defs(idx);
                if self.nodes[idx]
                    .content
                    .text(self.src)
                    .bytes()
                    .all(|c| c.is_ascii_whitespace())
                {
                    self.unlink(idx);
                }
            }
            Kind::Item(_) => {
                let node = &self.nodes[idx];
                let end = node
                    .children
                    .last()
                    .map_or(node.start_line, |&c| self.nodes[c].end_line);
                self.nodes[idx].end_line = end;
            }
            Kind::List(_) => {
                let tight = self.list_is_tight(idx);
                let node = &mut self.nodes[idx];
                if let Kind::List(d) = &mut node.kind {
                    d.tight = tight;
                }
                if let Some(&last) = self.nodes[idx].children.last() {
                    self.nodes[idx].end_line = self.nodes[last].end_line;
                }
            }
            _ => {}
        }
        self.tip = above;
    }

    fn ends_with_blank_line(&self, idx: usize, next: usize) -> bool {
        self.nodes[idx].end_line + 1 != self.nodes[next].start_line
    }

    fn list_is_tight(&self, list: usize) -> bool {
        let items = &self.nodes[list].children;
        for (i, &item) in items.iter().enumerate() {
            if let Some(&next) = items.get(i + 1) {
                if self.ends_with_blank_line(item, next) {
                    return false;
                }
            }
            let sub = &self.nodes[item].children;
            for w in sub.windows(2) {
                if self.ends_with_blank_line(w[0], w[1]) {
                    return false;
                }
            }
        }
        true
    }

    /// Parses link reference definitions at the start of a paragraph.
    /// Returns whether at least one was found.
    fn parse_reference_defs(&mut self, idx: usize) -> bool {
        let mut found = false;
        loop {
            let text = self.nodes[idx].content.text(self.src);
            if !text.starts_with('[') {
                break;
            }
            let n = inline::parse_reference(self.src, text, &mut self.refs);
            if n == 0 {
                break;
            }
            self.nodes[idx].content.consume(n);
            found = true;
        }
        found
    }

    // ----- line processing -----

    fn incorporate_line(&mut self, line: &'a str) {
        let mut container = 0;
        self.old_tip = self.tip;
        self.offset = 0;
        self.column = 0;
        self.blank = false;
        self.partially_consumed_tab = false;
        self.line_consumed = false;
        self.line_number += 1;
        self.line = line;

        let mut all_matched = true;
        while let Some(&last) = self.nodes[container].children.last() {
            if !self.nodes[last].open {
                break;
            }
            container = last;
            self.find_next_nonspace();
            match self.continue_block(container) {
                Continue::Matched => {}
                Continue::Failed => {
                    all_matched = false;
                }
                Continue::Consumed => return,
            }
            if !all_matched {
                container = self.nodes[container].parent;
                break;
            }
        }

        self.all_closed = container == self.old_tip;
        self.last_matched = container;

        // A table, like a paragraph, can be interrupted by other blocks.
        let mut matched_leaf =
            !matches!(self.nodes[container].kind, Kind::Paragraph | Kind::Table(_))
                && self.accepts_lines(container);

        while !matched_leaf {
            self.find_next_nonspace();
            if !self.indented && !self.maybe_special(self.byte(self.next_nonspace)) {
                self.advance_next_nonspace();
                break;
            }
            match self.try_starts(container) {
                Start::Container => container = self.tip,
                Start::Leaf => {
                    container = self.tip;
                    matched_leaf = true;
                }
                Start::None => {
                    self.advance_next_nonspace();
                    break;
                }
            }
        }

        if !self.all_closed && !self.blank && matches!(self.nodes[self.tip].kind, Kind::Paragraph) {
            // Lazy paragraph continuation.
            self.add_line();
        } else {
            self.close_unmatched_blocks();
            if self.line_consumed {
                // The line was used by a block start (a table delimiter row).
            } else if self.accepts_lines(container) {
                if let Kind::Table(head) = &mut self.nodes[container].kind {
                    let cells = split_row(&self.line[self.offset.min(self.line.len())..]).len();
                    head.autocompleted += head.alignments.len().saturating_sub(cells);
                    if head.autocompleted > MAX_AUTOCOMPLETED_CELLS {
                        self.finalize(container, self.line_number - 1);
                        container = self.add_child(Kind::Paragraph);
                        self.advance_next_nonspace();
                    }
                }
                self.add_line();
                if let Kind::HtmlBlock(t @ 1..=5) = self.nodes[container].kind {
                    if html_block_end(t, &self.line.as_bytes()[self.offset.min(self.line.len())..])
                    {
                        self.finalize(container, self.line_number);
                    }
                }
            } else if self.offset < line.len() && !self.blank {
                self.add_child(Kind::Paragraph);
                self.advance_next_nonspace();
                self.add_line();
            }
        }
    }

    fn accepts_lines(&self, idx: usize) -> bool {
        matches!(
            self.nodes[idx].kind,
            Kind::Paragraph | Kind::CodeBlock(_) | Kind::HtmlBlock(_) | Kind::Table(_)
        )
    }

    #[inline]
    fn maybe_special(&self, c: Option<u8>) -> bool {
        match c {
            Some(b'#' | b'`' | b'~' | b'*' | b'+' | b'_' | b'=' | b'<' | b'>' | b'-') => true,
            Some(b'0'..=b'9') => true,
            Some(b'|' | b':') => self.opts.tables,
            _ => false,
        }
    }

    fn continue_block(&mut self, idx: usize) -> Continue {
        match self.nodes[idx].kind {
            Kind::Document | Kind::List(_) => Continue::Matched,
            Kind::BlockQuote => {
                if !self.indented && self.byte(self.next_nonspace) == Some(b'>') {
                    self.advance_next_nonspace();
                    self.advance_offset(1, false);
                    if self.byte(self.offset).is_some_and(is_space_or_tab) {
                        self.advance_offset(1, true);
                    }
                    Continue::Matched
                } else {
                    Continue::Failed
                }
            }
            Kind::Item(d) => {
                if self.blank {
                    if self.nodes[idx].children.is_empty() {
                        return Continue::Failed;
                    }
                    self.advance_next_nonspace();
                    Continue::Matched
                } else if self.indent >= d.marker_offset + d.padding {
                    self.advance_offset(d.marker_offset + d.padding, true);
                    Continue::Matched
                } else {
                    Continue::Failed
                }
            }
            Kind::Heading(_) | Kind::ThematicBreak => Continue::Failed,
            Kind::CodeBlock(f) => {
                if f.fenced {
                    if self.indent <= 3 && self.byte(self.next_nonspace) == Some(f.ch) {
                        let rest = &self.line.as_bytes()[self.next_nonspace..];
                        let n = rest.iter().take_while(|&&c| c == f.ch).count();
                        if n >= f.len && rest[n..].iter().all(|&c| is_space_or_tab(c)) {
                            self.finalize(idx, self.line_number);
                            return Continue::Consumed;
                        }
                    }
                    let mut i = f.offset;
                    while i > 0 && self.byte(self.offset).is_some_and(is_space_or_tab) {
                        self.advance_offset(1, true);
                        i -= 1;
                    }
                    Continue::Matched
                } else if self.indent >= CODE_INDENT {
                    self.advance_offset(CODE_INDENT, true);
                    Continue::Matched
                } else if self.blank {
                    self.advance_next_nonspace();
                    Continue::Matched
                } else {
                    Continue::Failed
                }
            }
            Kind::HtmlBlock(t) => {
                if self.blank && (t == 6 || t == 7) {
                    Continue::Failed
                } else {
                    Continue::Matched
                }
            }
            Kind::Paragraph | Kind::Table(_) => {
                if self.blank {
                    Continue::Failed
                } else {
                    Continue::Matched
                }
            }
        }
    }

    fn try_starts(&mut self, container: usize) -> Start {
        let c = self.byte(self.next_nonspace);
        let too_deep = self.nodes[container].depth >= MAX_BLOCK_DEPTH;
        if !self.indented {
            match c {
                Some(b'>') if !too_deep => return self.start_block_quote(),
                Some(b'#') => {
                    if let s @ Start::Leaf = self.start_atx_heading() {
                        return s;
                    }
                }
                Some(b'`' | b'~') => {
                    if let s @ Start::Leaf = self.start_fenced_code() {
                        return s;
                    }
                }
                Some(b'<') => {
                    if let s @ Start::Leaf = self.start_html_block(container) {
                        return s;
                    }
                }
                _ => {}
            }
            if self.opts.tables {
                if let s @ Start::Leaf = self.start_table(container) {
                    return s;
                }
            }
            if matches!(c, Some(b'=' | b'-')) {
                if let s @ Start::Leaf = self.start_setext_heading(container) {
                    return s;
                }
            }
            if matches!(c, Some(b'*' | b'-' | b'_')) {
                if let s @ Start::Leaf = self.start_thematic_break() {
                    return s;
                }
            }
        }
        if !too_deep && (!self.indented || matches!(self.nodes[container].kind, Kind::List(_))) {
            if let s @ Start::Container = self.start_list_item(container) {
                return s;
            }
        }
        if self.indented && !matches!(self.nodes[self.tip].kind, Kind::Paragraph) && !self.blank {
            self.advance_offset(CODE_INDENT, true);
            self.close_unmatched_blocks();
            self.add_child(Kind::CodeBlock(Fence {
                fenced: false,
                ch: 0,
                len: 0,
                offset: 0,
            }));
            return Start::Leaf;
        }
        Start::None
    }

    fn start_block_quote(&mut self) -> Start {
        self.advance_next_nonspace();
        self.advance_offset(1, false);
        if self.byte(self.offset).is_some_and(is_space_or_tab) {
            self.advance_offset(1, true);
        }
        self.close_unmatched_blocks();
        self.add_child(Kind::BlockQuote);
        Start::Container
    }

    fn start_atx_heading(&mut self) -> Start {
        let rest = &self.line.as_bytes()[self.next_nonspace..];
        let level = rest.iter().take_while(|&&c| c == b'#').count();
        if level > 6 || rest.get(level).is_some_and(|&c| !is_space_or_tab(c)) {
            return Start::None;
        }
        self.advance_next_nonspace();
        self.advance_offset(level, false);
        self.close_unmatched_blocks();
        let idx = self.add_child(Kind::Heading(level as u8));
        // Strip the optional closing sequence.
        let line = self.line;
        let content = line[self.offset..].trim_end_matches([' ', '\t']);
        let b = content.as_bytes();
        let hashes = b.iter().rev().take_while(|&&c| c == b'#').count();
        let content = if hashes == b.len() {
            ""
        } else if hashes > 0 && is_space_or_tab(b[b.len() - hashes - 1]) {
            &content[..b.len() - hashes]
        } else {
            content
        };
        let content = content.trim_matches([' ', '\t']);
        self.nodes[idx].content.push_line(self.src, content, 0);
        self.offset = line.len();
        Start::Leaf
    }

    fn start_fenced_code(&mut self) -> Start {
        let rest = &self.line.as_bytes()[self.next_nonspace..];
        let ch = rest[0];
        let len = rest.iter().take_while(|&&c| c == ch).count();
        if len < 3 || (ch == b'`' && rest[len..].contains(&b'`')) {
            return Start::None;
        }
        self.close_unmatched_blocks();
        self.add_child(Kind::CodeBlock(Fence {
            fenced: true,
            ch,
            len,
            offset: self.indent,
        }));
        self.advance_next_nonspace();
        self.advance_offset(len, false);
        Start::Leaf
    }

    fn start_html_block(&mut self, container: usize) -> Start {
        let rest = &self.line.as_bytes()[self.next_nonspace..];
        let Some(t) = html_block_start(rest) else {
            return Start::None;
        };
        if t == 7
            && (matches!(self.nodes[container].kind, Kind::Paragraph)
                || (!self.all_closed
                    && !self.blank
                    && matches!(self.nodes[self.tip].kind, Kind::Paragraph)))
        {
            return Start::None;
        }
        self.close_unmatched_blocks();
        self.add_child(Kind::HtmlBlock(t));
        Start::Leaf
    }

    fn start_setext_heading(&mut self, container: usize) -> Start {
        if !matches!(self.nodes[container].kind, Kind::Paragraph) {
            return Start::None;
        }
        let rest = &self.line.as_bytes()[self.next_nonspace..];
        let ch = rest[0];
        let n = rest.iter().take_while(|&&c| c == ch).count();
        if !rest[n..].iter().all(|&c| is_space_or_tab(c)) {
            return Start::None;
        }
        self.close_unmatched_blocks();
        self.parse_reference_defs(container);
        if self.nodes[container]
            .content
            .text(self.src)
            .bytes()
            .all(|c| c.is_ascii_whitespace())
        {
            return Start::None;
        }
        self.nodes[container].kind = Kind::Heading(if ch == b'=' { 1 } else { 2 });
        self.tip = container;
        self.offset = self.line.len();
        Start::Leaf
    }

    fn start_thematic_break(&mut self) -> Start {
        let rest = &self.line.as_bytes()[self.next_nonspace..];
        let ch = rest[0];
        let mut count = 0;
        for &c in rest {
            if c == ch {
                count += 1;
            } else if !is_space_or_tab(c) {
                return Start::None;
            }
        }
        if count < 3 {
            return Start::None;
        }
        self.close_unmatched_blocks();
        self.add_child(Kind::ThematicBreak);
        self.offset = self.line.len();
        Start::Leaf
    }

    fn start_list_item(&mut self, container: usize) -> Start {
        if self.indent >= 4 {
            return Start::None;
        }
        let in_paragraph = matches!(self.nodes[container].kind, Kind::Paragraph);
        let rest = &self.line.as_bytes()[self.next_nonspace..];
        let mut data = ListData {
            bullet: 0,
            start: 1,
            delimiter: 0,
            marker_offset: self.indent,
            padding: 0,
            tight: true,
        };
        let marker_len = match rest.first() {
            Some(&c @ (b'*' | b'+' | b'-')) => {
                data.bullet = c;
                1
            }
            Some(b'0'..=b'9') => {
                let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
                if digits > 9 {
                    return Start::None;
                }
                let Some(&d @ (b'.' | b')')) = rest.get(digits) else {
                    return Start::None;
                };
                let start: u32 = std::str::from_utf8(&rest[..digits])
                    .unwrap()
                    .parse()
                    .unwrap();
                if in_paragraph && start != 1 {
                    return Start::None;
                }
                data.start = start;
                data.delimiter = d;
                digits + 1
            }
            _ => return Start::None,
        };
        match rest.get(marker_len) {
            None | Some(b' ' | b'\t') => {}
            _ => return Start::None,
        }
        if in_paragraph && rest[marker_len..].iter().all(|&c| is_space_or_tab(c)) {
            return Start::None;
        }

        self.advance_next_nonspace();
        self.advance_offset(marker_len, true);
        let spaces_start_col = self.column;
        let spaces_start_offset = self.offset;
        loop {
            self.advance_offset(1, true);
            if !(self.column - spaces_start_col < 5
                && self.byte(self.offset).is_some_and(is_space_or_tab))
            {
                break;
            }
        }
        let blank_item = self.offset >= self.line.len();
        let spaces_after = self.column - spaces_start_col;
        if !(1..5).contains(&spaces_after) || blank_item {
            data.padding = marker_len + 1;
            self.column = spaces_start_col;
            self.offset = spaces_start_offset;
            self.partially_consumed_tab = false;
            if self.byte(self.offset).is_some_and(is_space_or_tab) {
                self.advance_offset(1, true);
            }
        } else {
            data.padding = marker_len + spaces_after;
        }

        self.close_unmatched_blocks();
        let matches = match self.nodes[self.tip].kind {
            Kind::List(l) => l.bullet == data.bullet && l.delimiter == data.delimiter,
            _ => false,
        };
        if !matches {
            self.add_child(Kind::List(data));
        }
        self.add_child(Kind::Item(data));
        Start::Container
    }

    fn start_table(&mut self, container: usize) -> Start {
        if !matches!(self.nodes[container].kind, Kind::Paragraph) {
            return Start::None;
        }
        let line = &self.line[self.next_nonspace..];
        let Some(alignments) = parse_delimiter_row(line) else {
            return Start::None;
        };
        let text = self.nodes[container].content.text(self.src);
        let body = text.strip_suffix('\n').unwrap_or(text);
        let header_start = body.rfind('\n').map_or(0, |p| p + 1);
        let header = &body[header_start..];
        if split_row(header).len() != alignments.len() {
            return Start::None;
        }
        let mut head = Box::new(TableHead {
            alignments,
            header: Content::default(),
            autocompleted: 0,
        });
        match to_cow(self.src, header) {
            Cow::Borrowed(h) => head.header.push_line(self.src, h, 0),
            Cow::Owned(h) => {
                let buf = head.header.make_owned(self.src);
                buf.push_str(&h);
                buf.push('\n');
            }
        }
        self.close_unmatched_blocks();
        let table;
        if header_start == 0 {
            table = container;
            self.nodes[container].kind = Kind::Table(head);
            self.nodes[container].content = Content::default();
        } else {
            self.nodes[container].content.truncate(header_start);
            self.tip = container;
            self.finalize(container, self.line_number - 2);
            table = self.add_child(Kind::Table(head));
            self.nodes[table].start_line = self.line_number - 1;
        }
        self.tip = table;
        self.offset = self.line.len();
        self.line_consumed = true;
        Start::Leaf
    }

    // ----- conversion to the public tree -----

    fn convert_children(&self, idx: usize) -> Vec<Block<'a>> {
        self.nodes[idx]
            .children
            .iter()
            .map(|&c| self.convert(c))
            .collect()
    }

    fn inlines(&self, text: &str) -> Vec<Inline<'a>> {
        inline::parse(
            self.src,
            text,
            &self.refs,
            self.opts,
            &mut self.scratch.borrow_mut(),
        )
    }

    fn convert(&self, idx: usize) -> Block<'a> {
        let node = &self.nodes[idx];
        let src = self.src;
        match &node.kind {
            Kind::Paragraph => {
                let text = node.content.text(src).trim_matches(is_ascii_ws_char);
                Block::Paragraph(self.inlines(text))
            }
            Kind::Heading(level) => {
                let text = node.content.text(src).trim_matches(is_ascii_ws_char);
                Block::Heading(ast::Heading {
                    level: *level,
                    children: self.inlines(text),
                })
            }
            Kind::ThematicBreak => Block::ThematicBreak,
            Kind::BlockQuote => Block::BlockQuote(self.convert_children(idx)),
            Kind::List(d) => {
                let kind = if d.bullet != 0 {
                    ast::ListKind::Bullet(d.bullet)
                } else {
                    ast::ListKind::Ordered {
                        start: d.start,
                        delimiter: d.delimiter,
                    }
                };
                let items = node
                    .children
                    .iter()
                    .map(|&item| self.convert_item(item))
                    .collect();
                Block::List(ast::List {
                    kind,
                    tight: d.tight,
                    items,
                })
            }
            Kind::Document | Kind::Item(_) => unreachable!("converted by their parent"),
            Kind::CodeBlock(f) => self.convert_code(node, f),
            Kind::HtmlBlock(_) => {
                let text = node.content.text(src);
                let mut end = text.len();
                // Drop trailing line endings (and the spaces between them).
                let b = text.as_bytes();
                loop {
                    let mut j = end;
                    while j > 0 && b[j - 1] == b' ' {
                        j -= 1;
                    }
                    if j > 0 && b[j - 1] == b'\n' {
                        end = j - 1;
                    } else {
                        break;
                    }
                }
                Block::HtmlBlock(to_cow(src, &text[..end]))
            }
            Kind::Table(head) => self.convert_table(node, head),
        }
    }

    fn convert_item(&self, idx: usize) -> ast::ListItem<'a> {
        let node = &self.nodes[idx];
        let mut task = None;
        let mut children = Vec::with_capacity(node.children.len());
        for (i, &c) in node.children.iter().enumerate() {
            if i == 0 && self.opts.tasklists && matches!(self.nodes[c].kind, Kind::Paragraph) {
                let text = self.nodes[c].content.text(self.src);
                let b = text.as_bytes();
                if b.len() >= 4
                    && b[0] == b'['
                    && matches!(b[1], b' ' | b'x' | b'X')
                    && b[2] == b']'
                    && matches!(b[3], b' ' | b'\t' | b'\n')
                {
                    task = Some(b[1] != b' ');
                    let rest = text[4..].trim_matches(is_ascii_ws_char);
                    children.push(Block::Paragraph(self.inlines(rest)));
                    continue;
                }
            }
            children.push(self.convert(c));
        }
        ast::ListItem { task, children }
    }

    fn convert_code(&self, node: &Node, f: &Fence) -> Block<'a> {
        let src = self.src;
        let text = node.content.text(src);
        let virtual_nl = node.content.owned.is_none() && node.content.virtual_nl;
        if f.fenced {
            let (first, rest) = match text.find('\n') {
                Some(p) => (&text[..p], &text[p + 1..]),
                None => (text, ""),
            };
            let info = lift(src, unescape(first.trim_matches(is_ascii_ws_char)));
            let literal = if virtual_nl && !rest.is_empty() {
                Cow::Owned(format!("{rest}\n"))
            } else {
                to_cow(src, rest)
            };
            Block::CodeBlock(ast::CodeBlock {
                info,
                literal,
                fenced: true,
            })
        } else {
            // Drop trailing blank lines.
            let b = text.as_bytes();
            let mut end = b.len();
            if virtual_nl {
                // The last line has no line ending in the source.
                let line_start = text[..end].rfind('\n').map_or(0, |p| p + 1);
                if !b[line_start..end].iter().all(|&c| is_space_or_tab(c)) {
                    return Block::CodeBlock(ast::CodeBlock {
                        info: Cow::Borrowed(""),
                        literal: Cow::Owned(format!("{text}\n")),
                        fenced: false,
                    });
                }
                end = line_start;
            }
            loop {
                // `end` is just after a `\n` (or 0).
                if end == 0 {
                    break;
                }
                let line_start = text[..end - 1].rfind('\n').map_or(0, |p| p + 1);
                if b[line_start..end - 1].iter().all(|&c| is_space_or_tab(c)) {
                    end = line_start;
                } else {
                    break;
                }
            }
            Block::CodeBlock(ast::CodeBlock {
                info: Cow::Borrowed(""),
                literal: to_cow(src, &text[..end]),
                fenced: false,
            })
        }
    }

    fn convert_table(&self, node: &Node, head: &TableHead) -> Block<'a> {
        let alignments = &head.alignments;
        let text = node.content.text(self.src);
        let lines = text.split_terminator('\n');
        let header_line = head.header.text(self.src).trim_end_matches('\n');
        let cols = alignments.len();
        let row = |line: &str| -> Vec<Vec<Inline<'a>>> {
            let mut cells: Vec<Vec<Inline<'a>>> = split_row(line)
                .into_iter()
                .take(cols)
                .map(|cell| self.cell_inlines(cell))
                .collect();
            cells.resize_with(cols, Vec::new);
            cells
        };
        let header = row(header_line);
        let rows = lines.map(row).collect();
        Block::Table(ast::Table {
            alignments: alignments.to_vec(),
            header,
            rows,
        })
    }

    fn cell_inlines(&self, cell: &str) -> Vec<Inline<'a>> {
        if cell.contains("\\|") {
            let unescaped = cell.replace("\\|", "|");
            inline::parse(
                self.src,
                &unescaped,
                &self.refs,
                self.opts,
                &mut self.scratch.borrow_mut(),
            )
        } else {
            self.inlines(cell)
        }
    }
}

#[inline]
fn is_ascii_ws_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// Finds a byte eight bytes at a time; lines are short, so this beats the
/// setup cost of a general `memchr`.
#[inline]
fn find_byte(needle: u8, hay: &[u8]) -> Option<usize> {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let pattern = LO * needle as u64;
    let mut i = 0;
    while i + 8 <= hay.len() {
        let word = u64::from_le_bytes(hay[i..i + 8].try_into().unwrap()) ^ pattern;
        let found = word.wrapping_sub(LO) & !word & HI;
        if found != 0 {
            return Some(i + (found.trailing_zeros() / 8) as usize);
        }
        i += 8;
    }
    hay[i..].iter().position(|&c| c == needle).map(|p| i + p)
}

#[inline]
fn memchr2(a: u8, b: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&c| c == a || c == b)
}

/// Splits a table row into trimmed cells, honoring `\|` escapes.
fn split_row(line: &str) -> Vec<&str> {
    let line = line.trim_matches([' ', '\t']);
    let line = line.strip_prefix('|').unwrap_or(line);
    let b = line.as_bytes();
    let mut cells = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'|' => {
                cells.push(line[start..i].trim_matches([' ', '\t']));
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if start < b.len() {
        cells.push(line[start..].trim_matches([' ', '\t']));
    } else if cells.is_empty() {
        cells.push("");
    }
    cells
}

/// Parses a table delimiter row such as `| :-- | --: |`.
fn parse_delimiter_row(line: &str) -> Option<Vec<Alignment>> {
    let trimmed = line.trim_matches([' ', '\t']);
    if !trimmed.contains('|') && !trimmed.contains(':') {
        // A row like `---` is a setext underline or a thematic break.
        return None;
    }
    let mut out = Vec::new();
    for cell in split_row(trimmed) {
        let b = cell.as_bytes();
        let left = b.first() == Some(&b':');
        let right = b.len() > 1 && b.last() == Some(&b':');
        let dashes = &b[left as usize..b.len() - right as usize];
        if dashes.is_empty() || !dashes.iter().all(|&c| c == b'-') {
            return None;
        }
        out.push(match (left, right) {
            (true, true) => Alignment::Center,
            (true, false) => Alignment::Left,
            (false, true) => Alignment::Right,
            (false, false) => Alignment::None,
        });
    }
    Some(out)
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

fn starts_with_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && hay[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Detects the start condition of an HTML block (types 1 to 7).
fn html_block_start(s: &[u8]) -> Option<u8> {
    debug_assert_eq!(s.first(), Some(&b'<'));
    let rest = &s[1..];
    for tag in [&b"script"[..], b"pre", b"textarea", b"style"] {
        if starts_with_ci(rest, tag) {
            match rest.get(tag.len()) {
                None | Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'>') => return Some(1),
                _ => {}
            }
        }
    }
    if rest.starts_with(b"!--") {
        return Some(2);
    }
    if rest.starts_with(b"?") {
        return Some(3);
    }
    if rest.starts_with(b"![CDATA[") {
        return Some(5);
    }
    if rest.first() == Some(&b'!') && rest.get(1).is_some_and(u8::is_ascii_alphabetic) {
        return Some(4);
    }
    let name_start = usize::from(rest.first() == Some(&b'/'));
    let name_len = rest[name_start..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    if name_len > 0 {
        let name = &rest[name_start..name_start + name_len];
        let after = &rest[name_start + name_len..];
        let boundary = match after.first() {
            None | Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'>') => true,
            Some(b'/') => after.get(1) == Some(&b'>'),
            _ => false,
        };
        if boundary
            && BLOCK_TAGS
                .iter()
                .any(|t| t.as_bytes().eq_ignore_ascii_case(name))
        {
            return Some(6);
        }
    }
    let end = if name_start == 1 {
        crate::scan::closing_tag(s, 0)
    } else {
        crate::scan::open_tag(s, 0)
    }?;
    s[end..]
        .iter()
        .all(|c| c.is_ascii_whitespace())
        .then_some(7)
}

/// Checks the end condition of HTML block types 1 to 5 on a line.
fn html_block_end(t: u8, line: &[u8]) -> bool {
    let contains = |needle: &[u8]| line.windows(needle.len()).any(|w| w == needle);
    match t {
        1 => {
            let lower = line.to_ascii_lowercase();
            [&b"</script>"[..], b"</pre>", b"</textarea>", b"</style>"]
                .iter()
                .any(|n| lower.windows(n.len()).any(|w| w == *n))
        }
        2 => contains(b"-->"),
        3 => contains(b"?>"),
        4 => line.contains(&b'>'),
        5 => contains(b"]]>"),
        _ => false,
    }
}
