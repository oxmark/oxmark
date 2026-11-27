//! Reads Markdown on stdin and writes HTML on stdout.
//!
//! `cargo run --example md2html [--trusted] < README.md`

use std::io::{Read, Write};

fn main() {
    let trusted = std::env::args().any(|a| a == "--trusted");
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let html = if trusted {
        oxmark::HtmlOptions::trusted()
    } else {
        oxmark::HtmlOptions::default()
    };
    let out = oxmark::to_html_with(&input, &oxmark::Options::default(), &html);
    std::io::stdout().write_all(out.as_bytes()).unwrap();
}
