#!/usr/bin/env python3
"""Generate the Windows NSIS installer artwork from the Prinny app icon.

    python3 scripts/make-nsis-art.py

Writes `src-tauri/nsis/header.bmp` and `src-tauri/nsis/sidebar.bmp`, which
`bundle.windows.nsis.headerImage` / `.sidebarImage` in `src-tauri/tauri.conf.json`
point at. Both are committed, so this only needs re-running when the icon or the
wording changes.

Every constraint below was read out of the NSIS/Tauri sources, not remembered:

  * MUI_HEADERIMAGE_BITMAP       -> 150 x 57  (tauri-bundler installer.nsi:149)
  * MUI_WELCOMEFINISHPAGE_BITMAP -> 164 x 314 (tauri-bundler installer.nsi:137)
  * MUI's default BITMAP_STRETCH is "FitControl" (MUI2 Interface.nsh:65), so the
    bitmap is scaled to fill the control regardless. Emitting the exact size
    makes that a no-op rather than a resample.
  * 24-bit BI_RGB only. NSIS renders a 32-bit BMP's alpha channel as BLACK, so
    everything is composited onto an opaque ground and the written file is
    re-read and asserted before this script exits.
  * Tauri does not override MUI_BGCOLOR (default #FFFFFF) and does not define
    MUI_HEADERIMAGE_RIGHT, so the side of the header strip the tile lands on is
    up to MUI's dialog resource. The header tile therefore uses a WHITE ground
    so it blends either way. Do not make it dark.

Fonts: Inter, the face the webapp itself uses, lifted straight out of
`cinny/node_modules/@fontsource/inter` and converted in memory (needs
`fonttools` + `brotli`). Falls back to DejaVu Sans if either is unavailable, so
the script still runs on a bare checkout.
"""

import io
import os
import sys

from PIL import Image, ImageDraw, ImageFont

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICON = os.path.join(REPO, "src-tauri", "icons", "icon.png")
OUTDIR = os.path.join(REPO, "src-tauri", "nsis")
FONTDIR = os.path.join(REPO, "cinny", "node_modules", "@fontsource", "inter", "files")

# Sampled from src-tauri/icons/icon.png.
INDIGO_DEEP = (32, 24, 64)      # Prinny's body, shadow side
INDIGO_MID = (64, 48, 112)      # Prinny's body, lit side
INDIGO_LIT = (96, 96, 176)      # highlight
SCARF = (160, 64, 64)           # the scarf
CREAM = (240, 240, 240)         # belly / type on dark
SLATE = (110, 105, 130)         # muted type on white
LILAC = (196, 192, 220)         # muted type on dark

WHITE = (255, 255, 255)

SS = 4  # supersample factor: draw at 4x, resample once, keep the edges clean

_FALLBACKS = (
    "/usr/share/fonts/truetype/dejavu/DejaVuSans{}.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans{}.ttf",
)
_FALLBACK_SUFFIX = {400: "", 700: "-Bold", 800: "-Bold"}


def font(weight, size):
    """Inter at `weight`, or the best available substitute."""
    src = os.path.join(FONTDIR, f"inter-latin-{weight}-normal.woff2")
    if os.path.exists(src):
        try:
            from fontTools.ttLib import TTFont

            ttf = TTFont(src)
            ttf.flavor = None  # woff2 -> plain sfnt
            buf = io.BytesIO()
            ttf.save(buf)
            buf.seek(0)
            return ImageFont.truetype(buf, size)
        except ImportError:
            print("  note: fonttools/brotli missing — falling back from Inter", file=sys.stderr)
        except Exception as err:  # a corrupt or unexpected woff2 must not be silent
            print(f"  note: could not read {src} ({err}) — falling back", file=sys.stderr)
    else:
        print(f"  note: {src} not present (run npm ci in cinny/) — falling back", file=sys.stderr)

    suffix = _FALLBACK_SUFFIX[weight]
    for pattern in _FALLBACKS:
        path = pattern.format(suffix)
        if os.path.exists(path):
            return ImageFont.truetype(path, size)
    raise SystemExit("No usable font found: install fonttools+brotli, or DejaVu/Liberation.")


def logo(size):
    return Image.open(ICON).convert("RGBA").resize((size, size), Image.LANCZOS)


def vertical_gradient(size, top, bottom):
    w, h = size
    strip = Image.new("RGB", (1, h))
    px = strip.load()
    for y in range(h):
        t = y / max(h - 1, 1)
        px[0, y] = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
    return strip.resize((w, h), Image.BICUBIC)


