//! XSS protections of the default HTML options.

use oxmark::{to_html, to_html_with, HtmlOptions, Options, RawHtml};

fn html(md: &str) -> String {
    to_html(md)
}

#[test]
fn raw_html_is_escaped_by_default() {
    assert_eq!(
        html("<script>alert(1)</script>"),
        "&lt;script&gt;alert(1)&lt;/script&gt;\n"
    );
    assert_eq!(
        html("a <img src=x onerror=alert(1)> b"),
        "<p>a &lt;img src=x onerror=alert(1)&gt; b</p>\n"
    );
    assert_eq!(
        html("<div onclick=\"alert(1)\">\n\n*x*\n\n</div>"),
        "&lt;div onclick=&quot;alert(1)&quot;&gt;\n<p><em>x</em></p>\n&lt;/div&gt;\n"
    );
    assert_eq!(html("<!-- <script> -->"), "&lt;!-- &lt;script&gt; --&gt;\n");
}

#[test]
fn raw_html_can_be_omitted() {
    let opts = HtmlOptions {
        raw_html: RawHtml::Omit,
        ..HtmlOptions::default()
    };
    assert_eq!(
        to_html_with("a <b>c</b>\n\n<div>x</div>", &Options::default(), &opts),
        "<p>a <!-- raw HTML omitted -->c<!-- raw HTML omitted --></p>\n<!-- raw HTML omitted -->\n"
    );
}

#[test]
fn allowed_raw_html_keeps_the_tag_filter() {
    let opts = HtmlOptions {
        raw_html: RawHtml::Allow,
        ..HtmlOptions::default()
    };
    assert_eq!(
        to_html_with(
            "<b>ok</b> <script>x</script> <IFRAME src=y>",
            &Options::default(),
            &opts
        ),
        "<p><b>ok</b> &lt;script>x&lt;/script> &lt;IFRAME src=y></p>\n"
    );
}

#[test]
fn dangerous_schemes_are_dropped() {
    for url in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "vbscript:msgbox(1)",
        "file:///etc/passwd",
        "data:text/html;base64,PHNjcmlwdD4=",
        "&#106;avascript:alert(1)",
        "&#x6A;avascript&colon;alert(1)",
        "java&#x09;script:alert(1)",
        "<java\tscript:alert(1)>",
        "<\u{1}javascript:alert(1)>",
    ] {
        let out = html(&format!("[x]({url})"));
        assert!(
            out.contains("href=\"\"") || !out.contains("href"),
            "{url} gave {out}"
        );
        assert!(
            !out.to_ascii_lowercase().contains("script:"),
            "{url} gave {out}"
        );
    }
    assert_eq!(
        html("<javascript:alert(1)>"),
        "<p><a href=\"\">javascript:alert(1)</a></p>\n"
    );
    assert_eq!(
        html("[x]: javascript:alert(1)\n\n[x]"),
        "<p><a href=\"\">x</a></p>\n"
    );
    assert_eq!(
        html("![x](javascript:alert(1))"),
        "<p><img src=\"\" alt=\"x\" /></p>\n"
    );
}

#[test]
fn safe_urls_are_kept() {
    assert_eq!(
        html("[a](https://example.com/?q=1&r=2) [b](/relative) [c](#frag) [d](mailto:a@b.c)"),
        "<p><a href=\"https://example.com/?q=1&amp;r=2\">a</a> <a href=\"/relative\">b</a> \
         <a href=\"#frag\">c</a> <a href=\"mailto:a@b.c\">d</a></p>\n"
    );
    assert_eq!(
        html("![p](data:image/png;base64,iVBORw0KGgo=)"),
        "<p><img src=\"data:image/png;base64,iVBORw0KGgo=\" alt=\"p\" /></p>\n"
    );
    // A data URL is only allowed for images.
    assert_eq!(
        html("[p](data:image/png;base64,iVBORw0KGgo=)"),
        "<p><a href=\"\">p</a></p>\n"
    );
    // A colon after a slash is not a scheme.
    assert_eq!(
        html("[a](./javascript:x)"),
        "<p><a href=\"./javascript:x\">a</a></p>\n"
    );
}

#[test]
fn attributes_cannot_be_broken_out_of() {
    assert_eq!(
        html("[x](<a\" onmouseover=\"alert(1)> \"t\\\" onclick=\\\"y\")"),
        "<p><a href=\"a%22%20onmouseover=%22alert(1)\" title=\"t&quot; onclick=&quot;y\">x</a></p>\n"
    );
    assert_eq!(
        html("![a\" onerror=\"alert(1)](i.png)"),
        "<p><img src=\"i.png\" alt=\"a&quot; onerror=&quot;alert(1)\" /></p>\n"
    );
    assert_eq!(
        html("```x\"onclick=\"alert(1)\ncode\n```"),
        "<pre><code class=\"language-x&quot;onclick=&quot;alert(1)\">code\n</code></pre>\n"
    );
    assert_eq!(html("`<script>`"), "<p><code>&lt;script&gt;</code></p>\n");
}

#[test]
fn nul_is_replaced() {
    assert_eq!(html("a\0b"), "<p>a\u{FFFD}b</p>\n");
    assert_eq!(html("&#0;"), "<p>\u{FFFD}</p>\n");
}

#[test]
fn trusted_mode_renders_everything() {
    assert_eq!(
        to_html_with(
            "<script>x</script>\n\n[a](javascript:b)",
            &Options::default(),
            &HtmlOptions::trusted()
        ),
        "<script>x</script>\n<p><a href=\"javascript:b\">a</a></p>\n"
    );
}
