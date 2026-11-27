//! Runs the examples of the CommonMark and GFM specifications.

use oxmark::{to_html_with, HtmlOptions, Options};

struct Example {
    number: u64,
    section: String,
    markdown: String,
    html: String,
}

fn load(json: &str) -> Vec<Example> {
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|e| Example {
            number: e["example"].as_u64().unwrap(),
            section: e["section"].as_str().unwrap().to_owned(),
            markdown: e["markdown"].as_str().unwrap().to_owned(),
            html: e["html"].as_str().unwrap().to_owned(),
        })
        .collect()
}

fn run(examples: &[Example], options: &Options, normalize: fn(&str) -> String) {
    let mut failures = Vec::new();
    for e in examples {
        let actual = to_html_with(&e.markdown, options, &HtmlOptions::commonmark_trusted());
        if normalize(&actual) != normalize(&e.html) {
            failures.push(format!(
                "example {} ({})\n--- markdown\n{}--- expected\n{}--- actual\n{}",
                e.number, e.section, e.markdown, e.html, actual
            ));
        }
    }
    if !failures.is_empty() {
        let shown = failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} of {} examples failed:\n\n{}",
            failures.len(),
            examples.len(),
            shown
        );
    }
}

fn identity(s: &str) -> String {
    s.to_owned()
}

#[test]
fn commonmark_spec() {
    let examples = load(include_str!("fixtures/commonmark-0.31.2.json"));
    assert_eq!(examples.len(), 652);
    run(&examples, &Options::commonmark(), identity);
}

/// Normalizes the HTML of the GFM spec, written by hand with
/// `<input disabled="" type="checkbox">` style attributes.
fn normalize_gfm(s: &str) -> String {
    s.replace(
        "<input type=\"checkbox\" disabled=\"\" />",
        "<input disabled=\"\" type=\"checkbox\">",
    )
    .replace(
        "<input type=\"checkbox\" checked=\"\" disabled=\"\" />",
        "<input checked=\"\" disabled=\"\" type=\"checkbox\">",
    )
}

#[test]
fn gfm_extensions() {
    let examples = load(include_str!("fixtures/gfm-0.29-extensions.json"));
    let options = Options::gfm();
    let mut html = HtmlOptions::commonmark_trusted();
    html.tagfilter = true;
    let mut failures = Vec::new();
    for e in &examples {
        let actual = to_html_with(&e.markdown, &options, &html);
        if normalize_gfm(&actual) != normalize_gfm(&e.html) {
            failures.push(format!(
                "example {} ({})\n--- markdown\n{}--- expected\n{}--- actual\n{}",
                e.number, e.section, e.markdown, e.html, actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failed:\n\n{}",
        failures.len(),
        failures.join("\n")
    );
    let _ = run;
}
