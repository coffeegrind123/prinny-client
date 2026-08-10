# prinny-client Upstream Backport Log

Tracks upstream `cinnyapp/cinny-desktop` commits reviewed for this fork
(`coffeegrind123/prinny-client`, branch `main`).

The cinny **submodule** has its own log — `cinny/UPSTREAM_BACKPORT_LOG.md`. This
file covers the Tauri desktop shell only.

**Last sync:** 2026-08-10 (upstream `main` @ `5c57861`, v4.12.6)
**Start from:** `5c57861` — next time, fetch upstream and check commits AFTER this one

Status: `[x]` backported · `[-]` skipped · `[~]` partial/adapted · `[ ]` pending

## Why so much is skipped

The shell has diverged much further from upstream than the frontend has. Three
whole subsystems are ours and have no upstream counterpart, so upstream commits
touching their equivalents are **not applicable** rather than "skipped as noise":

| Subsystem | Ours | Upstream's |
|-----------|------|------------|
| CI / release | `.github/workflows/build.yml` + `auto-bump-cinny.yml`, `scripts/bump-version.mjs`, `release-notes.mjs`, `rename-prinny.mjs` | `tauri.yml`, `tauri2.yml`, `archive.yml`, `test.yml`, `scripts/update-version.mjs` — none of which exist here |
| Updater | frontend-driven; the Rust side only registers `tauri_plugin_updater` under `#[cfg(not(mobile))]` | a Rust-side `check()` in `setup()`, later gatekept behind a `updater` cargo feature |
| Versioning | independent line (4.11.x), stamped from the release tag by `bump-version.mjs` | semantic-release-style version stamps per release PR |

Android is ours outright — upstream cinny-desktop has no mobile target.

---

## 2026-08-10 sync session

Range: `24d34c7..upstream/main` (`5c57861`) — 27 commits.

