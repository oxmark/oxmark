//! The typed syntax tree produced by [`parse`](crate::parse).
//!
//! Text borrows from the input whenever possible: a [`Cow::Borrowed`] value
//! points straight into the source, and a [`Cow::Owned`] value is only built
//! when the text had to be transformed (entities, backslash escapes, content
//! split across container prefixes such as `>` or list indentation).
//!
//! The tree is faithful to the source: raw HTML, every URL and every
//! attribute are kept as written. Sanitizing happens when rendering, see
//! [`HtmlOptions`](crate::HtmlOptions).

use std::borrow::Cow;

/// A parsed Markdown document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document<'a> {
    /// The YAML frontmatter, as written, when
    /// [`Options::frontmatter`](crate::Options::frontmatter) is enabled.
    pub frontmatter: Option<Cow<'a, str>>,
    /// Top-level blocks, in source order.
    pub children: Vec<Block<'a>>,
    /// Footnotes that are referenced, in the order of their first reference.
    /// Always empty unless [`Options::footnotes`](crate::Options::footnotes)
    /// is enabled.
    pub footnotes: Vec<Footnote<'a>>,
}

/// A footnote definition (`[^label]: text`) that is referenced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footnote<'a> {
    /// The label, as written.
    pub label: Cow<'a, str>,
    /// The footnote number, starting at 1.
    pub index: u32,
    /// How many times the footnote is referenced.
    pub references: u32,
    /// The content of the definition.
    pub children: Vec<Inline<'a>>,
}

/// A reference to a footnote (`[^label]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootnoteReference<'a> {
    /// The label, as written.
    pub label: Cow<'a, str>,
    /// The footnote number, starting at 1.
    pub index: u32,
    /// Which reference to this footnote this is, starting at 1.
    pub occurrence: u32,
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block<'a> {
    /// A paragraph of inline content.
    Paragraph(Paragraph<'a>),
    /// An ATX (`# Title`) or setext (`Title\n===`) heading.
    Heading(Heading<'a>),
    /// A thematic break (`***`, `---`, `___`).
    ThematicBreak,
    /// A block quote (`> ...`).
    BlockQuote(BlockQuote<'a>),
    /// A bullet or ordered list.
    List(List<'a>),
    /// A fenced or indented code block.
    CodeBlock(CodeBlock<'a>),
    /// A raw HTML block, kept verbatim (without its final line ending).
    HtmlBlock(Cow<'a, str>),
    /// A GFM table.
    Table(Table<'a>),
    /// A Comark block component (`::name{props}` ... `::`).
    Component(Component<'a>),
}

/// A paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Paragraph<'a> {
    /// Attributes from a trailing `{...}`.
    pub attrs: Attributes<'a>,
    /// Inline content.
    pub children: Vec<Inline<'a>>,
}

/// A block quote.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockQuote<'a> {
    /// The kind of a GFM alert (`> [!NOTE]`), when
    /// [`Options::alerts`](crate::Options::alerts) is enabled.
    pub alert: Option<AlertKind>,
    /// The blocks inside the quote, without the alert marker.
    pub children: Vec<Block<'a>>,
}

/// The kind of a GFM alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    /// `[!NOTE]`
    Note,
    /// `[!TIP]`
    Tip,
    /// `[!IMPORTANT]`
    Important,
    /// `[!WARNING]`
    Warning,
    /// `[!CAUTION]`
    Caution,
}

impl AlertKind {
    /// The lowercase name: `note`, `tip`...
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::Note => "note",
            AlertKind::Tip => "tip",
            AlertKind::Important => "important",
            AlertKind::Warning => "warning",
            AlertKind::Caution => "caution",
        }
    }

    /// Reads a marker such as `!NOTE`, ignoring case.
    pub(crate) fn from_marker(marker: &str) -> Option<Self> {
        let name = marker.strip_prefix('!')?;
        [
            AlertKind::Note,
            AlertKind::Tip,
            AlertKind::Important,
            AlertKind::Warning,
            AlertKind::Caution,
        ]
        .into_iter()
        .find(|k| k.as_str().eq_ignore_ascii_case(name))
    }
}

/// A Comark block component:
///
/// ```md
/// ::card{title="Hello"}
/// Default slot content
///
/// #footer
/// Footer slot content
/// ::
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component<'a> {
    /// The name, in kebab-case (`::MyCard` gives `my-card`).
    pub name: Cow<'a, str>,
    /// Props from `{...}` and from a YAML block (`---` or
    /// ```` ```yaml [props] ````), in Comark's form: a boolean prop is
    /// `:name="true"` and a YAML value that is not a string is stored as JSON
    /// under `:name`.
    pub attrs: Attributes<'a>,
    /// The default slot: the content before the first named slot.
    pub children: Vec<Block<'a>>,
    /// Named slots (`#name`), in source order.
    pub slots: Vec<Slot<'a>>,
}

/// A named slot of a block component (`#name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot<'a> {
    /// The slot name.
    pub name: Cow<'a, str>,
    /// Attributes from `#name{...}`.
    pub attrs: Attributes<'a>,
    /// The slot content.
    pub children: Vec<Block<'a>>,
}

