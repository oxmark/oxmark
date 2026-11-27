# Test fixtures

- `commonmark-0.31.2.json`: every example of the
  [CommonMark Spec 0.31.2](https://spec.commonmark.org/0.31.2/), by John MacFarlane,
  licensed under [CC-BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).
- `gfm-0.29-extensions.json`: the extension examples (tables, task lists,
  strikethrough, autolinks, tag filter) of the
  [GitHub Flavored Markdown Spec 0.29](https://github.github.com/gfm/), licensed under
  [CC-BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).

Both were extracted from the `spec.txt` sources with `scripts/extract_spec.py`.
- `comark.json`: HTML rendered by [Comark](https://comark.dev) 0.9.2 (MIT) for
  heading ids and footnotes, generated with `scripts/comark_fixtures.mjs`.
