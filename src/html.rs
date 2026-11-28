//! HTML rendering, with sanitizing of raw HTML, URLs and attributes.

use crate::ast::{
    Alignment, Attribute, Attributes, Block, CodeBlock, Component, Document, Footnote, HtmlElement,
    Inline, Link, List, ListItem, ListKind, Node,
};
#[cfg(feature = "highlight")]
use crate::highlight::Highlight;
use crate::mermaid::Mermaid;
use crate::text::{decode_entities, parse_entity};
use crate::yaml::Value;

/// What to do with raw HTML (HTML blocks and inline tags) kept verbatim,
/// when [`Options::html`](crate::Options::html) is off.
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

/// The shape of the HTML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HtmlStyle {
    /// The HTML of Comark's renderer: a container holding a single paragraph
    /// renders its content without `<p>`, code blocks carry `data-language`
    /// and no final line ending, tables align with `style`, void tags have no
    /// slash...
    #[default]
    Comark,
    /// The HTML of the CommonMark reference implementation (`cmark-gfm`).
    CommonMark,
}

/// HTML rendering options. The defaults are safe for untrusted input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HtmlOptions<'a> {
    /// The shape of the HTML. Defaults to [`HtmlStyle::Comark`].
    pub style: HtmlStyle,
    /// What to do with raw HTML kept verbatim, when
    /// [`Options::html`](crate::Options::html) is off. Defaults to
    /// [`RawHtml::Escape`]. With `Options::html`, raw HTML is parsed into
    /// elements, which `sanitize` covers.
    pub raw_html: RawHtml,
    /// When `false` (the default), link and image URLs with a dangerous
    /// scheme (`javascript:`, `vbscript:`, `file:`, and `data:` except for
    /// PNG, GIF, JPEG and WebP images) are replaced by an empty URL.
    pub dangerous_urls: bool,
    /// When raw HTML is allowed, escape the tags that GFM filters
    /// (`<script>`, `<style>`, `<iframe>`, `<textarea>`, `<title>`, `<xmp>`,
    /// `<noembed>`, `<noframes>` and `<plaintext>`). Defaults to `true`.
    pub tagfilter: bool,
    /// Apply the rules of Comark's security plugin to the attributes of
    /// components, spans, HTML elements and other elements: event handlers
    /// (`on*`), markup sinks (`*html`, `srcdoc`...) and URL attributes with a
    /// dangerous scheme are dropped. Components and HTML elements that would
    /// run code (`script`, `style`, `iframe`, `object`...) are dropped with
    /// their content, and a `>` in an HTML comment is escaped. Defaults to
    /// `true`.
    pub sanitize: bool,
    /// In Comark style, render the single paragraph of a block quote, list
    /// item, component or slot without `<p>`, like Comark's `autoUnwrap`.
    /// Defaults to `true`.
    pub unwrap: bool,
    /// How ```` ```mermaid ```` fences are rendered. Defaults to
    /// [`Mermaid::Off`]: they are ordinary code blocks.
    pub mermaid: Mermaid<'a>,
    /// Syntax highlighting of code blocks and inline code with
    /// [oxlight](https://github.com/oxmark/oxlight), like Comark's highlight
    /// plugin. Defaults to `None`.
    #[cfg(feature = "highlight")]
    pub highlight: Option<Highlight<'a>>,
}

impl Default for HtmlOptions<'_> {
    fn default() -> Self {
        HtmlOptions {
            style: HtmlStyle::Comark,
            raw_html: RawHtml::Escape,
            dangerous_urls: false,
            tagfilter: true,
            sanitize: true,
            unwrap: true,
            mermaid: Mermaid::Off,
            #[cfg(feature = "highlight")]
            highlight: None,
        }
    }
}

impl HtmlOptions<'_> {
    /// Options that trust the input completely: raw HTML, every URL and every
    /// attribute are rendered as written, without the GFM tag filter.
    pub fn trusted() -> Self {
        HtmlOptions {
            raw_html: RawHtml::Allow,
            dangerous_urls: true,
            tagfilter: false,
            sanitize: false,
            ..HtmlOptions::default()
        }
    }

    /// [`trusted`](Self::trusted) options with the CommonMark style: the
    /// output of the CommonMark reference implementation in its unsafe mode.
    pub fn commonmark_trusted() -> Self {
        HtmlOptions {
            style: HtmlStyle::CommonMark,
            ..HtmlOptions::trusted()
        }
    }
}

/// Renders a document to HTML.
pub fn render_html(doc: &Document<'_>, options: &HtmlOptions<'_>) -> String {
    let mut out = String::with_capacity(256);
    render_html_to(doc, options, &mut out);
    out
}

