#!/usr/bin/env python3
"""Render snp's application icon, the icon of the Windows executable, from the banner's own sprites.

    python3 docs/assets/src/pixel_icon.py                 # crates/snp/assets/snp.ico and docs/assets/icon.png
    python3 docs/assets/src/pixel_icon.py --preview DIR   # also every size as a PNG and a contact sheet

One artwork, framed for each size the way a camera would frame it: the small sizes Windows uses in lists and
details views (16-40 px) are a close-up of the wizard's staff head, the glowing ">_" orb that names the tool; the
large sizes (48 px and up) pull back to the wizard holding it up on his bridge at night. Everything is drawn at one
pixel per pixel from the sprite maps and palette of pixel_logo.py (the README banner), so the icon and the banner
stay one family; 128 and 256 are the 64 px artwork scaled by 2 and 4 with nearest-neighbour (the banner's own
pixel size is 4). The tile is a rounded night-sky square with a light rim, legible on light and dark desktops.

The .ico holds 32-bit BMP images up to 64 px and PNG images above (the layout Windows itself uses), and
crates/snp/build.rs embeds it in snp.exe. Requires Pillow and numpy, like pixel_logo.py.
"""

from __future__ import annotations

import argparse
import io
import math
import struct
import sys
from pathlib import Path

import numpy as np
from PIL import Image

sys.dont_write_bytecode = True  # no __pycache__ next to the sources
sys.path.insert(0, str(Path(__file__).resolve().parent))
import pixel_logo as L  # noqa: E402  (the banner: palette, sprites, font, ordered-dither matrix)

ROOT = Path(__file__).resolve().parents[3]
ICO = ROOT / "crates" / "snp" / "assets" / "snp.ico"
PNG = ROOT / "docs" / "assets" / "icon.png"

P, hexc = L.P, L.hexc
B4 = L.B4
SMALL = (16, 20, 24, 32, 40)      # close-up of the orb
LARGE = (48, 64)                  # the wizard; 128 and 256 are 64 scaled
ALL = (16, 20, 24, 32, 40, 48, 64, 128, 256)

RIM, RIM_HI = hexc("#3d3775"), hexc("#5a52a0")


def bayer(x: int, y: int) -> float:
    return (B4[y % 4, x % 4] + 0.5) / 16.0


class Tile:
    """An n x n RGBA canvas clipped to a rounded square."""

    def __init__(self, n: int, radius: float) -> None:
        self.n = n
        self.a = np.zeros((n, n, 4), np.uint8)
        ys, xs = np.mgrid[0:n, 0:n]
        cx = np.clip(np.maximum(radius - (xs + 0.5), (xs + 0.5) - (n - radius)), 0, None)
        cy = np.clip(np.maximum(radius - (ys + 0.5), (ys + 0.5) - (n - radius)), 0, None)
        self.mask = cx * cx + cy * cy <= radius * radius + 0.35

    def inside(self, x: int, y: int) -> bool:
        return 0 <= x < self.n and 0 <= y < self.n and bool(self.mask[y, x])

    def put(self, x: int, y: int, c, a: float = 1.0) -> None:
        if self.inside(x, y) and (a >= 1 or a > bayer(x, y)):
            self.a[y, x, :3] = c
            self.a[y, x, 3] = 255

    def add(self, x: int, y: int, c, k: float) -> None:
        if self.inside(x, y):
            p = self.a[y, x, :3].astype(float) + np.array(c, float) * k
            self.a[y, x, :3] = np.clip(p, 0, 255).astype(np.uint8)

    def rect(self, x0: int, y0: int, x1: int, y1: int, c) -> None:
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                self.put(x, y, c)

    def sprite(self, x0: int, y0: int, rows, cmap) -> None:
        for dy, row in enumerate(rows):
            for dx, ch in enumerate(row):
                if ch in cmap:
                    self.put(x0 + dx, y0 + dy, cmap[ch])

    def glow(self, cx: float, cy: float, r: float, color, strength: float, steps: int = 5) -> None:
        if r <= 0:
            return
        for y in range(max(0, int(cy - r)), min(self.n, int(cy + r) + 2)):
            for x in range(max(0, int(cx - r)), min(self.n, int(cx + r) + 2)):
                d = math.hypot(x + 0.5 - cx, y + 0.5 - cy)
                v = max(0.0, 1.0 - d / r) ** 2 * strength * steps
                q = math.floor(v) + (1 if v - math.floor(v) > bayer(x, y) else 0)
                if q:
                    self.add(x, y, color, q / steps)

    def image(self) -> Image.Image:
        return Image.fromarray(self.a, "RGBA")


