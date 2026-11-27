# oxmark

A fast Rust implementation of Comark (Markdown with components).

> **Status:** early development. The parser passes the whole
> [CommonMark 0.31.2](https://spec.commonmark.org/0.31.2/) test suite
> (652/652 examples), the [GFM](https://github.github.com/gfm/) extension
> examples, and 259 of Comark's 260 runnable SPEC examples (the last one is a
> deliberate difference, see below).

## Features

- CommonMark 0.31.2, fully compliant.
- GFM extensions: tables, strikethrough, task lists, extended autolinks, and the
  raw HTML tag filter. All are enabled by default and can be turned off one by
  one.
- Comark syntax, checked against Comark's own SPEC examples:
  - block components (`::alert{type="info"}` ... `::`), nested with more
    colons, with YAML props (a `---` block or a ```` ```yaml [props] ```` fence)
    and named slots (`#title`);
  - inline components (`:badge[New]{color="green"}`, `:icon{name="x"}`);
  - spans (`[text]{.class}`) and attributes (`{.class #id key="value"}`) on
    inline elements, paragraphs, headings and list items;
  - YAML frontmatter, with `:attr="frontmatter.key"` bindings;
  - GFM alerts (`> [!NOTE]`);
  - raw HTML parsed into elements, with the Markdown between tags parsed too
    (`<div>\n**bold**\n</div>`), and tags left open closed at the end;
  - heading ids (flat or nested) and footnotes (opt-in), with the same slugs
    and markup as Comark.
- A typed syntax tree ([`oxmark::ast`](src/ast.rs)) that borrows text from the
  input whenever possible.
- An HTML renderer that is **safe by default** for untrusted input.
- Linear-time parsing on hostile input, bounded nesting, no `unsafe` code and
  no runtime dependencies.

## Usage

```rust
// Markdown to HTML, with safe defaults.
let html = oxmark::to_html("Hello *world*!");
assert_eq!(html, "<p>Hello <em>world</em>!</p>\n");

// Or work on the tree.
use oxmark::ast::{Block, Inline};

let doc = oxmark::parse("# Title\n\nSome `code`.");
match &doc.children[0] {
    Block::Heading(h) => assert_eq!(h.level, 1),
    _ => unreachable!(),
}
let html = oxmark::render_html(&doc, &oxmark::HtmlOptions::default());
```

Options are split in two: `oxmark::Options` selects the syntax extensions,
and `oxmark::HtmlOptions` controls rendering. `Options::default()` follows
Comark's defaults (GFM, heading ids, components, attributes, frontmatter and
alerts); `Options::gfm()` and `Options::commonmark()` give plain GFM and strict
CommonMark.

The HTML follows Comark by default (`HtmlStyle::Comark`): a component with a
single paragraph loses its `<p>`, list items keep theirs, code blocks carry
`data-language`, `data-filename` and `data-meta`, and void tags have no slash.
`HtmlStyle::CommonMark` gives the reference CommonMark output instead.

```rust
let html = oxmark::to_html("::alert{type=\"info\"}\nBe **careful**.\n::");
assert_eq!(html, "<alert type=\"info\">Be <strong>careful</strong>.</alert>\n");

let html = oxmark::to_html("A :badge[new]{.green} [word]{title=\"Hi\"}.");
assert_eq!(
    html,
    "<p>A <badge class=\"green\">new</badge> <span title=\"Hi\">word</span>.</p>\n"
);
```

```rust
let options = oxmark::Options { footnotes: true, ..Default::default() };
let html = oxmark::to_html_with(
    "Text[^1].\n\n[^1]: The note.",
    &options,
    &oxmark::HtmlOptions::default(),
);
assert!(html.contains(r##"<a href="#fn-1" id="fnref-1">[1]</a>"##));
```

Like in Comark, a `[text]` that is not a link is a span, so shortcut
reference links (`[label]` alone) are only links when components are off.

Differences with Comark, on purpose:

- Components and HTML elements that can run code or load content (`script`,
  `iframe`, `style`, `object`, ...) are dropped with their content, even when
  Comark's security plugin would keep them (see [Security](#security)).
- In text, `&` is escaped wherever a browser could read a character
  reference, so `\&amp;` shows `&amp;` as written.
- Footnote definitions hold inline Markdown (Comark keeps them as plain text),
  a footnote referenced several times gets unique ids (`fnref-a`, `fnref-a-2`),
  and heading text is not NFC-normalized before computing its id.

## Security

The default `HtmlOptions` are meant for untrusted input:

| Threat | Default behavior |
| --- | --- |
| Raw HTML (`<script>`, `<img onerror=...>`, HTML blocks) | Parsed into elements, like in Comark (`Options::html`), then rendered with the rules below: `<img src=x onerror=alert(1)>` gives `<img src="x">`. A `>` inside an HTML comment is escaped, so `--!>` cannot end it early. With `Options::html` off, raw HTML is escaped and shown as text (`RawHtml::Escape`), or dropped (`RawHtml::Omit`). |
| Dangerous URL schemes | `javascript:`, `vbscript:`, `file:` and `data:` URLs become an empty `href`/`src`. `data:image/{png,gif,jpeg,webp}` stays allowed for images. The check decodes entities and ignores case, leading control characters, tabs and line breaks, like browsers do. |
| Attribute injection | URLs are percent-encoded; titles, alt text, info strings and all text are HTML-escaped. |
| Attributes of components, HTML elements and `{...}` lists | Comark's security rules, on by default (`sanitize`): event handlers (`onclick`, `@click`, `:onClick`), `*html` props (`v-html`, `innerHTML`), `srcdoc`, `formaction`, unsafe URLs in `href`, `src`, `srcset`, `action`, `poster`, ... and `style` with `javascript:`, `expression(` or `url(data:text` are dropped. Values are checked as the browser reads them, with character references and JSON bindings decoded. |
| Dangerous components and HTML elements | `script`, `style`, `iframe`, `object`, `embed`, `base`, `meta`, `link`, `textarea`, `title`, `noscript`, `xmp` and other such tags are dropped with their content, also through `as` or `is`; an `object` never gets attributes. |
| `U+0000` | Replaced by `U+FFFD`, as the spec requires. |
| Denial of service | Pathological inputs (see `tests/pathological.rs`), including unclosed components, spans, attribute lists, props and HTML tags, run in linear time; container and inline nesting are capped at 100 levels, open HTML tags at 64; YAML nesting is capped; table auto-completion is capped. |

With `Options::html` off, `RawHtml::Allow` passes raw HTML through. It keeps the GFM tag filter
(`<script>`, `<style>`, `<iframe>`, ... are neutralized) unless `tagfilter` is
turned off, but it is not a sanitizer: only use it with trusted input or
sanitize the output. `HtmlOptions::trusted()` turns every protection off;
`HtmlOptions::commonmark_trusted()` does the same and matches the CommonMark
reference output.

## Performance

The parser follows the CommonMark reference algorithms with an arena-based
block tree and inline list, then builds the public tree in one pass. Text is
borrowed from the input when it needs no transformation. On a 1 MB document
(the CommonMark spec repeated) it renders HTML about 2.5 times faster than
`comrak`, and 1.3 to 1.8 times slower than `pulldown-cmark`, which streams
events and builds no tree. Closing that gap is ongoing work.

## Development

```sh
cargo test                          # spec suites, security and pathological tests
cargo run --example md2html < README.md
```

`tests/fixtures` holds the spec examples, extracted with
`scripts/extract_spec.py`, and Comark's SPEC examples, extracted from the
Comark repository with `scripts/extract_comark_spec.py`. `src/tables.rs` (HTML entities and Unicode classes)
is generated by `scripts/gen_tables.py`.

## Releases

Releases are managed by [release-please](https://github.com/googleapis/release-please) and published to crates.io from GitHub Actions with [trusted publishing](https://crates.io/docs/trusted-publishing), so no crates.io token is stored in the repository.

## License

Apache-2.0
