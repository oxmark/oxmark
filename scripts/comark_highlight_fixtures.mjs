// Renders code blocks and Mermaid fences with Comark's highlight and mermaid
// plugins, to build tests/fixtures/comark-highlight.json.
//
// Usage (in a folder with `comark`, `@comark/html` and `shiki` installed):
//   node scripts/comark_highlight_fixtures.mjs > tests/fixtures/comark-highlight.json
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { renderHtml } from '@comark/html'
import highlight from 'comark/plugins/highlight'
import mermaid from 'comark/plugins/mermaid'

const fixtures = join(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures/shiki')
const load = (path) => JSON.parse(readFileSync(join(fixtures, path), 'utf8'))
const theme = (name) => load(`themes/${name}.json`)
const languages = ['css', 'html', 'javascript', 'json', 'rust', 'typescript'].map((l) =>
  load(`langs/${l}.json`),
)

const ts = 'interface User {\n  name: string // the name\n  age?: number\n}\n\nconst user: User = { name: "Ada" }\n'
const rust = '/// Docs.\nfn main() {\n    let s = "a < b && c";\n    println!("{s}");\n}\n'

const cases = [
  { name: 'typescript', markdown: '```ts\n' + ts + '```\n' },
  { name: 'rust', markdown: '```rust\n' + rust + '```\n' },
  { name: 'json', markdown: '```json\n{"a": [1, true, null], "b": "x"}\n```\n' },
  { name: 'html with embedded css and js', markdown: '```html\n<style>a { color: red }</style>\n<script>let a = 1 < 2</script>\n<p class="x">Hi &amp; bye</p>\n```\n' },
  { name: 'html with embedded js', markdown: '```html\n<script>let a = 1 < 2</script>\n<p class="x">Hi & bye</p>\n<style>\n  .a { color: red }\n</style>\n```\n' },
  { name: 'filename, highlights and meta', markdown: '```ts [src/user.ts] {1,3-4} meta=1\n' + ts + '```\n' },
  { name: 'no language', markdown: '```\nplain <b> & text\n  indented\n```\n' },
  { name: 'text language', markdown: '```text\nplain <b>\n```\n' },
  { name: 'unknown language', markdown: '```nope\nfoo <bar>\n```\n' },
  { name: 'empty block', markdown: '```ts\n```\n' },
  { name: 'blank lines', markdown: '```ts\nconst a = 1\n\n\nconst b = 2\n```\n' },
  { name: 'indented code', markdown: '    let x = 1;\n' },
  { name: 'alias', markdown: '```rs\nfn x() {}\n```\n' },
  { name: 'tabs and whitespace', markdown: '```ts\n\tif (a) {\n\t\treturn   b\n\t}  \n```\n' },
  { name: 'nested in list and quote', markdown: '- item\n\n  ```ts\n  let a = 1\n  ```\n\n> ```json\n> [1]\n> ```\n' },
  { name: 'pre component with class', markdown: '::pre{.foo #bar}\n```ts\nlet a = 1\n```\n::\n' },
  { name: 'inline code', markdown: 'Run `const a = 1`{lang="ts"} or `x`{language=rust} but not `y` or `z`{lang=nope}.\n' },
  { name: 'single theme', themes: { light: 'github-light' }, markdown: '```ts\n' + ts + '```\n\nAnd `let a`{lang=ts}.\n' },
  { name: 'pre styles', preStyles: true, markdown: '```ts\nlet a = 1\n```\n' },
  { name: 'pre styles single theme', preStyles: true, themes: { light: 'github-light' }, markdown: '```ts\nlet a = 1\n```\n' },
  { name: 'italic theme', themes: { light: 'one-dark-pro' }, markdown: '```ts\n// comment\nthis.a = 1 // x\n```\n' },
  { name: 'inline code off', inlineCode: false, markdown: 'Run `let a`{lang=ts}.\n' },
  { name: 'mermaid', mermaid: true, markdown: '```mermaid\nflowchart LR\n  A[Start] --> B{Ok?}\n```\n' },
  { name: 'mermaid props', mermaid: true, markdown: '```mermaid {.big #chart title="My chart" height=200px}\ngraph TD\n  A --> B\n```\n' },
  { name: 'mermaid theme', mermaid: { theme: 'forest', themeDark: 'dark' }, markdown: '```mermaid\ngraph TD\n  A["a < b & c"] --> B\n```\n' },
  { name: 'mermaid with highlight', mermaid: true, highlight: true, markdown: '```mermaid\ngraph TD\n  A --> B\n```\n\n```ts\nlet a\n```\n' },
  { name: 'mermaid prefix', mermaid: true, markdown: '```mermaidjs\ngraph TD\n```\n\n    indented mermaid\n' },
  { name: 'mermaid in component', mermaid: true, markdown: '::note\n```mermaid\ngraph TD\n```\n::\n' },
]

const out = []
for (const c of cases) {
  const plugins = []
  const highlighting = c.highlight ?? !c.mermaid
  const themes = c.themes ?? { light: 'github-light', dark: 'github-dark' }
  if (highlighting) {
    plugins.push(
      highlight({
        themes: Object.fromEntries(Object.entries(themes).map(([k, v]) => [k, theme(v)])),
        languages,
        registerDefaultLanguages: false,
        registerDefaultThemes: false,
        preStyles: c.preStyles ?? false,
        inlineCode: c.inlineCode ?? true,
      }),
    )
  }
  if (c.mermaid) plugins.push(mermaid(c.mermaid === true ? {} : c.mermaid))
  const html = await renderHtml(c.markdown, { plugins })
  out.push({
    name: c.name,
    markdown: c.markdown,
    highlight: highlighting ? { themes, preStyles: !!c.preStyles, inlineCode: c.inlineCode ?? true } : null,
    mermaid: c.mermaid ? (c.mermaid === true ? {} : c.mermaid) : null,
    html,
  })
}
console.log(JSON.stringify(out, null, 1))
