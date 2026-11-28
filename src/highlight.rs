//! Syntax highlighting with [oxlight], a port of Comark's highlight plugin
//! (`comark/plugins/highlight`, which runs Shiki).

use oxlight::{font_style, HighlightOptions, Highlighter, ThemedToken};

use crate::ast::{Attributes, Code};
use crate::html::{escape_attr, escape_text, Attr, CodeInfo, Renderer};

/// The light theme Comark uses when none is given.
pub const DEFAULT_LIGHT_THEME: &str = "material-theme-lighter";
/// The dark theme Comark uses when none is given.
pub const DEFAULT_DARK_THEME: &str = "material-theme-palenight";

/// Syntax highlighting options, like the options of Comark's highlight
/// plugin.
///
/// The [`Highlighter`] holds the languages and themes; load them once and
/// share it between documents (and threads). A code block whose language is
/// not loaded keeps its text, with a plain `shiki` class, as in Comark.
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut highlighter = oxlight::Highlighter::new();
/// # let json = std::fs::read_to_string("tests/fixtures/shiki/langs/rust.json")?;
/// highlighter.load_language_json(&json)?;
/// # let light = std::fs::read_to_string("tests/fixtures/shiki/themes/github-light.json")?;
/// # let dark = std::fs::read_to_string("tests/fixtures/shiki/themes/github-dark.json")?;
/// highlighter.load_theme_json(&light)?;
/// highlighter.load_theme_json(&dark)?;
///
/// let html = oxmark::HtmlOptions {
///     highlight: Some(oxmark::Highlight::new(&highlighter).themes("github-light", "github-dark")),
///     ..Default::default()
/// };
/// let out = oxmark::to_html_with("```rust\nfn main() {}\n```", &Default::default(), &html);
/// assert!(out.starts_with(r#"<pre data-language="rust" class="shiki shiki-themes github-light github-dark dark:github-dark">"#));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Highlight<'a> {
    /// The languages and themes.
    pub highlighter: &'a Highlighter,
    /// The light theme (Comark's `themes.light`). When only one of `light`
    /// and `dark` is set, it is used for both and no CSS variables are
    /// written. With neither, Comark's defaults are used:
    /// [`DEFAULT_LIGHT_THEME`] and [`DEFAULT_DARK_THEME`].
    pub light: Option<&'a str>,
    /// The dark theme (Comark's `themes.dark`), written as `--shiki-dark`
    /// CSS variables. Setting it also adds a `dark:{name}` class to the
    /// `<pre>`, as in Comark.
    pub dark: Option<&'a str>,
    /// Highlight inline code with a `lang` (or `language`) attribute, as in
    /// `` `let a = 1`{lang="ts"} ``. Defaults to `true`.
    pub inline_code: bool,
    /// Give the `<pre>` a `style` with the background and foreground colors
    /// of the light theme, and of the dark theme as CSS variables
    /// (Comark's `preStyles`). Defaults to `false`.
    pub pre_styles: bool,
}

impl PartialEq for Highlight<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.highlighter, other.highlighter)
            && self.light == other.light
            && self.dark == other.dark
            && self.inline_code == other.inline_code
            && self.pre_styles == other.pre_styles
    }
}

impl<'a> Highlight<'a> {
    /// Highlighting with Comark's default themes.
    pub fn new(highlighter: &'a Highlighter) -> Self {
        Highlight {
            highlighter,
            light: None,
            dark: None,
            inline_code: true,
            pre_styles: false,
        }
    }

    /// Sets the light and dark themes.
    pub fn themes(mut self, light: &'a str, dark: &'a str) -> Self {
        self.light = Some(light);
        self.dark = Some(dark);
        self
    }

    /// Uses one theme, without CSS variables.
    pub fn theme(mut self, theme: &'a str) -> Self {
        self.light = Some(theme);
        self.dark = None;
        self
    }
}

/// Highlighting options resolved once per rendered document.
#[derive(Debug)]
pub(crate) struct State<'a> {
    config: &'a Highlight<'a>,
    /// The oxlight options; `lang` is set for each block.
    options: HighlightOptions,
    /// ` dark:{name}`, when a dark theme is set.
    dark_class: String,
    /// The `style` of `<pre>`, with `pre_styles`.
    pre_style: Option<String>,
}

