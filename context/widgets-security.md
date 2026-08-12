# Widgets: what the sandbox does and does not protect

Written alongside the widget implementation (Phase 8). The short version: third-party
widgets are contained by the iframe sandbox, and that containment depends entirely on
one property — that a widget is never same-origin with the app.

## The rule everything rests on

`plugins/widget/widgetUrl.ts` refuses any widget URL that is:

- not `https:`
- same-origin with the app (`url.origin === window.location.origin`)
- on a localhost/private/`.local` host
- carrying credentials in the URL

The same-origin rejection is the important one. A widget iframe carries
`allow-scripts allow-same-origin`. For a **cross-origin** widget that is fine: "same
origin" means the widget's own origin, so it gets its own storage and cannot reach
ours. For a **same-origin** widget it is the documented sandbox escape — the frame can
reach `window.parent`, strip the sandbox attribute, reload itself unsandboxed and read
the Matrix access token and megolm key store out of localStorage/IndexedDB.

Widget URLs come from room state, so anyone who can send state in a room picks them.
Without this check, adding a widget to a room would be account takeover for everyone who
opened it.

Verified against both deployment shapes, because the checks mask each other:

- Desktop shell (`http://localhost:44548`): same-origin URLs are caught by the scheme
  and private-host rules before the same-origin rule is reached.
- Hosted web (`https://prinny.app`): the same-origin rule is the one that fires, and it
  handles port normalisation (`https://prinny.app:443` is the same origin).

A subdomain such as `https://evil.prinny.app` is allowed. That is correct — it is a
different origin and cannot touch our storage — but it is worth knowing if the app ever
starts putting auth material in domain-scoped cookies. It does not today.

## Element Call is different, deliberately

`plugins/call/CallEmbed.ts` loads the Element Call bundle **from our own origin**, so
everything above does not hold for it and its capability allowlist is advisory only.
That is an accepted, documented risk: the bundle is treated as pinned first-party code.
The comment at `CallEmbed.getIframe` explains why neither sandbox token can be dropped
and what the real fix (a separate origin) would require.

This is why `CallWidgetDriver` and `GenericWidgetDriver` are separate subclasses of
`BaseWidgetDriver` with different capability policies, rather than one driver with a
flag. The two have genuinely different threat models and should not be able to drift
into each other by accident.

## What a third-party widget gets

- **Capabilities**: only what the user ticked in the consent prompt, stored per
  `widgetId|url` in `im.vector.setting.allowed_widgets` (the same account-data key
  Element uses, so grants interoperate). Changing a widget's URL invalidates its grants
  and re-prompts, so a widget cannot inherit permissions by taking over an existing id.
  Denials are remembered as denials — a prompt that reappears until you say yes trains
  people to say yes.
- **No capture**: the iframe is created with `allow=''`. No microphone, camera or
  display-capture, ever. On Android this is belt-and-braces: `MainActivity`'s
  `ALLOWED_CAPTURE_ORIGINS` already denies capture to any origin that is not the app's
  own.
- **No OpenID**: `GenericWidgetDriver.askOpenID` always returns `Blocked`. OpenID is how
  a widget turns an anonymous frame into an authenticated session as you on a
  third-party service, and there is no honest way to put that in a checklist next to
  "read messages".
- **No referrer**: `referrerPolicy="no-referrer"`, so the widget host does not learn
  which page framed it.

## CSP

`frame-src` in `tauri.conf.json` was a small allowlist of embed hosts (YouTube, Bandcamp,
Piped, soundcloak). Arbitrary widgets cannot work under that, and the failure mode is a
silently blank iframe, so it was widened to `'self' https:`.

The trade-off, stated plainly: this is weaker than the old allowlist. It still blocks
framing over `http:`, `data:` and `blob:`, and it does not affect `script-src` or
`connect-src` — an injection still cannot load or reach anything new. What it gives up
is "the app may only ever frame these five hosts". Since the product now deliberately
frames arbitrary user-chosen hosts, that property was already gone the moment widgets
shipped; the CSP now says so honestly rather than blocking the feature at runtime.

## Not implemented

- **Integration manager (scalar)**: no client. Widgets are added by URL. A scalar
  integration manager is an authenticated third-party service that provisions widgets and
  bots on your behalf; wiring one in means handing it an OpenID token, which contradicts
  the OpenID decision above. Revisit deliberately, not by accident.
- **Widget layout/pinning**: widgets open from the room menu rather than being pinned
  into the room view.
