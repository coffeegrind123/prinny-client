# Custom CSS

Settings → General → **Custom CSS**. Two layers, both applied after every app
stylesheet, stored in `localStorage` on this device only:

```
app CSS  ->  overrides (from the edited full file)  ->  snippets (settings box, verbatim)
```

The settings box stays empty by default and holds snippets. The full stylesheet
is edited outside the app: **Edit in Text Editor** hands the user Prinny's
complete CSS (~0.3 MB, ~8k lines, with the user's changes merged in) and every
save is imported.

## Why only the difference is stored

Storing the whole edited file would freeze every rule at the version it was
exported from and silently undo later style updates. On import the file is
diffed against the live stylesheet and only changed/new declarations and rules
are kept (`cinny/src/app/features/custom-css/cssModel.ts`).

- Deleting a rule or line from the file reverts it to the default; it never
  removes base styling. Hiding needs `display: none`, dropping a property `unset`.
- Both sides are parsed by the browser engine (an inert-document `<style>`), so
  equality is between canonical serialisations (`#FFF` == `rgb(255, 255, 255)`).
- Declarations are read from `style.cssText`, not longhands: a shorthand with
  `var()` expands to empty "pending substitution" longhands, which would make
  two different `var()`s compare equal.
- Invariant, verified in Chromium against the real build (1,567 rules
  including KaTeX/MapLibre/Prism): exporting and re-importing an unedited file
  yields **zero** overrides.

## Why class names are readable and stable

The file only works across updates if selectors survive them. vanilla-extract's
default names are position-derived hashes (`._10dxgc60`), so:

- `cinny/scripts/vite-readable-css.mjs` sets a custom `identifiers` function:
  `<file scope>_<debug id>` — `.RoomViewHeader_HeaderTopic`,
  `--folds-color_Background-Container`. Generic filenames (`style.css.ts`) use
  the directory name. The build **fails** if two styles produce one name.
- vanilla-extract only derives debug ids from variable names in its own
  `'debug'` mode, so a `pre` plugin runs its babel debug-id pass inside the
  compiler (admitted via `unstable_pluginFilter`), plus a small pass naming
  styles under literal object keys (`RadiiVariant['300']` → `RadiiVariant_300`).
- folds is vendored as source (`cinny/vendor/folds`, v2.7.1) because the npm
  package ships precompiled hashes. The built CSS was verified rule-for-rule
  and in the same order against the npm build. Local patches: its README.

## Base stylesheet

Read at runtime from `document.styleSheets` (`baseStylesheet.ts`), after
force-loading the three lazily loaded sheets (KaTeX, MapLibre, Prism) so the
file does not depend on which features were used this session. Same code in
dev and prod, and it is exactly what the running engine applies.

The injected layers are kept the **last** stylesheets in `<head>` by a
`MutationObserver`: lazy CSS appended later would otherwise win ties.

## Platform backends (`externalEditor.ts`)

| Platform | Flow |
|---|---|
| Tauri desktop | `custom_css_edit` (`src-tauri/src/custom_css.rs`) writes `<app data>/custom-css/prinny.css`, opens it with the default editor, polls mtime+size every 500 ms and emits `custom-css-changed` with the content. Listened to for the page's lifetime, so saves apply with settings closed. Fixed path, never from the page. |
| Android | `CustomCssEditorPlugin.kt`: writes `filesDir/custom-css/prinny.css`, `ACTION_EDIT` chooser through the FileProvider (`custom_css` path) with read+write grant, reads back on return. `<queries>` in the manifest makes editors visible to `queryIntentActivities`. Export/Import via the system document picker for editors that do not write back. |
| Chromium web | File System Access: save picker, handle polled every second. |
| Other browsers | Download + Import. |

## Not reachable from custom CSS

- Inline `style={{…}}` props (~900) need `!important`; colour ones mostly use
  theme variables, which can be redefined instead.
- Element Call and widget iframes are separate documents.
- Monochrome mode sets `body.style.filter` directly.

## Security

Local only, never account data: CSS can exfiltrate page content through
attribute selectors that load URLs, so synced CSS would turn an account
compromise into reading every client's screen.
