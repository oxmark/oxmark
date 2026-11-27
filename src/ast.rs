//! The typed syntax tree produced by [`parse`](crate::parse).
//!
//! Text borrows from the input whenever possible: a [`Cow::Borrowed`] value
//! points straight into the source, and a [`Cow::Owned`] value is only built
//! when the text had to be transformed (entities, backslash escapes, content
//! split across container prefixes such as `>` or list indentation).
//!
//! The tree is faithful to the source: raw HTML and every URL are kept as
//! written. Sanitizing happens when rendering, see
//! [`HtmlOptions`](crate::HtmlOptions).

use std::borrow::Cow;

/// A parsed Markdown document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document<'a> {
    /// Top-level blocks, in source order.
    pub children: Vec<Block<'a>>,
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block<'a> {
    /// A paragraph of inline content.
    Paragraph(Vec<Inline<'a>>),
    /// An ATX (`# Title`) or setext (`Title\n===`) heading.
    Heading(Heading<'a>),
    /// A thematic break (`***`, `---`, `___`).
    ThematicBreak,
    /// A block quote (`> ...`).
    BlockQuote(Vec<Block<'a>>),
    /// A bullet or ordered list.
    List(List<'a>),
    /// A fenced or indented code block.
    CodeBlock(CodeBlock<'a>),
    /// A raw HTML block, kept verbatim (without its final line ending).
    HtmlBlock(Cow<'a, str>),
    /// A GFM table.
    Table(Table<'a>),
}

/// A heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading<'a> {
    /// Level from 1 to 6.
    pub level: u8,
    /// Inline content.
    pub children: Vec<Inline<'a>>,
}

/// A bullet or ordered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List<'a> {
    /// Bullet or ordered, with the marker details.
    pub kind: ListKind,
    /// A tight list renders its paragraphs without `<p>` tags.
    pub tight: bool,
    /// The list items.
    pub items: Vec<ListItem<'a>>,
}

/// The kind of a [`List`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// A bullet list, with its marker (`-`, `+` or `*`).
    Bullet(u8),
    /// An ordered list.
    Ordered {
        /// The number of the first item.
        start: u32,
        /// The delimiter after the number (`.` or `)`).
        delimiter: u8,
    },
}

/// A list item.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListItem<'a> {
    /// `Some(checked)` for a GFM task list item (`- [ ]` or `- [x]`).
    pub task: Option<bool>,
    /// The blocks inside the item.
    pub children: Vec<Block<'a>>,
}

/// A fenced or indented code block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock<'a> {
    /// The info string of a fenced block (`rust` in ```` ```rust ````),
    /// unescaped. Empty for indented blocks.
    pub info: Cow<'a, str>,
    /// The code, each line ending with `\n`.
    pub literal: Cow<'a, str>,
    /// Whether the block was fenced rather than indented.
    pub fenced: bool,
}

impl CodeBlock<'_> {
    /// The language: the first word of the info string, if any.
    pub fn language(&self) -> Option<&str> {
        self.info
            .split([' ', '\t', '\n'])
            .next()
            .filter(|lang| !lang.is_empty())
    }
}

/// A GFM table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table<'a> {
    /// Column alignments; its length is the number of columns.
    pub alignments: Vec<Alignment>,
    /// The header row.
    pub header: Vec<Vec<Inline<'a>>>,
    /// Body rows. Each row has exactly `alignments.len()` cells.
    pub rows: Vec<Vec<Vec<Inline<'a>>>>,
}

/// The alignment of a table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    /// No alignment given (`---`).
    #[default]
    None,
    /// `:--`
    Left,
    /// `:-:`
    Center,
    /// `--:`
    Right,
}

