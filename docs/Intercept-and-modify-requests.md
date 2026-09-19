> 本页示例是现有 CDP 用法参考，不是完整客户端兼容承诺。实际核验版本、结果与缺口见 [SUMMARY](SUMMARY.md)。

CDP `Fetch.enable` provides request interception. Coverage must be tested per request type, frame/Worker, phase and parameter combination; this guide does not establish universal interception coverage.

## Block by resource type

Puppeteer:

```js
await page.setRequestInterception(true);

page.on('request', req => {
  if (['image', 'media', 'font'].includes(req.resourceType())) {
    req.abort();
  } else {
    req.continue();
  }
});
```

Playwright:

```js
await page.route('**/*', route => {
  if (['image', 'media', 'font'].includes(route.request().resourceType())) {
    route.abort();
  } else {
    route.continue();
  }
});
```

## Block by URL pattern

```js
// Puppeteer
page.on('request', req => {
  const url = req.url();
  if (url.includes('google-analytics.com') || url.includes('doubleclick.net')) {
    req.abort();
  } else {
    req.continue();
  }
});
```

```js
// Playwright
await page.route(/google-analytics\.com|doubleclick\.net/, route => route.abort());
```

## Modify headers

```js
// Puppeteer
page.on('request', req => {
  req.continue({
    headers: { ...req.headers(), 'X-Custom': 'value' },
  });
});
```

```js
// Playwright
await page.route('**/*', route => {
  route.continue({
    headers: { ...route.request().headers(), 'X-Custom': 'value' },
  });
});
```

## Return a fake response

```js
// Puppeteer
page.on('request', req => {
  if (req.url().endsWith('/api/feature-flags')) {
    req.respond({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ newDashboard: true }),
    });
  } else {
    req.continue();
  }
});
```

```js
// Playwright
await page.route('**/api/feature-flags', route => {
  route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ newDashboard: true }),
  });
});
```

## Strip analytics in production scrapes

```js
const BLOCK = [
  'google-analytics.com',
  'googletagmanager.com',
  'doubleclick.net',
  'facebook.net',
  'segment.io',
  'mixpanel.com',
  'hotjar.com',
];

page.on('request', req => {
  if (BLOCK.some(host => req.url().includes(host))) {
    req.abort();
  } else {
    req.continue();
  }
});
```

Built-in: `--stealth` ships with a tracker blocklist that handles most of these without per-script setup. See [Configure stealth and proxies](Configure-stealth-and-proxies.md).

## From the Rust library

The patterns above drive interception over CDP from Puppeteer or Playwright. If you embed the engine with the `obscura` crate, the same capability is a native API on `Page`: `on_request` / `on_response` callbacks, an `enable_interception()` channel that can block, mock, or rewrite requests, and `add_preload_script` to run code before the page's own scripts. See [Use as a Rust library](Use-as-a-Rust-library.md#intercept-requests).
