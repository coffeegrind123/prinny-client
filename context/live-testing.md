# Live testing against a real homeserver

Run the actual web client, logged in to a throwaway Matrix server, in Chrome,
and drive it with real keystrokes and clicks. Use this before a fix, not after
it. Reasoning about slate, React and the browser here has repeatedly produced
confident explanations that were wrong.

The "text types in backwards" bug had five fixes, each with a detailed and
plausible mechanism, and it survived all of them. With this setup the real
cause turned up in one session: picking an emoji from the board with the
keyboard left the caret at the start of the message. The same session found
that bare-letter message shortcuts ate the first letter typed. Neither was on
anyone's list of suspects.

```
 scripts/live-test/homeserver.sh ──► Conduit in Docker (127.0.0.1:8228)
                                         ▲
 scripts/live-test/serve.sh ──► vite preview of a production build (127.0.0.1:5199)
                                         ▲  the page talks to the homeserver
 Chrome (browser MCP) ◄── scripts/live-test/cdp.mjs  (trusted input, state readout)
```

## Quick start

From `prinny-desktop/`:

```bash
export PRINNY_LIVE_DIR=/tmp/prinny-live-test     # use your scratchpad in a Claude session

scripts/live-test/homeserver.sh up               # Conduit in Docker, waits until it answers
scripts/live-test/homeserver.sh seed             # alice + bob, room "Live Test", one message
scripts/live-test/serve.sh build                 # 4–15 min; see "Gotchas"
scripts/live-test/serve.sh start                 # http://127.0.0.1:5199/
```

Start the browser with the browser MCP: `start_browser` with `low_memory: true`
and `window_size: "1400x900"`. Then use `navigate` to open
`http://127.0.0.1:5199/`, and log in with:

```bash
node scripts/live-test/cdp.mjs login "$PRINNY_LIVE_DIR/alice.json"
```

Use the MCP's `screenshot` to look at the page. Use `cdp.mjs` for all input.

Teardown:

```bash
scripts/live-test/serve.sh stop
scripts/live-test/homeserver.sh down             # deletes the container and all its data
```

(also `stop_browser`)

## The one rule: input must be trusted

The browser MCP's input tools are not a keyboard:

| tool | what it really does | consequence |
|---|---|---|
| `press_key` | dispatches an untrusted synthetic `KeyboardEvent` | `isTrusted: false`, and the browser inserts nothing |
| `type_text`, `human_type` | inserts text via CDP with **no keydown at all** | skips the keydown path, which is where every composer bug has lived |

A test driven by these tools never exercises that path, so it can pass while
the bug is still there.

`cdp.mjs` sends `Input.dispatchKeyEvent` and `Input.dispatchMouseEvent`, the
same events a real keyboard and mouse produce:

```bash
node scripts/live-test/cdp.mjs click '[data-slate-editor]'
node scripts/live-test/cdp.mjs type 'hello world'          # one keyDown/keyUp per character
node scripts/live-test/cdp.mjs key Enter
node scripts/live-test/cdp.mjs hover '[data-message-id]'
node scripts/live-test/cdp.mjs composer                    # the diagnostic, see below
node scripts/live-test/cdp.mjs eval 'document.title'
```

The Chrome debugging port is read from the running Chrome's command line. Set
`CDP_PORT` if more than one Chrome is running.

### `composer` — read both carets, not just the text

```json
{
  "text": "hi 😄abcdef",
  "focus": "composer",
  "domCaret": "text:\"﻿\"@1",
  "model": { "selection": { "anchor": { "path": [0, 2], "offset": 0 }, … },
             "doc": [["hi ", "<emoticon>", ""]] }
}
```

The browser's caret (`domCaret`) and Slate's selection (`model`) can disagree,
and which one is wrong is the diagnosis. The model is read from the `editor`
prop on the `Editable`'s React fiber, so it needs no debug hooks in the app.
Run `composer` between steps, not only at the end. The emoji bug showed up as
`domCaret …@0` immediately after the board closed, before a single character had
been typed.

## Seeding the room

Seed events with the client-server API. It's faster and more exact than doing
it through the UI. The tokens are in `$PRINNY_LIVE_DIR/{alice,bob}.json`:

```bash
HS=$(jq -r .hs_base_url "$PRINNY_LIVE_DIR/alice.json")
A=$(jq -r .access_token "$PRINNY_LIVE_DIR/alice.json")
R=$(cat "$PRINNY_LIVE_DIR/room.txt")

# a message — the transaction id MUST be unique (see Gotchas)
curl -s -X PUT "$HS/_matrix/client/v3/rooms/$R/send/m.room.message/t$(date +%s%N)" \
  -H "Authorization: Bearer $A" -H 'Content-Type: application/json' \
  -d '{"msgtype":"m.text","body":"hi"}'

# an attachment: upload, then send m.audio / m.file / m.image pointing at it
MXC=$(curl -s -X POST "$HS/_matrix/media/v3/upload?filename=beat.wav" \
  -H "Authorization: Bearer $A" -H 'Content-Type: audio/wav' --data-binary @beat.wav | jq -r .content_uri)
```

