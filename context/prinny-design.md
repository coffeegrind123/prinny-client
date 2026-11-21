# Prinny design

The default design. It is selected when Settings → General → Appearance →
**Design** is `Prinny` (`uiDesign: 'prinny'`), which is also what an unset or
unknown value reads as. Its counterpart is `context/liquid-design.md`.

Prinny is folds (Cinny's component library, vendored at `cinny/vendor/folds`,
v2.7.2) plus Prinny's own palettes, icon set, layout and conventions. It is
flat and dense: solid fills, 1px rules between regions, small radii, and
almost no motion. Everything below is what the code does today, measured
from it, not an aspiration.

## Where it lives

| Layer | File (in `cinny/`) | What |
|---|---|---|
| Tokens | `vendor/folds/src/theme/config.css.ts` | Font, type scale, space, radii, borders, shadows, sizes, opacity (`config.*`) |
| Colour contract | `vendor/folds/src/theme/color.css.ts` | The `color.*` roles and folds' Light palette |
| Prinny palettes | `src/colors.css.ts` | Silver, Dark, Butter |
| Extra palettes | `src/app/themes/extra/` | 22 dark themes derived from upstream palettes (More Themes) |
| Font weights | `src/config.css.ts` | `onLightFontWeight` / `onDarkFontWeight` |
| Theme registry | `src/app/hooks/useTheme.ts` | Theme ids, kinds, body classes |
| Body classes | `src/app/pages/ThemeManager.tsx` | Applies the theme, the design, and the body-level settings |
| Icons | `src/folds/icons.tsx` | lucide-react behind folds' `Icons` names |
| App-wide CSS | `src/index.css` | Link and user colours, emoji font, mobile scaling, low animation |
| Shared styles | `src/app/styles/*.css.ts` | `ContainerColor`, modal scroll fix, patterns, text helpers |

**Never edit `vendor/folds` for design reasons.** Its README promises a build
that is rule-for-rule identical to the npm package, apart from the type fixes
it lists. Override from the app instead (`liquid/LiquidComponents.css.ts`
shows how), or wrap the component.

## Colour

### Roles

folds' contract has eight roles. The first three are containers; the last
five are accents with a `Main` and a `Container` set each.

| Role | Meaning in Prinny | Where |
|---|---|---|
| `Background` | Chrome: the frame around content | Space rail, room list, member list, headers of those, auth page |
| `Surface` | Content | Timeline page, settings pages, menus, modals |
| `SurfaceVariant` | Something set into content | Cards (`SequenceCard`), fields on content, code blocks, bubbles |
| `Primary` | The accent: selection, the main action | Selected chips, Send, switches |
| `Secondary` | Neutral controls | Most chips and icon buttons |
| `Success` / `Warning` / `Critical` | Status | Badges, banners, destructive actions |

Each container role has `Container`, `ContainerHover`, `ContainerActive`,
`ContainerLine` and `OnContainer`. Use the role's own hover and active colours
for interactive states. A fixed grey (`rgba(128,128,128,.15)`) over a themed
colour reads wrong on 25 themes out of 26.

**The pairing rule for fields:** a field takes the role its container does
not. A field on `Surface` is `Background`; a field on `Background` is
`Surface`. In the Light theme `Background` and `SurfaceVariant` are the same
`#F2F2F2`, so a `SurfaceVariant` field on a `Background` panel has no visible
edge at all. That was the member list search until 21.11.2026.

### Palettes

| Theme | Kind | Source |
|---|---|---|
| Light | light | folds `lightTheme` (accent `#1858D5`) |
| Silver | light | `colors.css.ts` (greyer, accent `#1245A8`) |
| Dark | dark | `colors.css.ts` (accent `#BDB6EC`, lavender) |
| Butter | dark | Dark with warm neutrals (`#1A1916` …, text `#FFFBDE`) |
| 22 extras | dark | `themes/extra`: each palette gives `bg0/bg1/bg2`, an accent, green/orange/red and `fg`; `derive.ts` computes every state colour from those |

Dark palettes use light, desaturated accents (`#85E0BA` green, `#E69D9D` red)
so they read on dark containers. That is why a control that must be a
saturated red in every theme (the redesigned call's End button,
`features/call/styles.css.ts`) carries its own hex colour.

Body classes besides the colour theme: `prism-light` / `prism-dark` (code
highlighting), the font-weight theme, and on dark themes either the theme's
own class (`.dark-theme`, `.butter-theme`) or `on-dark-variables`. Those set
`--tc-link` and the eight `--mx-uc-*` user colours in `index.css`, light
variants at 45% lightness and dark ones at 70–80%.

### Settings that act on colour

- **System Theme** picks between a light and a dark theme by OS preference.
- **Monochrome Mode** is `filter: grayscale(1)` on `<body>`.
- **More Themes** adds the 22 extras to the pickers.

## Type

