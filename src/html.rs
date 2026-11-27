//! HTML rendering, with sanitizing of raw HTML and URLs.

use crate::ast::{Alignment, Block, CodeBlock, Document, Footnote, Inline, Link, List, ListKind};

/// What to do with raw HTML (HTML blocks and inline tags) when rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RawHtml {
    /// Render raw HTML as escaped text, so it shows up literally.
    #[default]
    Escape,
    /// Drop raw HTML, leaving an `<!-- raw HTML omitted -->` comment.
    Omit,
    /// Pass raw HTML through untouched. Only use this with trusted input, or
    /// sanitize the output afterwards.
    Allow,
}

/// HTML rendering options. The defaults are safe for untrusted input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HtmlOptions {
    /// Raw HTML policy. Defaults to [`RawHtml::Escape`].
    pub raw_html: RawHtml,
    /// When `false` (the default), link and image URLs with a dangerous
    /// scheme (`javascript:`, `vbscript:`, `file:`, and `data:` except for
    /// PNG, GIF, JPEG and WebP images) are replaced by an empty URL.
    pub dangerous_urls: bool,
    /// When raw HTML is allowed, escape the tags that GFM filters
    /// (`<script>`, `<style>`, `<iframe>`, `<textarea>`, `<title>`, `<xmp>`,
    /// `<noembed>`, `<noframes>` and `<plaintext>`). Defaults to `true`.
    pub tagfilter: bool,
}

impl Default for HtmlOptions {
    fn default() -> Self {
        HtmlOptions {
            raw_html: RawHtml::Escape,
            dangerous_urls: false,
            tagfilter: true,
        }
    }
}

impl HtmlOptions {
    /// Options that trust the input completely: raw HTML and every URL are
    /// rendered as written, without the GFM tag filter. This matches the
    /// output of the CommonMark reference implementation in its unsafe mode.
    pub fn trusted() -> Self {
        HtmlOptions {
            raw_html: RawHtml::Allow,
            dangerous_urls: true,
            tagfilter: false,
        }
    }
}

/// Renders a document to HTML.
pub fn render_html(doc: &Document<'_>, options: &HtmlOptions) -> String {
    let mut out = String::with_capacity(256);
    render_html_to(doc, options, &mut out);
    out
}

/// Renders a document to HTML, appending to `out`.
pub fn render_html_to(doc: &Document<'_>, options: &HtmlOptions, out: &mut String) {
    let mut r = Renderer { out, opts: options };
    r.blocks(&doc.children, false);
    if !doc.footnotes.is_empty() {
        r.footnotes(&doc.footnotes);
    }
}

struct Renderer<'o, 'w> {
    out: &'w mut String,
    opts: &'o HtmlOptions,
}

