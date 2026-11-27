//! A fast Rust implementation of Comark (Markdown with components).
//!
//! The parser currently implements [CommonMark 0.31.2] and the
//! [GitHub Flavored Markdown] extensions (tables, strikethrough, task lists
//! and extended autolinks). Comark components come next.
//!
//! ```
//! let html = oxmark::to_html("Hello *world* <script>alert(1)</script>");
//! assert_eq!(
//!     html,
//!     "<p>Hello <em>world</em> &lt;script&gt;alert(1)&lt;/script&gt;</p>\n"
//! );
//! ```
//!
//! # Security
//!
//! The default [`HtmlOptions`] are safe for untrusted input: raw HTML is
//! escaped and URLs with a dangerous scheme (`javascript:`, `vbscript:`,
//! `file:`, non-image `data:`) are dropped. Text and attributes are always
//! escaped.
//!
//! [CommonMark 0.31.2]: https://spec.commonmark.org/0.31.2/
//! [GitHub Flavored Markdown]: https://github.github.com/gfm/

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ast;
mod autolink;
mod block;
mod html;
mod inline;
mod scan;
mod tables;
mod text;

pub use ast::Document;
pub use html::{
    escape_href, escape_html, is_safe_url, render_html, render_html_to, HtmlOptions, RawHtml,
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
}

impl Default for Options {
    /// Every GFM extension enabled.
    fn default() -> Self {
        Options::gfm()
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
        }
    }

    /// CommonMark with every GFM extension.
    pub const fn gfm() -> Self {
        Options {
            tables: true,
            strikethrough: true,
            tasklists: true,
            autolinks: true,
        }
    }
}

/// Parses Markdown with the default options (GFM).
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
