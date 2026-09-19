# Use with Playwright Python

> The qualified client path is unmodified official Playwright Python through
> `connect_over_cdp`. Method presence does not imply full Playwright or Chrome
> compatibility; see [SUMMARY](SUMMARY.md) and [TODO](TODO.md).

## Setup

```bash
obscura serve --port 9222
python -m pip install playwright==1.60.0
```

## Connect

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
        # Continue with the snippets below inside this function.

asyncio.run(main())
```

Use `connect_over_cdp`, not `connect`. Playwright's `connect` speaks
Playwright's own protocol, which Obscura does not implement.

The remaining snippets assume `page`, `context`, and `browser` are inside the
`main()` function above.

## Navigate and evaluate

```python
await page.goto("https://example.com", wait_until="load")
await page.goto("https://example.com", wait_until="networkidle")
title = await page.title()
items = await page.locator(".item").evaluate_all(
    "els => els.map(el => ({text: el.textContent, href: el.href}))"
)
```

Set `wait_until` explicitly for repeatable comparisons. Playwright uses
`networkidle`; Puppeteer's `networkidle0/2` values are outside the supported
client contract.

## Interact

```python
await page.get_by_label("Email").fill("alice@example.com")
await page.get_by_role("button", name="Submit").click()
await page.wait_for_selector("#dashboard")
await page.wait_for_function("window.appReady === true")
```

## Cookies

```python
await context.add_cookies([{
    "name": "session",
    "value": "abc123",
    "domain": "example.com",
    "path": "/",
}])
cookies = await context.cookies()
```

For session persistence across runs, see
[Persist cookies and storage](Persist-cookies-and-storage.md).

## Intercept requests

```python
async def route_request(route):
    if route.request.resource_type == "image":
        await route.abort()
    else:
        await route.continue_()

await page.route("**/*", route_request)
```

Coverage must be qualified per resource type, frame or Worker, request phase,
and parameter combination. See
[Intercept and modify requests](Intercept-and-modify-requests.md).

## Multiple pages

```python
page1 = await context.new_page()
page2 = await context.new_page()
await page1.goto("https://a.example.com")
await page2.goto("https://b.example.com")
```

Each page owns a V8 isolate. Pages on one CDP connection share its owner
thread, so synchronous work can delay that connection.

## Screenshots, scrolling, and PDF

```python
await page.set_viewport_size({"width": 1440, "height": 1000})
await page.screenshot(path="viewport.png")
await page.evaluate("window.scrollTo(0, 1200)")
await page.screenshot(path="scrolled.png")
await page.screenshot(path="full-page.png", full_page=True)
await page.pdf(path="page.pdf", format="A4", print_background=True)
```

A normal screenshot captures the live viewport and scroll position. Full-page
screenshots capture document space. PDF output is raster-backed.

## Screencasting

Playwright does not expose CDP screencasting as a page method. Attach a raw CDP
session, acknowledge every frame, and detach it when finished:

```python
import base64

client = await context.new_cdp_session(page)

async def on_frame(event):
    jpeg = base64.b64decode(event["data"])
    # Consume or forward jpeg here.
    await client.send("Page.screencastFrameAck", {
        "sessionId": event["sessionId"],
    })

client.on("Page.screencastFrame", on_frame)
await client.send("Page.startScreencast", {
    "format": "jpeg",
    "quality": 80,
    "maxWidth": 1280,
    "maxHeight": 720,
})
# Navigate, scroll, and interact.
await client.send("Page.stopScreencast")
await client.detach()
```

Frames are activity-driven page captures, not fixed-rate desktop video.

## Disconnect

```python
await browser.close()  # Leaves obscura serve running.
```

## Current limits

- Playwright video and tracing artifacts that require desktop capture are not implemented.
- BrowserContext storage-state save/restore remains limited; use `--storage-dir`.
- Service workers, native media, some Web APIs, long-tail CSS, and compositor behavior remain incomplete relative to Chromium.
- PDF text is not selectable/searchable and tagged PDF is not available.
