# prinny-client

Cinny Matrix client packaged as a desktop app via Tauri v2. Cross-compiles to Windows from Linux using the GNU toolchain.

**Repos:**
- Desktop shell: `coffeegrind123/prinny-client` (this repo)
- Frontend (submodule): `coffeegrind123/prinny` branch `main`

> **🚫 Never build Android locally.** No `tauri android build`, no `gradlew`
> (not even a single Kotlin-compile task), no `--target *-linux-android`. GitHub
> CI owns that build. Full rule and what to do instead: "Android build" below.

## Workspace directory: `prinny-mono/prinny-desktop`, not `prinny-client`

The checkout lives at `~/prinny-mono/prinny-desktop` even though the GitHub repo
is `coffeegrind123/prinny-client`. That is not a preference — **the path
`~/prinny-mono/prinny-client` is permanently unusable from inside the
container**, and the rename is the fix.

The 9p mount Docker Desktop exposes has a dentry-cache bug that can leave a
directory listing as `d?????????` with every `stat`/`open`/`cd` under it
returning ENOENT, while the directory is completely fine on the Windows side.
Three things were established by experiment on 16.08.2026, and are worth not
re-deriving:

1. **The bug is keyed to the NAME, not the directory.** Renaming the directory
   host-side to anything the container has not already cached makes it fully
   visible again, immediately — `.git`, `.secrets/`, the 23 GB `src-tauri/target`
   and all. Nothing is lost and nothing needs copying.
2. **The poisoned name stays poisoned.** After renaming away, `mkdir
   prinny-client` from the container still fails with `EEXIST` against an entry
   that no longer exists, and renaming the real directory back to
   `prinny-client` makes it invisible again. So the old name is burned for the
   life of the container.
3. **`drop_caches` is not available** — `/proc/sys/vm/drop_caches` is read-only
   in the container, so the cache cannot be cleared from here.

Two git settings are set **locally in this checkout** as a result, both required
for `git status` to mean anything:

| Setting | Where | Why |
|---|---|---|
| `core.fileMode false` | repo + `cinny` submodule | The mount flips the exec bit, so `Cargo.toml`, `lib.rs`, `rich_presence.rs` and `gradlew` show as modified with a zero-line diff. |
| `core.autocrlf true` | `cinny` submodule | The submodule worktree is CRLF (Windows git checked it out with `core.autocrlf=true` in its *system* config). Without matching it, `git diff` reports all 148 tracked files changed — 41,810 insertions and exactly 41,810 deletions, which is the signature of a pure line-ending diff and not a real one. |

Do NOT "fix" that CRLF diff by rewriting line endings. The bytes are correct;
only the container's git was reading them without the filter.

## Changelog Rules

**Every commit and push MUST include a `cinny/CHANGELOG.md` update.** That file is the source of truth for both:
- The in-app changelog viewer (the empty-state screen — `cinny/src/app/features/changelog/Changelog.tsx` imports `CHANGELOG.md?raw` and parses it).
- GitHub Releases (`scripts/release-notes.mjs` extracts the latest dated section for the release body — see "Cutting a release").

**Format — one entry per day, newest section at the top:**

```markdown
## DD.MM.YYYY

- `abc1234` Added <feature> — <description>
- `def5678` Fixed <bug> — <description>
- `ghi9012` Improved <thing> — <description>
```

**Rules:**
- Date format `DD.MM.YYYY` (European, matching Finland timezone — `date +%d.%m.%Y`).
- Each bullet starts with the 7-char commit SHA in backticks, then a verb: `Added` / `Fixed` / `Improved` / `Disabled` / `Removed`.
- ONE section per day at the TOP of the file. If today's section already exists, append to it — don't create a duplicate.
- Be specific — include component names, file paths, flag names, numbers.
- Group related changes from the same commit into one bullet.
- Don't bullet documentation-only or trivial changes — fold into the nearest feature bullet, or skip.
- Inline `code` spans with backticks render as styled inline tokens in the viewer; everything else is plain text.
- Use both prinny-client and cinny submodule commit SHAs as appropriate — the viewer links them all to the cinny commit page by default, which is fine for cross-linking either repo via GitHub's prefix lookup.

**Workflow — two commits, never `--amend`:**
```bash
# 1. Commit the CODE on its own, with no changelog edit in it.
cd cinny
git add -A && git commit -m "<short msg>"
SHA=$(git rev-parse --short=7 HEAD)      # the SHA the bullets must cite

# 2. Commit the CHANGELOG separately, citing that SHA.
vim CHANGELOG.md                          # bullets start with `$SHA`
git add CHANGELOG.md && git commit -m "Changelog for $SHA"
git push origin main
```

> **Do not fill the SHA in with `git commit --amend`.** Amending rewrites the
> commit, so the SHA you just read no longer exists — every bullet citing it
> 404s in the in-app changelog viewer, which links each SHA to GitHub. The
> changelog bullet must live in a *later* commit than the code it describes.
> This is why a code commit and its changelog commit always come in pairs
> (e.g. `c0b94a6` + `d93bda7`).

For prinny-client-only commits (e.g. workflow edits, CLAUDE.md), bullet them in `cinny/CHANGELOG.md` too — the user-facing changelog is single-source for both repos. Update the cinny submodule pointer in the same prinny-client commit.

## Cutting a release

**Every push to `main` is automatically a release.** No manual tagging needed.

The `create-release` job in `build.yml` auto-bumps the patch version from the latest `v*` tag, creates the tag + GitHub release atomically via `gh release create --target`, and the full pipeline runs:

1. `create-release` — auto-computes next version (e.g. `v4.11.11` → `v4.11.12`), runs `node scripts/release-notes.mjs` which extracts the latest dated section from `cinny/CHANGELOG.md` and appends a collapsed `<details>` block of the raw commit log for traceability; deletes old versioned releases, creates new release with tag
2. All 4 platform builds run in parallel, upload to the release
3. `archive` uploads a source zip
4. `release-update` runs `scripts/release.mjs` to generate `release.json` on the `tauri` tag — this powers the in-app updater

After the release CI completes, the `release.json` on the `tauri` tag will include the new version's platform entries, and desktop/Android clients will auto-detect the update.

**If the release CI fails**, fix the issue, commit, and push to main again — it'll auto-bump to the next version.

## Webapp distribution (self-hosters)

The cinny submodule auto-publishes a built copy of itself to the `webapp-release` orphan branch in `coffeegrind123/prinny` on every push to `main`. Self-hosters install with one clone and update with `git pull` — no `npm` needed at deploy time.

```bash
git clone -b webapp-release https://github.com/coffeegrind123/prinny.git /usr/share/webapps/prinny
cd /usr/share/webapps/prinny
git pull   # later, to update
```

The workflow lives at `cinny/.github/workflows/publish-webapp.yml`. It runs `npm run build`, then commits the contents of `dist/` plus everything under `cinny/.github/webapp-release-template/` (README + nginx.conf) to `webapp-release` as a single linear commit per build. History stays linear, so `git pull` always fast-forwards.

**Edits to the deploy-side files** (README, nginx.conf, install steps) are tracked in `cinny/.github/webapp-release-template/` on `main` and propagated on the next publish — never edit the `webapp-release` branch directly, it gets clobbered on every CI run.

**First-run note:** The publish workflow creates `webapp-release` as an orphan branch on its first execution if it doesn't exist. No manual seeding required.

## Fresh clone & build

```bash
git clone --recursive https://github.com/coffeegrind123/prinny-client.git
cd prinny-client

# Ensure submodule is on our branch
cd cinny
git fetch origin
git checkout main
npm ci
cd ..

# Install root deps and build for Windows
npm ci
source ~/.cargo/env
npm run tauri build -- --target x86_64-pc-windows-gnu
```

