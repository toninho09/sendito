#!/usr/bin/env python3
"""Renders the app's captured frames (target/media/frames/*.json, produced by
`cargo test -p sendito-ui --test media -- --ignored`) into PNG screenshots and
animated GIF recordings, cell by cell with the real colors and attributes.

Box-drawing characters (U+2500-257F) and block elements (U+2580-259F) are drawn
as shapes, the way terminal emulators do, so borders join without gaps whatever
the font's line height. Nerd Font icons (private use area) come from a Nerd Font
when one is installed.

Usage: render.py [FRAMES_DIR] [--out DIR] [--scale N] [--only PREFIX]
"""
import argparse
import glob
import json
import os
import unicodedata

from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.abspath(__file__))
FONT_DIRS = ["/usr/share/fonts/TTF", "/usr/share/fonts/truetype/dejavu", "/usr/share/fonts/truetype",
             "/usr/share/fonts", "/Library/Fonts", "/System/Library/Fonts", os.path.expanduser("~/.local/share/fonts")]

ap = argparse.ArgumentParser()
ap.add_argument("src", nargs="?", default=os.path.join(ROOT, "../../target/media/frames"))
ap.add_argument("--out", default=ROOT)
ap.add_argument("--scale", type=int, default=1)
ap.add_argument("--only", default="", help="render only frame files whose name starts with this")
ARGS = ap.parse_args()

S = max(1, ARGS.scale)
SIZE = 15 * S
PAD = 8 * S


def font(*names):
    for n in names:
        for d in FONT_DIRS:
            hits = glob.glob(os.path.join(d, "**", n), recursive=True)
            if hits:
                return ImageFont.truetype(sorted(hits)[0], SIZE)
    return None


REG = font("DejaVuSansMono.ttf") or ImageFont.load_default()
BOLD = font("DejaVuSansMono-Bold.ttf") or REG
ITAL = font("DejaVuSansMono-Oblique.ttf") or REG
BOLD_ITAL = font("DejaVuSansMono-BoldOblique.ttf") or BOLD
# Icons in the private use area (Nerd Fonts).
NERD = font("SymbolsNerdFontMono-Regular.ttf", "MesloLGSNerdFontMono-Regular.ttf", "*NerdFontMono-Regular.ttf")
ASC, DESC = REG.getmetrics()
CW = int(REG.getlength("M") + 0.5)
CH = ASC + DESC
LIGHT = max(1, round(CW * 0.11))
HEAVY = max(LIGHT + 1, round(CW * 0.26))

NAMED = {"black": (0, 0, 0), "red": (205, 49, 49), "green": (13, 188, 121), "yellow": (229, 229, 16), "blue": (36, 114, 200),
         "magenta": (188, 63, 188), "cyan": (17, 168, 205), "gray": (229, 229, 229), "darkgray": (102, 102, 102),
         "lightred": (241, 76, 76), "lightgreen": (35, 209, 139), "lightyellow": (245, 245, 67), "lightblue": (59, 142, 234),
         "lightmagenta": (214, 112, 214), "lightcyan": (41, 184, 219), "white": (255, 255, 255)}