def background(t: Tile, stars: list[tuple[int, int, int]]) -> None:
    """Night sky (dithered vertical gradient), stars, and the rim that keeps the tile visible on dark desktops."""
    n = t.n
    stops = [P["sky4"], P["sky3"], P["sky2"], P["sky1"]]
    for y in range(n):
        v = y / max(1, n - 1) * (len(stops) - 1)
        i = min(int(v), len(stops) - 2)
        for x in range(n):
            t.put(x, y, stops[i + 1] if (v - i) > bayer(x, y) else stops[i])
    for x, y, k in stars:
        if k == 2:
            L_twinkle(t, x, y)
        else:
            t.put(x, y, P["star2"] if k == 1 else P["star1"])
    for y in range(n):
        for x in range(n):
            if not t.mask[y, x]:
                continue
            edge = any(not t.inside(x + dx, y + dy) for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)))
            if edge:
                t.a[y, x, :3] = RIM_HI if y < n * 0.35 else RIM
                t.a[y, x, 3] = 255


def L_twinkle(t: Tile, x: int, y: int) -> None:
    t.put(x, y, P["star3"])
    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        t.put(x + dx, y + dy, P["star1"])


def orb(t: Tile, ox: int, oy: int, r: float, glyph: int, glow_r: float, claws: bool = True) -> None:
    """The staff head: a gold orb with the shell prompt ">_" (glyph = pixel size of the prompt)."""
    t.glow(ox + 0.5, oy + 0.5, glow_r, P["warm"], 0.5, steps=6)
    for y in range(int(oy - r - 1), int(oy + r + 2)):
        for x in range(int(ox - r - 1), int(ox + r + 2)):
            d = math.hypot(x - ox, y - oy)
            if d <= r:
                c = P["gold0"] if d <= r * 0.62 else P["gold1"]
                if d > r - 1.0 and (x - ox) + (y - oy) > r * 0.35:
                    c = P["gold2"]  # shade the lower right of the rim
                t.put(x, y, c)
    if claws:
        k = max(1, round(r * 0.87))
        for s in (-1, 1):
            t.put(ox + s * k, oy + round(r * 0.65), P["wood1"])
            t.put(ox + s * k, oy + round(r * 0.65) + 1, P["wood2"])
    g = glyph
    rows = L.FONT3[">"]
    gx, gy = ox - 3 * g, oy - (5 * g) // 2
    for yy, row in enumerate(rows):
        for xx, bit in enumerate(row):
            if bit == "#":
                t.rect(gx + xx * g, gy + yy * g, gx + xx * g + g - 1, gy + yy * g + g - 1, P["gold_ink"])
    cy = gy + 4 * g
    t.rect(ox + g, cy, ox + 3 * g, cy + g - 1, P["gold_ink"])


def staff(t: Tile, x0: int, y0: int, x1: int, y1: int) -> None:
    for i, (x, y) in enumerate(L.line_pts(x0, y0, x1, y1)):
        knot = i % 7 == 3
        t.put(x, y, P["wood2"] if knot else P["wood0"])
        t.put(x + 1, y, P["wood2"] if knot else P["wood1"])


# ---------------------------------------------------------------------------------------------- small: the orb

