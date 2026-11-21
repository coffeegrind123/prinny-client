# Liquid design

Settings → General → Appearance → **Design**: `Prinny` (default) or `Liquid`.
Stored as `uiDesign` in the settings blob. An unknown value reads as Prinny.

Liquid keeps every screen, column and control where Prinny has it. It changes
how they are drawn: glass panels floating on an ambient backdrop, an opaque
content card, capsule controls, iOS system colours and spring motion. The
visual reference is [nshelia/liquid-gpu](https://github.com/nshelia/liquid-gpu)
(an iOS 26/27 "Liquid Glass" kit for the web). **No code from it is used:** the
repository has no licence (its README lists "Choose a license" as a TODO), so
it is all rights reserved. Values below were re-derived from its documented
metrics and its CSS-fallback recipe, and implemented from scratch.

Its WebGPU renderer was not an option either way. It needs one full-viewport
`<canvas content="drawable">` owning every surface and Chrome's HTML-in-Canvas
API. WebKitGTK (the Linux shell) and Firefox have neither. Liquid uses
`backdrop-filter`, the reference's own fallback, which every webview Prinny
ships in supports.

## How it is wired

```
settings.uiDesign ──► useDesign() ──► ThemeManager ──► <body class="… prinny-liquid [prinny-liquid-dark]">
                                                              │
               src/app/design/liquid/*.css.ts  ◄──────────────┘  every rule is scoped under body.prinny-liquid
```

| File (`cinny/src/app/design/`) | What |
|---|---|
| `scope.ts` | `Design` enum, the body classes, `LIQUID` / `LIQUID_DARK` selector prefixes, `designClassNames()` |
| `useDesign.ts` | Reads the setting, validates it |
| `DesignSelector.tsx` | The segmented control in Appearance (a radio group; arrow keys move it) |
| `spring.ts` | Damped-spring step response sampled into CSS `linear()` easings; named springs |
| `liquid/LiquidTokens.css.ts` | The `design` contract (glass, backdrop, motion, radii, gutter) plus Liquid's values for folds' own `color` and `config` contracts, per appearance |
| `liquid/LiquidComponents.css.ts` | Overrides for folds components (Button, IconButton, Chip, Input, TextArea, Switch, Checkbox, Radio, Badge, Menu, MenuItem, Tooltip, Dialog, Modal, Overlay, Scroll) |
| `liquid/LiquidShell.css.ts` | The app shell: backdrop, rail, panels, gutters, page card, nav rows, composer, messages, settings cards, sign-in |
| `liquid/materials.ts`, `liquid/selector.ts` | Shared glass declarations; selector helpers for vanilla-extract classes |

### Why a scoped layer and not a second set of components

- **Prinny stays byte-for-byte what it was.** Nothing in the default design
  reads a Liquid token. The only edits to existing files are hooks: empty
  `PageRoot` / `Page` classes in `components/page/style.css.ts` and a
  `data-own` attribute on own messages in the bubble layout.
- **Switching is one class change.** No remount, no lost scroll position or
  composer draft.
- **Upstream syncs stay cheap.** Liquid lives in its own directory. The
  conflict-heavy files (`RoomNavItem`, `Room.tsx`, `UrlPreviewCard`) are not
  touched, and neither is `vendor/folds`: its README promises a rule-for-rule
  identical build, so the overrides import folds' stylesheets and target their
  classes from outside.

### Specificity

Every Liquid selector starts with `body.prinny-liquid` (0,1,1 before the
component part). That beats a folds recipe base plus a variant (0,2,0 at most)
whatever order the stylesheets load in. Liquid's token values are assigned on
`body.prinny-liquid`, which outranks the single theme/config class on `<body>`.

A vanilla-extract composed class is a space-separated list. Only the first
entry is the style's own identifier, so selectors go through `sel()` /
`base()` in `liquid/selector.ts`. Interpolating the whole string would produce
a descendant selector that silently matches nothing.

## Colours

Liquid brings its own palettes and follows only the **kind** (light/dark) of
the active theme. That includes the system theme and the More Themes picks:
Butter or Dracula selected means Liquid dark. Glass tinted from arbitrary
theme colours reads badly, and the palette's translucent chrome layer depends
on exact alpha values.

- **Content layer** (`Surface`, `SurfaceVariant`): opaque iOS system
  backgrounds (#FFFFFF / #F2F2F7 light, #1C1C1E / #2C2C2E dark). Content stays
  legible and cheap to draw.
- **Chrome layer** (`Background`): translucent, so the glass behind it shows
  through. Hover and active states are translucent fills (rgba 120,120,128),
  as on iOS.
- **Accents:** systemBlue, Green, Orange, Red. On light backgrounds the text
  colours use the accessible variants (#0066D6 on lens, #248A3D, #C93400,
  #D70015).

## Glass

| Material | Used by | Light | Dark |
|---|---|---|---|
| Regular | rail, room list, member list, composer | white 58 %, `blur(20px) saturate(1.8)` | rgb(40,40,44) 52 %, `saturate(1.6)` |
| Thick | menus, tooltips, dialogs, sign-in card | rgb(249,249,251) 84 %, `blur(32px)` | rgb(34,34,38) 84 % |

Edge (light): 0.67 px dark ring at 8 %, 1.33 px white line at 70 %, top
highlight. Edge (dark): top key light 28 %, bottom fill 8 %, 0.5 px ring
12 %. Both follow the reference's fallback structure (ring plus rim insets),
softened for large flat desktop panels.

Backdrop blur only pays off where content moves underneath. The big panels sit
over a static gradient, so their blur costs one pass per panel at
compositing. That is fine on desktop, but it is the first thing to drop if a
low-end device struggles.

**Do not write `WebkitBackdropFilter` next to `backdropFilter`.** The CSS
minifier treats the pair as duplicates when both hold the same `var()` and
keeps only the prefixed one, which Chromium ignores. Menus then shipped with
no blur at all; the built rule read
`-webkit-backdrop-filter:var(--…ThickFilter)` and nothing else. Prefixing is
left to the build targets. A WebKitGTK too old for the unprefixed property
gets the translucent fill without the frost, which is still legible.

## Shape

Radii (from the iOS metrics the reference cites): panel / inset-grouped 26,
sheet 28 (iOS 38, reduced for desktop modals), menu 22, row 12, field 14,
bubble 20. folds' `R300/R400/R500` become 8/12/18. Rounded buttons, chips,
icon buttons and badges become capsules. `radii="0"` and `"Inherit"` are left
alone.

## Motion

`spring.ts` evaluates the closed-form step response of a damped spring
(response = undamped period, ζ = damping ratio, mass 1) and samples 50 points
into `linear(…)`. The duration is the time until the spring stays within
0.001 of rest. Named springs: smooth (ζ 1, 0.5 s), snappy (0.85, 0.5),
bouncy (0.7, 0.5), interactive (0.85, 0.15), sheet (1, 0.3441), menuOpen
(0.75, 0.35), switchKnob (1, 0.3). They are evaluated at build time inside
`LiquidTokens.css.ts`, so no JavaScript runs per frame. Low animation mode collapses
them like every other transition (`index.css`).

Used for:
- press feedback on buttons, icon buttons and chips (scale 0.96);
- the switch knob, including the stretch while pressed;
- menus opening (scale from 0.92);
- modals presenting as sheets (rise 24 px);
- the overlay fading in with a 6 px blur.

## Layout on phones

At ≤ 640 px the nav is the whole screen, so the page root drops its padding
and the panels drop their radius and shadow. The rail keeps its floating
shape.

## Checking it

- `npm test`: `design/spring.test.ts` (curve shape, settle time, overshoot)
  and `design/scope.test.ts` (class mapping, value validation).
- Live: `context/live-testing.md`. Switch designs in Settings → General and
  compare against Prinny screenshots taken from the same build. With Prinny
  selected, the body must carry no `prinny-liquid` class and no Liquid rule
  can match.
- Custom CSS: the export-then-import zero-override invariant
  (`context/custom-css.md`) holds under both designs, because Liquid rules are
  ordinary stylesheet rules with readable names.