Register more users with `homeserver.sh` as a model (`m.login.dummy`
registration), then invite, join and leave them to produce membership churn.
Conduit serves **authenticated media**, so attachments take the same blob path
as on any modern homeserver. That path is where the unnamed-download bug lived.

After seeding, reload the page (`cdp.mjs reload`) so the client picks up the
events straight away.

## Recipes that found real bugs

**Emoji picked by keyboard lands at the start** (fixed in `focusEditorDOM`,
`cinny/src/app/components/editor/utils.ts`):

```bash
C="node scripts/live-test/cdp.mjs"
# the emoji button has no stable selector; tag it (the last aria-pressed button
# in the composer's button row)
$C eval "(() => { let n = document.querySelector('[data-slate-editor]');
  while (n && n.querySelectorAll('button[aria-pressed]').length < 2) n = n.parentElement;
  const b = [...n.querySelectorAll('button[aria-pressed]')];
  b[b.length - 1].setAttribute('data-t', 'emoji'); return b.length; })()"
$C click '[data-slate-editor]'; $C type 'hi '
$C click '[data-t=emoji]'; sleep 0.7
$C type smile; sleep 0.3           # let the board's search catch up, or Enter picks nothing
$C key Enter; sleep 0.5
$C composer                        # was: domCaret "hi "@0 — caret at the start
$C type abcdef; $C composer        # was: "abcdefhi 😄"
```

**First letter swallowed by a bare-letter shortcut** (fixed in
`MessageKeybinds.tsx`). Click a room in the sidebar so the pointer is off the
timeline and focus is outside the composer. Then `type foobar`: before the fix
the result was `oobar`, and the same happened for `e`, `p` and `r`. Always
include a control word that starts with an unbound letter (`bazqux`), so that
"everything fails" and "bound letters fail" can be told apart.

**A shortcut that should act.** Run `hover` on the row, blur the composer
(`eval 'document.activeElement.blur()'`), press the key, then check the effect
server-side. For example, read `m.room.pinned_events` over the API instead of
trusting what's on screen.

## Gotchas

| symptom | cause | do this |
|---|---|---|
| `vite` dev server: "Error during dependency optimization: Not implemented" | Vite 8's rolldown optimiser vs `@esbuild-plugins/node-globals-polyfill` | test a production build (`serve.sh`); there is no dev mode |
| build takes 15 min instead of 4 | memory pressure (another build, many sessions) | wait on the process's exit status, not a log grep; see the ENOMEM recipe in the global CLAUDE.md |
| second room's message never arrives | Conduit deduplicates transaction ids **per device across all rooms** | unique txn id per send (`t$(date +%s%N)`) |
| a click "does nothing", reproducibly | the "Connecting to …" banner appeared or disappeared and shifted the layout | `cdp.mjs click <css>` measures at the moment of the click; never reuse coordinates across steps |
| hovering a row does nothing | `react-aria`'s `useHover` needs the pointer to *enter* the element | `cdp.mjs hover` moves twice; check `el.matches(':hover')` if in doubt |
| Enter in the emoji board picks nothing | the search results had not updated yet | `sleep 0.3` between typing the query and Enter |
| `hover '[data-message-id]'` hovers nothing | the first match is scrolled out of view | target the row you mean (`:last-of-type`, or tag it by content) |
| a "message" row that is really a state event | `[data-message-id]` is on state-event rows too | select by content (`innerText.includes(…)`) when the test needs a message |
| homeserver unreachable from the page | inside the dev container, `localhost:8228` is the container itself | `homeserver.sh` uses `host.docker.internal` when `/.dockerenv` exists; override with `PRINNY_HS_URL` |
| test build ends up in the desktop app | building into `cinny/dist` | `serve.sh` builds into `$PRINNY_LIVE_DIR/dist` on purpose |
| `pkill -f "vite preview"` kills your own shell | the pattern matches the shell's own command line | `serve.sh stop`, which kills the server's own process group |
| `emoji` / other toolbar buttons have no stable selector | icon-only buttons with no label | tag them first with `eval`, e.g. walk up from `[data-slate-editor]` to the container holding the `button[aria-pressed]`s and set `data-t` on the one you want |

## What this setup cannot do

- **Desktop shell (Tauri) behaviour.** The Rust side is out of the loop:
  single-instance, notifications, the native media proxy, and WebView2's or
  WebKitGTK's own handling of downloads. Use the real app for those, over
  `claude-host-bridge` on Windows.
- **Android WebView and IMEs.** Chrome desktop's input pipeline is not the
  Android input manager that slate special-cases.
- **Federation.** It is off; there is one server. E2EE has not been exercised
  with this setup yet.
