> 本页示例是现有 CDP 用法参考，不是完整客户端兼容承诺。实际核验版本、结果与缺口见 [SUMMARY](SUMMARY.md)。

Obscura speaks the Chrome DevTools Protocol over WebSocket. Puppeteer and
Playwright can connect to its CDP endpoint for the supported workflows below.

## Start the server

```bash
obscura serve --port 9222
```

```
obscura listening on ws://127.0.0.1:9222
```

## Puppeteer

```bash
npm install puppeteer-core
```

```js
const puppeteer = require('puppeteer-core');

const browser = await puppeteer.connect({
  browserWSEndpoint: 'ws://127.0.0.1:9222',
});

const page = await browser.newPage();
await page.goto('https://example.com');
console.log(await page.title()); // "Example Domain"

await browser.disconnect();
```

Use `puppeteer-core`, not `puppeteer`. The `puppeteer` package bundles a Chrome download.

## Playwright

```bash
npm install playwright
```

```js
const { chromium } = require('playwright');

const browser = await chromium.connectOverCDP('ws://127.0.0.1:9222');
const context = browser.contexts()[0] || await browser.newContext();
const page = await context.newPage();

await page.goto('https://example.com');
console.log(await page.title());

await browser.close();
```

Use `connectOverCDP`, not `connect`. Playwright's `connect` speaks Playwright's own protocol, which obscura does not implement.

## `waitUntil`

Specify the client wait condition explicitly. Both client navigation APIs default to `load`; Puppeteer accepts `networkidle0/2`, while Playwright uses `networkidle`. Obscura's CLI wait levels are a separate interface.

## Compatibility boundary

The handlers cover navigation, evaluation, DOM/input, networking, cookies and render output, but method presence does not certify every parameter or event contract. Current utility-world IDs do not create independent globals, and several domains acknowledge all methods without implementing them. Use the fixed-client results and backlog in [SUMMARY](SUMMARY.md) and [TODO](TODO.md).

The following capture example uses Puppeteer APIs; Playwright equivalents are in its separate guide.

## Capture example

```js
await page.setViewport({ width: 1440, height: 1000 });
await page.screenshot({ path: 'viewport.png' });
await page.screenshot({ path: 'full-page.png', fullPage: true });
await page.pdf({ path: 'page.pdf', format: 'A4', printBackground: true });
```

Rendering is included in official binaries and requires `--features render`
for source builds. The client-specific guides cover scrolling, raw CDP
screencasting, and current output limits.

## Current limits

Each page owns a V8 isolate. Pages on one CDP connection share its owner thread, so synchronous work can delay that connection; separate isolates do not imply arbitrary parallel execution.
- PDF output is raster-backed; text is not selectable and tagged PDF,
  headers/footers, outlines, and full CSS paged media are not implemented.
- Service workers, native media playback, some Web APIs, and long-tail CSS or
  compositor effects are still incomplete relative to Chromium.