/// Renders a document to HTML, appending to `out`.
pub fn render_html_to(doc: &Document<'_>, options: &HtmlOptions<'_>, out: &mut String) {
    let mut r = Renderer {
        out,
        opts: options,
        comark: options.style == HtmlStyle::Comark,
        frontmatter_src: doc.frontmatter.as_deref(),
        frontmatter: None,
        scopes: Vec::new(),
        diagrams: 0,
        #[cfg(feature = "highlight")]
        highlight: options.highlight.as_ref().map(crate::highlight::State::new),
    };
    r.blocks(&doc.children, false);
    if !doc.footnotes.is_empty() {
        r.footnotes(&doc.footnotes);
    }
}

/// Tags that render without content or closing tag.
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Component names that are not rendered when sanitizing: they run code or
/// change how the rest of the page is read.
const BLOCKED_TAGS: &[&str] = &[
    "applet",
    "base",
    "embed",
    "frame",
    "frameset",
    "iframe",
    "link",
    "meta",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "plaintext",
    "script",
    "style",
    "textarea",
    "title",
    "xmp",
];

fn is_void(tag: &str) -> bool {
    VOID_TAGS.iter().any(|t| t.eq_ignore_ascii_case(tag))
}

/// An attribute to write: a name and a value.
pub(crate) type Attr<'s> = (&'s str, &'s str);

pub(crate) struct Renderer<'o, 'w> {
    pub(crate) out: &'w mut String,
    pub(crate) opts: &'o HtmlOptions<'o>,
    comark: bool,
    /// The frontmatter, parsed when a binding first needs it.
    frontmatter_src: Option<&'w str>,
    frontmatter: Option<Option<Value>>,
    /// The props of the enclosing components, for `props.x` bindings.
    scopes: Vec<Vec<(String, Value)>>,
    /// Mermaid diagrams rendered so far, for unique SVG ids.
    #[cfg_attr(not(feature = "mermaid"), allow(dead_code))]
    pub(crate) diagrams: usize,
    /// Highlighting options resolved once per document.
    #[cfg(feature = "highlight")]
    pub(crate) highlight: Option<crate::highlight::State<'o>>,
}

