// Renders Markdown samples with Comark to build tests/fixtures/comark.json.
//
// Usage (in a folder with `comark` and `@comark/html` installed):
//   node scripts/comark_fixtures.mjs > tests/fixtures/comark.json
import { renderHtml } from '@comark/html'
import footnotes from 'comark/plugins/footnotes'

const headings = [
  'Hello World', 'Hello World', 'hello-world-1', 'Hello World',
  'Hello *World* `x`', 'C++ / C#', 'snake_case 2nd', '日本語 テキスト',
  'Emoji 😀 rocks', '--lead--', 'Ünïcödé Ça', '42', 'a---b', 'Déjà vu‼',
  'x²', 'Ⅻ roman', '![img](a.png) text', '`code`', '---',
  '[Link](http://x) **bold**', 'Ça va ? Oui !', 'under_score__x', 'Ölçü 3.5',
]

const cases = [
  { name: 'heading ids', markdown: headings.map((h) => `## ${h}`).join('\n\n') + '\n' },
  { name: 'setext heading ids', markdown: 'Title\n=====\n\nTitle\n-----\n' },
  {
    name: 'footnotes',
    footnotes: true,
    markdown: 'Text[^a] and[^b].\n\n[^a]: First note\n[^b]: Second note\n\n[^c]: unused\n',
  },
  {
    name: 'footnote labels',
    footnotes: true,
    markdown: 'One[^note-1] two[^x_y] three[^é].\n\n[^note-1]: A\n[^x_y]: B\n[^é]: C\n',
  },
  { name: 'no footnotes', footnotes: true, markdown: 'Plain text.\n' },
]

const out = []
for (const c of cases) {
  const html = await renderHtml(c.markdown, { plugins: c.footnotes ? [footnotes()] : [] })
  out.push({ ...c, footnotes: !!c.footnotes, html })
}
console.log(JSON.stringify(out, null, 1))