impl Renderer<'_, '_> {
    #[inline]
    fn cr(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    #[inline]
    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn blocks(&mut self, blocks: &[Block<'_>], tight: bool) {
        for b in blocks {
            self.block(b, tight);
        }
    }

    fn block(&mut self, block: &Block<'_>, tight: bool) {
        match block {
            Block::Paragraph(inlines) => {
                if tight {
                    self.inlines(inlines);
                } else {
                    self.cr();
                    self.push("<p>");
                    self.inlines(inlines);
                    self.push("</p>\n");
                }
            }
            Block::Heading(h) => {
                let level = (b'0' + h.level) as char;
                self.cr();
                self.push("<h");
                self.out.push(level);
                if let Some(id) = &h.id {
                    self.push(" id=\"");
                    escape_html(self.out, id);
                    self.push("\"");
                }
                self.push(">");
                self.inlines(&h.children);
                self.push("</h");
                self.out.push(level);
                self.push(">\n");
            }
            Block::ThematicBreak => {
                self.cr();
                self.push("<hr />\n");
            }
            Block::BlockQuote(children) => {
                self.cr();
                self.push("<blockquote>\n");
                self.blocks(children, false);
                self.cr();
                self.push("</blockquote>\n");
            }
            Block::List(list) => self.list(list),
            Block::CodeBlock(code) => self.code_block(code),
            Block::HtmlBlock(html) => {
                self.cr();
                self.raw_html(html);
                self.cr();
            }
            Block::Table(table) => {
                self.cr();
                self.push("<table>\n<thead>\n<tr>\n");
                for (cell, &align) in table.header.iter().zip(&table.alignments) {
                    self.cell("th", align, cell);
                }
                self.push("</tr>\n</thead>\n");
                if !table.rows.is_empty() {
                    self.push("<tbody>\n");
                    for row in &table.rows {
                        self.push("<tr>\n");
                        for (cell, &align) in row.iter().zip(&table.alignments) {
                            self.cell("td", align, cell);
                        }
                        self.push("</tr>\n");
                    }
                    self.push("</tbody>\n");
                }
                self.push("</table>\n");
            }
        }
    }

    /// The footnote section, with the markup of Comark's footnotes plugin.
    fn footnotes(&mut self, footnotes: &[Footnote<'_>]) {
        self.cr();
        self.push("<section class=\"footnotes\">\n<hr />\n<h2 id=\"footnotes\">Footnotes</h2>\n<ol class=\"footnotes-list\">\n");
        for f in footnotes {
            self.push("<li id=\"fn-");
            footnote_id(self.out, &f.label);
            self.push("\">");
            self.inlines(&f.children);
            self.push(" <a href=\"#fnref-");
            footnote_id(self.out, &f.label);
            self.push("\" class=\"footnote-backref\">\u{21A9}</a></li>\n");
        }
        self.push("</ol>\n</section>\n");
    }

    fn cell(&mut self, tag: &str, align: Alignment, content: &[Inline<'_>]) {
        self.push("<");
        self.push(tag);
        match align {
            Alignment::None => {}
            Alignment::Left => self.push(" align=\"left\""),
            Alignment::Center => self.push(" align=\"center\""),
            Alignment::Right => self.push(" align=\"right\""),
        }
        self.push(">");
        self.inlines(content);
        self.push("</");
        self.push(tag);
        self.push(">\n");
    }

    fn list(&mut self, list: &List<'_>) {
        self.cr();
        let ordered = match list.kind {
            ListKind::Bullet(_) => {
                self.push("<ul>\n");
                false
            }
            ListKind::Ordered { start, .. } => {
                if start == 1 {
                    self.push("<ol>\n");
                } else {
                    self.push("<ol start=\"");
                    self.push(itoa(start, &mut [0; 10]));
                    self.push("\">\n");
                }
                true
            }
        };
        for item in &list.items {
            self.cr();
            self.push("<li>");
            if let Some(checked) = item.task {
                self.push(if checked {
                    "<input type=\"checkbox\" checked=\"\" disabled=\"\" /> "
                } else {
                    "<input type=\"checkbox\" disabled=\"\" /> "
                });
            }
            self.blocks(&item.children, list.tight);
            self.push("</li>\n");
        }
        self.cr();
        self.push(if ordered { "</ol>\n" } else { "</ul>\n" });
    }

    fn code_block(&mut self, code: &CodeBlock<'_>) {
        self.cr();
        self.push("<pre><code");
        if let Some(lang) = code.language() {
            self.push(" class=\"language-");
            escape_html(self.out, lang);
            self.push("\"");
        }
        self.push(">");
        escape_html(self.out, &code.literal);
        self.push("</code></pre>\n");
    }

    fn raw_html(&mut self, html: &str) {
        match self.opts.raw_html {
            RawHtml::Escape => escape_html(self.out, html),
            RawHtml::Omit => self.push("<!-- raw HTML omitted -->"),
            RawHtml::Allow if self.opts.tagfilter => tagfilter(self.out, html),
            RawHtml::Allow => self.push(html),
        }
    }

    fn inlines(&mut self, inlines: &[Inline<'_>]) {
        for inline in inlines {
            self.inline(inline);
        }
    }

    fn inline(&mut self, inline: &Inline<'_>) {
        match inline {
            Inline::Text(t) => escape_html(self.out, t),
            Inline::SoftBreak => self.push("\n"),
            Inline::HardBreak => self.push("<br />\n"),
            Inline::Code(t) => {
                self.push("<code>");
                escape_html(self.out, t);
                self.push("</code>");
            }
            Inline::Html(h) => self.raw_html(h),
            Inline::Emphasis(c) => {
                self.push("<em>");
                self.inlines(c);
                self.push("</em>");
            }
            Inline::Strong(c) => {
                self.push("<strong>");
                self.inlines(c);
                self.push("</strong>");
            }
            Inline::Strikethrough(c) => {
                self.push("<del>");
                self.inlines(c);
                self.push("</del>");
            }
            Inline::Link(link) => {
                self.push("<a href=\"");
                self.url(&link.url, false);
                self.push("\"");
                self.title(link);
                self.push(">");
                self.inlines(&link.children);
                self.push("</a>");
            }
            Inline::FootnoteReference(r) => {
                self.push("<sup class=\"footnote-ref\"><a href=\"#fn-");
                footnote_id(self.out, &r.label);
                self.push("\" id=\"fnref-");
                footnote_id(self.out, &r.label);
                if r.occurrence > 1 {
                    self.push("-");
                    self.push(itoa(r.occurrence, &mut [0; 10]));
                }
                self.push("\">[");
                self.push(itoa(r.index, &mut [0; 10]));
                self.push("]</a></sup>");
            }
            Inline::Image(link) => {
                self.push("<img src=\"");
                self.url(&link.url, true);
                self.push("\" alt=\"");
                alt_text(self.out, &link.children);
                self.push("\"");
                self.title(link);
                self.push(" />");
            }
        }
    }

    fn url(&mut self, url: &str, image: bool) {
        if self.opts.dangerous_urls || is_safe_url(url, image) {
            escape_href(self.out, url);
        }
    }

    fn title(&mut self, link: &Link<'_>) {
        if !link.title.is_empty() {
            self.push(" title=\"");
            escape_html(self.out, &link.title);
            self.push("\"");
        }
    }
}

fn alt_text(out: &mut String, inlines: &[Inline<'_>]) {
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) | Inline::Html(t) => escape_html(out, t),
            Inline::SoftBreak | Inline::HardBreak => out.push('\n'),
            Inline::Emphasis(c) | Inline::Strong(c) | Inline::Strikethrough(c) => alt_text(out, c),
            Inline::Link(l) | Inline::Image(l) => alt_text(out, &l.children),
            Inline::FootnoteReference(r) => {
                out.push('[');
                out.push_str(itoa(r.index, &mut [0; 10]));
                out.push(']');
            }
        }
    }
}

/// Writes a footnote label as an id, like Comark: characters other than
/// ASCII alphanumerics, `_` and `-` become `-<hex code>-`.
fn footnote_id(out: &mut String, label: &str) {
    for c in label.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            out.push('-');
            out.push_str(&format!("{:x}", c as u32));
            out.push('-');
        }
    }
}

fn itoa(mut n: u32, buf: &mut [u8; 10]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    std::str::from_utf8(&buf[i..]).unwrap()
}

/// Escapes `&`, `<`, `>` and `"`.
pub fn escape_html(out: &mut String, s: &str) {
    let b = s.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        // Skip eight bytes at a time when none of them needs escaping.
        if i + 8 <= b.len() && !needs_escape(u64::from_le_bytes(b[i..i + 8].try_into().unwrap())) {
            i += 8;
            continue;
        }
        let rep = match b[i] {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            _ => {
                i += 1;
                continue;
            }
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        i += 1;
        last = i;
    }
    out.push_str(&s[last..]);
}

/// Whether a word contains one of `&`, `<`, `>` or `"`.
#[inline]
fn needs_escape(word: u64) -> bool {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let has = |c: u8| {
        let x = word ^ (LO * c as u64);
        x.wrapping_sub(LO) & !x & HI
    };
    (has(b'&') | has(b'<') | has(b'>') | has(b'"')) != 0
}

/// Bytes kept as-is in URLs; the others are percent-encoded.
static HREF_SAFE: [bool; 256] = {
    let mut t = [false; 256];
    let mut c = 0;
    while c < 256 {
        let b = c as u8;
        t[c] = b.is_ascii_alphanumeric();
        c += 1;
    }
    let extra = b";/?:@&=+$,-_.!~*'()#";
    let mut i = 0;
    while i < extra.len() {
        t[extra[i] as usize] = true;
        i += 1;
    }
    t
};

/// Percent-encodes a URL (keeping existing `%XX` escapes) and escapes it for
/// an HTML attribute.
pub fn escape_href(out: &mut String, url: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let b = url.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if HREF_SAFE[c as usize] && c != b'&' {
            i += 1;
            continue;
        }
        if c == b'%'
            && b.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
            && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
        {
            i += 3;
            continue;
        }
        out.push_str(&url[last..i]);
        if c == b'&' {
            out.push_str("&amp;");
            i += 1;
        } else {
            // Encode every byte of the character.
            let len = match c {
                0xF0.. => 4,
                0xE0.. => 3,
                0xC0.. => 2,
                _ => 1,
            };
            for &byte in &b[i..i + len] {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 15) as usize] as char);
            }
            i += len;
        }
        last = i;
    }
    out.push_str(&url[last..]);
}

