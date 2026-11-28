//! A fast Rust implementation of Comark (Markdown with components).
//!
//! The parser implements [CommonMark 0.31.2], the [GitHub Flavored Markdown]
//! extensions (tables, strikethrough, task lists and extended autolinks) and
//! the Comark syntax: block and inline components with props and slots,
//! spans, attributes, frontmatter, alerts, heading ids and footnotes. Raw
//! HTML is parsed into elements, as in Comark.
//!
//! ```
//! let html = oxmark::to_html("Hello *world* <img src=x onerror=alert(1)>");
//! assert_eq!(html, "<p>Hello <em>world</em> <img src=\"x\"></p>\n");
//! ```
//!
//! # Security
//!
//! The default [`HtmlOptions`] are safe for untrusted input: HTML elements
//! go through the same rules as components, URLs with a dangerous scheme (`javascript:`, `vbscript:`,
//! `file:`, non-image `data:`) are dropped, component and attribute props go
//! through Comark's security rules, and components such as `script` or
//! `iframe` are dropped. Text and attributes are always escaped.
//!
//! [CommonMark 0.31.2]: https://spec.commonmark.org/0.31.2/
//! [GitHub Flavored Markdown]: https://github.github.com/gfm/

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ast;
mod autolink;
mod block;
mod html;
mod html_tree;
mod inline;
mod props;
mod scan;
mod slug;
mod tables;
mod text;
mod yaml;

pub use ast::Document;
pub use html::{
    escape_href, escape_html, is_safe_url, render_html, render_html_to, HtmlOptions, HtmlStyle,
    RawHtml,
};

/// Parsing options: which syntax extensions are enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// GFM tables.
    pub tables: bool,
    /// GFM strikethrough (`~~text~~`).
    pub strikethrough: bool,
    /// GFM task list items (`- [x] done`).
    pub tasklists: bool,
    /// GFM extended autolinks (`www.example.com`, `https://example.com`,
    /// `user@example.com` without angle brackets).
    pub autolinks: bool,
    /// Heading ids computed from the heading text, like Comark does
    /// (`## Hello World` gets `id="hello-world"`, repeats get `-1`, `-2`...).
    pub heading_ids: bool,
    /// Nested heading ids, like Comark's `headingIds: 'nested'`: a heading
    /// id starts with the id of the enclosing section heading (`h2` or
    /// deeper), so `## Setup` then `### Install` gives `setup-install`.
    pub nested_heading_ids: bool,
    /// Footnotes: `[^label]` references and single-line `[^label]: text`
    /// definitions, rendered like Comark's footnotes plugin.
    pub footnotes: bool,
    /// Comark components: block components (`::name` ... `::`) with props
    /// and slots, inline components (`:name[content]`) and spans (`[text]`).
    ///
    /// Like in Comark, a `[text]` that is not followed by `(` or `[` is a
    /// span, so shortcut reference links (`[label]` alone) are not links.
    pub components: bool,
    /// Comark attributes: `{.class #id key="value"}` after an inline element
    /// or at the end of a paragraph, heading or list item.
    pub attributes: bool,
    /// YAML frontmatter between `---` lines at the start of the document. It
    /// is kept in [`Document::frontmatter`] instead of being rendered.
    pub frontmatter: bool,
    /// GFM alerts: a block quote starting with `[!NOTE]`, `[!TIP]`,
    /// `[!IMPORTANT]`, `[!WARNING]` or `[!CAUTION]`.
    pub alerts: bool,
    /// Raw HTML parsed into elements, like Comark does: tags become
    /// [`ast::HtmlElement`]s and the Markdown between them becomes their
    /// children (`<div>\n**a**\n</div>` gives a `div` holding a `strong`).
    /// Only `<script>`, `<style>`, `<pre>`, `<textarea>` and comments start
    /// an HTML block; other tags start a paragraph.
    ///
    /// The elements are rendered with the same security rules as components
    /// (see [`HtmlOptions::sanitize`]). When this is off, raw HTML is kept
    /// as written and rendered by [`HtmlOptions::raw_html`].
    pub html: bool,
}

impl Default for Options {
    /// The Comark defaults: every GFM extension, heading ids, components,
    /// attributes, frontmatter, alerts and raw HTML as elements. Footnotes are off, as in Comark where they
    /// come from an optional plugin.
    fn default() -> Self {
        Options {
            heading_ids: true,
            components: true,
            attributes: true,
            frontmatter: true,
            alerts: true,
            html: true,
            ..Options::gfm()
        }
    }
}

impl Options {
    /// Strict CommonMark, without extensions.
    pub const fn commonmark() -> Self {
        Options {
            tables: false,
            strikethrough: false,
            tasklists: false,
            autolinks: false,
            heading_ids: false,
            nested_heading_ids: false,
            footnotes: false,
            components: false,
            attributes: false,
            frontmatter: false,
            alerts: false,
            html: false,
        }
    }

    /// CommonMark with every GFM extension, without any Comark extension.
    pub const fn gfm() -> Self {
        Options {
            tables: true,
            strikethrough: true,
            tasklists: true,
            autolinks: true,
            ..Options::commonmark()
        }
    }
}

/// Parses Markdown with the default options (the Comark syntax).
pub fn parse(input: &str) -> Document<'_> {
    parse_with(input, &Options::default())
}

/// Parses Markdown.
pub fn parse_with<'a>(input: &'a str, options: &Options) -> Document<'a> {
    if input.as_bytes().contains(&0) {
        // U+0000 is replaced for security reasons, as required by the spec.
        let cleaned = input.replace('\0', "\u{FFFD}");
        return block::BlockParser::new(&cleaned, options)
            .parse()
            .into_owned();
    }
    block::BlockParser::new(input, options).parse()
}

/// Converts Markdown to HTML with the default, safe options.
pub fn to_html(input: &str) -> String {
    to_html_with(input, &Options::default(), &HtmlOptions::default())
}

/// Converts Markdown to HTML.
pub fn to_html_with(input: &str, options: &Options, html: &HtmlOptions) -> String {
    let doc = parse_with(input, options);
    let mut out = String::with_capacity(input.len() + input.len() / 4);
    render_html_to(&doc, html, &mut out);
    out
}

#[doc = include_str!("../README.md")]
#[cfg(doctest)]
struct ReadmeDoctests;
