//! Mermaid diagrams drawn to SVG with oxmaid.
#![cfg(feature = "mermaid")]

use oxmark::oxmaid::RenderOptions;
use oxmark::{to_html_with, HtmlOptions, Mermaid, Options};

fn render(md: &str) -> String {
    let svg = RenderOptions::default();
    let html = HtmlOptions {
        mermaid: Mermaid::Svg(&svg),
        ..HtmlOptions::default()
    };
    to_html_with(md, &Options::default(), &html)
}

#[test]
fn draws_svg_in_the_markup_of_comarks_component() {
    let html = render("```mermaid\nflowchart LR\n  A[Start] --> B{Ok?}\n```\n\nAfter.");
    assert!(
        html.starts_with(
            r#"<div class="mermaid" style="display: flex; justify-content: center; width: 100%; height: auto;"><svg id="oxmaid-0""#
        ),
        "{html}"
    );
    assert!(html.contains(">Start<"));
    assert!(html.ends_with("</svg></div>\n<p>After.</p>\n"), "{html}");
}

#[test]
fn gives_each_diagram_its_own_id() {
    let html =
        render("```mermaid\ngraph TD\n  A --> B\n```\n\n```mermaid\ngraph TD\n  C --> D\n```\n");
    assert!(html.contains(r#"<svg id="oxmaid-0""#));
    assert!(html.contains(r#"<svg id="oxmaid-1""#));
    assert!(
        html.contains("#oxmaid-1 "),
        "styles are scoped to the diagram"
    );
}

#[test]
fn uses_the_props_of_the_fence() {
    let html = render(
        "```mermaid {.wide #flow title=\"Flow\" width=50% height=\"20rem\"}\ngraph TD\n  A --> B\n```\n",
    );
    assert!(
        html.starts_with(
            r#"<div class="mermaid wide" style="display: flex; justify-content: center; width: 50%; height: 20rem;" id="flow" title="Flow"><svg"#
        ),
        "{html}"
    );
}

#[test]
fn falls_back_to_a_code_block() {
    let html = render("```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n");
    assert_eq!(
        html,
        "<pre data-language=\"mermaid\"><code class=\"language-mermaid\">sequenceDiagram\n  A-&gt;&gt;B: hi</code></pre>\n"
    );
    let html = render("```mermaid\nflowchart LR\n  A --> \n```\n");
    assert!(
        html.starts_with("<pre data-language=\"mermaid\">"),
        "{html}"
    );
}

#[test]
fn keeps_other_code_blocks() {
    assert_eq!(
        render("```js\ngraph TD\n```\n\n    graph TD\n"),
        "<pre data-language=\"js\"><code class=\"language-js\">graph TD</code></pre>\n<pre><code>graph TD</code></pre>\n"
    );
}

/// Checks the tags of `html` (not its text) for what could run code or
/// load content: blocked elements, event handlers, unsafe URLs and styles.
fn assert_safe(html: &str) {
    let lower = html.to_ascii_lowercase();
    if lower.starts_with("<pre") {
        // The source shown as a code block: only escaped text.
        let tags = lower.matches('<').count();
        assert_eq!(tags, 4, "{html}");
        return;
    }
    for needle in [
        "expression(",
        "@import",
        "<script",
        "<iframe",
        "<object",
        "<embed",
    ] {
        assert!(!lower.contains(needle), "{needle} in {html}");
    }
    // Only references to the diagram's own markers and filters.
    for (i, _) in lower.match_indices("url(") {
        assert!(lower[i + 4..].starts_with("#oxmaid"), "{}", &html[i..]);
    }
    let mut rest = lower.as_str();
    while let Some(start) = rest.find('<') {
        let tag = &rest[start + 1..];
        let end = tag.find('>').expect("unclosed tag");
        let inner = &tag[..end];
        rest = &tag[end..];
        if inner.starts_with('/') {
            continue;
        }
        let mut parts = inner.split_whitespace();
        let name = parts.next().unwrap_or("");
        assert!(
            !["img", "image", "use", "animate", "set", "style"].contains(&name)
                || (name == "style" && inner == "style"),
            "<{inner}> in {html}"
        );
        for attr in parts {
            let attr_name = attr.split('=').next().unwrap();
            assert!(!attr_name.starts_with("on"), "{attr} in <{inner}>");
            assert!(
                !["srcdoc", "formaction", "xlink:href"].contains(&attr_name),
                "{attr}"
            );
            if attr_name == "href" {
                let value = attr.trim_start_matches("href=").trim_matches('"').trim();
                assert!(!value.contains("script:"), "{attr}");
            }
        }
    }
}

#[test]
fn diagrams_are_safe() {
    let payloads = [
        "flowchart LR\n  A[\"<img src=x onerror=alert(1)>\"] --> B[\"<script>alert(1)</script>\"]",
        "flowchart LR\n  A[\"<b onclick=alert(1)>x</b>\"] --> B",
        "flowchart LR\n  A[\"`**<img src=x onerror=alert(1)>**`\"] --> B",
        "flowchart LR\n  A --> B\n  click A \"javascript:alert(1)\"\n  click B href \" JaVaScRiPt:alert(1)\" _blank",
        "flowchart LR\n  A --> B\n  click A callback \"tooltip\"\n  click B call alert(1)",
        "flowchart LR\n  A --> B\n  style A fill:url(javascript:alert(1)),stroke:red",
        "flowchart LR\n  A --> B\n  classDef x fill:red}</style><script>alert(1)</script>\n  class B x",
        "flowchart LR\n  A --> B\n  classDef x</style><script>alert(1)</script> fill:red\n  linkStyle 0 stroke:expression(alert(1))",
        "flowchart LR\n  A -->|\"<iframe srcdoc=x>\"| B\n  subgraph s [\"<svg onload=alert(1)>\"]\n  C\n  end",
        "%%{init: {\"themeCSS\": \"</style><script>alert(1)</script>\", \"fontFamily\": \"</style><script>\"}}%%\nflowchart LR\n  A --> B",
        "---\nconfig:\n  themeCSS: \"@import url(x)\"\n---\nflowchart LR\n  A --> B",
        "flowchart LR\n  A[\"fa:fa-car&quot; onclick=&quot;alert(1)\"] --> B",
        "flowchart LR\n  A --> B@{ shape: rect, label: \"<img src=x onerror=alert(1)>\" }",
        "flowchart LR\n  accTitle: </title><script>alert(1)</script>\n  accDescr: <img src=x onerror=alert(1)>\n  A --> B",
    ];
    for p in payloads {
        let md = format!("```mermaid {{onclick=\"alert(1)\" :onload=\"x\" style=\"background:url(javascript:alert(1))\"}}\n{p}\n```\n");
        let html = render(&md);
        assert!(
            html.starts_with("<div class=\"mermaid\""),
            "not drawn: {p}\n{html}"
        );
        assert_safe(&html);
    }
}

#[test]
fn component_props_are_sanitized() {
    let html = to_html_with(
        "```mermaid {onclick=\"alert(1)\" .a}\ngraph TD\n  A[\"</mermaid><script>x</script>\"]\n```\n",
        &Options::default(),
        &HtmlOptions {
            mermaid: Mermaid::COMPONENT,
            ..HtmlOptions::default()
        },
    );
    assert_eq!(
        html,
        "<mermaid class=\"a\" content=\"graph TD\n  A[&quot;&lt;/mermaid&gt;&lt;script&gt;x&lt;/script&gt;&quot;]\n\"></mermaid>\n"
    );
}
