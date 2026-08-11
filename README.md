# Prinny Client <img align="right" width="90" height="128" src="src-tauri/icons/prinny-logo.png" alt="Prinny" />

[![Build](https://github.com/coffeegrind123/prinny-client/actions/workflows/build.yml/badge.svg)](https://github.com/coffeegrind123/prinny-client/actions/workflows/build.yml)
[![Latest](https://img.shields.io/github/v/release/coffeegrind123/prinny-client?label=latest)](https://github.com/coffeegrind123/prinny-client/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/coffeegrind123/prinny-client/total?label=downloads)](https://github.com/coffeegrind123/prinny-client/releases)

A Matrix chat client that actually feels native. Prinny Client is [Cinny](https://cinny.in) packaged with [Tauri v2](https://v2.tauri.app), shipping on Windows, macOS, Linux, and Android. Hard fork of [cinnyapp/cinny-desktop](https://github.com/cinnyapp/cinny-desktop). The frontend lives at [coffeegrind123/cinny](https://github.com/coffeegrind123/cinny) on the `main` branch.

## Download

Everything is on [GitHub Releases](https://github.com/coffeegrind123/prinny-client/releases).

| Platform | Download |
|----------|----------|
| Windows | [MSI](https://github.com/coffeegrind123/prinny-client/releases/latest/download/Prinny_desktop-x86_64.msi) · [NSIS setup](https://github.com/coffeegrind123/prinny-client/releases/latest/download/Prinny_desktop-x86_64-setup.exe) |
| macOS | [DMG](https://github.com/coffeegrind123/prinny-client/releases/latest/download/Prinny_desktop-universal.dmg) |
| Linux | [AppImage](https://github.com/coffeegrind123/prinny-client/releases/latest/download/Prinny_desktop-x86_64.AppImage) · [deb](https://github.com/coffeegrind123/prinny-client/releases/latest/download/Prinny_desktop-x86_64.deb) |
| Android | [APK](https://github.com/coffeegrind123/prinny-client/releases/latest/download/prinny-android-universal.apk) (sideload) |
| Web | [prinny.app/app](https://prinny.app/app/) (hosted) · [zip](https://github.com/coffeegrind123/prinny-client/releases/latest/download/prinny-webapp.zip) · [git self-host](#self-host-web) |

Desktop builds update themselves via `release.json`. Android checks on launch and downloads new APKs when available.

### Self-host (web)

Two ways, pick whichever matches how you deploy.

**Option A — Tagged zip.** Grab `prinny-webapp.zip` from the [latest release](https://github.com/coffeegrind123/prinny-client/releases/latest), unzip somewhere your webserver serves, point at the `dist/` directory inside. One-shot upload; re-download on each release.

```bash
curl -L -o prinny-webapp.zip \
  https://github.com/coffeegrind123/prinny-client/releases/latest/download/prinny-webapp.zip
unzip prinny-webapp.zip -d /usr/share/webapps/prinny   # extracts to /usr/share/webapps/prinny/dist
```

**Option B — git clone.** Track the [`webapp-release`](https://github.com/coffeegrind123/cinny/tree/webapp-release) branch of `coffeegrind123/cinny`. Install once, update with `git pull` — no npm, no build step. History is linear so pulls always fast-forward.

```bash
git clone -b webapp-release https://github.com/coffeegrind123/cinny.git /usr/share/webapps/prinny
cd /usr/share/webapps/prinny && git pull   # later, to update
```

The `webapp-release` branch ships its own `nginx.conf` and `README.md` covering the SPA-rewrite rules every webserver needs. The zip ships those alongside `dist/` too.

## Features

Everything below is what this fork adds on top of upstream [Cinny](https://cinny.in). Cinny itself already has a markdown editor, message layouts, slash commands, themes, quick switcher, and standard Matrix client features. These are the additions.

### Desktop notifications

| Feature | Description |
|---------|-------------|
| Native OS toasts | `tauri-plugin-notification` backed. Sender avatar, message body, click to jump to room |
| E2EE-aware | Waits for `MatrixEventEvent.Decrypted` before notifying — no ciphertext in toasts |
| Content formatting | Per-msgtype body: `m.text` shows sender + body, `m.image` shows "sent an image: filename" |
| Permission handling | Tauri WebView2 maps `denied`→`prompt` so Enable button always shows. 500ms polling fallback via `isPermissionGranted()` |
| Notification actions | "Open" button on toast, `onAction` listener navigates to room |
| Windows AppUserModelID | Toast notifications require NSIS install (Start Menu shortcut). Loose `.exe` silently skips |

### Message composer

| Feature | Description |
|---------|-------------|
| `Ctrl+/` shortcut panel | Discord-style modal showing all 49 keybinds grouped by category with keycap badges showing current bindings |
| Configurable keybinds | Settings → Keybinds: click any binding, press new combo to rebind. Reset per-binding or Reset All. Editor hotkeys and toolbar tooltips reflect configured bindings |
| Enter autocomplete | `:emoji:`, `/command`, `@user`, `#room` all complete first match on Enter (no arrow-key needed). First result auto-focused with visible highlight |
| Drag-and-drop attachments | Drop files anywhere on the window to add them to the open conversation's attachment list — same path as the paperclip button. Tauri's native drag-drop handler is disabled (`disable_drag_drop_handler()`) so HTML5 drag-drop fires in the WebView |

### Message timeline

| Feature | Description |
|---------|-------------|
| Reply highlights | Yellow left border + amber background when someone replies to your message |
| Timestamp position | Discord-style: timestamp next to username, `@user:server` on right only on hover (Modern layout) |
| NEW badge | Right-aligned green "NEW" badge with full-width green line divider (Discord-style) |
| Read receipts | Two modes (Settings → General → Read Receipt Style): Cinny default shows "is following the conversation" text live, Element-style shows tiny avatar dots at each person's last-read position |
| Mark as Unread | Right-click any message → sets read marker to previous event. Room gets unread badge, green NEW line at marked position |

### Sidebar and navigation

| Feature | Description |
|---------|-------------|
| Presence indicators | Online/busy/away dots on DM avatars and member list |
| DM unread filter | "..." menu next to Direct Messages → "Show unread only" collapses list to rooms with unread messages |
| Mobile swipe gestures | Right-edge swipe opens the active room, left-edge swipe goes back. Selected room highlights during swipe for visual feedback. Works on Home, Direct, and Space screens |
| Public server directory | Explore Community and the login screen both read a combined list of ~1150 public homeservers, merged daily from asra.gr, joinmatrix.org and privacydev.net at `prinny.app/api/servers.json`. Browse any server's public rooms, or filter by software, captcha, email and Tor policy when picking where to register |
| Homeserver autocomplete | The login/register homeserver field completes inline as you type (address-bar style) against that directory, with a full searchable browser behind the magnifier |
| hCaptcha registration | Some servers run hCaptcha behind the spec's `m.login.recaptcha` stage, which hands out an hCaptcha sitekey that Google's widget can never accept. The provider is detected from the sitekey format (Google keys start `6L`, hCaptcha keys are UUIDs) and the matching widget rendered |
| Sign-up fallback | Any registration step without a native dialog opens the homeserver's own `/_matrix/client/v3/auth/<stage>/fallback/web` page in a popup and resumes automatically. Servers that serve no fallback page get a plain explanation plus a link to their actual sign-up page, taken from the server directory |

### Desktop shell

| Feature | Description |
|---------|-------------|
| System tray | Minimize to tray on close (`minimizeToTray` setting). Tray icon with Show/Quit menu |
| Single instance | Only one copy ever runs. Launching again — shortcut, taskbar pin, Start menu — restores and focuses the existing window instead of starting a second client, including when it is hidden in the tray |
| Taskbar badge | macOS Dock + Linux taskbar: native `set_badge_count()` red badge with unread count |
| Windows taskbar overlays | `ITaskbarList3::SetOverlayIcon` with numbered 1-9 and 9+ ICO overlays |
| Desktop updater | `tauri-plugin-updater` checks `release.json` on GitHub Releases |
| Window state | `tauri-plugin-window-state` remembers size, position, maximized state |

### Embeds and media

| Feature | Description |
|---------|-------------|
| Discord-style embeds | 4px left accent border, theme surface background. Title, description, site name, and image in a clean card layout |
| Clickable embed images | Click any embed image to open it full-screen in the image viewer |
| Video embeds | `.mp4`/`.webm` links and `og:video` content render as inline HTML5 video players with controls |
| Audio embeds | `.mp3`/`.ogg`/`.wav`/`.flac`/`.m4a`/`.aac` links render as inline `<audio>` players |
| YouTube embeds | YouTube links render as embedded iframe video player. Settings → General → toggle redirects through `piped.private.coffee` for tracker-free playback (enabled by default) |
| Twitter/X via vxtwitter | Settings → General → toggle (enabled by default) fetches `api.vxtwitter.com` client-side and renders full videos and images inline. Uses `referrerPolicy="no-referrer"` to bypass `video.twimg.com`'s 403 on cross-origin Referer |
| SoundCloud via soundcloak | Settings → General → toggle (enabled by default) rewrites SoundCloud URLs through `sc1.maid.zone` and renders a direct streaming `<audio>` player |
| Bandcamp embeds | Detects Bandcamp `og:video EmbeddedPlayer` URLs and renders them as a 120px-tall inline iframe player |
| Image viewer | Full-screen dark backdrop (85% black). Floating top bar with back button, filename, zoom controls, and download — two separate pill groups with theme styling. Click backdrop or press Escape to close |
| Dismissable embeds | Each URL preview card has an `×` button to dismiss it locally without affecting the message |

### Mobile (Android)

| Feature | Description |
|---------|-------------|
| Foreground service | Persistent "Connected to Matrix" notification keeps WebSocket alive on GrapheneOS and de-Googled devices |
| Network detection | `ConnectivityManager.NetworkCallback` detects WiFi/mobile switch, reconnects automatically |
| Watchdog | `AlarmManager` 15-min wake during doze |
| UnifiedPush | `android-connector:3.0.10` via JitPack — no Firebase, no Play Services needed |
| Push notifications | `UnifiedPushReceiver` posts system notification when app is killed, forwards to plugin when alive |
| Android updater | `UpdateChecker.kt` fetches `release.json`, downloads APK via `DownloadManager`, opens for install |
| Safe-area padding | Notch/status bar padding on mobile (`env(safe-area-inset-*)`) |
| Cleartext traffic | `usesCleartextTraffic=true` for Matrix auth HTTP redirects (homeserver discovery, OIDC) |
| Universal APK | All 4 ABIs (arm64-v8a, armeabi-v7a, x86, x86_64) in one APK |

### Branding

| Feature | Description |
|---------|-------------|
| Prinny panic face | All 17 desktop icons (ICO, PNG, ICNS), favicon SVGs replaced |
| Build-time rename | `scripts/rename-prinny.mjs` via `beforeBuildCommand`: Cinny→Prinny across all source (Rust, Kotlin, TSX, JSON, XML, HTML, SVG) |
| Android package | `in.prinny.app` — directory, namespace, applicationId, Kotlin packages all consistent |
| About page | 96px Prinny logo, dynamic version from `package.json`, links to our repos |
| Release assets | All CI artifacts named `Prinny_desktop-*` / `prinny-android-*` |

### Build and CI

| Feature | Description |
|---------|-------------|
| Unified CI | Single `build.yml` triggers on push (build + artifacts) and `release: published` (upload to release + `release.json`) |
| 4 platforms | Windows (x86_64 MSI + NSIS), macOS (universal DMG), Linux (x86_64 AppImage + deb), Android (universal APK) |
| Native per-platform runners | Windows on `windows-latest` (MSI + NSIS), macOS on `macos-latest` (universal), Linux on `ubuntu-22.04`, Android on `ubuntu-24.04`. Local development can cross-compile Windows from Linux via `x86_64-pc-windows-gnu` + mingw-w64, which produces NSIS only — no MSI |
| Serial Rust builds | `mustRunAfter` chain in `RustPlugin.kt` prevents parallel Android linkers from OOMing |
| Release metadata | `scripts/release.mjs` generates `release.json` on `tauri` tag for desktop + Android updater |

## Build

You need Rust, Node.js 16+, and platform dev tools. Clone with submodules:

```bash
git clone --recursive https://github.com/coffeegrind123/prinny-client.git
cd prinny-client
cd cinny && git checkout main && npm ci && cd ..
npm ci
```

### Linux

```bash
sudo apt install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
npm run tauri build
```

Output lands in `src-tauri/target/release/bundle/` (AppImage, deb, rpm).

### macOS

```bash
npm run tauri build -- --target universal-apple-darwin
```

Output: `src-tauri/target/universal-apple-darwin/release/bundle/` (DMG, app bundle).

### Windows

```bash
# Cross-compile from Linux:
rustup target add x86_64-pc-windows-gnu
sudo apt install -y mingw-w64 nsis
npm run tauri build -- --target x86_64-pc-windows-gnu

# Or on Windows directly:
npm run tauri build
```

Output: `src-tauri/target/release/bundle/` (MSI, NSIS installer).

### Android

Needs SDK 36, NDK 27.0.12077973, and four Rust targets:

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android

export ANDROID_HOME=/path/to/sdk
export NDK_HOME=$ANDROID_HOME/ndk/27.0.12077973

npx tauri android build
```

Signing:

```bash
keytool -genkeypair -v -keystore debug.keystore -alias androiddebugkey \
  -keyalg RSA -keysize 2048 -validity 10000 \
  -dname "CN=Android Debug,O=Android,C=US" \
  -storepass android -keypass android

$ANDROID_HOME/build-tools/35.0.0/apksigner sign \
  --ks debug.keystore --ks-pass pass:android \
  --ks-key-alias androiddebugkey --key-pass pass:android \
  --out app-release-signed.apk \
  src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
```

## Dev

```bash
npm run tauri dev
```

Runs the Vite dev server and launches a Tauri window. For Android use `npx tauri android dev`.

## Contributing

Personal fork. Issues and PRs welcome, no formal process. Open an issue before writing code.

## License

AGPL-3.0-only, same as upstream.