impl<'a> State<'a> {
    pub(crate) fn new(config: &'a Highlight<'a>) -> Self {
        let light = config.light.or(config.dark).unwrap_or(DEFAULT_LIGHT_THEME);
        let dark = config.dark.or(config.light).unwrap_or(DEFAULT_DARK_THEME);
        let mut themes = vec![("light", light)];
        if light != dark {
            themes.push(("dark", dark));
        }
        let mut options = HighlightOptions::with_themes("", themes);
        // Comark renders the tokens itself, so these only affect tokenizing.
        options.tabindex = None;
        options.root_style = false;
        let dark_class = config
            .dark
            .map(|name| format!(" dark:{name}"))
            .unwrap_or_default();
        let pre_style = config.pre_styles.then(|| {
            let colors = |name: Option<&str>| {
                let theme = name.and_then(|n| config.highlighter.theme(n));
                let get = |key: &str| theme.and_then(|t| t.colors.get(key)).cloned();
                (get("editor.background"), get("editor.foreground"))
            };
            let mut styles = Vec::new();
            let (bg, fg) = colors(config.light);
            if let Some(bg) = bg {
                styles.push(format!("background-color:{bg}"));
            }
            if let Some(fg) = fg {
                styles.push(format!("color:{fg}"));
            }
            if config.light != config.dark {
                let (bg, fg) = colors(config.dark);
                if let Some(bg) = bg {
                    styles.push(format!("--shiki-dark-bg:{bg}"));
                }
                if let Some(fg) = fg {
                    styles.push(format!("--shiki-dark:{fg}"));
                }
            }
            styles.join(";")
        });
        State {
            config,
            options,
            dark_class,
            pre_style,
        }
    }

    /// Tokenizes `code`, or `None` when the language or a theme is missing.
    fn tokens(&mut self, code: &str, lang: &str) -> Option<oxlight::TokensResult> {
        self.options.lang.clear();
        self.options.lang.push_str(lang);
        self.config
            .highlighter
            .code_to_tokens(code, &self.options)
            .ok()
    }
}

