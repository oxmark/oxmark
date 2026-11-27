//! Heading ids, computed like Comark does.

use std::collections::{HashMap, HashSet};

use crate::ast::Inline;
use crate::tables::{MARK, SLUG};
use crate::text::in_ranges;

/// Turns heading text into an id: lowercase, whitespace runs become `-`,
/// only letters, marks, decimal and letter numbers, `_` and `-` are kept,
/// and an id starting with a digit gets a `_` prefix.
pub(crate) fn slugify(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_space = false;
    for c in lower.chars() {
        if c.is_whitespace() {
            if !in_space && !out.ends_with('-') {
                out.push('-');
            }
            in_space = true;
            continue;
        }
        in_space = false;
        let keep = if c.is_ascii() {
            c.is_ascii_alphanumeric() || c == '_' || c == '-'
        } else {
            in_ranges(SLUG, c)
        };
        // Collapse runs of hyphens.
        if keep && !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    let start = out
        .char_indices()
        .find(|&(_, c)| !(c == '-' || (!c.is_ascii() && in_ranges(MARK, c))))
        .map_or(out.len(), |(i, _)| i);
    let mut slug = out[start..].trim_end_matches('-').to_owned();
    if slug.starts_with(|c: char| c.is_ascii_digit()) {
        slug.insert(0, '_');
    }
    slug
}

/// Hands out unique heading ids: repeated slugs get `-1`, `-2`, ...
#[derive(Default)]
pub(crate) struct HeadingIds {
    counts: HashMap<String, usize>,
    used: HashSet<String>,
}

impl HeadingIds {
    pub(crate) fn next(&mut self, inlines: &[Inline<'_>]) -> Option<String> {
        let mut text = String::new();
        plain_text(inlines, &mut text);
        let slug = slugify(&text);
        if slug.is_empty() {
            return None;
        }
        let mut count = self.counts.get(&slug).copied().unwrap_or(0);
        let mut id = if count == 0 {
            slug.clone()
        } else {
            format!("{slug}-{count}")
        };
        while self.used.contains(&id) {
            count += 1;
            id = format!("{slug}-{count}");
        }
        self.counts.insert(slug, count + 1);
        self.used.insert(id.clone());
        Some(id)
    }
}

/// The text of inlines, without images, raw HTML or footnote references.
pub(crate) fn plain_text(inlines: &[Inline<'_>], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) => out.push_str(t),
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
            Inline::Emphasis(c) | Inline::Strong(c) | Inline::Strikethrough(c) => {
                plain_text(c, out)
            }
            Inline::Link(l) => plain_text(&l.children, out),
            Inline::Image(_) | Inline::Html(_) | Inline::FootnoteReference(_) => {}
        }
    }
}
