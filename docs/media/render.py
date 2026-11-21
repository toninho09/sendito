#!/usr/bin/env python3
"""Renders the app's captured frames (target/media/frames/*.json, produced by
`cargo test -p sendito-ui --test media -- --ignored`) into PNG screenshots and
animated GIF recordings, cell by cell with the real colors and attributes."""
import json, os, sys
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.abspath(__file__))
SRC = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "../../target/media/frames")
OUT = os.path.join(ROOT)
FONT_DIRS = ["/usr/share/fonts/TTF", "/usr/share/fonts/truetype/dejavu", "/Library/Fonts", "/System/Library/Fonts"]
SIZE = 15

def font(name):
    for d in FONT_DIRS:
        p = os.path.join(d, name)
        if os.path.exists(p):
            return ImageFont.truetype(p, SIZE)
    return ImageFont.load_default()

REG, BOLD = font("DejaVuSansMono.ttf"), font("DejaVuSansMono-Bold.ttf")
ITAL, BOLD_ITAL = font("DejaVuSansMono-Oblique.ttf"), font("DejaVuSansMono-BoldOblique.ttf")
ASC, DESC = REG.getmetrics()
CW = int(REG.getlength("M") + 0.5)
CH = ASC + DESC

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

def render(screen, light, caption=None, cursor=None):
    w, h = screen["width"], screen["height"]
    cap_h = CH + 10 if caption else 0
    dfg, dbg = ((43, 47, 58), (250, 250, 247)) if light else ((216, 220, 230), (27, 30, 40))
    img = Image.new("RGB", (w * CW + 16, h * CH + 16 + cap_h), dbg)
    d = ImageDraw.Draw(img)
    for y, row in enumerate(screen["rows"]):
        for x, (sym, fg, bg, flags) in enumerate(row):
            f, b = rgb(fg, dfg), rgb(bg, dbg)
            if "r" in flags:
                f, b = b, f
            if "d" in flags:
                f = blend(f, b, 0.45)
            px, py = 8 + x * CW, 8 + y * CH
            d.rectangle([px, py, px + CW, py + CH], fill=b)
            if sym and sym != " ":
                fnt = BOLD_ITAL if ("b" in flags and "i" in flags) else BOLD if "b" in flags else ITAL if "i" in flags else REG
                d.text((px, py), sym, font=fnt, fill=f)
            if "u" in flags:
                d.line([px, py + CH - 3, px + CW, py + CH - 3], fill=f)
            if "x" in flags:
                d.line([px, py + CH // 2, px + CW, py + CH // 2], fill=f)
    if cursor:
        # The terminal's text cursor, as a bar like most terminals draw it.
        cx, cy = cursor
        px, py = 8 + cx * CW, 8 + cy * CH
        d.rectangle([px, py + 1, px + 1, py + CH - 2], fill=dfg)
    if caption:
        y0 = h * CH + 16
        d.rectangle([0, y0, img.width, img.height], fill=(250, 220, 90) if not light else (40, 60, 120))
        d.text((12, y0 + 5), caption, font=BOLD, fill=(20, 20, 20) if not light else (255, 255, 255))
    return img

def main():
    shots = os.path.join(OUT, "screenshots")
    recs = os.path.join(OUT, "recordings")
    os.makedirs(shots, exist_ok=True)
    os.makedirs(recs, exist_ok=True)
    for name in sorted(os.listdir(SRC)):
        if not name.endswith(".json"):
            continue
        frames = json.load(open(os.path.join(SRC, name)))
        stem = name[:-5]
        light = "daylight" in stem
        if stem.startswith("screens-"):
            _, theme, size = stem.split("-")
            for f in frames:
                render(f["screen"], light, None, f.get("cursor")).save(os.path.join(shots, f"{size}-{theme}-{f['caption']}.png"))
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
