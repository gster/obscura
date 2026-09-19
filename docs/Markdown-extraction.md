> 当前用法：MCP 与有用的 CLI 保留，用于自动化和 agent 接入。下列命令对应现有代码；目标是统一 persona、强制 stealth 和唯一 primp 出口，见 [TODO](TODO.md)，这些迁移尚未实现。

`--dump markdown` converts the rendered page to markdown.

```bash
obscura fetch https://example.com --dump markdown
```

## What gets converted

- Headings (`<h1>` through `<h6>`)
- Paragraphs, line breaks
- Bold, italic, code spans
- Links (with `href`)
- Images (with `src` and `alt`)
- Ordered and unordered lists
- Block quotes
- Code blocks (`<pre>`, `<code>`)
- Tables

## What gets stripped

- `<script>`, `<style>`, `<noscript>`
- Inline styles
- ARIA attributes

This conversion is not a tracker detector and does not prevent requests already made during navigation.

## Save to file

```bash
obscura fetch https://docs.example.com/page --dump markdown -o page.md
```

## For RAG / LLM context

```bash
obscura fetch https://docs.example.com/page --dump markdown --quiet
```

`--quiet` strips info logging so the output is just markdown.

## Wait for SPA content

For pages that render content client-side:

```bash
obscura fetch https://my-spa.example --wait-until load --dump markdown
```

## Narrow to a region

`--selector` restricts the conversion to a CSS selector:

```bash
obscura fetch https://example.com --selector "main" --dump markdown
obscura fetch https://example.com --selector "article.post" --dump markdown
```

Useful for skipping nav, sidebars, and footers.
