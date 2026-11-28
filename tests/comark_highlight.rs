//! Code highlighting and Mermaid fences compared with Comark 0.9.2 and its
//! highlight and mermaid plugins (`tests/fixtures/comark-highlight.json`,
//! built by `scripts/comark_highlight_fixtures.mjs`).

use oxmark::{to_html_with, HtmlOptions, Mermaid, Options};
use serde_json::Value;

/// Known differences:
/// - Comark drops a Mermaid fence nested in a component (a Comark bug);
/// - in `<style>a {`, oxlight gives the `a` selector its own token, Shiki
///   does not, and oxmark escapes the `&` of `&amp;` in code (see the README).
const SKIPPED: &[&str] = &["mermaid in component", "html with embedded css and js"];

/// Comark indents nested blocks; line indents are ignored.
fn normalize(html: &str) -> String {
    html.trim_end()
        .lines()
        .map(|l| {
            if l.starts_with("<span") {
                l
            } else {
                l.trim_start()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cases() -> Vec<Value> {
    let json = include_str!("fixtures/comark-highlight.json");
    serde_json::from_str(json).unwrap()
}

fn mermaid_option(case: &Value) -> Mermaid<'_> {
    match &case["mermaid"] {
        Value::Null => Mermaid::Off,
        m => Mermaid::Component {
            theme: m["theme"].as_str(),
            theme_dark: m["themeDark"].as_str(),
        },
    }
}

#[cfg(feature = "highlight")]
fn highlighter() -> oxlight::Highlighter {
    let mut h = oxlight::Highlighter::new();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/shiki");
    for lang in ["css", "javascript", "html", "json", "rust", "typescript"] {
        let json = std::fs::read_to_string(format!("{dir}/langs/{lang}.json")).unwrap();
        h.load_language_json(&json).unwrap();
    }
    for theme in ["github-light", "github-dark", "one-dark-pro"] {
        let json = std::fs::read_to_string(format!("{dir}/themes/{theme}.json")).unwrap();
        h.load_theme_json(&json).unwrap();
    }
    h
}

#[test]
fn matches_comark() {
    #[cfg(feature = "highlight")]
    let hl = highlighter();
    let mut failures = Vec::new();
    let mut ran = 0;
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        if SKIPPED.contains(&name) {
            continue;
        }
        #[cfg_attr(not(feature = "highlight"), allow(unused_mut))]
        let mut html = HtmlOptions {
            mermaid: mermaid_option(&case),
            ..HtmlOptions::default()
        };
        if let Some(h) = case["highlight"].as_object() {
            #[cfg(not(feature = "highlight"))]
            {
                let _ = h;
                continue;
            }
            #[cfg(feature = "highlight")]
            {
                let mut config = oxmark::Highlight::new(&hl);
                config.light = h["themes"]["light"].as_str();
                config.dark = h["themes"]["dark"].as_str();
                config.pre_styles = h["preStyles"].as_bool().unwrap();
                config.inline_code = h["inlineCode"].as_bool().unwrap();
                html.highlight = Some(config);
            }
        }
        ran += 1;
        let markdown = case["markdown"].as_str().unwrap();
        let expected = case["html"].as_str().unwrap();
        let actual = to_html_with(markdown, &Options::default(), &html);
        if normalize(&actual) != normalize(expected) {
            failures.push(format!(
                "--- {name}\n{markdown}\nexpected:\n{expected}\nactual:\n{actual}"
            ));
        }
    }
    assert!(ran > 0);
    assert!(
        failures.is_empty(),
        "{} of {ran} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[cfg(feature = "highlight")]
#[test]
fn highlighted_code_is_safe() {
    let hl = highlighter();
    let html = HtmlOptions {
        highlight: Some(oxmark::Highlight::new(&hl).themes("github-light", "github-dark")),
        ..HtmlOptions::default()
    };
    let md = concat!(
        "```ts\nlet a = \"</code></pre><script>alert(1)</script>\" // <img src=x onerror=alert(1)>\n```\n\n",
        "```html\n<iframe srcdoc=\"<script>x</script>\"></iframe>\n```\n\n",
        "```nope\"><script>alert(1)</script>\nx\n```\n\n",
        "::pre{onclick=\"alert(1)\" .a style=\"background:url(javascript:alert(1))\"}\n```ts\nlet a\n```\n::\n\n",
        "Inline `</code><script>x</script>`{lang=ts onclick=\"alert(1)\"}.\n",
    );
    let out = to_html_with(md, &Options::default(), &html);
    let lower = out.to_ascii_lowercase();
    for needle in ["<script", "<iframe", "<img", "onclick", "javascript:"] {
        assert!(!lower.contains(needle), "{needle} in {out}");
    }
}