/// Renders a highlighted code block (Comark's `highlightCodeBlocks`).
pub(crate) fn code_block(
    r: &mut Renderer<'_, '_>,
    info: &CodeInfo<'_>,
    code: &str,
    own: &[Attr<'_>],
    extra: &[&Attributes<'_>],
) {
    let Some(state) = r.highlight.as_mut() else {
        return;
    };
    // Shiki highlights a block without language as plain text.
    let lang = if info.language.is_empty() {
        "text"
    } else {
        info.language
    };
    let result = state.tokens(code, lang);
    let mut class = match &result {
        Some(result) => format!("shiki {}", result.theme_name),
        None => String::from("shiki"),
    };
    class.push_str(&state.dark_class);
    let pre_style = state.pre_style.take();
    match &pre_style {
        Some(style) => r.open_tag_with_class("pre", own, extra, &class, &[("style", style)]),
        None => r.open_tag_with_class("pre", own, extra, &class, &[]),
    }
    if let Some(state) = r.highlight.as_mut() {
        state.pre_style = pre_style;
    }
    r.push("<code");
    if !info.language.is_empty() {
        r.push(" class=\"language-");
        escape_attr(r.out, info.language);
        r.push("\"");
    }
    r.push(">");
    match result {
        Some(result) => {
            for (i, line) in result.tokens.iter().enumerate() {
                if i > 0 {
                    r.push("\n");
                }
                let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
                if info.highlights.contains(&n) {
                    r.push("<span class=\"line highlight\" style=\"display: inline-block\">");
                } else {
                    r.push("<span class=\"line\" style=\"display: inline\">");
                }
                spans(r.out, line);
                r.push("</span>");
            }
        }
        None => escape_text(r.out, code),
    }
    r.push("</code></pre>\n");
}

/// Renders highlighted inline code, when it has a loaded `lang`. Returns
/// `false` when the code is not highlighted.
pub(crate) fn inline_code(r: &mut Renderer<'_, '_>, code: &Code<'_>) -> bool {
    let Some(state) = r.highlight.as_mut() else {
        return false;
    };
    if !state.config.inline_code {
        return false;
    }
    let lang = match code
        .attrs
        .get("lang")
        .or_else(|| code.attrs.get("language"))
    {
        Some(lang) => lang.trim(),
        None => return false,
    };
    // Comark highlights only the languages that are loaded. `ts-type` and
    // `vue-html` need grammar contexts, which oxlight does not support.
    if lang.is_empty() || state.config.highlighter.language(lang).is_none() {
        return false;
    }
    let Some(result) = state.tokens(&code.literal, lang) else {
        return false;
    };
    let class = format!("shiki {}{}", result.theme_name, state.dark_class);
    r.open_tag_with_class("code", &[], &[&code.attrs], &class, &[]);
    let mut empty = true;
    for (i, line) in result.tokens.iter().enumerate() {
        if i > 0 {
            r.push("\n");
        }
        empty &= line.is_empty();
        spans(r.out, line);
    }
    if empty && result.tokens.len() <= 1 {
        escape_text(r.out, &code.literal);
    }
    r.push("</code>");
    true
}

/// Comark's `tokensToSpans`: whitespace-only tokens are carried into the
/// next token, unless one of them is underlined or struck through.
fn spans(out: &mut String, line: &[ThemedToken]) {
    let decorated =
        |t: &ThemedToken| t.font_style & (font_style::UNDERLINE | font_style::STRIKETHROUGH) != 0;
    let mut carry: Option<(usize, usize)> = None;
    for (t, token) in line.iter().enumerate() {
        let can_merge = !decorated(token);
        let blank = !token.content.is_empty() && token.content.chars().all(char::is_whitespace);
        if can_merge && blank && t + 1 < line.len() {
            carry = Some((carry.map_or(t, |c| c.0), t + 1));
        } else if let Some((from, to)) = carry.take() {
            if can_merge {
                open_span(out, token);
                for c in &line[from..to] {
                    escape_text(out, &c.content);
                }
                escape_text(out, &token.content);
                out.push_str("</span>");
            } else {
                out.push_str("<span>");
                for c in &line[from..to] {
                    escape_text(out, &c.content);
                }
                out.push_str("</span>");
                open_span(out, token);
                escape_text(out, &token.content);
                out.push_str("</span>");
            }
        } else {
            open_span(out, token);
            escape_text(out, &token.content);
            out.push_str("</span>");
        }
    }
    if let Some((from, to)) = carry {
        out.push_str("<span>");
        for c in &line[from..to] {
            escape_text(out, &c.content);
        }
        out.push_str("</span>");
    }
}

/// Writes `<span style="...">`, with Shiki's `getTokenStyleObject`.
fn open_span(out: &mut String, token: &ThemedToken) {
    let start = out.len();
    out.push_str("<span style=\"");
    let styles = out.len();
    let mut sep = false;
    let mut decl = |out: &mut String, key: &str, value: &str| {
        if sep {
            out.push(';');
        }
        sep = true;
        escape_attr(out, key);
        out.push(':');
        escape_attr(out, value);
    };
    if let Some(style) = &token.html_style {
        for (k, v) in style {
            decl(out, k, v);
        }
    } else {
        if let Some(color) = token.color.as_deref().filter(|c| !c.is_empty()) {
            decl(out, "color", color);
        }
        if let Some(bg) = token.bg_color.as_deref().filter(|c| !c.is_empty()) {
            decl(out, "background-color", bg);
        }
        let fs = token.font_style;
        if fs & font_style::ITALIC != 0 {
            decl(out, "font-style", "italic");
        }
        if fs & font_style::BOLD != 0 {
            decl(out, "font-weight", "bold");
        }
        match (
            fs & font_style::UNDERLINE != 0,
            fs & font_style::STRIKETHROUGH != 0,
        ) {
            (true, true) => decl(out, "text-decoration", "underline line-through"),
            (true, false) => decl(out, "text-decoration", "underline"),
            (false, true) => decl(out, "text-decoration", "line-through"),
            (false, false) => {}
        }
    }
    if out.len() == styles {
        out.truncate(start);
        out.push_str("<span>");
    } else {
        out.push_str("\">");
    }
}