Inter Variable (`config.font.Inter`, with `--font-secondary` on `body`), with
`font-variant-ligatures: no-contextual` (rsms/inter#222). Emoji use Twemoji
when that setting is on (`--font-emoji`).

| Token | Size / line | Use |
|---|---|---|
| `D400` | 45 / 52 | Display |
| `H1`–`H6` | 35/40, 25/30, 20/26, 18/24, 16/22, 14/18 | Headings; room name `H3`, `H5` with a topic |
| `T500`–`T200` | 17/24, 15/22, 14/20, 12/18 | Body text; messages `T400`, menus `T300` |
| `B500`–`B300` | 16/20, 14/18, 12/16 | Button labels |
| `L400`, `O400`, `C400` | 12/16 | Labels, overlines, captions |

Letter spacing is 0 throughout. Weights come from `config.fontWeight`, which
the font-weight theme can retune per light/dark.

## Space and shape

- **Space:** `S100`–`S700` = 4, 8, 12, 16, 20, 24, 32 px (in rem).
- **Radii:**
  - `R300` 4 px: small chips, badges, menu items.
  - `R400` 8 px: buttons, fields, menus, modals, cards. This is the default.
  - `R500` 12 px: avatars, bubbles.
  - `Pill` for pills; `Round` (50%) for circles.
  - The one place with a mixed radius: messages highlight with
    `0 R400 R400 0`, flush with the timeline edge.
- **Borders:** `B300` 1 px for every rule between regions; `B400` 1.5 px for
  outlined controls; `B600` 3 px for the focus ring.
- **Shadows:** `E100`–`E400`, built from `color.Other.Shadow`. Black at 20% in
  light, 100% in dark, so they nearly vanish on dark themes by design.
- **Opacity:** `P300` 0.75 for secondary text, `Placeholder` 0.5,
  `Disabled` 0.38.

## Layout

```
┌──────┬───────────────┬─────────────────────────────────┬──────────────┐
│ rail │ room list     │ header (54)                     │ members      │
│ 66px │ 256 (180–520, │─────────────────────────────────│ 266          │
│      │ collapses to  │ timeline                        │ (200–560)    │
│      │ a 64px rail   │                                 │              │
│      │ below 140)    │ composer                        │              │
└──────┴───────────────┴─────────────────────────────────┴──────────────┘
```

- Columns are separated by 1px `Background.ContainerLine` rules. The rules
  between resizable columns are `ResizeHandle`s: same 1px, a 7px invisible
  grab area, Primary when hovered or dragged. Sizes are in
  `hooks/useResizablePane.ts`. A thread takes the members column's place
  (360, 280–720), and call chat is 456 (300–720).
- Headers are folds `Header` size 600: 54 px, with the same rule under them
  in every column, so the three headers line up.
- The optional shell settings (unified Home sidebar, DM rail buttons, top
  bar, Rooms as a space) all default off, and `hooks/useShellLayout.ts`
  decides the layout in one place.
- **Phones (≤ 640 px, `ScreenSize.Mobile`):**
  - The nav is the whole screen and the room slides over it.
  - The root font size is 115%, which scales every rem-based token
    together.
  - The room header drops two toolbar buttons. Members is desktop-only.
    Gallery is hidden because More Options already has it.
  - The unencrypted warning shrinks to its icon, because the toolbar
    leaves the room name very little width.

## Messages

| Layout | Shape |
|---|---|
| Modern (default) | Avatar column, name + time line, body |
| Compact | Time, name and body on one line |
| Bubble | Avatar, name, and a `SurfaceVariant` bubble with a corner arrow; `R500` radius |

Message spacing is a setting (`S0`–`S500`, default `S400`). Notices render
dimmed, emotes italic in the sender's colour, and code blocks as a
`SurfaceVariant` card with a language/Copy bar.

## Icons

lucide-react, mapped onto folds' `Icons` names in `src/folds/icons.tsx`.

- **Outline only.** Lucide has no solid variants, so `filled` is honoured only
  for names in `SOLID` (just `Star`). Every other icon ignores it, so **an
  icon cannot carry state by filling**. A toggle that shows state needs two
  different glyphs (the formatting toolbar uses `Alphabet` / `AlphabetUnderline`).
- **Variants are badged.** folds has `HashLock`, `HashGlobe`, `SpaceLock`,
  `VolumeHighLock`, …; Lucide does not. `badged(Base, Badge)` draws the base
  with its bottom-right corner cut away and a half-size lock, globe, search,
  plus or X badge there. Before 21.11.2026 every variant mapped to its bare
  base, so a room's access never showed and an invite-only voice room showed
  a muted speaker.
- One icon per meaning within a menu. Media Previews' default state uses the
  plain `Eye`, matching how notifications use the plain `Bell`. It used a
  gear, two rows above Room Settings' gear.

## Motion

Next to none, deliberately. Short ease transitions on hover colour; a 200ms
ease-out curve for the rail's hover nudge; modals fade/scale in over 200ms;
tooltips rise 2 px over 100 ms. **Low animation mode** (`body.low-animation-mode`)
collapses every animation and transition to 0.01 ms rather than removing it,
so anything whose end state matters still reaches it.

## Branding in the source

User-visible strings in the source still say "Cinny" (`index.html`, the auth
header, `initial_device_display_name`). That is on purpose. `scripts/rebrand.mjs`
rewrites them to Prinny in the published build, which keeps upstream syncs
from conflicting on every string. Do not hand-rename them. The auth page's
dark `#1a1a1a` backdrop and dot pattern are also deliberate branding
(`c5504db6`), in every theme.

## Custom CSS

Class names and variables are readable and stable
(`.folds-Button_Button_variant_Primary`, `--folds-color_Background-Container`)
so user stylesheets survive updates. See `context/custom-css.md`. When you add
a style, give it a name that says what it is, because that name is public API
to anyone with custom CSS.

## Checklist for new UI

1. Pick the container role by meaning (chrome / content / inset), not by the
   colour you want.
2. States come from the role (`ContainerHover`, `ContainerActive`). No fixed
   hex or rgba unless the colour must not follow the theme, and then say why
   in a comment.
3. Fields take the role their container does not.
4. Sizes in tokens: `config.space`, `config.radii`, `toRem()` for anything
   else, so phone scaling applies.
5. Check it in Light **and** Dark (Light is where `Background` equals
   `SurfaceVariant`), and at 400 px wide.
6. Icons: one glyph per meaning per surface, and never rely on `filled` for
   state.
7. Check it under Liquid too: the Liquid layer styles folds components and the
   shell from outside, so a new surface may need a rule in
   `src/app/design/liquid/`.