Output lands in `src-tauri/target/x86_64-pc-windows-gnu/release/`:
- `cinny.exe` — the application binary (34MB)
- `bundle/nsis/Prinny_x.y.z_x64-setup.exe` — NSIS installer (18MB)
- `bundle/nsis/Prinny_x.y.z_x64-setup.nsis.zip` — updater archive

There is no `bundle/msi/` — see "Windows ships NSIS only" below.

## Submodule setup

The `cinny/` submodule points to `coffeegrind123/prinny` (not upstream `cinnyapp/cinny`). Our `main` branch contains the Tauri notification plugin integration, e2ee decryption handling, and message content formatting.

```bash
# First time after clone:
cd cinny
git fetch origin
git checkout main
npm ci
cd ..

# To update the submodule pointer in the main repo after pushing cinny changes:
git add cinny && git commit -m "Update cinny submodule"
```

**If git submodule update pulls the wrong commit:** It tracks `origin/dev` by default. Always explicitly checkout `main` in the submodule after `git submodule update --init`.

## Upstream sync (cinny submodule)

The cinny submodule is a fork of `cinnyapp/cinny`. Periodically we cherry-pick upstream changes into our `main` branch.

**Tracking file:** `cinny/UPSTREAM_BACKPORT_LOG.md` — lists every upstream commit since our fork point with `[x]` (backported) / `[-]` (skipped as noise) / `[~]` (partial) status. The last entry marked **START HERE** tells you which SHA to start from on the next sync.

### Add upstream remote (one-time)

```bash
cd cinny
git remote add upstream https://github.com/cinnyapp/cinny.git
```

### Sync process

```bash
cd cinny
git fetch upstream --tags

# Check what's new since last sync
# Open UPSTREAM_BACKPORT_LOG.md, find the "START HERE" marker SHA
git log --oneline <last-synced-sha>..upstream/dev --reverse --no-merges

# Filter out noise: chore, CI deps bumps, release tags, docs-only changes
# Cherry-pick meaningful commits oldest-first
# Resolve conflicts: ALWAYS preserve our custom features:
#   - YouTube/Twitter embeds in UrlPreviewCard
#   - Presence badges in RoomNavItem  
#   - MobileSwipeBack in Room
#   - tauri-plugin entries in package-lock.json
#   - vxtwitter API client-side fetch (useVxTwitter setting) for Twitter/X media
#   - FocusTrap fallback for image viewer

# After all cherry-picks done, update UPSTREAM_BACKPORT_LOG.md with status
# of every new commit. Move the START HERE marker to the last upstream commit.

# Push
git push origin main

# Then update the submodule pointer from the parent repo:
cd /opt/openclaude-src/prinny-client
git add cinny
git commit -m "Update cinny submodule"
```

### Conflict-heavy files (expect conflicts every sync)

| File | Why |
|------|-----|
| `src/app/components/url-preview/UrlPreviewCard.tsx` | Heavily customized — YouTube (Piped), Twitter/X (vxtwitter direct media), Bandcamp og:video, SoundCloud (soundcloak), generic mp4/webm video, audio (mp3/ogg/etc.), dismiss button, expand/collapse. Upstream keeps this file simple. |
| `src/app/features/room-nav/RoomNavItem.tsx` | Presence badges + call permissions logic both modify imports and handlers |
| `src/app/features/room/Room.tsx` | Our MobileSwipeBack import vs upstream's call embed imports |
| `package-lock.json` | We have extra deps (tauri-plugin-mobile-push-api etc.) not in upstream |

## Upstream sync (this repo — the Tauri shell)

This repo is a fork of `cinnyapp/cinny-desktop`. Same idea as the submodule sync
above, different tracking file: **`UPSTREAM_BACKPORT_LOG.md`** at the repo root,
with its own **START HERE** marker.

```bash
git remote add upstream https://github.com/cinnyapp/cinny-desktop.git   # one-time
git fetch upstream
git log --oneline <START-HERE-sha>..upstream/main --reverse
```

Expect to skip most of it. Three subsystems are ours outright and have no
upstream counterpart, so upstream commits touching their equivalents are **not
applicable**, not merely noisy — the log's "Why so much is skipped" table has the
detail:

- **CI/release** — we have `build.yml` + `auto-bump-cinny.yml` and
  `scripts/bump-version.mjs`; upstream's `tauri.yml`/`tauri2.yml`/`archive.yml`/
  `test.yml`/`update-version.mjs` don't exist here.
- **Updater** — ours is frontend-driven; the Rust side only registers the plugin.
  Upstream's Rust-side `check()` in `setup()` (and everything gatekeeping it) is N/A.
- **Versioning** — independent 4.11.x line stamped from the release tag.

Android has no upstream counterpart at all.

**Two traps this sync paid for:**

1. `git cherry-pick -n` only wraps *conflicting* hunks in markers. The rest of the
   same region is auto-merged silently, so upstream's tail can land next to your
   resolution and contradict it — that's how a `use of moved value` error got
   committed. Grepping for `<<<<<<<` proves nothing. Re-read the whole region.
2. `cargo check ... 2>&1 | tail -20` reports the **exit code of `tail`**, so a
   failed build looks like a pass. Redirect to a file and check the real status,
   or use `${PIPESTATUS[0]}`.

### Checking Rust per target from the container

A fresh container can't `cargo check` anything until you install the deps:

```bash
# Linux (native) target — GTK/WebKit dev headers
apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev
cd src-tauri && cargo check

# Windows target — needs the mingw toolchain for tauri-winres' windres
apt-get install -y mingw-w64
cd src-tauri && cargo check --target x86_64-pc-windows-gnu
```

**macOS cannot be checked here, and don't burn time trying.** `ring`,
`objc2-exception-helper` and friends compile C against a real Apple SDK, so
`cargo check --target *-apple-darwin` dies in `cc-rs` regardless of the Rust
side. Stubbing one offender just moves the failure to the next. Anything under
`#[cfg(target_os = "macos")]` is therefore verified by the **`macos-check` job
in build.yml**, which gates `create-release` — treat a red there as the real
answer.

