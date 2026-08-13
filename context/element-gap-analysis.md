# Element ↔ Prinny feature gap analysis

**Compared:** `element-hq/element-web` @ `7711207` (web app version 1.12.25, 2026-08-11, shallow
clone at `~/element-web`) against `coffeegrind123/prinny` @ `fcecd833` (version 4.11.10) as vendored
in `prinny-client` @ `199a0aa`, plus the Tauri shell in `prinny-client/src-tauri`.

**Method:** file/marker sweep of both trees (msgtype constants, MSC identifiers, component
directories, settings keys, slash-command tables, dialog inventories), then read of the relevant
implementations on both sides. Every "missing" line below was verified by grep returning nothing in
the cinny tree, not inferred from memory. Where a thing exists but is thinner than Element's, it is
listed as *partial* with what is actually there.

Element merged `matrix-react-sdk` into this repo, so everything lives under `apps/web/src`
(web/frontend) and `apps/desktop/src` (Electron shell). Paths below are relative to those.

---

## Summary — the gaps worth acting on

Ranked by (user-visible value ÷ effort), highest first.

| # | Gap | Effort | Notes |
|---|-----|--------|-------|
| 1 | **Voice messages (record + send)** | M | Full plan in the appendix. Nothing exists today — no recorder, no waveform, no MSC3245 keys |
| 2 | **Voice-message *rendering*** (waveform, duration, seek) | S | We already play `m.audio`, but ignore `org.matrix.msc1767.audio` waveform/duration, so Element-sent voice notes look like file attachments |
| 3 | **Forward message** | S | Element `components/views/dialogs/ForwardDialog.tsx`. We have no forward path at all |
| 4 | **Threads panel / thread list** | L | We can *start* a thread and reply into one, but there is no thread view, no per-thread unread, no Threads Activity Centre |
| 5 | **Polls** | M | `m.poll.start` absent entirely — we do not even render other clients' polls (falls through to `UnsupportedContent`) |
| 6 | **Pinned-message banner** | S | We have a pin list in the room menu; Element also shows a banner at the top of the timeline |
| 7 | **Ask-to-join (knock) management** | S | We can set the `knock` join rule but there is no UI to see or approve knocks |
| 8 | **Message edit history** | S | We support editing; there is no "edited" history viewer |
| 9 | **Export chat** | M | Element exports HTML/JSON/plaintext with attachments |
| 10 | **Account settings: change password, deactivate, 3PID add/remove** | M | We only *display* bound emails, read-only. Password reset exists only on the login screen |
| 11 | **Location share (send)** | M | We render `geo:` as an OSM link. No picker, no map, no live-share beacons |
| 12 | **Desktop mic-permission verification + screen-share picker** | S/M | Prerequisite for #1 on Linux; also gates screen sharing in calls |

Everything else is enumerated below by area.

---

## A. Composer and sending

