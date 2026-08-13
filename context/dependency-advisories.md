# Dependency advisories

Standing decisions on Dependabot alerts that cannot be fixed by upgrading, with
the evidence behind each one. The point of this file is that the next person to
see the alert — or the next agent — does not have to re-derive the answer, and
does not "fix" it by pinning something that breaks the build.

**Re-check whenever Tauri publishes a minor release.** Both entries below are
waiting on the same thing: Tauri's Linux stack moving off gtk-rs 0.18.

## Currently dismissed

| Alert | Package | Severity | Reason | Waiting on |
|---|---|---|---|---|
| #3 | `rand 0.7.3` (GHSA-cq8v-f236-94qc) | low | not used | nothing — not shipped |
| #1 | `glib 0.18.5` (GHSA-wrw7-89jp-8q8g) | medium | tolerable risk | Tauri → gtk-rs 0.20 |

### `rand 0.7.3` — build-time only, not in any binary

Chain: `tauri-utils` → `kuchikiki` → `selectors 0.24` → *(build-dependency)*
`phf_codegen 0.8` → `phf_generator 0.8` → `rand 0.7.3`.

`selectors` uses `phf_codegen` from its **build script**, to generate perfect
hash tables at compile time. The crate never links into a shipped binary.
Verified rather than assumed, with the control run alongside it:

```console
$ cargo tree -i rand@0.7.3 -e normal
warning: nothing to print.

$ cargo tree -i rand@0.7.3 -e normal,build
rand v0.7.3
└── phf_generator v0.8.0
    └── phf_codegen v0.8.0
        [build-dependencies]
        └── selectors v0.24.0
            └── kuchikiki v0.8.8-speedreader
                └── tauri-utils v2.9.3
```

The advisory additionally requires a custom `log` logger that calls
`rand::rng()` and triggers a reseed inside it. A perfect-hash generator installs
no logger. Two independent reasons it cannot bite.

No fix is possible in place either: `0.7.3` is the last `0.7.x`, the fix is in
`0.8.6`, and `phf_generator 0.8` requires `rand ^0.7`.

### `glib 0.18.5` — real, but there is nowhere to upgrade to

Chain: `tauri` → `tray-icon` → `libappindicator` → `gtk 0.18` → `glib 0.18.5`,
and separately through `tao`/`wry` for the webview itself. It **is** in the
shipped Linux binary — this one is not hand-waved away.

Why it stays anyway, all four checked:

1. `glib 0.18.5` is the **last** `0.18.x`. There is no patched release in that
   range; the fix landed in `0.19.9` and `0.20.0`.
2. The whole gtk-rs 0.18 stack (`atk`, `cairo-rs`, `gdk`, `gio`, `gtk`, `pango`,
   `soup3`, `javascriptcore-rs`, `webkit2gtk`) requires `glib ^0.18`, so cargo
   cannot move it.
3. We are already on the latest `tauri` (2.11.5) and `tauri-utils` (2.9.3).
4. Even the newest `tao` (0.36.0), which Tauri does not use yet, **still
   requires `gtk ^0.18`**. So the fix does not exist upstream at all yet, and
   waiting for a Tauri patch release would not deliver it.

Scope: Linux only — the gtk crates are not in the Windows, macOS or Android
graphs. The impact is a `NULL` dereference crash in `VariantStrIter`, not
memory disclosure or code execution, and only if something iterates a `GVariant`
string array.

Forcing it with `[patch.crates-io]` was rejected: `glib 0.20` is an API break
for gtk-rs 0.18, so it trades a latent crash for a build that does not compile.

## When re-checking

```console
$ cd src-tauri
$ cargo tree -i glib -e normal --target x86_64-unknown-linux-gnu
$ curl -s https://index.crates.io/3/t/tao | tail -1 | python3 -c \
    "import sys,json; print([d for d in json.load(sys.stdin)['deps'] if d['name']=='gtk'])"
```

When that last command shows `gtk ^0.20` and Tauri has released against it,
`cargo update` should clear alert #1 on its own.