**`cargo check` needs `cinny/dist` to exist** — `tauri::generate_context!`
resolves `frontendDist` at compile time. If you haven't built the frontend, a
placeholder is enough for a Rust-only check (that's what `macos-check` does):

```bash
mkdir -p cinny/dist && echo '<!doctype html>' > cinny/dist/index.html
```

**Never judge a check by a piped exit code** — `cargo check … | tail -20`
reports *tail's* status, so a failed build reads as a pass. Redirect to a file,
or use `${PIPESTATUS[0]}`.

## Windows: single instance and the taskbar AppUserModelID

Two Windows behaviours that look like one bug ("I keep ending up with several
Prinnys, and the pinned icon opens yet another one"):

**1. Duplicate processes.** Closing the window *hides* it when `minimizeToTray`
is on (`cinny/src/app/hooks/useSystemTray.ts` calls `window.hide()` from
`onCloseRequested`). A hidden app looks shut down, so every later click on the
shortcut started another full copy — each with its own tray icon and its own
Matrix sync. Fixed with `tauri-plugin-single-instance`, whose callback runs in
the *first* instance and does `show()` → `unminimize()` → `set_focus()`.

> **It must be the FIRST plugin in the builder chain.** A duplicate has to exit
> before anything else initialises: `tauri-plugin-localhost` binds a TCP port,
> and `tauri-plugin-window-state` writes saved geometry on exit, so a duplicate
> that reaches either one fails to start or clobbers the live window's
> position on its way out.

**2. Two taskbar buttons.** Windows groups taskbar buttons by
**AppUserModelID**. A process that never sets one gets a per-executable default
that does not match the AUMID the NSIS installer stamped on the shortcut, so a
pinned Prinny and a running Prinny were two separate buttons. `lib.rs` now calls
`SetCurrentProcessExplicitAppUserModelID` with `context.config().identifier`
(`in.prinny.app`) before any window exists.

**The AUMID is shared with toasts — never let the two drift.** The Windows
toast path (`send_windows_message_toast`) passes the same
`app.config().identifier` to `Toast::new`, and Windows *silently drops* a toast
whose AUMID does not match a registered shortcut. Changing the bundle
identifier, or hardcoding a different string in either place, breaks
notifications with no error message.

Reuse the existing `context` when reading the identifier — `generate_context!()`
embeds the entire frontend bundle, so calling it a second time is not free.

## `cinny/dist` carries a baked-in base path

`build.config.ts` reads `PRINNY_BASE`, and Vite bakes that value into **every**
emitted asset URL. Two different builds come out of the same source:

| Command | Asset URLs | Who gets it |
|---|---|---|
| `npm run build` | `/assets/…` | self-hosters (`webapp-release`), the Tauri desktop/Android bundles |
| `PRINNY_BASE=/app/ npm run build` | `/app/assets/…` | https://prinny.app/app/ |

Both write to the same `cinny/dist/`, and nothing in the tree records which
one produced it. So **after building or verifying the `/app/` variant, re-run
plain `npm run build` before any desktop/Android build** — otherwise
`tauri.conf.json`'s `frontendDist` embeds a `dist/` whose assets all point at
`/app/…`, and the packaged app loads a white screen with 404s in the console.
Nothing fails at build time; the breakage only shows up at runtime.

Check which variant is sitting there:

```bash
grep -o '"/[a-z]*/*assets/index[^"]*"' cinny/dist/index.html | head -2
```

`"/assets/index-….js"` is the desktop-safe one. `"/app/assets/index-….js"` is not.

The subpath build is produced only by the **Rebuild and deploy to prinny.app**
step in `cinny/.github/workflows/publish-webapp.yml`, which runs after the
`webapp-release` publish — so CI never ships a `/app/`-based build anywhere
else. This trap is a local-workflow one.

## Critical: working directory

**The Bash tool's CWD persists between commands.** After `cd cinny`, all subsequent Bash calls run from there. `npm run tauri` fails with "Missing script: tauri" because `cinny/package.json` doesn't have that script. Always verify `pwd` before build commands.

```bash
# Safe one-liner from anywhere:
source ~/.cargo/env && cd /opt/openclaude-src/prinny-client && npm run tauri build -- --target x86_64-pc-windows-gnu
```

**Also:** `source ~/.cargo/env` must run in every Bash call — shell state is not preserved.

## Cross-compiling to Windows from Linux

### Prerequisites

| Tool | Package / Install | Purpose |
|------|------------------|---------|
| Rust | `rustup` (rust-lang.org) | Tauri backend compiler |
| Windows Rust target | `rustup target add x86_64-pc-windows-gnu` | Cross-compile to Windows |
| mingw-w64 | `apt install mingw-w64` | GNU linker for Windows target |
| NSIS | `apt install nsis` | Build Windows installer |
| Tauri Linux deps | `apt install libwebkit2gtk-4.1-dev libappindicator3-dev patchelf libsoup-3.0-dev libjavascriptcoregtk-4.1-dev` | Tauri build dependencies |
| Node.js | >= 16.0.0 | Frontend build |

### Cargo linker config

Required at `src-tauri/.cargo/config.toml`:

```toml
[target.x86_64-pc-windows-gnu]
linker = "/usr/bin/x86_64-w64-mingw32-gcc"

[target.x86_64-pc-windows-gnu.tauri]
runner = ""
```

Without this, Cargo uses the system `cc` which produces Linux ELF binaries.

### Build flow

1. `npm run tauri build -- --target x86_64-pc-windows-gnu`
2. `beforeBuildCommand` fires: `cd cinny && npm run build` (Vite → `cinny/dist/`)
3. `tauri::generate_context!()` embeds `frontendDist` (`../cinny/dist`) — panics if dist missing
4. Cargo compiles Rust for `x86_64-pc-windows-gnu`
5. `makensis` creates the Windows installer `.exe`

### Cross-compile caveats

- **`tauri dev` won't work from Linux for Windows.** Needs native Windows display + WebView2.
- **MSI is not built at all**, on any host — `bundle.targets` in `tauri.conf.json` is an explicit list that omits `"msi"`. See "Windows ships NSIS only" below. (It was already impossible on a Linux cross-compile; now it is off everywhere, deliberately.)
- **Code signing is skipped.** Unsigned binaries are functional.
- **`__TAURI_BUNDLE_TYPE` patch fails on cross-compile.** Non-blocking warning.
- **Updater needs `TAURI_SIGNING_PRIVATE_KEY`.** Unset for local dev; warning is harmless.
- **`nsis_tauri_utils.dll` auto-downloaded** from GitHub on first build. Cached afterwards.
- **`makensis.exe` symlink needed:** `ln -sf /usr/bin/makensis /usr/bin/makensis.exe`

## Android build

> ### 🚫 NEVER run an Android build locally (user directive, 16.08.2026)
>
> **Android is built in GitHub CI, and only there.** Do not run `npx tauri
> android build`, `gradlew`/`./gradlew` (including single tasks like
> `:app:compileUniversalDebugKotlin`), `cargo build/check --target
> *-linux-android`, `apksigner`, or anything else that compiles or packages the
> Android app on this machine — not to "just check that Kotlin compiles", not
> to reproduce a CI failure, and not because a change looks risky. Push and let
> the workflow build it.
>
> The rest of this section documents how CI does it, and the constraints it
> works under. It is reference material, **not** an invitation to run it here.
>
> **What to do instead when Android code changes:**
> - Rust: `cargo check` a *non-Android* target (Windows GNU is the fastest set
>   up here) — it still type-checks everything outside `#[cfg(target_os =
>   "android")]`.
> - Kotlin: read it carefully; there is no local compile. Treat a red Android
>   job in CI as the compile step.
> - Frontend: `npm run build` + `npx tsc --noEmit` + `npx eslint` in `cinny/`,
>   which are cheap and cover the JS half of any Android feature.
> - ACL/capability changes: the generated `acl-manifests.json` and
>   `capabilities.json` under `src-tauri/target/<target>/debug/build/cinny-*/out/`
>   can be inspected after any `cargo check`, on any target, to prove what the
>   frontend is actually allowed to invoke.

### Prerequisites

| Tool | Path / Install | Purpose |
|------|---------------|---------|
| Android SDK | `/opt/android-sdk/` | Platform tools, build tools, platform APIs |
| Android NDK | `/opt/android-sdk/ndk/27.0.12077973/` | Native (Rust) code compiler for Android |
| Rust Android targets | `rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android` | Cross-compile Rust to Android ABIs |
| Tauri CLI | `@tauri-apps/cli@2.7.1` (npm root dep) | `npx tauri android build` entry point |
| JDK 21 | `/opt/jdk-21.0.10+7/` | keytool for signing |
| apksigner | `/opt/android-sdk/build-tools/35.0.0/apksigner` | APK signing |

### Environment variables

**All required** — the build fails without each one:

```bash
export ANDROID_HOME=/opt/android-sdk
export ANDROID_SDK_ROOT=/opt/android-sdk
export NDK_HOME=/opt/android-sdk/ndk/27.0.12077973
```

Also run `source ~/.cargo/env` before building (shell state not preserved between Bash calls).

### Build memory requirements (CRITICAL)

This build runs on a **21GB RAM machine with 6GB swap**. The `src-tauri/target/` directory accumulates 15+ GB of stale artifacts. When swap is full, the combined Rust + Gradle build triggers OOM (exit 137).

**The three rules that prevent OOM:**

1. **Always `cargo clean` before every build** — frees 7-16GB of stale artifacts.
2. **Always `CARGO_BUILD_JOBS=1`** — each cargo instance is single-threaded, so compilation uses ~1 core instead of all 12.
3. **Serialized `rustBuild*` tasks via `mustRunAfter` in `RustPlugin.kt`** — this is the critical fix. By default, Gradle runs all 4 `rustBuild*` tasks (one per ABI) in parallel. Even with `CARGO_BUILD_JOBS=1`, parallel cargo instances reach the linking step at overlapping times, and each linker consumes several GB of RAM. The `mustRunAfter` chain forces them to run one at a time (aarch64 → armv7 → i686 → x86_64), so only one linker is active at any moment. Total Rust time with serial builds: ~9 minutes (vs OOM in parallel). See `src-tauri/gen/android/buildSrc/.../RustPlugin.kt:63-69`.

**Note on `abiList`/`targetList` in `gradle.properties`:** These were found to be **ignored** in practice — the build compiled all 4 ABIs regardless of the property values. The serialization fix in `RustPlugin.kt` is what actually controls build behavior. The properties are left in `gradle.properties` as documentation of intent.

**Killing stale build processes (if OOM or interruption leaves orphans):**

The Gradle RustPlugin spawns `cargo`, `rustc`, and `cc` child processes that survive the parent being killed. An OOM'd or interrupted build can leave 4+ cargo instances running in the background, each consuming CPU and RAM. Check and kill them before retrying:

```bash
# Check for stale build processes
ps aux | grep -E "cargo.*cinny|rustc.*cinny|tauri android" | grep -v grep

# Kill all stale build processes
pkill -f "cargo build.*cinny" 2>/dev/null
pkill -f "tauri android build" 2>/dev/null
# If processes persist (D state = stuck in kernel), force kill:
kill -9 $(pgrep -f "cargo.*cinny") 2>/dev/null
```

Also check `free -h` before building — if swap is full (>5GB used), something else is leaking. Common culprits: Chrome (browser MCP, ~400MB), Ghidra (JVM with `-Xmx4g`), old Gradle daemons. The Bun process for openclaude itself uses ~800MB RSS.

**Before every build:**

```bash
# Kill any stale processes from previous failed builds first
pkill -f "cargo build.*cinny" 2>/dev/null

cd /opt/openclaude-src/prinny-client/src-tauri
cargo clean                          # frees 7-16GB
rm -rf gen/android/app/build         # clean Gradle build output
rm -rf gen/android/buildSrc/build    # clean Gradle buildSrc (needed after RustPlugin.kt changes)
```

The Gradle JVM heap is capped at 2GB (`-Xmx2048m` in `gradle.properties`). This is intentional — keep the constraint.

### End-to-end build flow

```bash
# 1. Clean (frees 15+ GB — essential for OOM avoidance)
source ~/.cargo/env
cd /opt/openclaude-src/prinny-client/src-tauri
cargo clean
rm -rf gen/android/app/build

# 2. Build release APK (CARGO_BUILD_JOBS=1 is CRITICAL — prevents OOM)
cd /opt/openclaude-src/prinny-client
CARGO_BUILD_JOBS=1 \
ANDROID_HOME=/opt/android-sdk \
ANDROID_SDK_ROOT=/opt/android-sdk \
NDK_HOME=/opt/android-sdk/ndk/27.0.12077973 \
npx tauri android build

# 3. Sign with debug keystore
/opt/android-sdk/build-tools/35.0.0/apksigner sign \
  --ks debug.keystore --ks-pass pass:android \
  --ks-key-alias androiddebugkey --key-pass pass:android \
  --out app-release-signed.apk \
  src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk

# 4. Verify
/opt/android-sdk/build-tools/35.0.0/apksigner verify app-release-signed.apk
```

Output: `app-release-signed.apk` (119MB, universal — all 4 ABIs, signed, installable).

### Debug keystore (one-time setup)

```bash
/opt/jdk-21.0.10+7/bin/keytool -genkeypair -v \
  -keystore debug.keystore -alias androiddebugkey \
  -keyalg RSA -keysize 2048 -validity 10000 \
  -dname "CN=Android Debug,O=Android,C=US" \
  -storepass android -keypass android
```

The keystore is at the repo root (`debug.keystore`) and is `.gitignore`d — never commit it. This local `debug.keystore` is only for **manual sideloading during dev** — it is per-developer and unrelated to the in-app updater.

### CI release signing & the updater (CRITICAL — stable key)

**Every release APK must be signed with the SAME key, or in-app updates fail.** Android rejects an update whose signature doesn't match the installed app (`INSTALL_FAILED_UPDATE_INCOMPATIBLE` — "App not installed"): the APK downloads fine but won't install over the previous one, even after the user grants "install unknown apps." This is NOT a delta/patch problem — it's a signing-key mismatch.

The old CI ran `keytool -genkeypair` in the Sign APK step, minting a **fresh random key every release** — so every update was un-installable. That was "fixed" by committing a stable key, `prinny-ci.keystore`, to this **public** repo with its password hardcoded in `build.yml`. See the compromise warning below: that key is dead, and the CI now signs from repo secrets only.

#### 🚨 The committed `prinny-ci.keystore` is COMPROMISED — permanently

`prinny-ci.keystore` (alias `prinny`, store/key pass `prinny-updater`, both published in this repo) was committed to a **public** repository. Treat it as belonging to everyone:

- **Anyone can sign an APK with it.** Android's update rule is "same signing key = same app", so a third-party APK signed with this key installs *over* a user's Prinny install as a silent in-place update, inheriting its data and permissions. That is the whole threat, and it applies to every device that ever installed a `prinny-ci.keystore`-signed build.
- **Deleting the file does not undo it.** The key remains in git history and in every already-published GitHub release. History is deliberately *not* rewritten — rewriting would break every fork/clone and would not un-publish what has been downloadable for months. The only real remediation is rotation.
- **Rotating forces exactly one manual reinstall for every existing Android user.** Their installed app was signed with the old key; Android refuses any update signed by a different key (`INSTALL_FAILED_UPDATE_INCOMPATIBLE` — "App not installed"). Users must uninstall and install the new APK once. Unavoidable, and the same caveat as rotating the desktop minisign keypair. Announce it in the changelog/release notes.
- Never reuse the old key "just for continuity". Continuity is worth less than a signing key the public holds.

#### Current flow: secrets only, or the build fails

The Sign APK step in `.github/workflows/build.yml` has **no committed-keystore fallback**. It requires all three secrets and exits non-zero with a clear error if any is unset — a red build beats silently shipping an APK signed with a key everyone has.

| Secret | Value |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | `base64 -w0 your-release.keystore` |
| `ANDROID_KEYSTORE_PASSWORD` | Store password (also used as the key password) |
| `ANDROID_KEY_ALIAS` | Key alias inside the keystore (no default — must be set) |

Generate a fresh release key locally, keep it offline (a password manager / hardware-backed store — **not** this repo), and upload it as the secret:

```bash
keytool -genkeypair -v \
  -keystore prinny-release.keystore -alias <alias> \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -dname "CN=Prinny Client,O=Prinny,C=FI" \
  -storepass "<strong-pass>" -keypass "<strong-pass>"
base64 -w0 prinny-release.keystore   # -> ANDROID_KEYSTORE_BASE64
```

Handling details in the workflow, all of them load-bearing:

- The password is passed to `apksigner` as `--ks-pass env:KS_PASS --key-pass env:KS_PASS`, never `pass:…` — a `pass:` argument is readable by any other process on the runner via the process table.
- The decoded `ci-signing.keystore` is removed by a `trap … EXIT`, so it is deleted on the failure path too (the old unconditional `rm` after the sign command never ran if signing failed).
- Every `*.keystore` / `*.jks` is `.gitignore`d now, not just `debug.keystore`.
- **Existing users on the old random-key or `prinny-ci.keystore` builds need ONE manual reinstall** to land on the new key; after that, in-app updates work normally.

### How the build works (Tauri v2 Android internals)

1. `npx tauri android build` starts a TCP server on localhost
2. `beforeBuildCommand` fires: `cd cinny && npm run build` (Vite → `cinny/dist/`)
3. Gradle's `rustBuild*` tasks connect back to the Tauri CLI via TCP and invoke `cargo build --target <target> --release` once per ABI
4. Each target's `libapp_lib.so` is symlinked into `jniLibs/<abi>/`
5. Gradle packages the APK (or AAB — also produced)

The build compiles 4 ABIs by default (arm64-v8a, armeabi-v7a, x86, x86_64) producing a "universal" APK. The `gradle.properties` `abiList` and `targetList` were found to be **ignored** — the Tauri CLI always passes all 4 targets regardless. To control which ABIs are built, modify the hardcoded lists in `RustPlugin.kt` directly.

**Serial build ordering:** `RustPlugin.kt` uses `mustRunAfter` constraints to serialize the 4 `rustBuild*` tasks. Without this, Gradle runs them in parallel and multiple linkers exhaust 21GB RAM → OOM. Each task waits for the previous one to finish before starting.

### APK output

| Variant | Path | Signed? |
|---------|------|---------|
| Release (universal) | `src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk` | No |
| Release AAB | `src-tauri/gen/android/app/build/outputs/bundle/universalRelease/app-universal-release.aab` | No |
| After signing | `app-release-signed.apk` (repo root) | Yes (debug key) |

### Matrix cleartext fix

Android 9+ blocks HTTP (cleartext) traffic in WebViews by default. Matrix auth flows (homeserver discovery, OIDC redirects) hit HTTP even when the final endpoint is HTTPS. The fix is in `src-tauri/gen/android/app/build.gradle.kts`:

```kotlin
// defaultConfig — applies to both debug and release
manifestPlaceholders["usesCleartextTraffic"] = "true"
```

This was previously `"false"` for release builds, causing `ERR_CLEARTEXT_NOT_PERMITTED` in the WebView.

### Foreground service (GrapheneOS background notifications)

On de-Googled devices without FCM, Android kills background apps aggressively. A foreground service with a persistent notification keeps the process alive so the Matrix WebSocket stays connected.

**Architecture:**

```
JS: startForegroundService()
  → plugin:foreground|start_foreground
  → ForegroundServicePlugin.kt (Tauri plugin)
  → ForegroundService.kt (Android Service with startForeground())
  → Persistent notification "Cinny - Connected to Matrix"
```

**Startup:** The foreground service starts automatically in `useUnifiedPush.ts` when the Matrix client connects. On cleanup (client stop), it stops the service.

**Permissions required:**
- `FOREGROUND_SERVICE` — to start a foreground service
- `FOREGROUND_SERVICE_DATA_SYNC` — Android 14+ foreground service type
- `POST_NOTIFICATIONS` — to show the persistent notification (Android 13+)

**Files:**
| File | Role |
|------|------|
| `ForegroundService.kt` | Android Service, creates notification channel, calls `startForeground()` |
| `ForegroundServicePlugin.kt` | Tauri plugin with `startForeground`/`stopForeground`/`isForegroundRunning` commands |
| `ForegroundService.kt` notification channel | `cinny_foreground`, IMPORTANCE_LOW, no badge |

**To disable:** Remove `await startForegroundService()` from `useUnifiedPush.ts:67`. Users with FCM don't need it.

### Key files (Android)

| File | Role |
|------|------|
| `src-tauri/src/lib.rs` | `#[cfg_attr(mobile, tauri::mobile_entry_point)]`, conditional plugin loading |
| `src-tauri/Cargo.toml` | `tauri-plugin-mobile-push` + mobile-gated deps |
| `src-tauri/capabilities/mobile.json` | Mobile capability permissions |
| `src-tauri/capabilities/desktop.json` | Desktop-only perms (global-shortcut moved here) |
| `src-tauri/gen/android/app/build.gradle.kts` | `usesCleartextTraffic`, minSdk/targetSdk, build types |
| `src-tauri/gen/android/gradle.properties` | JVM heap, ABI list |
| `src-tauri/gen/android/tauri.settings.gradle` | Rust target list |
| `src-tauri/gen/android/buildSrc/.../RustPlugin.kt` | Gradle→cargo bridge, `mustRunAfter` serialization |
| `src-tauri/gen/android/app/src/main/java/.../UnifiedPushPlugin.kt` | UnifiedPush Tauri plugin (Android) |
| `src-tauri/gen/android/app/src/main/java/.../UnifiedPushReceiver.kt` | UP MessagingReceiver (Android) |
| `src-tauri/gen/android/app/src/main/java/.../ForegroundService.kt` | Foreground service for background WebSocket (GrapheneOS) |
| `src-tauri/gen/android/app/src/main/java/.../ForegroundServicePlugin.kt` | Foreground service Tauri plugin (start/stop from JS) |
| `src-tauri/gen/android/app/src/main/AndroidManifest.xml` | Permissions, service/receiver declarations |
| `src-tauri/gen/android/settings.gradle` | JitPack repo for UP connector |
| `src-tauri/tauri.conf.json` | `beforeBuildCommand`, `frontendDist`, bundle identifier |
| `cinny/src/app/hooks/useUnifiedPush.ts` | UP registration + Matrix pusher hook |
| `cinny/src/app/utils/mobile-push.ts` | UP Tauri command wrappers |
| `cinny/src/index.css` | Safe-area padding for device notches |
| `.gitignore` | Excludes `*.apk`, `*.idsig`, `*.keystore`, `*.jks` |

### Iteration (edit → test on device)

**Not something the agent runs** — see the rule at the top of this section. The
loop is: edit, push to `main`, let CI build and release, install the APK it
produced from the GitHub release.

1. Edit source in `cinny/src/` or `src-tauri/`
2. Run the frontend gates locally (`npm run build`, `tsc`, `eslint` in `cinny/`)
   and `cargo check` a non-Android target
3. Commit + changelog + push; CI builds and signs the APK
4. Install it from the release on the device

### Iteration (Linux → Windows)

1. Edit source in `cinny/src/` or `src-tauri/`
2. `source ~/.cargo/env && cd /opt/openclaude-src/prinny-client && npm run tauri build -- --target x86_64-pc-windows-gnu`
3. Copy `cinny.exe` or the NSIS installer to Windows
4. Install and run from Start Menu (required for toast notifications — see below)

For faster Rust checking: `cargo check --target x86_64-pc-windows-gnu` in `src-tauri/`.

## Windows Desktop Notifications

### Architecture

```
JS: sendNotification({title, body})
  → @tauri-apps/plugin-notification (npm package, installed in cinny/)
  → Tauri IPC → plugin:notification|notify
  → Rust: tauri_plugin_notification::init()
  → Windows Toast Notification API
  → Action Center toast popup
```

### Required setup (already done in this repo)

1. **`src-tauri/src/lib.rs`:** `.plugin(tauri_plugin_notification::init())`
2. **`src-tauri/tauri.conf.json`:** `"withGlobalTauri": true` — **critical**, Tauri v2 defaults this to `false`
3. **`src-tauri/capabilities/migrated.json`:** `"notification:default"` permission
4. **`cinny/package.json`:** `@tauri-apps/plugin-notification` dependency
5. **`cinny/src/app/utils/desktop-notifications.ts`:** Tauri/browser wrapper with `isTauri()`, `requestPermission()`, `sendNotification()`
6. **`cinny/src/app/pages/client/ClientNonUIFeatures.tsx`:** Uses `sendDesktopNotification()` with Matrix msgtype-aware body formatting; waits for `MatrixEventEvent.Decrypted` for e2ee rooms
7. **`cinny/src/app/hooks/usePermission.ts`:** Maps `denied→prompt` in Tauri (WebView2 default); 500ms polling fallback

### Pitfalls (in the order we hit them)

1. **`window.Notification` polyfill doesn't cover `requestPermission()` on Windows.** WebView2 doesn't support the Notification API natively. **Fix:** Use `@tauri-apps/plugin-notification` npm package directly.

2. **`window.__TAURI__` not injected.** Tauri v2 defaults `withGlobalTauri` to `false`. `isTauri()` always returned `false`, Enable button never appeared. **Fix:** Set `"withGlobalTauri": true` in tauri.conf.json.

3. **"Notification permission is blocked" on fresh install.** WebView2 defaults `Notification.permission` to `'denied'`. The Enable button only shows for `'prompt'`. **Fix:** `getNotificationState()` maps anything not `'granted'` to `'prompt'` when running in Tauri.

4. **"Nothing happens" when clicking Enable.** Windows desktop apps don't have a browser-style permission popup — notifications are managed in Windows Settings. The plugin's `requestPermission()` returns the OS state. On a properly installed app (NSIS → Start Menu shortcut → AppUserModelID), it returns `'granted'` immediately.

5. **Toast shows "New message from $name" instead of content.** The original code used a hardcoded body. **Fix:** Extract `msgtype` and `body` from Matrix event content, format per type (`m.text`/`m.image`/`m.video`/`m.audio`/`m.file`).

6. **Notifications show encrypted payload in e2ee rooms.** `Timeline` event fires before decryption completes. `getContent()` returns encrypted blob. **Fix:** Check `mEvent.isEncrypted()`, wait for `MatrixEventEvent.Decrypted`, then send notification.

7. **Submodule pulled wrong branch after `git submodule update --remote`.** Tracks `origin/dev` by default. **Fix:** Always explicitly `git checkout main` in the submodule.

### Windows AppUserModelID

Toast notifications require the app to have an AppUserModelID, which Windows assigns to *installed* applications (ones with Start Menu shortcuts). A loose `.exe` silently fails.

| Scenario | Works? |
|----------|--------|
| Installed via NSIS | Yes |
| Loose `.exe` | No — pin to Start or create shortcut manually |
| `tauri dev` on Windows | Shows PowerShell icon |

### Notification content format

| Message type | Toast body |
|-------------|-----------|
| `m.text` | `Alice: hello world` |
| `m.image` | `Alice sent an image: photo.jpg` |
| `m.video` | `Alice sent a video: clip.mp4` |
| `m.audio` | `Alice sent an audio clip: voice.ogg` |
| `m.file` | `Alice sent a file: document.pdf` |
| Encrypted | Same as above (waits for decryption first) |
| Unknown/empty | `New message from Alice` |

### Key files

| File | Role |
|------|------|
| `src-tauri/src/lib.rs` | Plugin init (notification, opener, localhost, window-state) |
| `src-tauri/tauri.conf.json` | `withGlobalTauri: true`, build config |
| `src-tauri/capabilities/migrated.json` | `notification:default` permission |
| `src-tauri/.cargo/config.toml` | Windows GNU linker config |
| `src-tauri/Cargo.toml` | Rust deps (tauri-plugin-notification, etc.) |
| `.gitmodules` | Submodule → `coffeegrind123/prinny` |
| `cinny/src/app/utils/desktop-notifications.ts` | Tauri/browser notification wrapper |
| `cinny/src/app/pages/client/ClientNonUIFeatures.tsx` | Runtime notification dispatch |
| `cinny/src/app/features/settings/notifications/SystemNotification.tsx` | Permission UI |
| `cinny/src/app/hooks/usePermission.ts` | Permission state with Tauri remapping |

## Windows ships NSIS only — the MSI is off on purpose

`bundle.targets` in `src-tauri/tauri.conf.json` used to be `"all"`, which on a
Windows runner builds **both** the NSIS `-setup.exe` and the WiX `.msi`. It is
now an explicit list — `["nsis", "deb", "rpm", "appimage", "app", "dmg"]` — i.e.
everything `"all"` used to mean, minus `"msi"`.

**Nothing ever consumed the MSI.** `scripts/release.mjs` builds `release.json`
by matching `/\.nsis\.zip$/` and `/\.nsis\.zip\.sig$/` against the release
assets (release.mjs:84, 87, 166). There is no `.msi` branch and never was, so
the MSI was built, renamed and uploaded on every Windows release for zero
consumers.

**And shipping it was actively harmful, not merely wasteful:**

1. **It breaks the pinned/Start-Menu shortcut, which breaks notifications.**
   Tauri's MSI update path is uninstall-then-reinstall; NSIS upgrades in place.
   A re-created shortcut orphans the pin and the AppUserModelID that the
   shortcut carries. That AUMID is load-bearing here — see "Windows: single
   instance and the taskbar AppUserModelID" above. `lib.rs` calls
   `SetCurrentProcessExplicitAppUserModelID` with the bundle identifier so a
   pinned and a running Prinny are one taskbar button, and
   `send_windows_message_toast` passes the same identifier to `Toast::new`.
   Windows **silently drops** a toast whose AUMID does not match a registered
   shortcut, so a shortcut lost to an MSI reinstall takes desktop
   notifications with it and reports no error anywhere.
2. **The MSI was never rebranded.** `src-tauri/wix/banner.bmp` and
   `dialogImage.bmp` are still upstream Cinny artwork — blue `#2A62A6` and the
   Cinny bird — so anyone who installed the `.msi` got a Cinny-branded
   installer for a product called Prinny.
3. It cost a WiX toolchain download plus an extra bundle pass on every Windows
   build.

**Re-enabling it is a one-word change and everything needed is still in the
tree:** put `"msi"` back in `bundle.targets`, and un-comment the four lines
flagged in `.github/workflows/build.yml` (one `Move-Item` in *Rename
artifacts*, plus the `.msi` path in *Upload artifacts* and *Upload to
release* — those two were deleted rather than commented, because `path:` and
`files:` are YAML block scalars where `#` is literal text and @actions/glob
runs minimatch with `nocomment: true`). The `bundle.windows.wix` block and
`src-tauri/wix/*.bmp` are intentionally left in place for exactly that.

### Installer branding (NSIS)

`bundle.windows.nsis` in `tauri.conf.json`:

| Key | Asset | Constraint |
|---|---|---|
| `installerIcon` | `src-tauri/icons/icon.ico` | Becomes `MUI_ICON` |
| `headerImage` | `src-tauri/nsis/header.bmp` | **150 × 57**, 24-bit BMP |
| `sidebarImage` | `src-tauri/nsis/sidebar.bmp` | **164 × 314**, 24-bit BMP — Welcome + Finish pages |

`bundle.copyright` is also now set (`"Prinny - AGPL-3.0-only"`). Tauri's NSIS
template does `BrandingText "${COPYRIGHT}"`, and NSIS renders an **empty**
`BrandingText` as its own default — so with `copyright: ""` the installer
footer read *"Nullsoft Install System v3.x"*. The same string lands in the
`.exe` VERSIONINFO `LegalCopyright`.

> **Validate `tauri.conf.json` against the CLI's schema, NOT the Rust crate's.**
> These are two different schemas at two different versions, and the one that
> gates the build is the older of them. `package.json` pins
> `@tauri-apps/cli@2.7.1`, which ships `node_modules/@tauri-apps/cli/config.schema.json`
> at `schema.tauri.app/config/**2.7.0**` with `additionalProperties: false`,
> while `Cargo.lock` puts `tauri-utils` at 2.9.3 / `config/**2.11.3**`. A key
> added to the Rust config struct between those versions validates fine against
> the crate and is a **hard build failure** on every platform, including Android,
> because the JS CLI validates the whole config before it does anything.
>
> This cost a release: `uninstallerIcon` exists in tauri-utils 2.9.3 but not in
> CLI 2.7.1, and all four platform jobs died with ``error on `bundle > windows >
> nsis` … is not valid under any of the schemas listed in the 'anyOf' keyword``.
> Note the failure names the *whole* `nsis` object, not the offending key, so
> read the schema rather than guessing which key it means:
>
> ```bash
> python3 -c "import json;print(sorted(json.load(open('node_modules/@tauri-apps/cli/config.schema.json'))['definitions']['NsisConfig']['properties']))"
> ```
>
> Bumping the CLI to match the crates would allow `uninstallerIcon` and
> `uninstallerHeaderImage`; until then the uninstaller uses NSIS's stock icon.

**The BMP rules are not style preferences, they are format constraints:**

- **24-bit `BI_RGB` only.** NSIS renders a 32-bit BMP's alpha channel as
  *black*, so a PNG-with-transparency converted naively produces a black box.
  `scripts/make-nsis-art.py` composites onto an opaque ground and asserts
  `bpp == 24 && compression == 0` on the file it just wrote.
- **Produce them at the exact pixel size.** MUI's default
  `MUI_HEADERIMAGE_BITMAP_STRETCH` is `FitControl` (Interface.nsh:65) — the
  bitmap is scaled to fill the control whatever its size, so matching the
  size exactly makes the stretch a no-op instead of a resample.
- **The header tile has a white ground on purpose.** Tauri does not override
  `MUI_BGCOLOR`, so the header strip behind the bitmap is `#FFFFFF`. Tauri's
  template also does not define `MUI_HEADERIMAGE_RIGHT`, so which side of the
  header the tile lands on is decided by MUI's dialog resource; a white ground
  blends either way. Do not give the header a dark ground.

Regenerate both with `python3 scripts/make-nsis-art.py` (Pillow required). The
script derives its palette from `src-tauri/icons/icon.png` and sets its type in
Inter, the same face the webapp uses.

## In-app updater (Windows, Linux, macOS)

`tauri-plugin-updater` requires a real minisign signature when a `pubkey` is configured. Empty signatures crash on download with **"Invalid encoding in minisign data"** (`tauri-plugin-updater-2.x/src/updater.rs:712` → `Signature::decode("")` → `InvalidEncoding`). There is no runtime flag to skip verification — the call site is unconditional. Every emitted platform MUST have a valid signature.

All three desktop platforms are signed. Android handles updates natively (`UpdateChecker.kt`).

### Required GitHub Actions secrets

| Secret | Source |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | Single-line base64 contents of the minisign private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password that protects the private key |

Local copies live in `.secrets/` (gitignored). Public key is committed at `src-tauri/tauri.conf.json:plugins.updater.pubkey`. **If you rotate the keypair, existing installs cannot verify the new signatures — users need one manual reinstall to pick up the new compiled-in pubkey.**

Regenerate with `npx @tauri-apps/cli signer generate --ci --password <pwd> --write-keys .secrets/prinny-updater.key --force`.

### Per-platform updater archive format

| Platform | Updater archive | Sig | Install mechanism |
|---|---|---|---|
| Windows | `.nsis.zip` | `.nsis.zip.sig` | Runs the embedded NSIS `.exe` installer in installer-mode |
| Linux | `.AppImage.tar.gz` | `.AppImage.tar.gz.sig` | Replaces the running AppImage (only works if installed AS AppImage; .deb installs can't auto-update) |
| macOS | `.app.tar.gz` | `.app.tar.gz.sig` | Replaces the .app bundle. Universal binary is reused for both `darwin-x86_64` and `darwin-aarch64` |
| Android | `.apk` | `.apk.sha256` (not minisign) | Native `UpdateChecker.kt` via `DownloadManager` + install prompt |

### Flow

1. Every desktop CI job builds with `--config '{"bundle":{"createUpdaterArtifacts":"v1Compatible"}}'` and the two secrets in env. Tauri produces the platform's updater archive + `.sig`.
2. `scripts/release.mjs` walks the release assets, pairs each archive with its sig, emits one entry in `platforms` per platform — only when BOTH the archive and the sig were uploaded.
3. On launch, `useUpdateCheck` calls `@tauri-apps/plugin-updater` `check()`. Tauri picks the right platform entry, downloads the archive, verifies against the compiled-in pubkey, extracts, replaces, relaunches.

### Key files

| File | Role |
|---|---|
| `src-tauri/tauri.conf.json:plugins.updater.pubkey` | Compiled-in minisign pubkey |
| `.github/workflows/build.yml` (windows-x86_64, linux-x86_64, macos-universal jobs) | `createUpdaterArtifacts:"v1Compatible"` + signing env, uploads updater archive + `.sig` |
| `scripts/release.mjs` | Generates `release.json` — emits each desktop platform only when both archive + sig are present |
| `cinny/src/app/hooks/useUpdateCheck.ts` | Tauri plugin-updater `check()` on all desktop targets |
| `.secrets/` | Local copies of the minisign keypair + password (gitignored) |

### Pitfalls

- Emitting a platform with an empty signature (e.g. via the old `{ signature: '', ...obj }` fallback) makes the minisign decode error fire for every user on that platform. The `emit()` helper in `release.mjs` requires BOTH fields.
- `--config` overrides apply at build time only — the compiled-in pubkey wins at runtime. If you ever need to disable verification temporarily, remove the pubkey from `tauri.conf.json` directly.
- The `mv` for `.sig` files in CI has no fallback — missing sig means the secrets aren't set, and we want the build to fail loudly rather than silently ship an unsigned release that breaks the updater for everyone.
- macOS .app.tar.gz is universal — same artifact serves both `darwin-x86_64` and `darwin-aarch64` in `release.json`.
- Linux AppImage updater only works if the user is running the `.AppImage`. Users installed via `.deb` can't auto-update through Tauri — they need apt/manual reinstall.

## Common gotchas (lessons from broken CI builds)

### Removing a Rust module (e.g. yt-dlp)

When removing a Rust feature, you MUST hit ALL of these or CI breaks:

| Thing to remove | Location | Missed? CI result |
|---|---|---|
| `mod` declarations | `lib.rs` top | Compile error: can't find module |
| `generate_handler![]` entries | `lib.rs` builder chain | Compile error: unresolved import |
| Plugin function | `lib.rs` | Compile error (if still called) |
| `.plugin(...)` call | `lib.rs` builder chain | Compile error: function not found |
| Cargo dependencies | `Cargo.toml` | Unused dep warning (not fatal, but sloppy) |
| Kotlin plugin file | `src-tauri/gen/android/.../*.kt` | **Gradle discovers it anyway — build fails with Kotlin compilation errors** |
| Frontend utilities | `cinny/src/` | Vite build fails if still imported |
| Settings fields | `settings.ts` + `General.tsx` | Vite build fails if still imported |

### Deleting Kotlin files from gen/android/ (CRITICAL)

**Gradle auto-discovers ALL `.kt` files under `gen/android/app/src/main/java/`.** Even if nothing in Rust references the plugin anymore, Gradle compiles every Kotlin file it finds. If the file has bitrotted (unresolved references, type mismatches, stale API calls), the Android CI build fails.

**The fix:** `git rm` the file. Just `rm`-ing from disk isn't enough — git still tracks it, and CI checks out the committed version. The deletion must be committed and pushed.

```bash
git rm src-tauri/gen/android/app/src/main/java/in/prinny/app/YtDlpPlugin.kt
git commit -m "Remove YtDlpPlugin.kt (no longer referenced by lib.rs)"
```

### Submodule workflow (CRITICAL — two-step push)

When you change files inside `cinny/src/`:

```bash
# Step 1: Commit and push in the submodule (to the main branch)
cd /opt/openclaude-src/prinny-client/cinny
git add -A
git commit -m "Fix: describe your change"
git push origin main

# Step 2: From the MAIN REPO ROOT, update the submodule pointer
cd /opt/openclaude-src/prinny-client    # ← MUST be at repo root
git add cinny
git commit -m "Update cinny submodule"
unset GITHUB_TOKEN && git push
```

**If `git add cinny` says "pathspec did not match any files":** you're inside the submodule directory. `cd` to the main repo root first. The `cinny/` path only exists from the parent repo's perspective — inside the submodule it's just `.`.

### folds Text component renders as `<p>` (block element)

`Text` from folds defaults to `as="p"`, which is `display: block`. Nesting a block element inside `inline-flex` containers (like keycap pills) causes garbled/layered text. Use a plain `<span>` with inline styles for inline keycap labels instead.

```tsx
// WRONG — Text renders as <p>, breaks inline-flex layout
<Box style={{ display: 'inline-flex' }}>
  <Text size="T200">{key}</Text>
</Box>

// RIGHT — plain span inherits inline-flex context
<span style={{ display: 'inline-flex', fontSize: '12px' }}>
  {key}
</span>
```

### React fragments in flex layouts break vertical stacking

`<>...</>` (React fragments) unwrap children directly into the parent DOM element. In a `display: flex; flex-direction: row` container (like CompactLayout), siblings become flex items side by side. This makes URL previews appear to the right of message text instead of below.

```tsx
// WRONG — fragment siblings become flex items in parent row
<>
  <MessageTextBody>...</MessageTextBody>
  <UrlPreviewHolder>...</UrlPreviewHolder>
</>

// RIGHT — Box direction="Column" forces vertical stacking
<Box direction="Column">
  <MessageTextBody>...</MessageTextBody>
  <UrlPreviewHolder>...</UrlPreviewHolder>
</Box>
```

This applies to `MText`, `MEmote`, and `MNotice` in `MsgTypeRenderers.tsx` — all three were fixed.

### CSP needs explicit frame-src for iframes

Without `frame-src`, the CSP falls back to `default-src`. While our `default-src` is permissive, being explicit prevents surprises. YouTube/Twitter iframe embeds need:

```json
"csp": "... frame-src 'self' https: http:; ..."
```

### blob: URLs can't be opened externally

The OS doesn't understand `blob:` scheme URLs. Two fixes needed:

1. **Rust new-window handler** — skip `opener().open_url()` for blob: URLs
2. **Frontend click interceptor** — catch clicks on `<a href="blob:...">` and trigger a download via a temporary anchor element

```rust
// lib.rs on_new_window handler
if url.scheme() != "blob" {
    let _ = app_handle.opener().open_url(url.as_str(), None::<&str>);
}
NewWindowResponse::Deny
```

### HTML5 drag-and-drop is disabled by default in Tauri v2

Tauri v2 webviews intercept native OS drag-and-drop and emit Tauri events to Rust — but the WebView's browser-level `dragenter`/`dragover`/`drop` events do **not** fire. Any frontend `useEffect` registering a global `drop` listener silently never runs.

The unlock is one call on `WebviewWindowBuilder`:

```rust
window_builder = window_builder.disable_drag_drop_handler();
```

After this, dropped files arrive as a normal `DragEvent` with a populated `dataTransfer.files` — the frontend `useGlobalDropListener` in `src/app/pages/App.tsx` routes them through `setGlobalDropHandler`, which `RoomInput.tsx` registers to push files into the attachment list as if the paperclip button had been used.

### Twitter/X media: a `<video>` CANNOT strip its own Referer — fetch it to a blob

`video.twimg.com` returns 403 on any cross-origin Referer. Past attempts:

1. Direct `<video src="https://video.twimg.com/...">` → 403 (Referer header sent)
2. `<iframe src="https://fxtwitter.com/u/status/123">` → fxtwitter returns the full Twitter SPA, not a media player — video doesn't render in an iframe
3. `<video src={mediaURL} referrerPolicy="no-referrer">` → **also 403.** This one
   looked like it worked and did not. `referrerpolicy` is a content attribute on
   `<img>`, `<iframe>`, `<link>`, `<script>` and `<a>` — the HTML spec defines
   nothing of the sort on media elements, and `'referrerPolicy' in
   HTMLVideoElement.prototype` is `false`. React writes the attribute, the
   engine ignores it, the document policy applies, twimg 403s. Symptom: Twitter
   GIFs/videos dead with `MediaError code 4` while ordinary GIF links played
   fine — because those render as `<img>`, where the attribute IS honoured.

Working approach:

1. Detect Twitter/X URL in `getTwitterId` (`UrlPreviewCard.tsx`)
2. Client-side `fetch('https://api.vxtwitter.com/.../status/{id}')` (CORS-friendly)
3. Fetch the media itself with the referrer stripped and hand the element a
   blob — `useResolvedMediaSrc` in `GifMedia.tsx`. Inside the Tauri shell that
   is the Rust `fetch_remote_bytes` proxy; on the web (and as the shell's
   fallback) it is `fetchNoReferrerBlobUrl`, i.e. `fetch(url, { referrerPolicy:
   'no-referrer' })`. `fetch()` does honour the policy where a media element
   cannot, and twimg serves CORS, so this is the one path that works in a
   browser.
4. Do **not** set `crossOrigin` — it buys nothing here (it does not affect the
   Referer, which is the thing twimg checks).

Measured in Chromium at `https://prinny.app`, same URL, same session:

| request | result |
|---|---|
| `<video referrerpolicy="no-referrer" src=…mp4>` | 403, `MediaError code 4` |
| `fetch(…, { referrerPolicy: 'no-referrer' })` | 200, `video/mp4`, plays from blob |

### folds `<Scroll>` does not scroll inside `<Modal flexHeight>` when it is wrapped

`flexHeight` sets `height: unset` on the modal, so no ancestor has a definite
height and folds' `Scroll { height: 100% }` cannot resolve its percentage — it
falls back to `auto`, grows to its full content height, has nothing to scroll,
and the modal's `overflow: hidden` clips the rest. A long list simply ends flat
with no scrollbar.

It only bites when the `Scroll` is wrapped in a `<Box grow="Yes">`. As a
*direct* flex child of the modal column it is flex-sized and shrinks correctly,
which is why some modals were fine and others were not:

| shape | scrolls? |
|---|---|
| `Modal > [Header, Scroll]` | yes — leave alone |
| `Modal > Box(col) > [Header, Box grow > Scroll]` | **no** — needs the fix |

Fix is `ModalFlexScroll` in `cinny/src/app/styles/Modal.css.ts` (`height: auto;
flex: 1 1 0; min-height: 0`) on the `<Scroll>`. **Do not apply it to the direct
shape** — there it collapses the modal to header height, because a `flex-basis:
0` child leaves the content-sized modal with no content to size to.

### Use cargo check before committing Rust changes

A full platform build takes 20+ minutes in CI. `cargo check` catches compilation errors in seconds:

```bash
source ~/.cargo/env && cargo check 2>&1 | grep "^error"
```

### Verify npm build before pushing

Vite catches import errors and type issues. Runs in under a minute:

```bash
cd cinny && npm run build 2>&1 | tail -5
# Should end with: ✓ built in ...
```
