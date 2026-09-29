# Intercept and modify requests

> The qualified client path is official Playwright Python. `Fetch.enable`
> coverage must still be tested per request type, frame or Worker, phase, and
> parameter combination; these examples do not establish universal coverage.

The snippets below run inside an `async def` with an existing Playwright
`page`; the complete connection scaffold is in
[Use with Playwright](Use-with-Playwright.md).

## Block by resource type

```python
async def block_assets(route):
    if route.request.resource_type in {"image", "media", "font"}:
        await route.abort()
    else:
        await route.continue_()

await page.route("**/*", block_assets)
```

## Block by URL pattern

```python
import re

async def block_tracking(route):
    await route.abort()

await page.route(
    re.compile(r"google-analytics\.com|doubleclick\.net"),
    block_tracking,
)
```

## Modify headers

```python
async def add_header(route):
    await route.continue_(headers={
        **route.request.headers,
        "X-Custom": "value",
    })

await page.route("**/*", add_header)
```

## Return a fake response

```python
import json

async def fulfill_flags(route):
    await route.fulfill(
        status=200,
        content_type="application/json",
        body=json.dumps({"newDashboard": True}),
    )

await page.route("**/api/feature-flags", fulfill_flags)
```

## Strip analytics in production scrapes

```python
BLOCK = {
    "google-analytics.com",
    "googletagmanager.com",
    "doubleclick.net",
    "facebook.net",
    "segment.io",
    "mixpanel.com",
    "hotjar.com",
}

async def block_analytics(route):
    if any(host in route.request.url for host in BLOCK):
        await route.abort()
    else:
        await route.continue_()

await page.route("**/*", block_analytics)
```

Every build also ships with an optional tracker blocklist; requests are allowed
by default. See
[Configure stealth and proxies](Configure-stealth-and-proxies.md).

## From the Rust library

The examples above drive interception over CDP from official Playwright Python.
Puppeteer is deprecated and is not a compatibility target. If you embed the
engine with the `obscura` crate, the same capability is a native API on `Page`:
`on_request` / `on_response` callbacks, an `enable_interception()` channel that
can block, mock, or rewrite requests, and `add_preload_script` to run code before
the page's own scripts. See
[Use as a Rust library](Use-as-a-Rust-library.md#intercept-requests).
