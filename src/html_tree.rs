//! Raw HTML as elements, like Comark's tree walk.
//!
//! Comark keeps a stack of open HTML tags across blocks, the way
//! `hast-util-raw` feeds `parse5`: an open tag pushes a frame, the Markdown
//! that follows fills it, and a matching close tag pops it, closing the
//! frames opened inside it. This module runs the same walk over the parsed
//! tree, turning [`Inline::Html`] and [`Block::HtmlBlock`] into
//! [`HtmlElement`]s and comments.

use std::borrow::Cow;
use std::mem;

use crate::ast::{
    Attribute, Attributes, Block, Document, Element, HtmlElement, Inline, Node, Paragraph,
};
use crate::scan;
use crate::text::decode_entities;

/// Tags that stay inline in a block frame (Comark's `inlineTags`).
const INLINE_TAGS: &[&str] = &[
    "strong", "em", "del", "code", "a", "span", "sub", "sup", "kbd",
];

/// Tags whose body is kept verbatim.
const RAW_TAGS: &[&str] = &["style", "pre", "script", "textarea"];

/// Tags without content.
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Open tags beyond this many open frames, counting the enclosing scopes,
/// are ignored, which bounds the depth of the tree.
const MAX_DEPTH: usize = 64;

/// A node during the walk.
enum Item<'a> {
    Inline(Inline<'a>),
    Block(Block<'a>),
    Element(HtmlElement<'a>),
    Comment(Cow<'a, str>),
}

/// What a processed node gives: nothing, one node, or several siblings.
enum Out<'a> {
    None,
    One(Item<'a>),
    Fragment(Vec<Item<'a>>),
}

struct Frame<'a> {
    /// The tag, or `None` for a comment spanning several paragraphs.
    tag: Option<Cow<'a, str>>,
    attrs: Attributes<'a>,
    children: Vec<Item<'a>>,
    /// Whether the frame outlived the paragraph that opened it.
    block: bool,
    /// Inline nodes waiting to be wrapped in a paragraph of a block frame.
    pending: Vec<Item<'a>>,
}

#[derive(Default)]
struct Walk<'a> {
    stack: Vec<Frame<'a>>,
    /// The frames open in the enclosing scopes.
    outer_depth: usize,
}

/// Turns the raw HTML of a document into elements.
pub(crate) fn transform(doc: &mut Document<'_>) {
    if !doc.children.iter().any(block_has_html)
        && !doc.footnotes.iter().any(|f| inlines_have_html(&f.children))
    {
        return;
    }
    let mut w = Walk::default();
    let mut nodes = Vec::new();
    for block in mem::take(&mut doc.children) {
        let (out, sibling) = w.block(block);
        // The frames still open span past this block.
        for frame in &mut w.stack {
            frame.block = true;
        }
        if sibling {
            push_out(&mut nodes, out);
        } else {
            let free = w.deliver_block(out);
            push_out(&mut nodes, free);
        }
    }
    w.close_frames(&mut nodes, 0, true);
    doc.children = into_blocks(nodes);
    for footnote in &mut doc.footnotes {
        footnote.children = w.scoped_inlines(mem::take(&mut footnote.children));
    }
}

fn block_has_html(block: &Block<'_>) -> bool {
    match block {
        Block::HtmlBlock(_) => true,
        Block::Paragraph(p) => inlines_have_html(&p.children),
        Block::Heading(h) => inlines_have_html(&h.children),
        Block::BlockQuote(q) => q.children.iter().any(block_has_html),
        Block::List(l) => l
            .items
            .iter()
            .any(|i| i.children.iter().any(block_has_html)),
        Block::Table(t) => t
            .header
            .iter()
            .chain(t.rows.iter().flatten())
            .any(|c| inlines_have_html(c)),
        Block::Component(c) => {
            c.children.iter().any(block_has_html)
                || c.slots
                    .iter()
                    .any(|s| s.children.iter().any(block_has_html))
        }
        _ => false,
    }
}

fn inlines_have_html(inlines: &[Inline<'_>]) -> bool {
    inlines.iter().any(|i| match i {
        Inline::Html(_) => true,
        Inline::Text(t) => t.starts_with("<!--"),
        Inline::Emphasis(e) | Inline::Strong(e) | Inline::Strikethrough(e) | Inline::Span(e) => {
            inlines_have_html(&e.children)
        }
        Inline::Link(l) => inlines_have_html(&l.children),
        Inline::Component(c) => inlines_have_html(&c.children),
        _ => false,
    })
}

fn push_out<'a>(nodes: &mut Vec<Item<'a>>, out: Out<'a>) {
    match out {
        Out::None => {}
        Out::One(item) => nodes.push(item),
        Out::Fragment(items) => nodes.extend(items),
    }
}

impl<'a> Walk<'a> {
    fn block(&mut self, block: Block<'a>) -> (Out<'a>, bool) {
        // Without HTML and without open frames, the walk changes nothing.
        if self.stack.is_empty() && !block_has_html(&block) {
            return (Out::One(Item::Block(block)), false);
        }
        let block = match block {
            Block::Paragraph(p) => return self.paragraph(p.attrs, p.children),
            Block::HtmlBlock(html) => return self.paragraph(Attributes::default(), split(html)),
            Block::Heading(mut h) => {
                h.children = self.inline_children(mem::take(&mut h.children));
                Block::Heading(h)
            }
            Block::BlockQuote(mut q) => {
                q.children = self.scoped_blocks(mem::take(&mut q.children), true);
                Block::BlockQuote(q)
            }
            Block::List(mut l) => {
                for item in &mut l.items {
                    item.children = self.scoped_blocks(mem::take(&mut item.children), true);
                }
                Block::List(l)
            }
            Block::Table(mut t) => {
                for cell in t.header.iter_mut().chain(t.rows.iter_mut().flatten()) {
                    *cell = self.scoped_inlines(mem::take(cell));
                }
                Block::Table(t)
            }
            Block::Component(mut c) => {
                c.children = self.scoped_blocks(mem::take(&mut c.children), false);
                for slot in &mut c.slots {
                    slot.children = self.scoped_blocks(mem::take(&mut slot.children), false);
                }
                Block::Component(c)
            }
            other => other,
        };
        (Out::One(Item::Block(block)), false)
    }

    /// Comark's `paragraph_open`. The flag tells that the result sits beside
    /// the frames still open (`</div><div>`), not inside them.
    fn paragraph(&mut self, attrs: Attributes<'a>, inlines: Vec<Inline<'a>>) -> (Out<'a>, bool) {
        let starts_with_html = matches!(inlines.first(), Some(Inline::Html(_)));
        let depth_before = self.stack.len();
        let mut children = self.inlines(inlines, true);
        // An HTML root that started before this paragraph was closed in it.
        if depth_before > 0 && self.stack.is_empty() {
            if children.len() == 1 && is_array(&children[0]) {
                return (Out::One(children.pop().unwrap()), false);
            }
            return (paragraph(attrs, children), false);
        }
        // Free content went into the open frames; what is left was closed
        // beside them.
        if !self.stack.is_empty() {
            for frame in &mut self.stack {
                flush(frame);
            }
            let sibling = !children.is_empty();
            return (fragment(children), sibling);
        }
        if children.is_empty() {
            return (Out::None, false);
        }
        if children.len() == 1
            && is_array(&children[0])
            && starts_with_html
            && (tag_of(&children[0]).is_none_or(|t| !INLINE_TAGS.contains(&t))
                || matches!(&children[0], Item::Element(e) if e.block))
        {
            let mut item = children.pop().unwrap();
            if let Item::Element(e) = &mut item {
                e.block = true;
            }
            return (Out::One(item), false);
        }
        if children.iter().all(|c| matches!(c, Item::Element(_))) {
            return (Out::Fragment(children), false);
        }
        (paragraph(attrs, children), false)
    }

    /// Comark's `processInline`. With `nest`, free nodes go into the open
    /// frame; otherwise only the frames opened in this run receive them.
    fn inlines(&mut self, inlines: Vec<Inline<'a>>, nest: bool) -> Vec<Item<'a>> {
        let depth = self.stack.len();
        let mut nodes = Vec::with_capacity(inlines.len());
        let mut after_html = false;
        for inline in inlines {
            let is_html = matches!(inline, Inline::Html(_));
            let out = self.inline(inline, after_html);
            after_html = is_html;
            let out = if nest || self.stack.len() > depth {
                self.deliver_inline(out)
            } else {
                out
            };
            push_out(&mut nodes, out);
        }
        nodes
    }

    /// The children of an inline container: frames opened inside it close
    /// at its end.
    fn inline_children(&mut self, inlines: Vec<Inline<'a>>) -> Vec<Inline<'a>> {
        let depth = self.stack.len();
        let mut items = self.inlines(inlines, false);
        self.close_frames(&mut items, depth, false);
        into_inlines(items)
    }

    /// Inline content with its own stack (a table cell, a footnote).
    fn scoped_inlines(&mut self, inlines: Vec<Inline<'a>>) -> Vec<Inline<'a>> {
        let outer = mem::take(&mut self.stack);
        self.outer_depth += outer.len();
        let mut items = self.inlines(inlines, true);
        self.close_frames(&mut items, 0, true);
        self.outer_depth -= outer.len();
        self.stack = outer;
        into_inlines(items)
    }

    /// The blocks of a container, with their own stack so that tags opened
    /// inside cannot capture the container's siblings. In a component, the
    /// blocks are not delivered to the open frames (Comark's slot walk).
    fn scoped_blocks(&mut self, blocks: Vec<Block<'a>>, deliver: bool) -> Vec<Block<'a>> {
        let outer = mem::take(&mut self.stack);
        self.outer_depth += outer.len();
        let mut items = Vec::with_capacity(blocks.len());
        for block in blocks {
            let (out, _) = self.block(block);
            let out = if deliver {
                self.deliver_inline(out)
            } else {
                out
            };
            push_out(&mut items, out);
        }
        self.close_frames(&mut items, 0, true);
        self.outer_depth -= outer.len();
        self.stack = outer;
        into_blocks(items)
    }

    fn inline(&mut self, inline: Inline<'a>, after_html: bool) -> Out<'a> {
        let inline = match inline {
            Inline::Text(t) => {
                if t.is_empty() {
                    return Out::None;
                }
                if let Some(out) = self.comment_tag(&t) {
                    return out;
                }
                Inline::Text(t)
            }
            // Line breaks inside open HTML separate content, they are not text.
            Inline::SoftBreak if !self.stack.is_empty() || after_html => return Out::None,
            Inline::Html(raw) => return self.html(raw),
            Inline::Emphasis(e) => Inline::Emphasis(self.element(e)),
            Inline::Strong(e) => Inline::Strong(self.element(e)),
            Inline::Strikethrough(e) => Inline::Strikethrough(self.element(e)),
            Inline::Span(e) => Inline::Span(self.element(e)),
            Inline::Link(mut l) => {
                l.children = self.inline_children(mem::take(&mut l.children));
                Inline::Link(l)
            }
            Inline::Component(mut c) => {
                c.children = self.inline_children(mem::take(&mut c.children));
                Inline::Component(c)
            }
            other => other,
        };
        Out::One(Item::Inline(inline))
    }

    fn element(&mut self, mut e: Element<'a>) -> Element<'a> {
        if self.stack.is_empty() && !inlines_have_html(&e.children) {
            return e;
        }
        e.children = self.inline_children(mem::take(&mut e.children));
        e
    }

    /// Comark's `html_inline`: open tags push a frame, close tags pop it.
    fn html(&mut self, raw: Cow<'a, str>) -> Out<'a> {
        match parse_tag(&raw) {
            Tag::Comment(range) => Out::One(Item::Comment(sub(&raw, range))),
            Tag::Other => match self.comment_tag(&raw) {
                Some(out) => out,
                None => Out::One(Item::Comment(raw)),
            },
            Tag::Close(name) => {
                let Some(index) = self.stack.iter().rposition(|f| {
                    f.tag
                        .as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case(name))
                }) else {
                    // HTML parsers read a stray `</br>` as `<br>`.
                    return if name.eq_ignore_ascii_case("br") {
                        Out::One(Item::Element(element("br".into(), Attributes::default())))
                    } else {
                        Out::None
                    };
                };
                let mut closed = None;
                while self.stack.len() > index {
                    let item = frame_item(self.stack.pop().unwrap());
                    if self.stack.len() > index {
                        let free = self.deliver_inline(Out::One(item));
                        debug_assert!(matches!(free, Out::None));
                    } else {
                        closed = Some(item);
                    }
                }
                closed.map_or(Out::None, Out::One)
            }
            Tag::Open {
                name,
                attrs,
                self_closing,
            } => {
                let tag = normalize_tag(sub(&raw, name));
                let attrs = parse_attrs(&raw, attrs);
                if self_closing {
                    return Out::One(Item::Element(element(tag, attrs)));
                }
                if self.outer_depth + self.stack.len() < MAX_DEPTH {
                    self.stack.push(Frame {
                        tag: Some(tag),
                        attrs,
                        children: Vec::new(),
                        block: false,
                        pending: Vec::new(),
                    });
                }
                Out::None
            }
        }
    }

    /// `<!--` and `-->` met as text or as an unfinished tag, when blank lines
    /// split a comment: the same stack as elements, with no tag.
    fn comment_tag(&mut self, content: &Cow<'a, str>) -> Option<Out<'a>> {
        let end = content.find("-->");
        if let (Some(i), Some(Frame { tag: None, .. })) = (end, self.stack.last()) {
            if i > 0 {
                let text = Item::Inline(Inline::Text(sub(content, 0..i)));
                let free = self.deliver_inline(Out::One(text));
                debug_assert!(matches!(free, Out::None));
            }
            let frame = self.stack.pop().unwrap();
            return Some(Out::One(frame_item(frame)));
        }
        if !content.starts_with("<!--") {
            return None;
        }
        match end {
            Some(end) if end >= 4 => Some(Out::One(Item::Comment(sub(content, 4..end)))),
            Some(_) => Some(Out::One(Item::Comment(Cow::Borrowed("")))),
            None => {
                if self.outer_depth + self.stack.len() < MAX_DEPTH {
                    let body = sub(content, 4..content.len());
                    let children = if body.is_empty() {
                        Vec::new()
                    } else {
                        vec![Item::Inline(Inline::Text(body))]
                    };
                    self.stack.push(Frame {
                        tag: None,
                        attrs: Attributes::default(),
                        children,
                        block: true,
                        pending: Vec::new(),
                    });
                }
                Some(Out::None)
            }
        }
    }

    /// Nests a finished block into the open frame, or returns it.
    fn deliver_block(&mut self, out: Out<'a>) -> Out<'a> {
        let Some(top) = self.stack.last_mut() else {
            return out;
        };
        if !matches!(out, Out::None) {
            flush(top);
        }
        push_out(&mut top.children, out);
        Out::None
    }

    /// Nests a node into the open frame, or returns it. Inline content of a
    /// block frame is gathered into a paragraph.
    fn deliver_inline(&mut self, out: Out<'a>) -> Out<'a> {
        let Some(top) = self.stack.last_mut() else {
            return out;
        };
        match out {
            Out::None => {}
            Out::One(item) if top.block && is_inlineish(&item) => top.pending.push(item),
            Out::One(item) => {
                flush(top);
                top.children.push(item);
            }
            Out::Fragment(items) => {
                flush(top);
                top.children.extend(items);
            }
        }
        Out::None
    }

    /// Closes the frames above `depth`, each into the next outer one and the
    /// outermost into `children`.
    fn close_frames(&mut self, children: &mut Vec<Item<'a>>, depth: usize, block: bool) {
        while self.stack.len() > depth {
            let mut frame = self.stack.pop().unwrap();
            if block {
                frame.block = true;
            }
            let item = frame_item(frame);
            let nested = self.stack.len() > depth;
            match self.stack.last_mut() {
                Some(top) if nested => top.children.push(item),
                _ => children.push(item),
            }
        }
    }
}

fn element<'a>(tag: Cow<'a, str>, attrs: Attributes<'a>) -> HtmlElement<'a> {
    HtmlElement {
        tag,
        attrs,
        block: false,
        children: Vec::new(),
    }
}

fn paragraph<'a>(attrs: Attributes<'a>, children: Vec<Item<'a>>) -> Out<'a> {
    if children.is_empty() {
        return Out::None;
    }
    Out::One(Item::Block(Block::Paragraph(Paragraph {
        attrs,
        children: into_inlines(children),
    })))
}

fn fragment(mut items: Vec<Item<'_>>) -> Out<'_> {
    match items.len() {
        0 => Out::None,
        1 => Out::One(items.pop().unwrap()),
        _ => Out::Fragment(items),
    }
}

fn flush(frame: &mut Frame<'_>) {
    if !frame.pending.is_empty() {
        let pending = mem::take(&mut frame.pending);
        frame.children.push(Item::Block(Block::Paragraph(Paragraph {
            attrs: Attributes::default(),
            children: into_inlines(pending),
        })));
    }
}

fn frame_item(mut frame: Frame<'_>) -> Item<'_> {
    flush(&mut frame);
    let Some(tag) = frame.tag else {
        // A comment spanning paragraphs: its text, with the blank lines.
        let mut body = String::new();
        for child in &frame.children {
            body.push_str("\n\n");
            flatten_item(child, &mut body);
        }
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        return Item::Comment(Cow::Owned(body));
    };
    Item::Element(HtmlElement {
        tag,
        attrs: frame.attrs,
        block: frame.block,
        children: frame.children.into_iter().map(into_node).collect(),
    })
}

fn flatten_item(item: &Item<'_>, out: &mut String) {
    match item {
        Item::Inline(i) => flatten_inline(i, out),
        Item::Block(Block::Paragraph(p)) => p.children.iter().for_each(|i| flatten_inline(i, out)),
        Item::Block(_) | Item::Comment(_) => {}
        Item::Element(e) => flatten_nodes(&e.children, out),
    }
}

fn flatten_nodes(nodes: &[Node<'_>], out: &mut String) {
    for node in nodes {
        match node {
            Node::Inline(i) => flatten_inline(i, out),
            Node::Block(Block::Paragraph(p)) => {
                p.children.iter().for_each(|i| flatten_inline(i, out))
            }
            Node::Block(Block::HtmlElement(e)) => flatten_nodes(&e.children, out),
            Node::Block(_) => {}
        }
    }
}

fn flatten_inline(inline: &Inline<'_>, out: &mut String) {
    match inline {
        Inline::Text(t) => out.push_str(t),
        Inline::SoftBreak => out.push('\n'),
        Inline::Code(c) => out.push_str(&c.literal),
        Inline::HtmlElement(e) => flatten_nodes(&e.children, out),
        Inline::Emphasis(e) | Inline::Strong(e) | Inline::Strikethrough(e) | Inline::Span(e) => {
            e.children.iter().for_each(|i| flatten_inline(i, out))
        }
        Inline::Link(l) => l.children.iter().for_each(|i| flatten_inline(i, out)),
        Inline::Component(c) => c.children.iter().for_each(|i| flatten_inline(i, out)),
        _ => {}
    }
}

/// Whether Comark sees the node as an array (an element), not a string.
fn is_array(item: &Item<'_>) -> bool {
    !matches!(item, Item::Inline(Inline::Text(_) | Inline::SoftBreak))
}

/// The tag of a node, `None` for a comment.
fn tag_of<'s>(item: &'s Item<'_>) -> Option<&'s str> {
    Some(match item {
        Item::Element(e) => &e.tag,
        Item::Comment(_) => return None,
        Item::Inline(i) => match i {
            Inline::Strong(_) => "strong",
            Inline::Emphasis(_) => "em",
            Inline::Strikethrough(_) => "del",
            Inline::Span(_) => "span",
            Inline::Link(_) => "a",
            Inline::Code(_) => "code",
            Inline::Image(_) => "img",
            Inline::HardBreak => "br",
            Inline::FootnoteReference(_) => "sup",
            Inline::Component(c) => &c.name,
            Inline::HtmlElement(e) => &e.tag,
            _ => return None,
        },
        Item::Block(_) => "div",
    })
}

fn is_inlineish(item: &Item<'_>) -> bool {
    match item {
        Item::Inline(Inline::Text(_) | Inline::SoftBreak) | Item::Comment(_) => true,
        Item::Block(_) => false,
        _ => tag_of(item).is_none_or(|t| INLINE_TAGS.contains(&t)),
    }
}

fn into_inline(item: Item<'_>) -> Option<Inline<'_>> {
    match item {
        Item::Inline(i) => Some(i),
        Item::Element(e) => Some(Inline::HtmlElement(Box::new(e))),
        Item::Comment(c) => Some(Inline::HtmlComment(c)),
        // Blocks never reach inline content.
        Item::Block(_) => None,
    }
}

fn into_inlines(items: Vec<Item<'_>>) -> Vec<Inline<'_>> {
    items.into_iter().filter_map(into_inline).collect()
}

fn into_node(item: Item<'_>) -> Node<'_> {
    match item {
        Item::Block(b) => Node::Block(b),
        Item::Element(e) if e.block => Node::Block(Block::HtmlElement(e)),
        other => Node::Inline(into_inline(other).unwrap()),
    }
}

/// Blocks from nodes; inline nodes left at block level are gathered into
/// paragraphs.
fn into_blocks(items: Vec<Item<'_>>) -> Vec<Block<'_>> {
    let mut blocks = Vec::with_capacity(items.len());
    let mut inlines = Vec::new();
    for item in items {
        let block = match item {
            Item::Block(b) => b,
            Item::Element(e) => Block::HtmlElement(e),
            Item::Comment(c) => Block::HtmlComment(c),
            Item::Inline(i) => {
                inlines.push(i);
                continue;
            }
        };
        if !inlines.is_empty() {
            blocks.push(Block::Paragraph(Paragraph {
                attrs: Attributes::default(),
                children: mem::take(&mut inlines),
            }));
        }
        blocks.push(block);
    }
    if !inlines.is_empty() {
        blocks.push(Block::Paragraph(Paragraph {
            attrs: Attributes::default(),
            children: inlines,
        }));
    }
    blocks
}

/// A part of a `Cow`, borrowed when the `Cow` is.
fn sub<'a>(s: &Cow<'a, str>, range: std::ops::Range<usize>) -> Cow<'a, str> {
    match s {
        Cow::Borrowed(b) => Cow::Borrowed(&b[range]),
        Cow::Owned(o) => Cow::Owned(o[range].to_owned()),
    }
}

/// Lowercases a tag name written all in uppercase (`DIV`); a name with a
/// lowercase letter keeps its case, for component lookup.
fn normalize_tag(tag: Cow<'_, str>) -> Cow<'_, str> {
    if tag.bytes().any(|c| c.is_ascii_lowercase()) {
        tag
    } else {
        Cow::Owned(tag.to_ascii_lowercase())
    }
}

enum Tag<'s> {
    /// The range of the comment text, between `<!--` and `-->`.
    Comment(std::ops::Range<usize>),
    Close(&'s str),
    Open {
        name: std::ops::Range<usize>,
        attrs: std::ops::Range<usize>,
        self_closing: bool,
    },
    Other,
}

/// Comark's `parseHtmlInline`.
fn parse_tag(raw: &str) -> Tag<'_> {
    let start = raw.len() - raw.trim_start().len();
    let end = raw.trim_end().len();
    let t = &raw[start..end];
    if t.starts_with("<!--") && t.ends_with("-->") {
        let from = start + 4;
        let to = (end - 3).max(from);
        return Tag::Comment(from..to);
    }
    let b = t.as_bytes();
    if b.len() < 3 || b[0] != b'<' || b[b.len() - 1] != b'>' {
        return Tag::Other;
    }
    let is_name = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'-');
    let close = b[1] == b'/';
    let mut i = if close { 2 } else { 1 };
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    if !b.get(i).is_some_and(u8::is_ascii_alphabetic) {
        return Tag::Other;
    }
    let name_start = i;
    while i < b.len() && is_name(b[i]) {
        i += 1;
    }
    let name_end = i;
    if close {
        return if t[name_end..t.len() - 1].trim().is_empty() {
            Tag::Close(&t[name_start..name_end])
        } else {
            Tag::Other
        };
    }
    let inner_end = b.len() - 1;
    let slash = inner_end > name_end && b[inner_end - 1] == b'/';
    let attrs_end = if slash { inner_end - 1 } else { inner_end };
    if attrs_end > name_end && !b[name_end].is_ascii_whitespace() {
        return Tag::Other;
    }
    let name = &t[name_start..name_end];
    let void = VOID_TAGS.iter().any(|v| v.eq_ignore_ascii_case(name));
    Tag::Open {
        name: start + name_start..start + name_end,
        attrs: start + name_end..start + attrs_end.max(name_end),
        self_closing: slash || void,
    }
}

/// Comark's `parseHtmlAttributes`: `key="v"`, `key='v'`, `key=v` and bare
/// `key`. Names are lowercased and values have their references decoded.
fn parse_attrs<'a>(raw: &Cow<'a, str>, range: std::ops::Range<usize>) -> Attributes<'a> {
    let mut list = Vec::new();
    let s = &raw[range.clone()];
    let b = s.as_bytes();
    let is_name = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.' | b'-');
    let mut i = 0;
    while i < b.len() {
        if !is_name(b[i]) {
            i += 1;
            continue;
        }
        let name_start = i;
        while i < b.len() && is_name(b[i]) {
            i += 1;
        }
        let name = &s[name_start..i];
        let mut j = i;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let mut value = None;
        if b.get(j) == Some(&b'=') {
            j += 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            match b.get(j) {
                Some(&q @ (b'"' | b'\'')) => {
                    if let Some(len) = s[j + 1..].find(q as char) {
                        value = Some((j + 1, j + 1 + len));
                        i = j + 2 + len;
                    }
                }
                Some(_) => {
                    let mut k = j;
                    while k < b.len()
                        && !b[k].is_ascii_whitespace()
                        && !matches!(b[k], b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
                    {
                        k += 1;
                    }
                    if k > j {
                        value = Some((j, k));
                        i = k;
                    }
                }
                None => {}
            }
        }
        let name = if name.bytes().any(|c| c.is_ascii_uppercase()) {
            Cow::Owned(name.to_ascii_lowercase())
        } else {
            sub(
                raw,
                range.start + name_start..range.start + name_start + name.len(),
            )
        };
        let value = match value {
            // A boolean attribute.
            None => Cow::Borrowed("true"),
            Some((from, to)) => match decode_entities(&s[from..to]) {
                Cow::Borrowed(_) => sub(raw, range.start + from..range.start + to),
                Cow::Owned(o) => Cow::Owned(o),
            },
        };
        list.push(Attribute { name, value });
    }
    Attributes::from_list(list, false)
}

/// Splits an HTML block into tags and text, like Comark's `htmlToTokens`.
/// The body of `<style>`, `<pre>`, `<script>` and `<textarea>` is kept
/// verbatim, tags included.
fn split(html: Cow<'_, str>) -> Vec<Inline<'_>> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut memo = scan::HtmlMemo::default();
    let mut pos = 0;
    let mut text_start = 0;
    while pos < b.len() {
        let starts_tag = b[pos] == b'<'
            && b.get(pos + 1)
                .is_some_and(|&c| matches!(c, b'!' | b'?' | b'/') || c.is_ascii_alphabetic());
        let Some(end) = starts_tag
            .then(|| scan::html_tag(b, pos, &mut memo))
            .flatten()
        else {
            pos += 1;
            continue;
        };
        if pos > text_start {
            push_text(&mut out, sub(&html, text_start..pos), false);
        }
        out.push(Inline::Html(sub(&html, pos..end)));
        let raw = raw_tag_name(&b[pos..end]);
        pos = end;
        text_start = pos;
        if let Some(name) = raw {
            match raw_closer(b, pos, name) {
                Some((at, close_end)) => {
                    push_text(&mut out, sub(&html, pos..at), true);
                    out.push(Inline::Html(sub(&html, at..close_end)));
                    pos = close_end;
                    text_start = pos;
                }
                None => {
                    push_text(&mut out, sub(&html, pos..b.len()), true);
                    return out;
                }
            }
        }
    }
    if text_start < b.len() {
        push_text(&mut out, sub(&html, text_start..b.len()), false);
    }
    out
}

/// The name of a raw-text open tag (`<pre>`, not `<pre/>`).
fn raw_tag_name(tag: &[u8]) -> Option<&'static str> {
    if tag.get(1) == Some(&b'/') || tag.ends_with(b"/>") {
        return None;
    }
    let len = tag[1..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    let name = &tag[1..1 + len];
    RAW_TAGS
        .iter()
        .copied()
        .find(|t| t.as_bytes().eq_ignore_ascii_case(name))
}

/// The position and end of `</name>` from `from`.
fn raw_closer(b: &[u8], from: usize, name: &str) -> Option<(usize, usize)> {
    let mut i = from;
    while let Some(p) = b[i..].iter().position(|&c| c == b'<') {
        let at = i + p;
        let n = at + 2 + name.len();
        if b.get(at + 1) == Some(&b'/')
            && b.len() >= n
            && b[at + 2..n].eq_ignore_ascii_case(name.as_bytes())
        {
            let mut j = n;
            while j < b.len() && b[j] <= b' ' {
                j += 1;
            }
            if b.get(j) == Some(&b'>') {
                return Some((at, j + 1));
            }
        }
        i = at + 1;
    }
    None
}

/// Text between tags of an HTML block. Unless verbatim, a run with a line
/// break loses its leading indent and its final line break.
fn push_text<'a>(out: &mut Vec<Inline<'a>>, text: Cow<'a, str>, verbatim: bool) {
    let text = if verbatim { text } else { literal_text(text) };
    if !text.is_empty() {
        out.push(Inline::Text(text));
    }
}

fn literal_text(text: Cow<'_, str>) -> Cow<'_, str> {
    let b = text.as_bytes();
    if !b.iter().any(|&c| c == b'\n' || c == b'\r') {
        return text;
    }
    let mut start = 0;
    let first_break = b.iter().position(|&c| c == b'\n' || c == b'\r').unwrap();
    if b[..first_break].iter().all(u8::is_ascii_whitespace) {
        match b.iter().position(|c| !c.is_ascii_whitespace()) {
            Some(p) => start = p,
            None => return Cow::Borrowed(""),
        }
    }
    let mut end = b.len();
    let mut e = end;
    while e > 0 && matches!(b[e - 1], b' ' | b'\t') {
        e -= 1;
    }
    if e > 0 && matches!(b[e - 1], b'\n' | b'\r') {
        e -= 1;
        if e > 0 && b[e] == b'\n' && b[e - 1] == b'\r' {
            e -= 1;
        }
        while e > 0 && matches!(b[e - 1], b' ' | b'\t') {
            e -= 1;
        }
        if e >= start {
            end = e;
        }
    }
    if start >= end {
        return Cow::Borrowed("");
    }
    sub(&text, start..end)
}