| # | SHA | Status | Description | Notes |
|---|-----|--------|-------------|-------|
| 1 | `8b7bced` | `[-]` | chore: Release v4.12.1 (#577) | Release/version stamp — independent version line |
| 2 | `2b61520` | `[~]` | fix: update CSP to allow reordering rooms inside space (#579) | **Took the CSP half.** Our CSP had no `style-src` at all, so it fell back to `default-src`, which lacks `'unsafe-inline'` — pragmatic-drag-and-drop sets inline styles on the drag preview, so reordering rooms inside a space was broken here too. **Did not take `.disable_drag_drop_handler()`**: we keep Tauri's native drag-drop handler on purpose, because WebView2 hands JS zero-byte `File` stubs from `dataTransfer.files` when the OS path is bypassed. See the comment above the window builder in `src-tauri/src/lib.rs` |
| 3 | `9809657` | `[-]` | chore: add script for updates (#578) | Adds upstream's `scripts/update-version.mjs`; we have `scripts/bump-version.mjs`, which resolves the version from the release tag instead |
| 4 | `36887ea` | `[-]` | chore: Release v4.12.2 (#580) | Release stamp |
| 5 | `a823e45` | `[-]` | chore: fix script to not break on windows (#581) | Fix to `update-version.mjs`, which we don't have |
| 6 | `b670ef0` | `[-]` | chore: update config.json to match with web code (#583) | Featured-communities curation (adds LibreWolf/stickers/videogames spaces, swaps `#cinny` for `#tuwunel`). Editorial, not a fix — same call as upstream cinny `0b99d85` |
| 7 | `9d12943` | `[-]` | chore: use shx for cross platform compatibility (#582) | Supports `update-version.mjs`; ~595 lockfile lines for tooling we don't run |
| 8 | `c776640` | `[-]` | fix: add tauri updater check (#584) | N/A — adds a Rust-side updater check in `setup()`. Ours is frontend-driven |
| 9 | `36e3df8` | `[x]` | chore: remove node-fetch and update actions/github to v9.1.1 (#587) | Applies directly. `scripts/release.mjs` imported `node-fetch` although the release job runs on Node 22, where global fetch has always been available; node-fetch v3 is fetch-spec-compatible so `getAssetSign()` is unchanged. Bumped `@actions/github` 6.0.0 → 9.1.1 with it |
| 10 | `cc886d9` | `[-]` | chore(deps): lock file maintenance (#589) | Upstream's dep set, not ours (they have no tauri-plugin-mobile-push, no Android) |
| 11 | `8b61a21` | `[-]` | chore(deps): lock file maintenance (#590) | Same |
| 12 | `a7b9c0c` | `[-]` | chore: Release v4.12.3 (#591) | Release stamp |
| 13 | `a81397d` | `[-]` | fix(ci): specify tag name explicitly | Touches `archive.yml`/`tauri.yml` — neither exists here |
| 14 | `860dd37` | `[-]` | chore: Release v4.12.4 | Release stamp |
| 15 | `3236388` | `[-]` | fix: skip the updater silently to stop crash on flatpak (#596) | N/A — guards the Rust-side updater check from `c776640`. We don't have it, and we don't ship a flatpak |
| 16 | `0660885` | `[~]` | chore: update script to bump version in cargo.lock (#595) | **Adapted.** The underlying bug was real here: `bump-version.mjs` stamped `Cargo.toml` but not `Cargo.lock`, so a `--locked`/`--frozen` build would fail and a normal build would leave CI with a dirty tree. Upstream shells out to `cargo update --workspace`; we patch the `name = "cinny"` entry directly so the script stays offline-safe — it runs as `beforeBuildCommand` in every platform job, including Android |
| 17 | `e26c1fe` | `[-]` | fix: gatekeep updater behind feature flag (#598) | N/A — cargo-feature-gates the Rust updater we don't have. Also untracks `src-tauri/gen/schemas/*`; we keep those tracked because our Android build depends on the generated tree |
| 18 | `0330391` | `[-]` | chore: Release v4.12.5 (#597) | Release stamp |
| 19 | `733a262` | `[-]` | chore: bump softprops/action-gh-release 3.0.0 → 3.0.1 (#594) | Touches `archive.yml`/`tauri.yml` only |
| 20 | `fb48aea` | `[-]` | chore: bump actions/checkout 6.0.2 → 7.0.0 (#593) | Mostly upstream-only workflows. `lockfile.yml` is shared, but checkout v7 is a runtime-major bump and we can't exercise our release pipeline from here — deliberately deferred rather than bumped blind. **Revisit next sync** |
| 21 | `7424e8a` | `[-]` | chore: build binary for windows without updater (#599) | Adds `tauri2.yml` + a patch file to build an updater-less Windows binary. Our Windows build always ships the updater |
| 22 | `6be0078` | `[-]` | chore: Revise installer download links in README (#600) | Docs, upstream's links |
| 23 | `b699b19` | `[-]` | chore: Add hint to have signature decoded for verification (#606) | Docs |
| 24 | `2cd5829` | `[x]` | fix: bypass proxy for embedded localhost UI (#605) | Applies directly — we serve the frontend through `tauri-plugin-localhost` on 127.0.0.1:44548 exactly as upstream does, so an env-set HTTP proxy would swallow the request and the window would come up blank. Dropped the trailing whitespace upstream left behind and documented the loop |
| 25 | `69e8640` | `[-]` | chore: bump softprops/action-gh-release 3.0.0 → 3.0.2 (#607) | Upstream-only workflows |
| 26 | `0f08c3a` | `[x]` | fix: permanently white MacOS titlebar (#610) | Applies — we ship a macOS universal build. Adapted: upstream collapsed the builder into one expression, which here would have dropped our `#[cfg(not(mobile))]` title/inner_size blocks and re-added `.disable_drag_drop_handler()`. Kept our `mut` builder and added the macOS branch as another cfg block; gated the `TitleBarStyle` import on macOS since `title_bar_style` is itself macOS-only |
| 27 | `5c57861` | `[-]` | chore: Release v4.12.6 (#611) | Release stamp — **START HERE next sync** |

---

## Summary

- **Backported:** 4 commits (2 clean, 2 adapted)
- **Skipped:** 23 commits (8 release stamps, 6 upstream-only CI, 5 updater/tooling N/A, 2 docs, 2 lockfile maintenance)
- **Verified:** `cargo check` clean on `x86_64-unknown-linux-gnu`; `tauri.conf.json` parses

### Not verified by this sync

- The **macOS** branch (`0f08c3a`) is `#[cfg(target_os = "macos")]`, so `cargo check`
  on Linux compiles it out, and cross-checking against `x86_64-apple-darwin` fails in
  the dev container (`objc2-exception-helper` needs a real Apple SDK). What *was*
  checked statically, against `tauri-2.11.5` source: `TitleBarStyle` is exported
  unconditionally from `tauri`, `WebviewWindowBuilder::title_bar_style(self, TitleBarStyle) -> Self`
  exists under `#[cfg(target_os = "macos")]`, and the `Transparent` variant exists in
  `tauri-utils`. First macOS CI run is the real check.
- The **Windows** target can't be checked here either — `cargo check --target
  x86_64-pc-windows-gnu` panics in `tauri-winres` because `x86_64-w64-mingw32-windres`
  isn't installed in the container. The changed code is not Windows-specific.

## Do NOT do a `-X ours` SHA-sync merge in this repo

The submodule clears its "N commits behind" badge with
`git merge upstream/dev -X ours` after each sync. **That does not transfer here** —
it was attempted this sync and aborted. `-X ours` only decides *conflicting*
hunks; non-conflicting upstream hunks still apply, and in this repo they land in
`src-tauri/src/lib.rs`, `Cargo.toml` and `Cargo.lock` — precisely where we diverge.
The trial merge produced:

- **A duplicate updater registration.** Upstream's `#[cfg(feature = "updater")]`
  plugin block merged in *alongside* our `#[cfg(not(mobile))]` one — both active on
  desktop, registering `tauri-plugin-updater` twice.
- **Upstream's blocking-dialog update check** re-added to `setup()`, on top of our
  frontend-driven updater — two update prompts.
- **`tauri-plugin-updater` made `optional = true`** behind a new cargo feature,
  while our unconditional `#[cfg(not(mobile))]` block still references it — doesn't
  compile without the feature.
- **The window builder re-broken** the same way the `0f08c3a` cherry-pick was
  (stray `;`, duplicated macOS line) — doesn't compile either.
- **`src-tauri/gen/schemas/macOS-schema.json` deleted** (7093 lines) and
  modify/delete conflicts on four more schema files we track for the Android build.
- **`Cargo.lock` reverted** to upstream's dependency set (1529 lines).

So this repo **stays showing as behind on GitHub**, by choice. The badge is
cosmetic; this log is the real record of what was and wasn't taken. Re-evaluate
only if the updater and CI subsystems ever converge with upstream again.

## Process (reference for future syncs)

```bash
# 1. Fetch upstream (one-time: git remote add upstream https://github.com/cinnyapp/cinny-desktop.git)
git fetch upstream

# 2. Check what's new since the "START HERE" marker above
git log --oneline 5c57861..upstream/main --reverse

# 3. Triage against the "Why so much is skipped" table — most upstream churn
#    lands on CI/updater/versioning subsystems we replaced wholesale.
# 4. Cherry-pick the rest oldest-first: git cherry-pick -n <sha>
# 5. Resolve conflicts, then RE-READ the whole resolved region. `-n` leaves the
#    tail of a hunk auto-merged OUTSIDE the conflict markers, so "no markers
#    left" does not mean "coherent code" — that is how a moved-value error got
#    committed during this sync.
# 6. cargo check BEFORE committing (see "Critical: working directory" in CLAUDE.md)
# 7. Update this file, move the START HERE marker, push main.
```
