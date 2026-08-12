# Telegram-style bot support

Prinny renders `app.prinny.bot.*` — bot command menus, inline keyboards with
callback queries, reply keyboards and force-reply prompts.

The protocol is specified in **`spec/app.prinny.bot.md`** in
[coffeegrind123/prinny-bot](https://github.com/coffeegrind123/prinny-bot), which
is also the reference bot framework. Read that first; this file only covers the
client side.

## Why a new schema

Matrix has no equivalent of Telegram's `setMyCommands`, inline keyboards or
callback queries. The only prior art is [MSC1485 "Hint buttons in
messages"](https://github.com/matrix-org/matrix-spec-proposals/issues/3812) — an
issue linking a Google Doc and an abandoned riot-web PR, with no schema and no
implementations. `mautrix-telegram` does not forward Telegram's `reply_markup`
into Matrix either, so bridged Telegram bots arrive with their keyboards
stripped.

## Vendored protocol module

`src/types/matrix/bot/` is a **verbatim copy** of `src/protocol/` from the
`@prinny/bot` package — constants, types, and the sanitiser. Only the `.js`
import extensions are dropped, for cinny's bundler resolution.

It is duplicated rather than depended on so the client needs no dependency on
the bot framework: a client must be able to render these events without being
able to send them.

**Keep it byte-identical.** `validate.ts` is the boundary between an arbitrary
room member and the renderer; a local "improvement" that diverges from the bot
side is how a keyboard starts rendering differently from the way it was sent.

## What was added

| File | Role |
|---|---|
| `src/types/matrix/bot/` | Vendored protocol: constants, types, sanitiser |
| `src/app/hooks/useBotInfo.ts` | Bots advertising in a room; `useIsBot` for the badge |
| `src/app/hooks/useBotCommands.ts` | Bot commands merged for the `/` menu, with `@bot` disambiguation |
| `src/app/hooks/useBotCallback.ts` | Sends a press, waits for the answer, times out at 15s |
| `src/app/hooks/useBotReplyKeyboard.ts` | The composer state a bot has asked for, derived from the timeline |
| `src/app/components/message/content/BotKeyboard.tsx` | Inline keyboard under a message |
| `src/app/features/room/BotReplyKeyboard.tsx` | Quick-reply bar above the composer |
| `src/app/features/room/BotMenuButton.tsx` | Telegram's chat menu button, next to the composer |
| `src/app/components/UrlConfirmDialog.tsx` | The one confirmation shown before any bot-supplied URL |
| `src/app/components/BotBadge.tsx` | The BOT tag |
| `src/app/components/BotStartLinkHandler.tsx` | `prinny.app/bot/{mxid}?start=…` deep links |
| `src/app/plugins/bot-deeplink.ts` | Deep link parsing |
| `src/app/utils/bot.ts` | Strips the fallback listing when real buttons render |

Modified: `RoomTimeline.tsx` (renders the keyboard, substitutes the clean body),
`RoomInput.tsx` (quick-reply bar, force-reply, unknown-command fix),
`CommandAutocomplete.tsx` (bot commands in the menu), `Message.tsx` and
`MembersDrawer.tsx` (badge), `settings.ts` + `General.tsx` (two toggles),
`types/matrix/room.ts` (event type enums).

## The fallback, and why the body gets swapped

Every keyboard message carries its buttons twice: as
`app.prinny.bot.reply_markup`, and as a numbered listing appended to `body` so
clients without button support still show something actionable. The sender puts
the un-annotated text in `app.prinny.bot.plain_body`.

So when we draw buttons we render `plain_body` (`botDisplayContent` in
`utils/bot.ts`) and the user never sees `[1] Deploy / [2] Cancel`. When we do
not — the setting is off, or the markup failed sanitisation — `body` is left
alone, because then the listing is exactly what the user needs.

Both decisions are driven by the same `renderBotKeyboards` read in
`RoomTimeline`, so they cannot disagree and leave a stray listing on screen.

## The menu button opens the autocomplete

`BotMenuButton` does not open a menu of its own. It types `/` into the composer
and lets the existing command autocomplete take over, because that list already
shows exactly what Telegram's menu shows — names, descriptions, and usage hints
besides. A second widget would mean two pieces of UI that have to agree about
the same data.

It inserts a leading space when the cursor sits at the end of a word: the
autocomplete keys off the word before the cursor, so a `/` glued to `hello`
would not read as a command query and the menu would silently fail to open.

The button only renders when a bot in the room published commands or asked for
a menu button. A `menu_button` of `{ type: 'url' }` turns it into a link button
instead, behind the same confirmation as any other bot-supplied URL.

## Two traps worth knowing

**RoomInput used to swallow every unknown `/command`.** The final branch of the
command chain in `submit()` resets the editor and returns whether or not it
found a handler — so a bot command was silently eaten and never sent, with
nothing to explain why. `isBuiltInCommand` now gates that branch, and anything
this client does not implement keeps its leading `/` and goes out as an ordinary
message. That is also what Telegram does: a bot command is just text the bot
parses.

**Reply keyboards are derived, not stored.** `useBotReplyKeyboard` scans the
live timeline backwards for the most recent reply-keyboard-family markup. The
timeline already syncs across devices and survives a reload, so deriving costs
nothing and cannot drift from what the bot actually said. Writing account data
per keyboard message would have added a multi-client write race for no gain.
Only the user's own dismissal is local, and only for the session.

## Client obligations

These are in the spec, and the reasons matter more than the rules:

- Markup renders only from the event's own sender. `getLatestEdit` already
  discards edits from anyone else, so an edit cannot introduce a keyboard.
- Labels are text, never markup. The sanitiser strips control characters **and
  Unicode bidi overrides** — a bare U+202E in a label makes "Cancel" and
  "Deploy" render in swapped positions.
- URL buttons confirm first, showing the real host. The label is whatever the
  sender wrote; the host is the one part they cannot misrepresent.
- Schemes are allowlisted to `https:`, `http:`, `matrix:`.
- A button carrying two actions renders **disabled**, not "the first one wins" —
  guessing is how a `url` button gets clicked as though it were a harmless
  `callback_data` one.
- Presses are debounced, and confined to the originating room.
- `Settings → General → Show Bot Buttons` turns the whole thing off.

## Deep links

Two forms, both accepted:

```
https://prinny.app/bot/{mxid}?start={payload}
prinny://bot/{mxid}?start={payload}
```

The https one is shareable and handles in-app clicks and the web app. The
`prinny:` scheme is what actually reaches an installed client from outside —
an https link opened elsewhere goes to the browser, and catching it would need
Android App Links plus a hosted `assetlinks.json`. It is registered in
`tauri.conf.json` (`deep-link.desktop.schemes`) and in `AndroidManifest.xml`
alongside the existing `matrix:` filter.

Following one opens or creates a DM with that account and sends
`/start {payload}`. Both are done in the user's name, so `BotStartLinkHandler`
confirms first, showing the account and the exact message.
Payloads are restricted to Telegram's `[A-Za-z0-9_-]{1,64}`, and the MXID is
decoded exactly once so `%2540` cannot smuggle an `@` past the check.

It hooks the same two paths as `MatrixLinkHandler`: capture-phase clicks in the
app, and `onOpenUrl` from the Tauri deep-link plugin for links opened from
outside.
