# ZG checkout example

This is a standalone acceptance example for the Obscura SDK, based on the old
ZG Playwright workflow. It supports one adult, one direct outbound flight, no
selected seat, and no extra baggage. It does not integrate with an airline
scheduler, send business notifications, or modify the old ZG project.

## Offline end-to-end test

```bash
PYTHONPATH=bindings/python/src python examples/zg/run.py \
  --binary /absolute/autopilot-browser-runtime
```

The local fixture serves dynamic pages and real HTTP requests. The shared flow
logs in, chooses a flight, fills passenger details, verifies the summary, submits
one simulated payment, and reads a simulated PNR from the response. All fixture
passenger data is fictional. `SIMULATED_BOOKED` is not an airline booking.

`--scenario` also accepts `no_flight`, `wrong_flight`, `price_change`, and
`payment_failure`. The first three must send no payment request; payment failure
must send exactly one and return failure. Automated cases live in the SDK tests.

## Website verification, before payment

```bash
PYTHONPATH=bindings/python/src python examples/zg/run.py \
  --binary /absolute/autopilot-browser-runtime --live \
  --config /private/zg-test.json \
  --credentials /Users/zg/work/autopilot/routers/zg/config.py
```

The credentials loader parses only literal `ZG_ACCOUNT` and `ZG_PASSWORD`
assignments (including tuple unpacking) with Python AST; it never imports that module. The optional `--proxy`
is forwarded explicitly to the runtime. The existing runtime accepts an HTTP
proxy endpoint without embedded authentication; it does not obtain a proxy from
the old production service.

The private JSON uses the shape shown in `fixture.CONFIG`, with actual authorized
passenger details and a chosen future flight. `origin` and `destination` are the
website's displayed city labels. Amount and currency are the expected total;
a mismatch stops checkout. The live run always uses the ZIPAIR website URL.
`allowed_origins` defaults to the website, BFF API, image host and the observed ZIPAIR OAuth chain (`consols`, `hydra`, `idp-app`, `idp-approval`) plus its `zipair-idp.s3-ap-northeast-1.amazonaws.com` script host. It may
list additional legitimate website resource origins.

The `review` object supplies `flight_selector`, `passenger_selector` and
`amount_selector` for the website's actual summary. The default `data-testid`
selectors belong to the synthetic fixture and are not established website
selectors. They must be verified against the live checkout DOM before website
acceptance can pass. Missing travel data, unavailable login, unsupported website
controls, or failed summary matching leave website acceptance blocked.

The shared flow stops after verifying the displayed summary and seeing the
credit-card choice. Live mode returns `READY_FOR_PAYMENT` before opening or
filling card fields; only the loopback fixture branch can invoke simulated
payment. There is no real payment command. Human challenges, additional regional
forms, and site changes must be reported, not swallowed or bypassed.

Outputs contain stage names, elapsed times and sanitized error codes. Do not
commit private configuration, credentials, screenshots, or network payloads.

The runner uses the SDK's `macos_chrome152` identity. See the SDK README for the
pinned headed-Chrome reference, primp transport, and remaining fingerprint limits.

## Current live evidence

A real SDK session has reached the payment-information page through the guest
checkout flow. Cookie consent was dismissed on the homepage and remained absent
through checkout. Passenger and receipt submissions were compared with the
headed Chrome HAR; screenshots and private request evidence are kept outside the
repository. No card data was entered and the payment-entry button was not clicked.

This is separate from an unattended run of the shared example above: its login
callback and website selectors still require follow-up. The live evidence does
not establish complete fingerprint parity or completion of all release gates.
