//! Hostile inputs that must parse in roughly linear time and without
//! overflowing the stack. Adapted from cmark's pathological tests.

use std::time::{Duration, Instant};

fn check(name: &str, input: &str) {
    let start = Instant::now();
    let html = oxmark::to_html(input);
    let elapsed = start.elapsed();
    assert!(
        !html.is_empty() || input.trim().is_empty(),
        "{name}: empty output"
    );
    // Generous bound for unoptimized builds on slow CI machines.
    assert!(
        elapsed < Duration::from_secs(10),
        "{name}: took {elapsed:?} for {} bytes",
        input.len()
    );
    eprintln!("{name}: {elapsed:?} for {} bytes", input.len());
}

fn rep(s: &str, n: usize) -> String {
    s.repeat(n)
}

#[test]
fn emphasis() {
    check(
        "nested strong emph",
        &format!("{}b{}", rep("*a **a ", 30000), rep(" a** a*", 30000)),
    );
    check("many emph closers with no openers", &rep("a_ ", 50000));
    check("many emph openers with no closers", &rep("_a ", 50000));
    check("mismatched openers and closers", &rep("*a_ ", 50000));
    check(
        "openers and closers multiple of 3",
        &format!("a**b{}", rep("c* ", 50000)),
    );
    check(
        "deep emphasis",
        &format!("{}a{}", rep("*", 50000), rep("*", 50000)),
    );
    check("many delimiters", &rep("*", 100000));
    check("strikethrough", &rep("~~a ", 50000));
}

#[test]
fn links() {
    check("many link closers with no openers", &rep("a]", 50000));
    check("many link openers with no closers", &rep("[a", 50000));
    check("link openers and emph closers", &rep("[ a_", 50000));
    check("pattern [ (](", &rep("[ (](", 50000));
    check(
        "nested brackets",
        &format!("{}a{}", rep("[", 50000), rep("]", 50000)),
    );
    check(
        "nested images",
        &format!("{}a{}", rep("![", 20000), rep("](u)", 20000)),
    );
    check("unclosed links A", &rep("[a](<b", 30000));
    check("unclosed links B", &rep("[a](b", 30000));
    check("unclosed titles", &rep("[a](b \"", 30000));
    check(
        "many references",
        &format!("{}\n{}", rep("[a]: /b\n", 20000), rep("[a] ", 20000)),
    );
    check("long label", &format!("[{}]", rep("a", 100000)));
    let options = oxmark::Options {
        footnotes: true,
        ..oxmark::Options::default()
    };
    let input = format!("{}\n\n{}", rep("x[^a] [^b", 20000), rep("[^a]: n\n", 20000));
    let start = Instant::now();
    oxmark::to_html_with(&input, &options, &oxmark::HtmlOptions::default());
    assert!(start.elapsed() < Duration::from_secs(10), "footnotes");
    check("many headings", &rep("# Title\n", 20000));
}

#[test]
fn inline_misc() {
    check(
        "backticks",
        &(1..3000)
            .map(|i| format!("e{}", "`".repeat(i)))
            .collect::<String>(),
    );
    check("unclosed comments", &format!("</{}", rep("<!--", 30000)));
    check("unclosed processing instructions", &rep("<?", 30000));
    check("unclosed CDATA", &rep("<![CDATA[", 30000));
    check("unclosed declarations", &rep("<!A", 30000));
    check("entities", &rep("&amp;&#123;&#x1F600;&bogus;", 20000));
    check("autolinks", &rep("www.a.com http://b.org c@d.ef ", 20000));
}

#[test]
fn blocks() {
    check("nested block quotes", &format!("{}a", rep("> ", 50000)));
    check(
        "nested lists on one line",
        &format!("{}a", rep("- ", 50000)),
    );
    check(
        "deeply nested lists",
        &(0..1000)
            .map(|i| format!("{}* a\n", "  ".repeat(i)))
            .collect::<String>(),
    );
    check("many lines", &rep("a\n", 100000));
    check("many paragraphs", &rep("a\n\n", 100000));
    check("many fences", &rep("```\n", 50000));
    check("NUL bytes", &rep("a\0", 50000));
    check(
        "wide table",
        &format!(
            "{}|\n{}|\n{}",
            rep("| a ", 1000),
            rep("| - ", 1000),
            rep("x\n", 5000)
        ),
    );
}