def flatten(im, ground):
    """RGBA -> RGB over an opaque ground. NSIS paints leftover alpha black."""
    base = Image.new("RGB", im.size, ground)
    base.paste(im, (0, 0), im)
    return base


def save_bmp(im, name, expect_size):
    assert im.mode == "RGB", f"{name}: must be RGB, got {im.mode}"
    assert im.size == expect_size, f"{name}: must be {expect_size}, got {im.size}"

    path = os.path.join(OUTDIR, name)
    im.save(path, "BMP")

    # Verify what actually landed on disk rather than what PIL was asked for —
    # a 32-bit or RLE-compressed BMP is accepted by NSIS and renders wrong.
    with open(path, "rb") as fh:
        head = fh.read(54)
    w = int.from_bytes(head[18:22], "little")
    h = int.from_bytes(head[22:26], "little")
    bpp = int.from_bytes(head[28:30], "little")
    comp = int.from_bytes(head[30:34], "little")
    print(f"  {name:12} {w}x{h}  {bpp}bpp  compression={comp}  {os.path.getsize(path):,} bytes")
    assert (w, h) == expect_size, f"{name}: header says {w}x{h}"
    assert bpp == 24, f"{name}: expected 24bpp, got {bpp}"
    assert comp == 0, f"{name}: expected BI_RGB (0), got {comp}"


def make_header():
    """150x57 tile, drawn inside MUI's white header strip."""
    W, H = 150, 57
    canvas = Image.new("RGBA", (W * SS, H * SS), WHITE + (255,))
    draw = ImageDraw.Draw(canvas)

    # Thin indigo rule along the bottom, tying the tile to the sidebar art
    # without putting a hard edge against the white strip.
    draw.rectangle([0, (H - 2) * SS, W * SS, H * SS], fill=INDIGO_MID + (255,))

    canvas.alpha_composite(logo(44 * SS), (8 * SS, 4 * SS))

    draw.text((58 * SS, 16 * SS), "Prinny", font=font(800, 17 * SS), fill=INDIGO_DEEP + (255,))
    draw.text((59 * SS, 34 * SS), "Matrix client", font=font(400, 8 * SS), fill=SLATE + (255,))

    save_bmp(flatten(canvas.resize((W, H), Image.LANCZOS), WHITE), "header.bmp", (W, H))


def make_sidebar():
    """164x314 full-bleed panel for the Welcome and Finish pages."""
    W, H = 164, 314
    canvas = vertical_gradient((W * SS, H * SS), INDIGO_DEEP, INDIGO_MID).convert("RGBA")

    # Soft radial highlight so the mark doesn't sit flat on the gradient.
    glow = Image.new("RGBA", (W * SS, H * SS), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    cx, cy, r = 82 * SS, 108 * SS, 74 * SS
    for i in range(28, 0, -1):
        t = i / 28
        rr = int(r * (0.55 + 0.45 * t))
        gd.ellipse([cx - rr, cy - rr, cx + rr, cy + rr], fill=INDIGO_LIT + (int(7 * (1 - t)),))
    canvas = Image.alpha_composite(canvas, glow)
    draw = ImageDraw.Draw(canvas)

    canvas.alpha_composite(logo(124 * SS), (20 * SS, 46 * SS))

    def centered(text, f, y, fill):
        w = draw.textbbox((0, 0), text, font=f)[2]
        draw.text(((W * SS - w) // 2, y), text, font=f, fill=fill)

    tag = font(400, 10 * SS)
    centered("Prinny", font(800, 30 * SS), 190 * SS, CREAM + (255,))
    centered("A Matrix client that", tag, 226 * SS, LILAC + (255,))
    centered("actually feels native", tag, 240 * SS, LILAC + (255,))

    # Scarf-coloured rule + the catchphrase, so the panel reads as Prinny
    # rather than as a generic dark installer sidebar.
    draw.rectangle([70 * SS, 264 * SS, 94 * SS, 266 * SS], fill=SCARF + (255,))
    centered("dood!", font(700, 9 * SS), 276 * SS, (170, 96, 104, 255))

    save_bmp(flatten(canvas.resize((W, H), Image.LANCZOS), INDIGO_DEEP), "sidebar.bmp", (W, H))


if __name__ == "__main__":
    if not os.path.exists(ICON):
        raise SystemExit(f"Missing {ICON}")
    os.makedirs(OUTDIR, exist_ok=True)
    print(f"Writing NSIS artwork to {OUTDIR}:")
    make_header()
    make_sidebar()
