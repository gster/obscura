> 本页示例是现有 CDP 用法参考，不是完整客户端兼容承诺。实际核验版本、结果与缺口见 [SUMMARY](SUMMARY.md)。
> 启动命令必须显式选择 persona；示例使用 `windows_chrome145`。

Obscura speaks the Chrome DevTools Protocol over WebSocket. The supported
client path is official Playwright Python through `connect_over_cdp`.

## Start the server

```bash
obscura --persona windows_chrome145 serve --port 9222
```

```
obscura listening on ws://127.0.0.1:9222
```

## Playwright

```bash
python -m pip install playwright==1.60.0
```

```python
import asyncio
from playwright.async_api import async_playwright

async def main():
    async with async_playwright() as pw:
        browser = await pw.chromium.connect_over_cdp(
            "ws://127.0.0.1:9222/devtools/browser"
        )
        context = browser.contexts[0]
        page = await context.new_page()
        await page.goto("https://example.com")
        print(await page.title())
        await browser.close()

asyncio.run(main())
```

Use `connect_over_cdp`, not `connect`. Playwright's `connect` speaks
Playwright's own protocol, which Obscura does not implement. The qualified
client version is pinned in [SUMMARY](SUMMARY.md).

If CDP Bearer authentication is configured, use the official Python client's
`headers` option:

```python
browser = await pw.chromium.connect_over_cdp(
    "http://127.0.0.1:9222",
    headers={"Authorization": f"Bearer {token}"},
)
```

## `waitUntil`

Specify the client wait condition explicitly. Playwright defaults to `load` and
uses `networkidle`. Obscura's CLI wait levels are a separate interface.

## Compatibility boundary

The handlers cover navigation, evaluation, DOM/input, networking, cookies and render output, but method presence does not certify every parameter or event contract. Current utility-world IDs do not create independent globals. The first observed Playwright 1.60 method slice is recorded in [`automation-cdp-profile.json`](../tools/unblocked/automation-cdp-profile.json); unlisted methods and unvalidated parameter shapes are not qualified. Use the fixed-client results and backlog in [SUMMARY](SUMMARY.md) and [TODO](TODO.md).

Puppeteer compatibility is deprecated. It is not a product target or release
gate, and the compatibility profile is no longer expanded or maintained for
Puppeteer. Historical Puppeteer initializer entries may remain until their
removal is shown not to affect Playwright or shared raw CDP behavior.

## Capture example

```python
await page.set_viewport_size({"width": 1440, "height": 1000})
await page.screenshot(path="viewport.png")
await page.screenshot(path="full-page.png", full_page=True)
await page.pdf(path="page.pdf", format="A4", print_background=True)
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
