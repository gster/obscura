# ZG checkout to payment review

This example uses unmodified official Playwright Python over Obscura's CDP
server. It targets one adult, one direct outbound flight, Standard fare, and
no selected seat, package, or extra baggage. The intended stop is the payment
method page, before opening card fields or submitting payment.

## Local fixture

From the repository root:

```bash
uv run --project tools/unblocked --frozen --python 3.12 python examples/zg/run.py
```

The runner starts `target/release/obscura serve` with
`windows_chrome145`, connects with `chromium.connect_over_cdp`, and creates
a fresh browser context. The dynamic loopback fixture exercises login, search,
passenger entry, review, and the payment choice. A successful run returns
`READY_FOR_PAYMENT` and checks that the fixture received zero payment
requests. `--scenario no_flight`, `wrong_flight`, and `price_change` must
fail before payment.

## Live ZIPAIR run

For a KUL to NRT flight on 2026-11-11 with a Faker-generated Japanese test
passenger:

```bash
uv run --project tools/unblocked --frozen --python 3.12 --with Faker \
  python examples/zg/run.py --live --faker-jp --departure-date 2026-11-11
```

Live checkout uses a guest session with `macos_chrome153` and device scale 2.
It currently supports KUL to NRT and `USD - US$`. No login credentials are
required. Pass `--proxy http://...` for a configured network exit.

The generated passenger uses Faker's `ja_JP` romanized name, birth date,
phone number, and random passport-shaped value. The email ends in
`@example.invalid`. The flow selects the first available ZG Standard flight
on the requested date, then checks its flight number, date, and passenger at
booking review and the total at payment selection. A private JSON `--config`
can instead specify the itinerary and passenger fields from `fixture.CONFIG`,
optionally including an expected `flight_number`, `departure_time`, and
`amount`. Keep the origin, destination, and currency values shown above for
the live flow. Fixture configuration also accepts `review` selector overrides.

## Verification status

The local fixture reaches `READY_FOR_PAYMENT` without a payment request.
A computer-use Chrome Incognito control reached ZIPAIR payment selection for
this itinerary. Obscura reached the passenger page but stalled while loading;
another run encountered failed Nuxt resource requests and returned home.
Live passenger and later selectors remain unqualified until that blocker is
resolved. Fixture success does not establish live checkout completion.

The runner prints stage names, elapsed times, and sanitized error codes. Keep
private configuration and any diagnostic captures outside the repository.
An HTTP 403 or Cloudflare block on the landing page is reported as
`SITE_BLOCKED_AT_LANDING`; it is not a payment-stage result. A fresh anonymous
Chrome control may help qualify whether the same endpoint and network exit
are also blocked.