/// Whether a URL is safe to render in `href` or `src`.
///
/// The check mirrors how browsers read a scheme: leading spaces and control
/// characters are skipped, tabs and line endings inside it are ignored, and
/// the comparison is case-insensitive.
pub fn is_safe_url(url: &str, image: bool) -> bool {
    let b = url.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] <= b' ' {
        i += 1;
    }
    let mut scheme = [0u8; 16];
    let mut len = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        match c {
            b'\t' | b'\n' | b'\r' => continue,
            b':' => break,
            c if c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.') => {
                if len == scheme.len() {
                    return true;
                }
                scheme[len] = c.to_ascii_lowercase();
                len += 1;
            }
            // Not a scheme: a relative URL.
            _ => return true,
        }
        if i == b.len() {
            return true;
        }
    }
    match &scheme[..len] {
        b"javascript" | b"vbscript" | b"file" => false,
        b"data" => {
            image && {
                let rest = url[i..].trim_start().to_ascii_lowercase();
                ["image/png", "image/gif", "image/jpeg", "image/webp"]
                    .iter()
                    .any(|t| {
                        rest.starts_with(t)
                            && matches!(rest.as_bytes().get(t.len()), Some(b';' | b','))
                    })
            }
        }
        _ => true,
    }
}

const FILTERED_TAGS: &[&str] = &[
    "title",
    "textarea",
    "style",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "script",
    "plaintext",
];

/// Writes raw HTML, escaping the `<` of the tags filtered by GFM.
fn tagfilter(out: &mut String, html: &str) {
    let b = html.as_bytes();
    let mut last = 0;
    for (i, _) in html.match_indices('<') {
        let mut j = i + 1;
        if b.get(j) == Some(&b'/') {
            j += 1;
        }
        let filtered = FILTERED_TAGS.iter().any(|tag| {
            let end = j + tag.len();
            b.len() >= end
                && b[j..end].eq_ignore_ascii_case(tag.as_bytes())
                && matches!(
                    b.get(end),
                    None | Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
                )
        });
        if filtered {
            out.push_str(&html[last..i]);
            out.push_str("&lt;");
            last = i + 1;
        }
    }
    out.push_str(&html[last..]);
}