def xterm(i):
    base = [NAMED[k] for k in ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "gray", "darkgray", "lightred",
                                "lightgreen", "lightyellow", "lightblue", "lightmagenta", "lightcyan", "white"]]
    if i < 16:
        return base[i]
    if i < 232:
        i -= 16
        v = [0, 95, 135, 175, 215, 255]
        return (v[i // 36], v[(i // 6) % 6], v[i % 6])
    g = 8 + 10 * (i - 232)
    return (g, g, g)


def rgb(c, default):
    if c == "reset":
        return default
    if c.startswith("#"):
        return tuple(int(c[i:i + 2], 16) for i in (1, 3, 5))
    if c.startswith("i") and c[1:].isdigit():
        return xterm(int(c[1:]))
    return NAMED.get(c, default)


def blend(a, b, t):
    return tuple(int(a[i] * (1 - t) + b[i] * t) for i in range(3))


# ----- box drawing ----------------------------------------------------------

def box_spec(ch):
    """(weights {U,R,D,L: 1 light / 2 heavy / 3 double}, arc, dashes) or None."""
    try:
        name = unicodedata.name(ch)
    except ValueError:
        return None
    if not name.startswith("BOX DRAWINGS ") or "DIAGONAL" in name:
        return None
    name = name[len("BOX DRAWINGS "):]
    arc = "ARC" in name
    dashes = 0
    for word, n in (("DOUBLE DASH", 2), ("TRIPLE DASH", 3), ("QUADRUPLE DASH", 4)):
        if word in name:
            dashes = n
            name = name.replace(word, "")
    w = {}
    weight = 1
    for group in name.split(" AND "):
        words = group.split()
        gw = None
        for word in words:
            if word in ("LIGHT", "SINGLE"):
                gw = 1
            elif word == "HEAVY":
                gw = 2
            elif word == "DOUBLE":
                gw = 3
        if gw is not None:
            weight = gw
        for word in words:
            dirs = {"UP": "U", "DOWN": "D", "LEFT": "L", "RIGHT": "R", "VERTICAL": "UD", "HORIZONTAL": "LR"}.get(word, "")
            for d in dirs:
                w[d] = weight
    return (w, arc, dashes) if w else None


def draw_box(d, px, py, spec, fg):
    w, arc, dashes = spec
    cx, cy = px + CW // 2, py + CH // 2
    if arc:
        r = CW // 2
        t = LIGHT
        (ox, oy, start) = {
            frozenset("DR"): (r, r, 180), frozenset("DL"): (-r, r, 270),
            frozenset("UL"): (-r, -r, 0), frozenset("UR"): (r, -r, 90),
        }[frozenset(w)]
        ccx, ccy = cx + ox, cy + oy
        h = t // 2
        d.arc([ccx - r - h, ccy - r - h, ccx + r + (t - h) - 1, ccy + r + (t - h) - 1], start, start + 90, fill=fg, width=t)
        # Straight parts from the end of the arc to the cell edges.
        if "D" in w:
            d.rectangle([cx - h, ccy, cx - h + t - 1, py + CH], fill=fg)
        if "U" in w:
            d.rectangle([cx - h, py, cx - h + t - 1, ccy], fill=fg)
        if "R" in w:
            d.rectangle([ccx, cy - h, px + CW, cy - h + t - 1], fill=fg)
        if "L" in w:
            d.rectangle([px, cy - h, ccx, cy - h + t - 1], fill=fg)
        return

    def thick(k):
        return HEAVY if k == 2 else LIGHT

    # Half the widest crossing line, so corners and tees join.
    hv = max([thick(w[k]) for k in "UD" if k in w] or [0])
    hh = max([thick(w[k]) for k in "LR" if k in w] or [0])

    def seg(x0, y0, x1, y1):
        if not dashes:
            d.rectangle([x0, y0, x1, y1], fill=fg)
            return
        horizontal = (x1 - x0) > (y1 - y0)
        length = (x1 - x0 + 1) if horizontal else (y1 - y0 + 1)
        step = length / dashes
        for i in range(dashes):
            a = int(i * step)
            b = int((i + 0.6) * step)
            if horizontal:
                d.rectangle([x0 + a, y0, x0 + b, y1], fill=fg)
            else:
                d.rectangle([x0, y0 + a, x1, y0 + b], fill=fg)

    for k, wt in w.items():
        if wt == 3:
            off = LIGHT + max(1, LIGHT)
            for o in (-off, off):
                if k == "U":
                    seg(cx + o - LIGHT // 2, py, cx + o - LIGHT // 2 + LIGHT - 1, cy + off)
                elif k == "D":
                    seg(cx + o - LIGHT // 2, cy - off, cx + o - LIGHT // 2 + LIGHT - 1, py + CH)
                elif k == "L":
                    seg(px, cy + o - LIGHT // 2, cx + off, cy + o - LIGHT // 2 + LIGHT - 1)
                elif k == "R":
                    seg(cx - off, cy + o - LIGHT // 2, px + CW, cy + o - LIGHT // 2 + LIGHT - 1)
            continue
        t = thick(wt)
        h = t // 2
        if k == "U":
            seg(cx - h, py, cx - h + t - 1, cy + (hh + 1) // 2)
        elif k == "D":
            seg(cx - h, cy - hh // 2, cx - h + t - 1, py + CH)
        elif k == "L":
            seg(px, cy - h, cx + (hv + 1) // 2, cy - h + t - 1)
        elif k == "R":
            seg(cx - hv // 2, cy - h, px + CW, cy - h + t - 1)


def draw_block(d, px, py, o, fg, bg):
    """Block elements U+2580-259F. Returns False when not handled."""
    x1, y1 = px + CW, py + CH

    def rect(fx0, fy0, fx1, fy1, color=fg):
        d.rectangle([px + round(CW * fx0), py + round(CH * fy0), px + round(CW * fx1) - 1, py + round(CH * fy1) - 1], fill=color)

    if o == 0x2580:
        rect(0, 0, 1, 0.5)
    elif 0x2581 <= o <= 0x2588:
        rect(0, 1 - (o - 0x2580) / 8, 1, 1)
    elif 0x2589 <= o <= 0x258F:
        rect(0, 0, (0x2590 - o) / 8, 1)
    elif o == 0x2590:
        rect(0.5, 0, 1, 1)
    elif 0x2591 <= o <= 0x2593:
        d.rectangle([px, py, x1 - 1, y1 - 1], fill=blend(bg, fg, (o - 0x2590) * 0.25))
    elif o == 0x2594:
        rect(0, 0, 1, 1 / 8)
    elif o == 0x2595:
        rect(7 / 8, 0, 1, 1)
    elif 0x2596 <= o <= 0x259F:
        quads = {0x2596: "3", 0x2597: "4", 0x2598: "1", 0x2599: "134", 0x259A: "14", 0x259B: "123",
                 0x259C: "124", 0x259D: "2", 0x259E: "23", 0x259F: "234"}[o]
        for q in quads:
            fx, fy = (0 if q in "13" else 0.5), (0 if q in "12" else 0.5)
            rect(fx, fy, fx + 0.5, fy + 0.5)
    else:
        return False
    return True


def draw_braille(d, px, py, o, fg):
    """Braille patterns U+2800-28FF as dots (many fonts lack them)."""
    bits = o - 0x2800
    pos = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)]
    r = max(1, CW // 8)
    for i, (cx, cy) in enumerate(pos):
        if bits & (1 << i):
            x = px + round(CW * (0.3 + 0.4 * cx))
            y = py + round(CH * (0.14 + 0.24 * cy))
            d.ellipse([x - r, y - r, x + r, y + r], fill=fg)


def is_pua(o):
    return 0xE000 <= o <= 0xF8FF or 0xF0000 <= o <= 0xFFFFD


# ----- frames ---------------------------------------------------------------

def render(screen, light, caption=None, cursor=None):
    w, h = screen["width"], screen["height"]
    cap_h = CH + 10 * S if caption else 0
    dfg, dbg = ((43, 47, 58), (250, 250, 247)) if light else ((216, 220, 230), (27, 30, 40))
    img = Image.new("RGB", (w * CW + 2 * PAD, h * CH + 2 * PAD + cap_h), dbg)
    d = ImageDraw.Draw(img)
    for y, row in enumerate(screen["rows"]):
        for x, (sym, fg, bg, flags) in enumerate(row):
            f, b = rgb(fg, dfg), rgb(bg, dbg)
            if "r" in flags:
                f, b = b, f
            if "d" in flags:
                f = blend(f, b, 0.45)
            px, py = PAD + x * CW, PAD + y * CH
            d.rectangle([px, py, px + CW, py + CH], fill=b)
            if sym and sym != " ":
                o = ord(sym[0])
                spec = box_spec(sym[0]) if 0x2500 <= o <= 0x257F else None
                if spec:
                    draw_box(d, px, py, spec, f)
                elif 0x2800 <= o <= 0x28FF:
                    draw_braille(d, px, py, o, f)
                elif not draw_block(d, px, py, o, f, b):
                    if is_pua(o) and NERD:
                        fnt = NERD
                    else:
                        fnt = BOLD_ITAL if ("b" in flags and "i" in flags) else BOLD if "b" in flags else ITAL if "i" in flags else REG
                    d.text((px, py), sym, font=fnt, fill=f)
            if "u" in flags:
                d.line([px, py + CH - 3 * S, px + CW, py + CH - 3 * S], fill=f, width=S)
            if "x" in flags:
                d.line([px, py + CH // 2, px + CW, py + CH // 2], fill=f, width=S)
    if cursor:
        # The terminal's text cursor, as a bar like most terminals draw it.
        cx, cy = cursor
        px, py = PAD + cx * CW, PAD + cy * CH
        d.rectangle([px, py + S, px + S, py + CH - 2 * S], fill=dfg)
    if caption:
        y0 = h * CH + 2 * PAD
        d.rectangle([0, y0, img.width, img.height], fill=(250, 220, 90) if not light else (40, 60, 120))
        d.text((12 * S, y0 + 5 * S), caption, font=BOLD, fill=(20, 20, 20) if not light else (255, 255, 255))
    return img


def is_light(stem):
    return any(t in stem for t in ("daylight", "linen"))


def main():
    shots = os.path.join(ARGS.out, "screenshots")
    recs = os.path.join(ARGS.out, "recordings")
    os.makedirs(shots, exist_ok=True)
    os.makedirs(recs, exist_ok=True)
    for name in sorted(os.listdir(ARGS.src)):
        if not name.endswith(".json") or not name.startswith(ARGS.only):
            continue
        frames = json.load(open(os.path.join(ARGS.src, name)))
        stem = name[:-5]
        light = is_light(stem)
        if stem.startswith("screens-"):
            # screens-THEME-SIZE[-VARIANT]
            parts = stem.split("-")
            theme, size = parts[1], parts[2]
            variant = "-".join(parts[3:])
            prefix = f"{size}-{theme}" + (f"-{variant}" if variant else "")
            for f in frames:
                render(f["screen"], light, None, f.get("cursor")).save(os.path.join(shots, f"{prefix}-{f['caption']}.png"))
        else:
            imgs = [render(f["screen"], light, f["caption"], f.get("cursor")) for f in frames]
            durs = [max(f["ms"], 400) for f in frames]
            durs[-1] = max(durs[-1], 2500)
            imgs[0].save(os.path.join(recs, f"{stem}.gif"), save_all=True, append_images=imgs[1:], duration=durs, loop=0, optimize=True)
            # Key frames as PNG too (for reviewers that cannot play GIFs).
            kdir = os.path.join(recs, stem)
            os.makedirs(kdir, exist_ok=True)
            for i, (im, f) in enumerate(zip(imgs, frames)):
                im.save(os.path.join(kdir, f"{i:02d}.png"))
    print(f"wrote {shots} and {recs}")


main()