/// One attribute: a name and its value, kept as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute<'a> {
    /// The name. Comark writes a boolean or JSON prop with a `:` prefix.
    pub name: Cow<'a, str>,
    /// The value, with its quotes removed.
    pub value: Cow<'a, str>,
}

/// The attributes of an element, in order, each name at most once.
///
/// They come from Comark's `{.class #id key="value" bool}` syntax.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attributes<'a>(pub Vec<Attribute<'a>>);

impl<'a> Attributes<'a> {
    /// Whether there is no attribute.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value of an attribute.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_ref())
    }

    /// The attributes, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Attribute<'a>> {
        self.0.iter()
    }

    /// Sets an attribute, keeping its position if it exists. A `class` is
    /// appended to the existing classes instead.
    pub fn set(&mut self, name: Cow<'a, str>, value: Cow<'a, str>) {
        match self.0.iter_mut().find(|a| a.name == name) {
            Some(a) if name == "class" => {
                let v = a.value.to_mut();
                v.push(' ');
                v.push_str(&value);
            }
            Some(a) => a.value = value,
            None => self.0.push(Attribute { name, value }),
        }
    }

    /// Adds every attribute of `other`, like [`set`](Self::set).
    pub fn merge(&mut self, other: Attributes<'a>) {
        for a in other.0 {
            self.set(a.name, a.value);
        }
    }

    /// Detaches the attributes from their source.
    pub fn into_owned(self) -> Attributes<'static> {
        Attributes(
            self.0
                .into_iter()
                .map(|a| Attribute {
                    name: Cow::Owned(a.name.into_owned()),
                    value: Cow::Owned(a.value.into_owned()),
                })
                .collect(),
        )
    }
}