| Feature | Element | Prinny | Verdict |
|---|---|---|---|
| Voice messages | `audio/VoiceRecording.ts`, `audio/VoiceMessageRecording.ts`, `components/views/rooms/VoiceRecordComposerTile.tsx`, `utils/createVoiceMessageContent.ts` | — | **missing** (see appendix) |
| Polls | `components/views/elements/PollCreateDialog.tsx`, `components/views/messages/MPollBody.tsx`, `PollHistoryDialog` | — | **missing** — no `m.poll.start` anywhere |
| Location share | `components/views/location/*` (maplibre-gl), `shareLocation.ts` | render-only: `MsgTypeRenderers.tsx:392` `MLocation` → OSM link | **partial** |
| Live location (beacons) | `components/views/beacon/*`, `stores/OwnBeaconStore.ts`, MSC3672 | — | **missing** |
| Stickers | integration-manager sticker picker (`components/views/rooms/Stickerpicker.tsx`) | own image-pack system + Telegram pack import | **ours is better** |
| Rich-text (WYSIWYG) composer | `@vector-im/matrix-wysiwyg`, `components/views/rooms/wysiwyg_composer/` | Slate markdown editor + toolbar | **parity-ish**, not worth porting |
| LaTeX / maths | `katex`, labs `feature_latex_maths` | — | missing (niche) |
| Spoilers | `/spoiler` command + `data-mx-spoiler` rendering | editor mark + rendering (`editor/output.ts:30`, `react-custom-html-parser.tsx:493`); no `/spoiler` command | parity |
| Slash commands | 35: `spoiler plain html jumptodate nick myroomnick roomavatar myroomavatar myavatar topic roomname invite part remove ban unban ignore unignore devtools addwidget verify discardsession rainbow rainbowme help whois rageshake query msg holdcall unholdcall converttodm converttoroom me` | 21: `me notice shrug startdm join leave invite disinvite kick ban unban ignore unignore myroomnick myroomavatar converttodm converttoroom tableflip unflip delete acl` (`app/hooks/useCommands.ts:140`) | missing: `nick` (global display name), `topic`, `roomname`, `roomavatar`, `myavatar`, `whois`, `msg`, `query`, `help`, `plain`, `html`, `rainbow`, `verify`, `discardsession`, `upgraderoom` (`spoiler` is covered by the editor's spoiler mark) |
| Drag-and-drop upload | yes | yes (ours also disables Tauri's native handler) | parity |
| Caption / filename on attachments | yes | yes | parity |

## B. Timeline and message actions

| Feature | Element | Prinny | Verdict |
|---|---|---|---|
| Forward message | `ForwardDialog.tsx` (room picker + preview) | — | **missing** |
| Edit history | `MessageEditHistoryDialog.tsx`, `utils/MessageDiffUtils.tsx` (word diff) | — | **missing** |
| Pinned messages | `PinnedMessagesCard.tsx` + `PinnedMessageBanner.tsx` | `features/room/room-pin-menu/` list only | **partial** (no banner) |
| Report message | `ReportEventDialog.tsx` | yes (`Message.tsx:592`) | parity |
| Report room / report user | `ReportRoomDialog.tsx`, user-info report | — | missing |
| Bulk redact ("remove recent messages" for a user) | `BulkRedactDialog.tsx` | — | missing (moderation) |
| View source | yes | yes | parity |
| Read receipts | avatar row | both styles (ours has a Cinny/Element toggle) | **ours is ahead** |
| Chat effects (`/confetti`, snowfall, fireworks, hearts, rainfall, spaceinvaders) | `effects/` | — | missing (fun, cheap) |
| Big emoji, autoplay gif/video toggles, code line numbers, syntax highlight detection | settings toggles | partially (media autoload) | minor |
| Jump to date (MSC3030) | labs `feature_jump_to_date` | shipped: `features/room/jump-to-time/` | **ours is ahead** |
| Mark as unread | yes | yes | parity |
| Hidden events / dev mode | yes | yes | parity |

## C. Threads

We have exactly one half of threads: `Message.tsx:1102` offers **Reply in Thread**, and
`RoomTimeline.tsx:1040` sets `rel_type: 'm.thread'`. Everything downstream of that is missing:

- `components/structures/ThreadPanel.tsx` — per-room thread list ("My threads" / "All threads")
- `components/structures/ThreadView.tsx` — the thread timeline with its own composer
- `components/views/spaces/threads-activity-centre/` — global unread-threads indicator
- thread notification counts, thread read receipts, "N replies · last reply at …" summary tile on the root event

Consequence today: a threaded conversation started in Element appears in our main timeline as loose
replies, and thread-only unreads do not surface. This is the largest *structural* gap and the one
that will keep biting as more of the ecosystem uses threads by default.

## D. Rooms, spaces, moderation

| Feature | Element | Prinny | Verdict |
|---|---|---|---|
| Knock / ask-to-join management | `components/views/rooms/RoomKnocksBar.tsx` + labs `feature_ask_to_join` | join-rule switch only (`JoinRulesSwitcher.tsx`) | **missing** the approval UI |
| Room upgrade | `RoomUpgradeDialog`, `RoomUpgradeWarningBar` | `common-settings/general/RoomUpgrade.tsx` + `RoomTombstone` | parity |
| Restricted join rules (space membership) | `ManageRestrictedJoinRuleDialog.tsx` | `JoinRulesSwitcher` — check depth | verify |
| Mjolnir / policy-list ban lists | `MjolnirUserSettingsTab.tsx`, `mjolnir/` | — | missing |
| Server ACL editing | `/acl` + room settings | `/acl` command exists | parity-ish |
| Message retention policy | labs `feature_retention` | — | missing |
| Invite rules / block invites (MSC4155) | `InviteRulesAccountSettings.tsx` | — | missing |
| Media preview controls (MSC4278: hide media/avatars from unknown rooms) | `MediaPreviewAccountSettings.tsx` | — | missing (privacy) |
| Widgets & integration manager | `WidgetStore`, `ExtensionsCard`, `SetIntegrationManager`, Jitsi | only the Element Call widget driver (`app/plugins/call/CallWidgetDriver.ts`) | **partial by design** — general widget support is a deliberate decision, not an oversight |
| Video rooms | labs `feature_video_rooms` / `feature_element_call_video_rooms` | — | missing |
| Space hierarchy / add existing / subspaces | `SpaceHierarchy`, `AddExistingToSpaceDialog`, `CreateSubspaceDialog` | `features/lobby`, `features/add-existing`, space settings | parity |
| Room directory | yes | yes + ~1150-server public directory | **ours is ahead** |
| Breadcrumbs (recent rooms) | yes | — | minor |

## E. Calls and capture

We embed Element Call as a widget with our own driver (`app/plugins/call/`), so group calls, video,
and (in principle) screen share come from EC itself. Element additionally has:

- **Legacy 1:1 MatrixRTC calls** (`LegacyCallHandler.tsx`, `components/views/voip/`), including
  PSTN dial-pad (`DialPad.tsx`), call hold, and PiP (`PipContainer.tsx`). Not obviously worth
  porting if EC covers DM calls for us — worth confirming EC is invoked for 1:1.
- **Screen-share source picker on desktop** (`apps/desktop/src/displayMediaCallback.ts`): Electron
  needs an explicit source chooser for `getDisplayMedia`. Tauri has no equivalent wired up, so
  desktop screen sharing inside EC is probably dead. **Verify before assuming.**
- **Media device settings** (`VoiceUserSettingsTab.tsx`: input/output device pickers, echo
  cancellation, noise suppression, auto gain, `webRtcAllowPeerToPeer`, `fallbackICEServerAllowed`).
  Ours is `app/state/callPreferences.ts` — three booleans (mic/video/sound), no device selection.

**Capture-permission plumbing status (verified):**

- Android: fully wired — `RECORD_AUDIO`/`CAMERA`/`MODIFY_AUDIO_SETTINGS` in the manifest,
  `MainActivity.kt:248` `onPermissionRequest` with an origin allowlist
  (`ALLOWED_CAPTURE_ORIGINS` = localhost:44548 / 127.0.0.1:44548 / tauri.localhost) plus a
  `FOREGROUND_SERVICE_MICROPHONE` service. Voice messages inherit this for free.
- Desktop: the frontend is served from `http://localhost:44548`, which **is** a secure context, so
  `getUserMedia` is permitted by spec. But there is **no** `permission-request` handler anywhere in
  `src-tauri/src/*.rs`. On WebKitGTK (Linux) an unhandled media permission request is denied by
  default. Instrument this first: log the result of
  `navigator.mediaDevices.getUserMedia({audio:true})` on each platform build before building UI on
  top of it.
- Note the allowlist consequence: if Element Call is ever loaded from a remote origin in the
  Android WebView, its mic request is denied by that same gate. Check which origin the EC iframe
  actually has on Android.

## F. Search and data

| Feature | Element | Prinny | Verdict |
|---|---|---|---|
| Server-side search | yes | yes | parity |
| Encrypted-room search | Seshat native index on desktop (`apps/desktop/src/seshat.ts`, `EventIndexPanel.tsx`); **web has none** | live client-side streaming search (`features/message-search/RoomMessageResults.tsx:73`) | **ours is ahead on web/mobile**, behind on desktop (no persistent index → slow on big rooms) |
| Export chat | `utils/exportUtils/` — HTML / JSON / plaintext, attachments, size limits | — | **missing** |
| File panel (per-room media list) | `components/structures/FilePanel.tsx` | — | missing |
| Notification panel | yes | `pages/client/inbox/Notifications.tsx` | parity |

## G. Account, security, sessions

| Feature | Element | Prinny | Verdict |
|---|---|---|---|
| Change password (logged in) | `ChangePassword.tsx` | — (reset only, from the login screen) | **missing** |
| Deactivate account | `DeactivateAccountDialog.tsx` | — | **missing** |
| 3PID add/remove (email, phone) | `AddRemoveThreepids.tsx` | read-only email list (`account/ContactInfo.tsx`) | **partial** |
| Identity server / discovery settings | `SetIdServer.tsx`, `settings/discovery/` | — | missing |
| Cross-signing, key backup, recovery key | `settings/encryption/*` | `BackupRestore.tsx`, `SecretStorage.tsx`, `settings/devices/LocalBackup.tsx` | parity |
| Device verification (SAS/emoji, manual) | yes | `DeviceVerification.tsx`, `ManualVerification.tsx` | parity |
| Session manager (rename, IP, last-seen, bulk sign-out) | `SessionManagerTab.tsx` | `settings/devices/` (`DeviceTile` shows last-seen IP) | mostly parity — check bulk sign-out and rename |
| QR login / device pairing (MSC4108) | labs `feature_login_with_qr` | — | missing |
| Device dehydration | `useOwnDevices.ts` + rust-crypto | — | missing (advanced) |
| "Exclude insecure devices" | labs `feature_exclude_insecure_devices` | — | missing |
| Ignored users | yes | `account/IgnoredUserList.tsx` | parity |
| OIDC / SSO / token login | yes | yes | parity |
| Registration incl. captcha & fallback stages | yes | yes + hCaptcha detection + fallback popup | **ours is ahead** |

## H. Desktop shell (Electron vs Tauri)

| Feature | Element desktop | Prinny (Tauri) | Verdict |
|---|---|---|---|
| Tray, window state, single instance, badge/overlay | yes | yes | parity (ours has Windows `SetOverlayIcon`) |
| Auto-updater | yes | yes (`release.json`) | parity |
| Spell check with language picker | `SpellCheckSettings.tsx` + Electron API | — | missing; WebView2/WebKitGTK spellcheck needs wiring |
| Auto-launch at login | `Electron.autoLaunch` | — | missing |
| Warn before exit | `Electron.warnBeforeExit` | — (we minimise to tray) | n/a-ish |
| Always-show menu bar / custom titlebar | yes | `menu.rs` | verify |
| Hardware acceleration toggle | `Electron.enableHardwareAcceleration` | — | missing |
| Content protection (block screenshots) | `Electron.enableContentProtection` | — | missing (Tauri has `set_content_protected`) |
| Deep links (`element://`, `matrix:` URIs) | `protocol.ts` | — | **missing** — `matrix:` URI handling is a real gap for invite links |
| Screen-share source picker | `displayMediaCallback.ts` | — | missing (see §E) |
| Local encrypted search index | Seshat | — | see §F |
| Save-image / native context menu | `save-image.ts`, `vectormenu.ts` | verify | verify |

## I. Deliberately *not* worth porting

- **PostHog analytics / rageshake / bug-report dialogs** — telemetry pipeline to Element's servers.
- **Voice broadcast** — removed from Element; the directory no longer exists. Don't resurrect it.
- **Jitsi widget** — legacy; EC supersedes it.
- **Sliding sync / simplified sliding sync labs** — depends on homeserver support (MSC4186); only
  interesting if we hit sync-time pain on large accounts.
- **Module API (`@matrix-org/react-sdk-module-api`)** — an enterprise extension point.
- **Release announcements / user onboarding checklists** — product surface for a mass-market app.

## J. Where Prinny is already ahead (don't regress these)

Public server directory + homeserver autocomplete; hCaptcha/registration fallback; client-side
encrypted search; configurable keybinds with a `Ctrl+/` panel; jump-to-date shipped (labs in
Element); Discord-style embeds (vxtwitter, soundcloak, Piped, Bandcamp, Bluesky); Telegram sticker
import; read-receipt style toggle; notification content modes; UnifiedPush + foreground service on
Android; per-message dismissable previews.

---

# Appendix — voice messages, implementation plan

## What Element actually sends (verified, `utils/createVoiceMessageContent.ts`)

```jsonc
{
  "body": "Voice message",
  "msgtype": "m.audio",
  "url": "mxc://…",                      // or "file": { …EncryptedFile } in E2EE rooms
  "info": { "duration": 4213, "mimetype": "audio/ogg", "size": 12345 },

  // MSC1767 extensible events + MSC3245 rendering hint
  "org.matrix.msc1767.text": "Voice message",
  "org.matrix.msc1767.file": { "url": "mxc://…", "name": "Voice message.ogg",
                               "mimetype": "audio/ogg", "size": 12345 },
  "org.matrix.msc1767.audio": { "duration": 4213, "waveform": [/* 44 ints, 0–1024 */] },
  "org.matrix.msc3245.voice": {}          // presence of this key = render as a voice message
}
```

Detection on the receive side is `!!content["org.matrix.msc2516.voice"] || !!content["org.matrix.msc3245.voice"]`
(`utils/EventUtils.ts:217`).

Recording parameters (`audio/VoiceRecording.ts`): opus-recorder 8.x, **ogg/opus**, mono,
48 kHz, **24 kbps**, `encoderApplication: 2048` (voice), max length 900 s with a 10 s warning,
`RECORDING_PLAYBACK_SAMPLES = 44` waveform buckets, amplitudes clamped to 0–1 then multiplied by
1024 when sent.

## Where it plugs into our tree

Our upload pipeline already does everything except produce the blob:
`RoomInput.tsx:324 handleSendUpload` → `msgContent.ts:129 getAudioMsgContent(item, mxc)` →
`sendMessage`, with `browser-encrypt-attachment` handling E2EE (`item.encInfo`). So:

1. **`src/app/plugins/voice-recorder/`** (new)
   - `VoiceRecorder.ts`: `getUserMedia({ audio: { channelCount: 1, noiseSuppression: true,
     echoCancellation: true, autoGainControl: true } })` → opus-recorder with the worker path
     imported through Vite (`import encoderPath from 'opus-recorder/dist/encoderWorker.min.js?url'`).
     Emits `{ waveform: number[44], timeSeconds }` on a tick and resolves to a `Blob` of
     `audio/ogg`.
   - Waveform: Element uses an `AudioWorklet` (`audio/RecorderWorklet.ts`) feeding a
     `FixedRollingArray(44)`. An `AnalyserNode` polled at ~20 Hz into the same rolling array is
     equivalent for our purposes and avoids shipping a worklet through Vite; decide once, measure
     the visual result, don't guess.
   - Hard cap at 900 s and a countdown, same as Element, so we never build a 40 MB "message".
2. **`msgContent.ts`**: add `getVoiceMsgContent(item, mxc, durationMs, waveform)` mirroring the JSON
   above. Keep `getAudioMsgContent` for ordinary files — and **while you are in there, add
   `info.duration` to it**; we currently omit duration for every audio upload, which makes our
   attachments render worse in other clients too.
3. **Composer UI** (`features/room/RoomInput.tsx`): mic button next to the paperclip. States:
   idle → recording (live waveform + mm:ss + stop) → preview (play/pause + waveform + send/discard).
   Element's equivalents to crib from: `LiveRecordingWaveform.tsx`, `LiveRecordingClock.tsx`,
   `RecordingPlayback.tsx`, `PlayPauseButton.tsx`. Wire the recorder's blob into the existing
   `TUploadItem` path so encryption, progress, retry and cancellation are inherited rather than
   reimplemented.
4. **Rendering** (`components/message/MsgTypeRenderers.tsx` `MAudio` / `AudioContent`): if
   `org.matrix.msc3245.voice` (or `org.matrix.msc2516.voice`) is present, render the voice variant —
   waveform bars from `org.matrix.msc1767.audio.waveform` (scale /1024), duration from
   `.duration`, click-to-seek on the bars. Fall back to the current player when either key is
   absent, and compute the waveform locally from the decoded `AudioBuffer` when the sender did not
   provide one (Element does this in `audio/Playback.ts`).
5. **Settings**: input-device picker + AGC/echo/noise toggles alongside the existing call
   preferences (`app/state/callPreferences.ts`), modelled on Element's `VoiceUserSettingsTab`.

## Platform prerequisites — instrument before building

- **Linux/WebKitGTK**: no `permission-request` handler exists in `src-tauri/src/*.rs`. Log the
  outcome of `navigator.mediaDevices.getUserMedia({audio:true})` on an AppImage build **first**; if
  it rejects with `NotAllowedError`, the fix is a `connect_permission_request` handler on the
  WebKit web view (or the equivalent Tauri v2 hook) and it must land before any UI work.
- **Windows/WebView2** and **macOS/WKWebView**: same one-line probe. macOS needs
  `NSMicrophoneUsageDescription` in the bundle Info.plist, and `tauri.conf.json` currently has **no
  `infoPlist` block at all** (`bundle.macOS` sets `frameworks`, `entitlements: null` and nothing
  else) — without that key macOS kills the process on first mic access rather than prompting.
- **Android**: already covered by `MainActivity.kt`'s allowlist and manifest; the app origin
  (`http://localhost:44548`) is on the list. Expect a runtime prompt on first record.
- **Codec check**: opus-recorder is a WASM encoder, so it does not depend on the WebView's
  `MediaRecorder` codec support — this is the main reason to prefer it over
  `MediaRecorder('audio/webm;codecs=opus')`, whose availability varies across WKWebView/WebKitGTK
  and which produces webm that some Matrix clients will not inline-play.

## Definition of done

- Record → preview → send in an unencrypted **and** an encrypted room, on desktop and Android.
- The event carries all five MSC keys; Element Web renders it as a voice message with our waveform.
- Voice messages sent from Element render in Prinny with waveform, duration and seek.
- Cancel mid-recording releases the mic (no lingering capture indicator / no lingering Android
  foreground-service mic type).
- Recording stops and uploads cleanly when the app is backgrounded on Android.