SMALL_SPEC = {  # n: (orb centre x, y, radius, glyph pixel size, stars)
    16: (8, 7, 5.6, 1, [(2, 3, 0), (13, 12, 0)]),
    20: (10, 8, 6.6, 1, [(3, 3, 0), (16, 15, 0), (16, 3, 1)]),
    24: (12, 10, 7.8, 1, [(3, 4, 0), (20, 18, 0), (19, 3, 1)]),
    32: (16, 13, 10.4, 2, [(4, 5, 1), (27, 24, 0), (26, 4, 2), (5, 25, 0)]),
    40: (20, 16, 13.0, 2, [(5, 6, 1), (34, 30, 0), (33, 5, 2), (6, 32, 0), (30, 13, 0)]),
}


def small(n: int) -> Tile:
    ox, oy, r, g, stars = SMALL_SPEC[n]
    t = Tile(n, max(2.0, n * 0.16))
    background(t, stars)
    sw = max(2, n // 10)  # staff width
    top = oy + int(r * 0.8)
    for y in range(top, n):
        for k in range(sw):
            x = ox - sw // 2 + k
            knot = (y - top) % 6 == 3
            t.put(x, y, P["wood2"] if knot else (P["wood0"] if k < (sw + 1) // 2 else P["wood1"]))
    orb(t, ox, oy, r, g, glow_r=r * 2.1)
    return t


# ---------------------------------------------------------------------------------------------- large: the wizard

def wizard_scene(t: Tile, dx: int, dy: int, ground: bool) -> None:
    """The wizard in the banner's "point" pose, staff raised forward; (dx, dy) shifts the 64 px layout."""
    wx, wy = 31 + dx, 17 + dy
    pose = L.POSES["point"]
    if ground:
        deck = wy + 40
        for y in range(deck, t.n):
            for x in range(t.n):
                if y == deck:
                    c = P["stone5"] if x % 9 else P["stone4"]
                elif (y - deck) % 3 == 0 or (x + (6 if (y - deck) // 3 % 2 else 0)) % 12 == 0:
                    c = P["stone1"]
                else:
                    c = P["stone3"] if (y - deck) % 3 == 1 else P["stone2"]
                t.put(x, y, c)
    bx, by = wx + pose["base"][0], wy + pose["base"][1]
    tx, ty = wx + pose["top"][0], wy + pose["top"][1]
    ln = math.hypot(tx - bx, ty - by)
    ox, oy = round(tx + (tx - bx) / ln * 4), round(ty + (ty - by) / ln * 4)
    t.glow(ox + 0.5, oy + 0.5, 19, P["warm"], 0.42, steps=6)
    t.sprite(wx, wy, L.WIZ, L.WIZ_C)
    for row_i, row in enumerate(L.WIZ):  # warm rim light from the staff on the wizard's front edge
        inner = [i for i, ch in enumerate(row) if ch not in ".O"]
        if inner and row[inner[0]] in "hHrLws":
            t.add(wx + inner[0], wy + row_i, P["warm"], 0.35)
    staff(t, bx, by, tx, ty)
    hx, hy = wx + pose["hand"][0], wy + pose["hand"][1]
    pts = L.line_pts(wx + 6, wy + 20, hx + 1, hy)
    for x, y in pts:
        t.rect(x - 1, y - 1, x + 1, y + 1, L.SLEEVE_LO)
    for x, y in pts:
        t.put(x, y, L.SLEEVE)
        t.put(x, y - 1, L.SLEEVE_HI)
    t.rect(hx, hy, hx + 1, hy + 1, L.WIZ_C["s"])
    t.put(hx + 1, hy + 1, L.WIZ_C["S"])
    orb(t, ox, oy, 4.6, 1, glow_r=0, claws=True)


def moon(t: Tile, cx: int, cy: int, r: float) -> None:
    t.glow(cx + 0.5, cy + 0.5, r * 2.2, P["halo1"], 0.6, steps=3)
    for y in range(int(cy - r - 1), int(cy + r + 2)):
        for x in range(int(cx - r - 1), int(cx + r + 2)):
            if math.hypot(x - cx, y - cy) <= r and math.hypot(x - cx - r * 0.55, y - cy + r * 0.3) > r * 0.85:
                edge = math.hypot(x - cx, y - cy) > r - 1
                t.put(x, y, P["moon_sh"] if edge and x < cx else P["moon"])


def large(n: int) -> Tile:
    t = Tile(n, n * 0.16)
    if n == 64:
        background(t, [(8, 40, 0), (12, 52, 0), (55, 30, 0), (47, 5, 1), (58, 44, 0), (5, 8, 2), (40, 3, 0)])
        moon(t, 53, 11, 5.0)
        wizard_scene(t, 0, 0, ground=True)
    else:  # 48: waist up, same artwork
        background(t, [(4, 34, 0), (43, 30, 0), (37, 3, 1), (3, 5, 2)])
        moon(t, 41, 9, 4.0)
        wizard_scene(t, -8, -3, ground=False)
    return t


# ---------------------------------------------------------------------------------------------- .ico

def bmp_entry(img: Image.Image) -> bytes:
    """A 32-bit DIB as an icon image: BITMAPINFOHEADER (height doubled), BGRA rows bottom-up, 1-bit AND mask."""
    w, h = img.size
    a = np.asarray(img.convert("RGBA"))
    bgra = a[::-1, :, [2, 1, 0, 3]].tobytes()
    mask_row = ((w + 31) // 32) * 4
    andmask = b"\x00" * (mask_row * h)
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(bgra) + len(andmask), 0, 0, 0, 0)
    return header + bgra + andmask


def png_entry(img: Image.Image) -> bytes:
    buf = io.BytesIO()
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def write_ico(images: dict[int, Image.Image], path: Path) -> None:
    sizes = sorted(images)
    blobs = [png_entry(images[s]) if s > 64 else bmp_entry(images[s]) for s in sizes]
    out = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    for s, blob in zip(sizes, blobs):
        out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(out + b"".join(blobs))


def render_all() -> dict[int, Image.Image]:
    imgs: dict[int, Image.Image] = {n: small(n).image() for n in SMALL}
    imgs.update({n: large(n).image() for n in LARGE})
    imgs[128] = imgs[64].resize((128, 128), Image.NEAREST)
    imgs[256] = imgs[64].resize((256, 256), Image.NEAREST)
    return imgs


def contact_sheet(imgs: dict[int, Image.Image], bg: tuple[int, int, int]) -> Image.Image:
    """Every size at 1x, then the small ones blown up 8x, on a desktop-like background."""
    pad = 16
    w = sum(imgs[s].width for s in ALL) + pad * (len(ALL) + 1)
    blow = [imgs[s].resize((s * 8, s * 8), Image.NEAREST) for s in (16, 32, 48)] + [imgs[64].resize((384, 384), Image.NEAREST)]
    w = max(w, sum(b.width for b in blow) + pad * (len(blow) + 1))
    h = pad * 3 + 256 + max(b.height for b in blow)
    sheet = Image.new("RGBA", (w, h), bg + (255,))
    x = pad
    for s in ALL:
        sheet.alpha_composite(imgs[s], (x, pad + 256 - s))
        x += s + pad
    x = pad
    for b in blow:
        sheet.alpha_composite(b, (x, pad * 2 + 256))
        x += b.width + pad
    return sheet


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--preview", type=Path, help="also write each size and contact sheets to this directory")
    args = ap.parse_args()
    imgs = render_all()
    write_ico({s: imgs[s] for s in ALL}, ICO)
    imgs[256].save(PNG, optimize=True)
    print(f"wrote {ICO.relative_to(ROOT)} ({ICO.stat().st_size} bytes, sizes {', '.join(map(str, ALL))}) "
          f"and {PNG.relative_to(ROOT)}")
    if args.preview:
        args.preview.mkdir(parents=True, exist_ok=True)
        for s in ALL:
            imgs[s].save(args.preview / f"icon-{s}.png")
        contact_sheet(imgs, (255, 255, 255)).save(args.preview / "sheet-light.png")
        contact_sheet(imgs, (32, 32, 32)).save(args.preview / "sheet-dark.png")


if __name__ == "__main__":
    main()