/// A heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading<'a> {
    /// Level from 1 to 6.
    pub level: u8,
    /// The id: an `{#id}` attribute, or the id computed from the text when
    /// [`Options::heading_ids`](crate::Options::heading_ids) is enabled and
    /// the text gives a non-empty id.
    pub id: Option<Cow<'a, str>>,
    /// Attributes from a trailing `{...}`, without the id.
    pub attrs: Attributes<'a>,
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
    /// Attributes from a trailing `{...}` on the first line of the item.
    pub attrs: Attributes<'a>,
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
    Code(Code<'a>),
    /// Raw inline HTML, kept verbatim.
    Html(Cow<'a, str>),
    /// Emphasis (`*a*`).
    Emphasis(Element<'a>),
    /// Strong emphasis (`**a**`).
    Strong(Element<'a>),
    /// GFM strikethrough (`~~a~~`).
    Strikethrough(Element<'a>),
    /// A Comark span (`[text]` or `[text]{.class}`).
    Span(Element<'a>),
    /// A Comark inline component (`:name`, `:name[content]{props}`).
    Component(InlineComponent<'a>),
    /// A link.
    Link(Link<'a>),
    /// An image; its children are the alternative text.
    Image(Link<'a>),
    /// A reference to a footnote.
    FootnoteReference(FootnoteReference<'a>),
}

/// An inline element with attributes and inline content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Element<'a> {
    /// Attributes from a following `{...}`.
    pub attrs: Attributes<'a>,
    /// Inline content.
    pub children: Vec<Inline<'a>>,
}

/// A code span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code<'a> {
    /// The code.
    pub literal: Cow<'a, str>,
    /// Attributes from a following `{...}`.
    pub attrs: Attributes<'a>,
}

/// A Comark inline component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineComponent<'a> {
    /// The name, as written.
    pub name: Cow<'a, str>,
    /// Props from a following `{...}`.
    pub attrs: Attributes<'a>,
    /// The content between `[` and `]`.
    pub children: Vec<Inline<'a>>,
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
    /// Attributes from a following `{...}`.
    pub attrs: Attributes<'a>,
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
            frontmatter: self.frontmatter.map(|f| cow_owned!(f)),
            children: blocks_owned(self.children),
            footnotes: self
                .footnotes
                .into_iter()
                .map(|f| Footnote {
                    label: cow_owned!(f.label),
                    index: f.index,
                    references: f.references,
                    children: inlines_owned(f.children),
                })
                .collect(),
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
            Block::Paragraph(p) => Block::Paragraph(Paragraph {
                attrs: p.attrs.into_owned(),
                children: inlines_owned(p.children),
            }),
            Block::Heading(h) => Block::Heading(Heading {
                level: h.level,
                id: h.id.map(|id| cow_owned!(id)),
                attrs: h.attrs.into_owned(),
                children: inlines_owned(h.children),
            }),
            Block::ThematicBreak => Block::ThematicBreak,
            Block::BlockQuote(q) => Block::BlockQuote(BlockQuote {
                alert: q.alert,
                children: blocks_owned(q.children),
            }),
            Block::List(l) => Block::List(List {
                kind: l.kind,
                tight: l.tight,
                items: l
                    .items
                    .into_iter()
                    .map(|i| ListItem {
                        task: i.task,
                        attrs: i.attrs.into_owned(),
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
            Block::Component(c) => Block::Component(Component {
                name: cow_owned!(c.name),
                attrs: c.attrs.into_owned(),
                children: blocks_owned(c.children),
                slots: c
                    .slots
                    .into_iter()
                    .map(|s| Slot {
                        name: cow_owned!(s.name),
                        attrs: s.attrs.into_owned(),
                        children: blocks_owned(s.children),
                    })
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
            Inline::Code(c) => Inline::Code(Code {
                literal: cow_owned!(c.literal),
                attrs: c.attrs.into_owned(),
            }),
            Inline::Html(t) => Inline::Html(cow_owned!(t)),
            Inline::Emphasis(e) => Inline::Emphasis(e.into_owned()),
            Inline::Strong(e) => Inline::Strong(e.into_owned()),
            Inline::Strikethrough(e) => Inline::Strikethrough(e.into_owned()),
            Inline::Span(e) => Inline::Span(e.into_owned()),
            Inline::Component(c) => Inline::Component(InlineComponent {
                name: cow_owned!(c.name),
                attrs: c.attrs.into_owned(),
                children: inlines_owned(c.children),
            }),
            Inline::Link(l) => Inline::Link(l.into_owned()),
            Inline::Image(l) => Inline::Image(l.into_owned()),
            Inline::FootnoteReference(r) => Inline::FootnoteReference(FootnoteReference {
                label: cow_owned!(r.label),
                index: r.index,
                occurrence: r.occurrence,
            }),
        }
    }
}

impl Element<'_> {
    /// Detaches the element from its source by copying every borrowed text.
    pub fn into_owned(self) -> Element<'static> {
        Element {
            attrs: self.attrs.into_owned(),
            children: inlines_owned(self.children),
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
            attrs: self.attrs.into_owned(),
            children: inlines_owned(self.children),
        }
    }
}
