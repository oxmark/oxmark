//! ```` ```mermaid ```` fences: Comark's mermaid plugin, and server-side SVG
//! with [oxmaid](https://github.com/oxmark/oxmaid).

use crate::ast::{Attributes, CodeBlock};
use crate::html::Renderer;

/// How ```` ```mermaid ```` fences are rendered.
///
/// A fence is a Mermaid diagram when its info string starts with `mermaid`,
/// as in Comark. Props can follow in braces:
/// ```` ```mermaid {.wide #flow title="Flow"} ````. They go through the same
/// security rules as component props.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum Mermaid<'a> {
    /// Mermaid fences are ordinary code blocks, as in Comark without its
    /// mermaid plugin.
    #[default]
    Off,
    /// The output of Comark's mermaid plugin, for diagrams drawn in the
    /// browser: `<mermaid content="...">`, with the props of the fence and
    /// the themes as attributes.
    Component {
        /// The `theme` attribute (Comark's `theme` option).
        theme: Option<&'a str>,
        /// The `theme-dark` attribute (Comark's `themeDark` option).
        theme_dark: Option<&'a str>,
    },
    /// Diagrams drawn to SVG with oxmaid, in the markup that Comark's
    /// `Mermaid` component renders in the browser:
    /// `<div class="mermaid" style="..."><svg>...</svg></div>`.
    ///
    /// The `id` of each SVG is the [`oxmaid::RenderOptions::id_prefix`]
    /// followed by `-0`, `-1`... in document order; give each document of a
    /// page its own prefix. A diagram that oxmaid cannot draw (a syntax
    /// error, or a diagram type that is not ported yet) is rendered as a
    /// code block.
    #[cfg(feature = "mermaid")]
    Svg(&'a oxmaid::RenderOptions),
}

impl Mermaid<'_> {
    /// Comark's mermaid plugin with no theme.
    pub const COMPONENT: Mermaid<'static> = Mermaid::Component {
        theme: None,
        theme_dark: None,
    };
}

/// Whether a code block is a Mermaid diagram (Comark: a fence whose info
/// starts with `mermaid`).
pub(crate) fn is_mermaid(code: &CodeBlock<'_>) -> bool {
    code.fenced && code.info.trim_start().starts_with("mermaid")
}

/// The props written after `mermaid` in the info string.
fn props<'a>(info: &'a str) -> Attributes<'a> {
    let rest = info.trim_start();
    let rest = rest.get("mermaid".len()..).unwrap_or("").trim();
    match rest.find('{') {
        Some(i) => crate::props::search(rest, rest, i)
            .map(|p| p.attrs)
            .unwrap_or_default(),
        None => Attributes::default(),
    }
}

pub(crate) fn render(r: &mut Renderer<'_, '_>, code: &CodeBlock<'_>, extra: &[&Attributes<'_>]) {
    let attrs = props(&code.info);
    let mut lists: Vec<&Attributes<'_>> = Vec::with_capacity(extra.len() + 1);
    lists.push(&attrs);
    lists.extend_from_slice(extra);
    match r.opts.mermaid {
        Mermaid::Off => unreachable!("mermaid rendering is off"),
        Mermaid::Component { theme, theme_dark } => {
            component(r, &code.literal, &lists, theme, theme_dark)
        }
        #[cfg(feature = "mermaid")]
        Mermaid::Svg(options) => svg(r, code, &lists, options),
    }
}

/// Comark's mermaid plugin: `<mermaid {props} theme theme-dark content>`.
fn component(
    r: &mut Renderer<'_, '_>,
    source: &str,
    lists: &[&Attributes<'_>],
    theme: Option<&str>,
    theme_dark: Option<&str>,
) {
    let mut after: Vec<(&str, &str)> = Vec::with_capacity(3);
    if let Some(theme) = theme {
        after.push(("theme", theme));
    }
    if let Some(theme) = theme_dark {
        after.push(("theme-dark", theme));
    }
    after.push(("content", source));
    r.cr();
    r.push("<mermaid");
    write_attrs(r, lists, &[], &after);
    r.push("></mermaid>\n");
}

/// Writes the attributes of `lists`, skipping `skip`, then `after`.
fn write_attrs(
    r: &mut Renderer<'_, '_>,
    lists: &[&Attributes<'_>],
    skip: &[&str],
    after: &[(&str, &str)],
) {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for list in lists {
        for a in list.iter() {
            let (name, value) = (&*a.name, &*a.value);
            if skip.contains(&name) || after.iter().any(|(n, _)| *n == name) {
                continue;
            }
            match seen.iter_mut().find(|(n, _)| *n == name) {
                Some(entry) => entry.1 = value,
                None => seen.push((name, value)),
            }
        }
    }
    for (name, value) in seen.iter().chain(after) {
        r.attr(name, value);
    }
}

#[cfg(feature = "mermaid")]
fn svg(
    r: &mut Renderer<'_, '_>,
    code: &CodeBlock<'_>,
    lists: &[&Attributes<'_>],
    options: &oxmaid::RenderOptions,
) {
    let diagram = match oxmaid::parse(&code.literal) {
        Ok(diagram) => diagram,
        Err(_) => {
            render_as_code(r, code, lists);
            return;
        }
    };
    let mut options = options.clone();
    options.id_prefix = format!("{}-{}", options.id_prefix, r.diagrams);
    r.diagrams += 1;
    let svg = diagram.to_svg(&options);

    // The props of Comark's `Mermaid` component: `class`, `width` and
    // `height` shape the wrapper; the other attributes fall through to it.
    let prop = |name: &str| {
        lists
            .iter()
            .rev()
            .find_map(|l| l.get(name))
            .filter(|v| !v.is_empty())
    };
    let mut class = String::from("mermaid");
    for list in lists {
        for a in list.iter().filter(|a| a.name == "class") {
            class.push(' ');
            class.push_str(a.value.trim());
        }
    }
    let style = format!(
        "display: flex; justify-content: center; width: {}; height: {};",
        css_size(prop("width")).unwrap_or("100%"),
        css_size(prop("height")).unwrap_or("auto"),
    );
    r.cr();
    r.push("<div");
    r.attr("class", class.trim_end());
    r.attr("style", &style);
    write_attrs(
        r,
        lists,
        &["class", "width", "height", "theme", "theme-dark", "content"],
        &[],
    );
    r.push(">");
    // The SVG is built by oxmaid, which escapes text and attributes, keeps
    // only plain inline tags in labels and drops unsafe links and styles.
    r.push(&svg);
    r.push("</div>\n");
}

/// A CSS length made of safe characters (`100%`, `20rem`, `auto`...).
#[cfg(feature = "mermaid")]
fn css_size(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|v| {
        !v.is_empty()
            && v.len() <= 32
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"%.-".contains(&b))
    })
}

/// A diagram oxmaid cannot draw is shown as its source.
#[cfg(feature = "mermaid")]
fn render_as_code(r: &mut Renderer<'_, '_>, code: &CodeBlock<'_>, lists: &[&Attributes<'_>]) {
    r.plain_code_block(code, &lists[1..]);
}