impl Renderer<'_, '_> {
    #[inline]
    pub(crate) fn cr(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    #[inline]
    pub(crate) fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    #[inline]
    fn text(&mut self, s: &str) {
        if self.comark {
            escape_text(self.out, s);
        } else {
            escape_html(self.out, s);
        }
    }

    /// A void tag ending: ` />` in CommonMark style, `>` in Comark style.
    #[inline]
    fn void_end(&mut self) {
        self.push(if self.comark { ">" } else { " />" });
    }

    fn blocks(&mut self, blocks: &[Block<'_>], tight: bool) {
        for b in blocks {
            self.block(b, tight);
        }
    }

    /// The content of a container. In Comark style, a single paragraph is
    /// rendered without `<p>` (Comark's `autoUnwrap`).
    fn container(&mut self, blocks: &[Block<'_>]) {
        match blocks {
            [Block::Paragraph(p)] if self.comark && self.opts.unwrap => self.inlines(&p.children),
            _ => self.blocks(blocks, false),
        }
    }

    /// The attributes a single paragraph lends to its container when it is
    /// unwrapped.
    fn unwrapped_attrs<'b>(&self, blocks: &'b [Block<'_>]) -> Option<&'b Attributes<'b>> {
        match blocks {
            [Block::Paragraph(p)] if self.comark && self.opts.unwrap && !p.attrs.is_empty() => {
                Some(&p.attrs)
            }
            _ => None,
        }
    }

    pub(crate) fn open_tag(&mut self, tag: &str, extra: &[Attr<'_>], attrs: &[&Attributes<'_>]) {
        self.push("<");
        self.push(tag);
        self.attrs(tag, extra, attrs);
        self.push(">");
    }

    /// Writes attributes: `extra` first, then each list in order. A later
    /// value replaces an earlier one at its position, and classes are joined.
    fn attrs(&mut self, tag: &str, extra: &[Attr<'_>], lists: &[&Attributes<'_>]) {
        if self.opts.sanitize && tag.eq_ignore_ascii_case("object") {
            return;
        }
        let count = lists.iter().map(|l| l.0.len()).sum::<usize>() + extra.len();
        if count == 0 {
            return;
        }
        let single = match lists {
            [] => extra.len() <= 1 || no_dupes(extra.iter().map(|a| a.0)),
            [l] if extra.is_empty() => true,
            _ => false,
        };
        if single {
            for &(name, value) in extra {
                self.attr(name, value);
            }
            for l in lists {
                for Attribute { name, value } in l.iter() {
                    self.attr(name, value);
                }
            }
            return;
        }
        let mut merged: Vec<(&str, std::borrow::Cow<'_, str>)> = Vec::with_capacity(count);
        let all = extra.iter().copied().chain(
            lists
                .iter()
                .flat_map(|l| l.iter().map(|a| (&*a.name, &*a.value))),
        );
        let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (name, value) in all {
            let found = if count <= 16 {
                merged.iter().position(|(n, _)| *n == name)
            } else {
                index.get(name).copied()
            };
            if found.is_none() && count > 16 {
                index.insert(name, merged.len());
            }
            match found.map(|i| &mut merged[i]) {
                Some((_, v)) if name == "class" => {
                    let v = v.to_mut();
                    v.push(' ');
                    v.push_str(value);
                }
                Some((_, v)) => *v = std::borrow::Cow::Borrowed(value),
                None => merged.push((name, std::borrow::Cow::Borrowed(value))),
            }
        }
        for (name, value) in &merged {
            self.attr(name, value);
        }
    }

    /// Opens a tag whose `class` is set by the renderer, like Comark's
    /// plugins do: `own` attributes first, then `lists` (where a user class
    /// gives its position to the class, written `"{class} . {user}"`), then
    /// `after`.
    #[cfg_attr(not(feature = "highlight"), allow(dead_code))]
    pub(crate) fn open_tag_with_class(
        &mut self,
        tag: &str,
        own: &[Attr<'_>],
        lists: &[&Attributes<'_>],
        class: &str,
        after: &[Attr<'_>],
    ) {
        use std::borrow::Cow;
        let count = own.len() + lists.iter().map(|l| l.0.len()).sum::<usize>() + 1;
        let mut merged: Vec<(&str, Cow<'_, str>)> = Vec::with_capacity(count + after.len());
        merged.extend(own.iter().map(|&(n, v)| (n, Cow::Borrowed(v))));
        let mut user: Option<(usize, String)> = None;
        for list in lists {
            for a in list.iter() {
                let (name, value) = (&*a.name, &*a.value);
                if name == "class" {
                    match &mut user {
                        Some((_, c)) => {
                            c.push(' ');
                            c.push_str(value);
                        }
                        None => {
                            user = Some((merged.len(), value.to_owned()));
                            merged.push(("class", Cow::Borrowed("")));
                        }
                    }
                } else if let Some(i) = merged.iter().position(|(n, _)| *n == name) {
                    merged[i].1 = Cow::Borrowed(value);
                } else {
                    merged.push((name, Cow::Borrowed(value)));
                }
            }
        }
        let value = match &user {
            Some((_, u)) if !u.trim().is_empty() => format!("{class} . {}", u.trim()),
            _ => class.to_owned(),
        };
        match user {
            Some((i, _)) => merged[i].1 = Cow::Owned(value),
            None => merged.push(("class", Cow::Owned(value))),
        }
        merged.extend(after.iter().map(|&(n, v)| (n, Cow::Borrowed(v))));
        self.push("<");
        self.push(tag);
        for (name, value) in &merged {
            self.attr(name, value);
        }
        self.push(">");
    }

    pub(crate) fn attr(&mut self, name: &str, value: &str) {
        if let Some(key) = name.strip_prefix(':') {
            if let Some(v) = self.resolve(value) {
                match v {
                    Value::Bool(true) => self.attr(key, "true"),
                    Value::Bool(false) | Value::Null => {}
                    Value::String(s) | Value::Number(s) => self.attr(key, &s),
                    v => self.attr(key, &v.to_json()),
                }
                return;
            }
        }
        let key = name.strip_prefix(':').unwrap_or(name);
        if !is_safe_attr_name(key) || (name == "id" && value.is_empty()) {
            return;
        }
        if self.opts.sanitize && !is_safe_attr(name, value) {
            return;
        }
        self.push(" ");
        self.push(key);
        if value == "true" && self.comark {
            return;
        }
        self.push("=\"");
        escape_attr(self.out, value);
        self.push("\"");
    }

    /// Resolves a Comark binding (`:title="frontmatter.title"`) against the
    /// frontmatter or the props of the enclosing component.
    fn resolve(&mut self, path: &str) -> Option<Value> {
        let mut parts = path.split('.');
        let root = parts.next()?;
        let mut current = match root {
            "frontmatter" => {
                let src = self.frontmatter_src?;
                self.frontmatter
                    .get_or_insert_with(|| crate::yaml::parse(src).ok().flatten())
                    .as_ref()?
            }
            "props" => {
                let key = parts.next()?;
                let scope = self.scopes.last()?;
                &scope.iter().find(|(k, _)| k == key)?.1
            }
            _ => return None,
        };
        for part in parts {
            current = match current {
                Value::Object(entries) => &entries.iter().find(|(k, _)| k == part)?.1,
                Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current.clone())
    }

    /// Makes the props of a component the scope of the bindings inside it.
    fn push_scope(&mut self, attrs: &Attributes<'_>) {
        let mut scope = Vec::with_capacity(attrs.0.len());
        for a in attrs.iter() {
            match a.name.strip_prefix(':') {
                Some(key) => {
                    if let Some(v) = self.resolve(&a.value) {
                        scope.push((key.to_owned(), v));
                    }
                }
                None => scope.push((a.name.to_string(), Value::String(a.value.to_string()))),
            }
        }
        self.scopes.push(scope);
    }

    fn block(&mut self, block: &Block<'_>, tight: bool) {
        match block {
            Block::Paragraph(p) => {
                if tight {
                    self.inlines(&p.children);
                } else {
                    self.cr();
                    self.open_tag("p", &[], &[&p.attrs]);
                    self.inlines(&p.children);
                    self.push("</p>\n");
                }
            }
            Block::Heading(h) => {
                let tag =
                    ["h1", "h2", "h3", "h4", "h5", "h6"][usize::from(h.level.clamp(1, 6)) - 1];
                self.cr();
                match &h.id {
                    Some(id) => self.open_tag(tag, &[("id", id)], &[&h.attrs]),
                    None => self.open_tag(tag, &[], &[&h.attrs]),
                }
                self.inlines(&h.children);
                self.push("</");
                self.push(tag);
                self.push(">\n");
            }
            Block::ThematicBreak => {
                self.cr();
                self.push("<hr");
                self.void_end();
                self.push("\n");
            }
            Block::BlockQuote(q) => match q.alert {
                Some(kind) => {
                    let attrs = Attributes(vec![Attribute {
                        name: "as".into(),
                        value: kind.as_str().into(),
                    }]);
                    self.blockquote(&q.children, &[&attrs])
                }
                None => self.blockquote(&q.children, &[]),
            },
            Block::List(list) => self.list(list, &[]),
            Block::CodeBlock(code) => self.code_block(code, &[]),
            Block::HtmlBlock(html) => {
                self.cr();
                self.raw_html(html);
                self.cr();
            }
            Block::Table(table) => self.table(table, &[]),
            Block::Component(c) => self.component(c),
            Block::HtmlElement(e) => {
                self.cr();
                self.html_element(e);
                self.push("\n");
            }
            Block::HtmlComment(c) => {
                self.cr();
                self.comment(c);
                self.push("\n");
            }
        }
    }

    fn blockquote(&mut self, children: &[Block<'_>], extra: &[&Attributes<'_>]) {
        self.cr();
        let mut lists: Vec<&Attributes<'_>> = Vec::new();
        if let Some(a) = self.unwrapped_attrs(children) {
            lists.push(a);
        }
        lists.extend_from_slice(extra);
        self.open_tag("blockquote", &[], &lists);
        if !self.comark {
            self.push("\n");
        }
        self.container(children);
        self.cr();
        self.push("</blockquote>\n");
    }

    fn table(&mut self, table: &crate::ast::Table<'_>, extra: &[&Attributes<'_>]) {
        self.cr();
        self.open_tag("table", &[], extra);
        self.push("\n<thead>\n<tr>\n");
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

    /// The footnote section, with the markup of Comark's footnotes plugin.
    fn footnotes(&mut self, footnotes: &[Footnote<'_>]) {
        self.cr();
        self.push("<section class=\"footnotes\">\n<hr");
        self.void_end();
        self.push("\n<h2 id=\"footnotes\">Footnotes</h2>\n<ol class=\"footnotes-list\">\n");
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
        let (css, html) = match align {
            Alignment::None => ("", ""),
            Alignment::Left => (" style=\"text-align:left\"", " align=\"left\""),
            Alignment::Center => (" style=\"text-align:center\"", " align=\"center\""),
            Alignment::Right => (" style=\"text-align:right\"", " align=\"right\""),
        };
        self.push(if self.comark { css } else { html });
        self.push(">");
        self.inlines(content);
        self.push("</");
        self.push(tag);
        self.push(">\n");
    }

    fn list(&mut self, list: &List<'_>, extra: &[&Attributes<'_>]) {
        self.cr();
        let mut buf = [0; 10];
        let tag = match list.kind {
            ListKind::Bullet(_) => "ul",
            ListKind::Ordered { .. } => "ol",
        };
        let mut own: Vec<Attr<'_>> = Vec::new();
        if let ListKind::Ordered { start, .. } = list.kind {
            if start != 1 {
                own.push(("start", itoa(start, &mut buf)));
            }
        }
        if self.comark && list.items.iter().any(|i| i.task.is_some()) {
            own.push(("class", "contains-task-list"));
        }
        self.open_tag(tag, &own, extra);
        self.push("\n");
        for item in &list.items {
            self.item(item, list.tight);
        }
        self.cr();
        self.push("</");
        self.push(tag);
        self.push(">\n");
    }

    fn item(&mut self, item: &ListItem<'_>, tight: bool) {
        self.cr();
        let comark = self.comark;
        let task_class: &[Attr<'_>] = if comark && item.task.is_some() {
            &[("class", "task-list-item")]
        } else {
            &[]
        };
        // Comark keeps every paragraph, tight or not, and unwraps a single one.
        let tight = tight && !comark;
        let unwrap =
            comark && self.opts.unwrap && matches!(item.children.as_slice(), [Block::Paragraph(_)]);
        match (unwrap, item.children.first()) {
            (true, Some(Block::Paragraph(p))) => {
                self.open_tag("li", task_class, &[&p.attrs, &item.attrs])
            }
            _ => self.open_tag("li", task_class, &[&item.attrs]),
        }
        let checkbox = |r: &mut Self, checked: bool| {
            r.push(match (comark, checked) {
                (true, true) => {
                    "<input class=\"task-list-item-checkbox\" type=\"checkbox\" disabled checked> "
                }
                (true, false) => {
                    "<input class=\"task-list-item-checkbox\" type=\"checkbox\" disabled> "
                }
                (false, true) => "<input type=\"checkbox\" checked=\"\" disabled=\"\" /> ",
                (false, false) => "<input type=\"checkbox\" disabled=\"\" /> ",
            });
        };
        match (item.task, item.children.split_first()) {
            // Comark puts the checkbox inside the first paragraph.
            (Some(checked), Some((Block::Paragraph(p), rest))) if comark && !tight && !unwrap => {
                self.open_tag("p", &[], &[&p.attrs]);
                checkbox(self, checked);
                self.inlines(&p.children);
                self.push("</p>\n");
                self.blocks(rest, false);
            }
            (task, _) => {
                if let Some(checked) = task {
                    checkbox(self, checked);
                }
                if unwrap {
                    self.container(&item.children);
                } else {
                    self.blocks(&item.children, tight);
                }
            }
        }
        self.push("</li>\n");
    }

    pub(crate) fn code_block(&mut self, code: &CodeBlock<'_>, extra: &[&Attributes<'_>]) {
        if !matches!(self.opts.mermaid, Mermaid::Off) && crate::mermaid::is_mermaid(code) {
            crate::mermaid::render(self, code, extra);
            return;
        }
        self.plain_code_block(code, extra);
    }

    /// A code block that is not a Mermaid diagram.
    pub(crate) fn plain_code_block(&mut self, code: &CodeBlock<'_>, extra: &[&Attributes<'_>]) {
        self.cr();
        #[cfg(feature = "highlight")]
        let highlighting = self.highlight.is_some();
        #[cfg(not(feature = "highlight"))]
        let highlighting = false;
        if !self.comark && !highlighting {
            self.push("<pre><code");
            if let Some(lang) = code.language() {
                self.push(" class=\"language-");
                escape_html(self.out, lang);
                self.push("\"");
            }
            self.push(">");
            escape_html(self.out, &code.literal);
            self.push("</code></pre>\n");
            return;
        }
        let info = CodeInfo::parse(&code.info);
        let mut own: Vec<Attr<'_>> = Vec::with_capacity(4);
        let highlights = info.highlights_json();
        for part in &info.order {
            match part {
                InfoPart::Language => own.push(("data-language", info.language)),
                InfoPart::Highlights => own.push(("data-highlights", &highlights)),
                InfoPart::Filename => own.push(("data-filename", &info.filename)),
                InfoPart::Meta => own.push(("data-meta", info.meta)),
            }
        }
        let literal = code.literal.strip_suffix('\n').unwrap_or(&code.literal);
        #[cfg(feature = "highlight")]
        if highlighting {
            crate::highlight::code_block(self, &info, literal, &own, extra);
            return;
        }
        self.open_tag("pre", &own, extra);
        self.push("<code");
        if !info.language.is_empty() {
            self.push(" class=\"language-");
            escape_attr(self.out, info.language);
            self.push("\"");
        }
        self.push(">");
        escape_text(self.out, literal);
        self.push("</code></pre>\n");
    }

    fn component(&mut self, c: &Component<'_>) {
        let name: &str = &c.name;
        if self.blocked(name, &c.attrs) {
            return;
        }
        // `::ul`, `::table`... around a single element of that kind give
        // their attributes to it.
        if c.slots.is_empty() {
            if let [child] = c.children.as_slice() {
                let wrapped = match (name, child) {
                    ("ul", Block::List(l)) if matches!(l.kind, ListKind::Bullet(_)) => true,
                    ("ol", Block::List(l)) => matches!(l.kind, ListKind::Ordered { .. }),
                    ("table", Block::Table(_))
                    | ("blockquote", Block::BlockQuote(_))
                    | ("pre", Block::CodeBlock(_)) => true,
                    _ => false,
                };
                if wrapped && self.comark {
                    match child {
                        Block::List(l) => self.list(l, &[&c.attrs]),
                        Block::Table(t) => self.table(t, &[&c.attrs]),
                        Block::BlockQuote(q) => {
                            let alert;
                            let mut lists: Vec<&Attributes<'_>> = Vec::new();
                            if let Some(kind) = q.alert {
                                alert = Attributes(vec![Attribute {
                                    name: "as".into(),
                                    value: kind.as_str().into(),
                                }]);
                                lists.push(&alert);
                            }
                            lists.push(&c.attrs);
                            self.blockquote(&q.children, &lists)
                        }
                        Block::CodeBlock(code) => self.code_block(code, &[&c.attrs]),
                        _ => unreachable!(),
                    }
                    return;
                }
            }
        }
        self.cr();
        let mut lists: Vec<&Attributes<'_>> = Vec::with_capacity(2);
        if let Some(a) = self.unwrapped_attrs(&c.children) {
            lists.push(a);
        }
        lists.push(&c.attrs);
        self.open_tag(name, &[], &lists);
        if is_void(name) {
            self.push("\n");
            return;
        }
        self.push_scope(&c.attrs);
        self.container(&c.children);
        for slot in &c.slots {
            self.cr();
            let mut lists: Vec<&Attributes<'_>> = Vec::with_capacity(2);
            if let Some(a) = self.unwrapped_attrs(&slot.children) {
                lists.push(a);
            }
            lists.push(&slot.attrs);
            self.open_tag("template", &[("name", &slot.name)], &lists);
            self.container(&slot.children);
            self.push("</template>\n");
        }
        self.scopes.pop();
        self.push("</");
        self.push(name);
        self.push(">\n");
    }

    /// Whether a component must not be rendered at all.
    fn blocked(&self, name: &str, attrs: &Attributes<'_>) -> bool {
        self.opts.sanitize
            && (is_blocked_tag(name)
                || ["as", "is", ":as", ":is"]
                    .iter()
                    .filter_map(|k| attrs.get(k))
                    .any(|v| is_blocked_tag(v.trim_matches('"'))))
    }

    /// An element parsed from raw HTML, with the security rules of
    /// components. In Comark style, a single paragraph inside it is unwrapped.
    fn html_element(&mut self, e: &HtmlElement<'_>) {
        let tag: &str = &e.tag;
        if self.blocked(tag, &e.attrs) {
            return;
        }
        let unwrapped = match significant(&e.children) {
            Some(Node::Block(Block::Paragraph(p)))
                if self.comark && self.opts.unwrap && tag != "p" =>
            {
                Some(p)
            }
            _ => None,
        };
        match unwrapped {
            Some(p) => self.open_tag(tag, &[], &[&p.attrs, &e.attrs]),
            None => self.open_tag(tag, &[], &[&e.attrs]),
        }
        if is_void(tag) {
            return;
        }
        match unwrapped {
            Some(p) => self.inlines(&p.children),
            None => {
                for node in &e.children {
                    match node {
                        Node::Block(b) => self.block(b, false),
                        Node::Inline(i) => self.inline(i),
                    }
                }
            }
        }
        self.push("</");
        self.push(tag);
        self.push(">");
    }

    /// An HTML comment. When sanitizing, a `>` in it is escaped so that the
    /// comment cannot end early (`--!>`) and let markup out.
    fn comment(&mut self, body: &str) {
        self.push("<!--");
        if self.opts.sanitize {
            for (i, part) in body.split('>').enumerate() {
                if i > 0 {
                    self.push("&gt;");
                }
                self.push(part);
            }
        } else {
            self.push(body);
        }
        self.push("-->");
    }

    fn raw_html(&mut self, html: &str) {
        match self.opts.raw_html {
            RawHtml::Escape => {
                if self.comark {
                    escape_text(self.out, html)
                } else {
                    escape_html(self.out, html)
                }
            }
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

    fn element(&mut self, tag: &str, attrs: &Attributes<'_>, children: &[Inline<'_>]) {
        self.open_tag(tag, &[], &[attrs]);
        self.inlines(children);
        self.push("</");
        self.push(tag);
        self.push(">");
    }

    fn inline(&mut self, inline: &Inline<'_>) {
        match inline {
            Inline::Text(t) => self.text(t),
            Inline::SoftBreak => self.push("\n"),
            Inline::HardBreak => {
                self.push("<br");
                self.void_end();
                self.push("\n");
            }
            Inline::Code(c) => {
                #[cfg(feature = "highlight")]
                if crate::highlight::inline_code(self, c) {
                    return;
                }
                self.open_tag("code", &[], &[&c.attrs]);
                self.text(&c.literal);
                self.push("</code>");
            }
            Inline::Html(h) => self.raw_html(h),
            Inline::HtmlElement(e) => self.html_element(e),
            Inline::HtmlComment(c) => self.comment(c),
            Inline::Emphasis(e) => self.element("em", &e.attrs, &e.children),
            Inline::Strong(e) => self.element("strong", &e.attrs, &e.children),
            Inline::Strikethrough(e) => self.element("del", &e.attrs, &e.children),
            Inline::Span(e) => self.element("span", &e.attrs, &e.children),
            Inline::Component(c) => {
                if self.blocked(&c.name, &c.attrs) {
                    return;
                }
                self.open_tag(&c.name, &[], &[&c.attrs]);
                if !is_void(&c.name) {
                    self.push_scope(&c.attrs);
                    self.inlines(&c.children);
                    self.scopes.pop();
                    self.push("</");
                    self.push(&c.name);
                    self.push(">");
                }
            }
            Inline::Link(link) => {
                self.push("<a href=\"");
                self.url(&link.url, false);
                self.push("\"");
                self.title(link);
                self.attrs("a", &[], &[&link.attrs]);
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
                self.push("\"");
                if self.comark {
                    self.title(link);
                    self.push(" alt=\"");
                    alt_text(self.out, &link.children);
                    self.push("\"");
                    self.attrs("img", &[], &[&link.attrs]);
                    self.push(">");
                } else {
                    self.push(" alt=\"");
                    alt_text(self.out, &link.children);
                    self.push("\"");
                    self.title(link);
                    self.push(" />");
                }
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

/// The only child that is not blank text, if there is one.
fn significant<'n, 'a>(nodes: &'n [Node<'a>]) -> Option<&'n Node<'a>> {
    let mut found = None;
    for node in nodes {
        match node {
            Node::Inline(Inline::Text(t)) if t.trim().is_empty() => {}
            Node::Inline(Inline::SoftBreak) => {}
            n => {
                if found.is_some() {
                    return None;
                }
                found = Some(n);
            }
        }
    }
    found
}

fn no_dupes<'s>(mut names: impl Iterator<Item = &'s str>) -> bool {
    let mut seen: [&str; 8] = [""; 8];
    let mut n = 0;
    names.all(|name| {
        if seen[..n].contains(&name) || n == seen.len() {
            return false;
        }
        seen[n] = name;
        n += 1;
        true
    })
}

fn is_blocked_tag(tag: &str) -> bool {
    BLOCKED_TAGS.iter().any(|t| t.eq_ignore_ascii_case(tag))
}

/// `^[a-zA-Z_:][a-zA-Z0-9_:.-]*$`: names that cannot break out of a tag.
fn is_safe_attr_name(name: &str) -> bool {
    let b = name.as_bytes();
    match b.split_first() {
        Some((&c, rest)) => {
            (c.is_ascii_alphabetic() || c == b'_' || c == b':')
                && rest
                    .iter()
                    .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.' | b'-'))
        }
        None => false,
    }
}

/// The name of the DOM property an attribute writes, as Comark's security
/// plugin computes it: lowercase, without binding prefixes and separators.
fn normalize_attr_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let mut s = lower.as_str();
    loop {
        if let Some(rest) = s.strip_prefix("v-bind:") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix([':', '.', '^']) {
            s = rest;
        } else {
            break;
        }
    }
    let on;
    if let Some(rest) = s.strip_prefix('@').or_else(|| s.strip_prefix("v-on:")) {
        on = format!("on{rest}");
        s = &on;
    }
    s.chars()
        .filter(|c| !matches!(c, ':' | '_' | '-'))
        .collect()
}

/// Comark's security rules for one attribute.
fn is_safe_attr(name: &str, value: &str) -> bool {
    let n = normalize_attr_name(name);
    if n.starts_with("on")
        || n.ends_with("html")
        || matches!(
            n.as_str(),
            "srcdoc" | "formaction" | "textcontent" | "visvnode"
        )
    {
        return false;
    }
    // Browsers decode the character references kept by `escape_attr`, and
    // renderers JSON-decode bindings: check what they will see.
    let entities = decode_entities(value);
    let json;
    let value = match name
        .starts_with(':')
        .then(|| json_string(&entities))
        .flatten()
    {
        Some(s) => {
            json = s;
            json.as_str()
        }
        None => &entities,
    };
    if n == "attributename" {
        return !value.to_ascii_lowercase().contains("href");
    }
    if n == "as" || n == "is" {
        let b = value.as_bytes();
        return b.first().is_some_and(u8::is_ascii_alphabetic)
            && b.iter()
                .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            && !is_blocked_tag(value);
    }
    if n.ends_with("srcset") {
        return value
            .split(',')
            .filter_map(|candidate| candidate.split_ascii_whitespace().next())
            .all(|url| is_safe_url(url, true));
    }
    if n == "style" {
        let compact: String = value
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '\\' | '"' | '\''))
            .flat_map(char::to_lowercase)
            .collect();
        return !(compact.contains("javascript:")
            || compact.contains("vbscript:")
            || compact.contains("expression(")
            || compact.contains("url(data:text"));
    }
    let url_prop = matches!(
        n.as_str(),
        "to" | "action" | "poster" | "cite" | "background" | "ping" | "longdesc"
    ) || ["href", "src", "url", "uri", "link"]
        .iter()
        .any(|s| n.ends_with(s) || n.strip_suffix('s').is_some_and(|m| m.ends_with(s)));
    if url_prop {
        return is_safe_url(value, n.ends_with("src") || n == "poster");
    }
    true
}

/// Decodes a JSON string literal (`"..."`), if `value` is one.
fn json_string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            c => out.push(c),
        }
    }
    Some(out)
}

#[derive(Debug, Clone, Copy)]
enum InfoPart {
    Language,
    Highlights,
    Filename,
    Meta,
}

/// A code block info string, read like Comark: a language, `{1-3,5}` line
/// highlights and a `[filename]` in any order, and the rest as metadata.
pub(crate) struct CodeInfo<'s> {
    pub(crate) language: &'s str,
    pub(crate) highlights: Vec<u32>,
    filename: String,
    meta: &'s str,
    order: Vec<InfoPart>,
}

impl<'s> CodeInfo<'s> {
    fn parse(info: &'s str) -> Self {
        let mut ci = CodeInfo {
            language: "",
            highlights: Vec::new(),
            filename: String::new(),
            meta: "",
            order: Vec::new(),
        };
        let mut rest = info.trim();
        let lang_len = rest
            .find(|c: char| c.is_whitespace() || "[{}\"'<>`".contains(c))
            .unwrap_or(rest.len());
        if lang_len > 0 {
            ci.language = &rest[..lang_len];
            ci.order.push(InfoPart::Language);
            rest = rest[lang_len..].trim();
        }
        loop {
            if let Some(body) = rest.strip_prefix('{') {
                let Some(close) = body.find('}').filter(|&c| c > 0) else {
                    break;
                };
                for part in body[..close].split(',') {
                    let part = part.trim();
                    if let Some((a, b)) = part.split_once('-') {
                        if let (Some(a), Some(b)) = (parse_int(a), parse_int(b)) {
                            if b.saturating_sub(a) <= 1000 {
                                let mut i = a;
                                while i <= b && ci.highlights.len() < 1000 {
                                    ci.highlights.push(i);
                                    i += 1;
                                }
                            }
                        }
                    } else if let Some(n) = parse_int(part) {
                        if ci.highlights.len() < 1000 {
                            ci.highlights.push(n);
                        }
                    }
                }
                if !ci.highlights.is_empty()
                    && !ci.order.iter().any(|p| matches!(p, InfoPart::Highlights))
                {
                    ci.order.push(InfoPart::Highlights);
                }
                rest = body[close + 1..].trim();
            } else if rest.starts_with('[') {
                let mut depth = 0i32;
                let mut end = None;
                for (i, c) in rest.char_indices() {
                    if c == '[' {
                        depth += 1;
                    } else if c == ']' {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                }
                let Some(end) = end else { break };
                ci.filename = rest[1..end].replace('\\', "");
                if !ci.order.iter().any(|p| matches!(p, InfoPart::Filename)) {
                    ci.order.push(InfoPart::Filename);
                }
                rest = rest[end + 1..].trim();
            } else {
                break;
            }
        }
        if !rest.is_empty() {
            ci.meta = rest;
            ci.order.push(InfoPart::Meta);
        }
        ci
    }

    fn highlights_json(&self) -> String {
        let mut s = String::from("[");
        for (i, n) in self.highlights.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(itoa(*n, &mut [0; 10]));
        }
        s.push(']');
        s
    }
}

/// `Number.parseInt(s, 10)` for the non-negative numbers of line ranges.
fn parse_int(s: &str) -> Option<u32> {
    let s = s.trim();
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    s[..digits].parse().ok()
}

fn alt_text(out: &mut String, inlines: &[Inline<'_>]) {
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Html(t) => escape_html(out, t),
            Inline::Code(c) => escape_html(out, &c.literal),
            Inline::SoftBreak | Inline::HardBreak => out.push('\n'),
            Inline::Emphasis(e)
            | Inline::Strong(e)
            | Inline::Strikethrough(e)
            | Inline::Span(e) => alt_text(out, &e.children),
            Inline::Component(c) => alt_text(out, &c.children),
            Inline::Link(l) | Inline::Image(l) => alt_text(out, &l.children),
            Inline::FootnoteReference(r) => {
                out.push('[');
                out.push_str(itoa(r.index, &mut [0; 10]));
                out.push(']');
            }
            Inline::HtmlElement(e) => {
                for node in &e.children {
                    if let Node::Inline(i) = node {
                        alt_text(out, std::slice::from_ref(i));
                    }
                }
            }
            Inline::HtmlComment(_) => {}
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

/// Escapes text like Comark: `<` and `>`, and `&` where a browser could read
/// it as the start of a character reference, so the text reads back as written.
pub(crate) fn escape_text(out: &mut String, s: &str) {
    escape_with(out, s, false)
}

/// Escapes an attribute value like Comark: `<`, `>`, `"`, and `&` except
/// where it starts a character reference. The sanitizer checks values with
/// those references decoded, as a browser will read them.
pub(crate) fn escape_attr(out: &mut String, s: &str) {
    escape_with(out, s, true)
}

fn escape_with(out: &mut String, s: &str, attr: bool) {
    let b = s.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if i + 8 <= b.len() && !needs_escape(u64::from_le_bytes(b[i..i + 8].try_into().unwrap())) {
            i += 8;
            continue;
        }
        let rep = match b[i] {
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' if attr => "&quot;",
            // In an attribute, a reference is kept only when the sanitizer
            // decodes it the same way. In text, `&` is escaped wherever a
            // browser could read a reference.
            b'&' if if attr {
                parse_entity(&s[i..]).is_none()
            } else {
                b.get(i + 1)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'#')
            } =>
            {
                "&amp;"
            }
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