/// An inline element.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Inline<'a> {
    /// Literal text, with entities and backslash escapes already resolved.
    Text(Cow<'a, str>),
    /// A soft line break (a plain line ending inside a paragraph).
    SoftBreak,
    /// A hard line break (two trailing spaces or a trailing backslash).
    HardBreak,
    /// A code span.
    Code(Cow<'a, str>),
    /// Raw inline HTML, kept verbatim.
    Html(Cow<'a, str>),
    /// Emphasis (`*a*`).
    Emphasis(Vec<Inline<'a>>),
    /// Strong emphasis (`**a**`).
    Strong(Vec<Inline<'a>>),
    /// GFM strikethrough (`~~a~~`).
    Strikethrough(Vec<Inline<'a>>),
    /// A link.
    Link(Link<'a>),
    /// An image; its children are the alternative text.
    Image(Link<'a>),
}

/// The target and content of a link or an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link<'a> {
    /// The destination, unescaped but not percent-encoded nor sanitized.
    pub url: Cow<'a, str>,
    /// The title, unescaped. Empty when there is none.
    pub title: Cow<'a, str>,
    /// The kind of syntax that produced the link.
    pub kind: LinkKind,
    /// The link text, or the image description.
    pub children: Vec<Inline<'a>>,
}

/// How a [`Link`] was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `[text](url)` or a reference link.
    Normal,
    /// `<https://example.com>` or a GFM extended autolink.
    Autolink,
    /// `<user@example.com>` or a GFM extended email autolink; `url` starts
    /// with `mailto:`.
    Email,
}

macro_rules! cow_owned {
    ($c:expr) => {
        Cow::Owned($c.into_owned())
    };
}

impl Document<'_> {
    /// Detaches the document from its source by copying every borrowed text.
    pub fn into_owned(self) -> Document<'static> {
        Document {
            children: blocks_owned(self.children),
        }
    }
}

pub(crate) fn blocks_owned(blocks: Vec<Block<'_>>) -> Vec<Block<'static>> {
    blocks.into_iter().map(Block::into_owned).collect()
}

pub(crate) fn inlines_owned(inlines: Vec<Inline<'_>>) -> Vec<Inline<'static>> {
    inlines.into_iter().map(Inline::into_owned).collect()
}

impl Block<'_> {
    /// Detaches the block from its source by copying every borrowed text.
    pub fn into_owned(self) -> Block<'static> {
        match self {
            Block::Paragraph(c) => Block::Paragraph(inlines_owned(c)),
            Block::Heading(h) => Block::Heading(Heading {
                level: h.level,
                children: inlines_owned(h.children),
            }),
            Block::ThematicBreak => Block::ThematicBreak,
            Block::BlockQuote(c) => Block::BlockQuote(blocks_owned(c)),
            Block::List(l) => Block::List(List {
                kind: l.kind,
                tight: l.tight,
                items: l
                    .items
                    .into_iter()
                    .map(|i| ListItem {
                        task: i.task,
                        children: blocks_owned(i.children),
                    })
                    .collect(),
            }),
            Block::CodeBlock(c) => Block::CodeBlock(CodeBlock {
                info: cow_owned!(c.info),
                literal: cow_owned!(c.literal),
                fenced: c.fenced,
            }),
            Block::HtmlBlock(h) => Block::HtmlBlock(cow_owned!(h)),
            Block::Table(t) => Block::Table(Table {
                alignments: t.alignments,
                header: t.header.into_iter().map(inlines_owned).collect(),
                rows: t
                    .rows
                    .into_iter()
                    .map(|r| r.into_iter().map(inlines_owned).collect())
                    .collect(),
            }),
        }
    }
}

impl Inline<'_> {
    /// Detaches the inline from its source by copying every borrowed text.
    pub fn into_owned(self) -> Inline<'static> {
        match self {
            Inline::Text(t) => Inline::Text(cow_owned!(t)),
            Inline::SoftBreak => Inline::SoftBreak,
            Inline::HardBreak => Inline::HardBreak,
            Inline::Code(t) => Inline::Code(cow_owned!(t)),
            Inline::Html(t) => Inline::Html(cow_owned!(t)),
            Inline::Emphasis(c) => Inline::Emphasis(inlines_owned(c)),
            Inline::Strong(c) => Inline::Strong(inlines_owned(c)),
            Inline::Strikethrough(c) => Inline::Strikethrough(inlines_owned(c)),
            Inline::Link(l) => Inline::Link(l.into_owned()),
            Inline::Image(l) => Inline::Image(l.into_owned()),
        }
    }
}

impl Link<'_> {
    /// Detaches the link from its source by copying every borrowed text.
    pub fn into_owned(self) -> Link<'static> {
        Link {
            url: cow_owned!(self.url),
            title: cow_owned!(self.title),
            kind: self.kind,
            children: inlines_owned(self.children),
        }
    }
}
