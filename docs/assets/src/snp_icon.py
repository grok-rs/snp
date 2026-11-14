#!/usr/bin/env python3
"""Render snp's application icon, the icon of the Windows executable (ADR-130).

    python3 docs/assets/src/snp_icon.py                 # crates/snp/assets/snp.ico and docs/assets/icon.png
    python3 docs/assets/src/snp_icon.py --preview DIR   # also every size as a PNG

The snp edition of the grok-rs crab emblem (grok-rs-emblem.png: the crab on a gear, wrapped in octopus
tentacles, from grok-rs.github.io): the badge on the crab's shell becomes snp's glowing gold ">_" prompt, the
staff light of the README banner. You Shell Not Pass: the crab's shell is the shell. From 64 px up the icon is the
whole emblem; at 48 px and below, where the emblem turns to mush, it is the crab's shell and its badge, drawn as a
glyph (with tentacle tips from 20 px).

Everything is drawn with Pillow and numpy at 4x and downsampled. The .ico holds 32-bit BMP images up to 64 px and
PNG images above (the layout Windows itself uses); crates/snp/build.rs embeds it in snp.exe.
"""

from __future__ import annotations

import argparse
import io
import math
import struct
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
EMBLEM = HERE / "grok-rs-emblem.png"
ICO = ROOT / "crates" / "snp" / "assets" / "snp.ico"
PNG = ROOT / "docs" / "assets" / "icon.png"

EMBLEM_SIZES = (256, 128, 64)
GLYPH_SIZES = (48, 40, 32, 24, 20, 16)
BADGE = (208.8, 226.7, 31.6)  # centre and radius of the shell badge in grok-rs-emblem.png (413 px)

GOLD, GOLD_HOT = (255, 196, 77), (255, 244, 200)
DISC_IN, DISC_MID, DISC_OUT, DISC_RIM = (46, 39, 80), (23, 18, 42), (12, 9, 22), (11, 7, 18)


def radial(size: int, cx: float, cy: float, r: float, stops: list[tuple[float, tuple[int, int, int]]]) -> np.ndarray:
    """An RGB radial gradient as a float array."""
    ys, xs = np.mgrid[0:size, 0:size]
    t = np.clip(np.hypot(xs - cx, ys - cy) / r, 0, 1)
    out = np.zeros((size, size, 3))
    for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
        k = np.clip((t - t0) / (t1 - t0), 0, 1)[..., None]
        sel = ((t >= t0) & (t <= t1))[..., None]
        out = np.where(sel, np.array(c0) * (1 - k) + np.array(c1) * k, out)
    return out


def badge(canvas: Image.Image, cx: float, cy: float, r: float, stroke: float) -> None:
    """The dark disc with the glowing >_ prompt, drawn onto an RGBA canvas (all units in canvas px)."""
    n = canvas.width
    disc = Image.new("L", canvas.size, 0)
    d = ImageDraw.Draw(disc)
    d.ellipse((cx - r * 1.026, cy - r * 1.026, cx + r * 1.026, cy + r * 1.026), fill=255)
    rim = Image.new("RGBA", canvas.size, DISC_RIM + (255,))
    canvas.alpha_composite(Image.composite(rim, Image.new("RGBA", canvas.size), disc))
    inner = Image.new("L", canvas.size, 0)
    ImageDraw.Draw(inner).ellipse((cx - r * 0.974, cy - r * 0.974, cx + r * 0.974, cy + r * 0.974), fill=255)
    grad = radial(n, cx - r * 0.08, cy - r * 0.18, r * 1.05, [(0, DISC_IN), (0.7, DISC_MID), (1, DISC_OUT)])
    fill = Image.fromarray(np.dstack([grad, np.full((n, n), 255)]).astype(np.uint8), "RGBA")
    canvas.alpha_composite(Image.composite(fill, Image.new("RGBA", canvas.size), inner))
    sheen = Image.new("RGBA", canvas.size)
    ImageDraw.Draw(sheen).ellipse((cx - r * 0.66, cy - r * 0.77, cx + r * 0.46, cy - r * 0.31), fill=(255, 255, 255, 18))
    canvas.alpha_composite(sheen)

    s = r / 78  # the prompt is designed on a disc of radius 78
    chevron = [(cx - 40 * s, cy - 28 * s), (cx - 8 * s, cy), (cx - 40 * s, cy + 28 * s)]
    cursor = [(cx + 4 * s, cy + 30 * s), (cx + 40 * s, cy + 30 * s)]

    def prompt(colour: tuple[int, int, int], width: float) -> Image.Image:
        layer = Image.new("RGBA", canvas.size)
        g = ImageDraw.Draw(layer)
        for line in (chevron, cursor):
            g.line(line, fill=colour + (255,), width=max(1, round(width)), joint="curve")
            for x, y in line:  # round caps
                g.ellipse((x - width / 2, y - width / 2, x + width / 2, y + width / 2), fill=colour + (255,))
        return layer

    core = prompt(GOLD, stroke * s)
    for radius, strength in ((9 * s, 1.0), (3 * s, 0.9)):
        glow = core.filter(ImageFilter.GaussianBlur(radius))
        a = np.asarray(glow).astype(float)
        a[..., 3] = np.clip(a[..., 3] * strength * 1.6, 0, 255)
        canvas.alpha_composite(Image.fromarray(a.astype(np.uint8), "RGBA"))
    canvas.alpha_composite(core)
    canvas.alpha_composite(prompt(GOLD_HOT, stroke * s * 0.38))


def emblem(size: int) -> Image.Image:
    """The whole grok-rs emblem with snp's badge, at `size` px."""
    work = 1024
    k = work / 413
    canvas = Image.open(EMBLEM).convert("RGBA").resize((work, work), Image.LANCZOS)
    cx, cy, r = (v * k for v in BADGE)
    badge(canvas, cx, cy, r, stroke=14)
    return canvas.resize((size, size), Image.LANCZOS)


def bezier(p0, p1, p2, p3, steps: int = 24) -> list[tuple[float, float]]:
    return [tuple((1 - t) ** 3 * a + 3 * (1 - t) ** 2 * t * b + 3 * (1 - t) * t * t * c + t ** 3 * e
                  for a, b, c, e in zip(p0, p1, p2, p3)) for t in (i / steps for i in range(steps + 1))]


def glyph(size: int) -> Image.Image:
    """The crab's shell and its badge, at `size` px (drawn on a 256 grid at 4x)."""
    work, u = 1024, 4.0
    canvas = Image.new("RGBA", (work, work))
    d = ImageDraw.Draw(canvas)
    if size >= 20:  # tentacle tips curling out from behind the shell
        for sx, sy in ((1, 1), (-1, 1), (1, -1), (-1, -1)):
            def m(x: float, y: float) -> tuple[float, float]:
                return ((128 + sx * (x - 128)) * u, (128 + sy * (y - 128)) * u)
            pts = (bezier(m(40, 92), m(10, 60), m(40, 20), m(74, 40)) + bezier(m(74, 40), m(56, 46), m(52, 66), m(66, 80)))
            d.polygon(pts, fill=(91, 82, 184, 255), outline=(29, 22, 64, 255), width=round(8 * u))
    shell = [(78, 34), (178, 34), (228, 92), (210, 196), (160, 232), (96, 232), (46, 196), (28, 92)]
    mask = Image.new("L", canvas.size, 0)
    ImageDraw.Draw(mask).polygon([(x * u, y * u) for x, y in shell], fill=255)
    ys = np.linspace(0, 1, work)[:, None, None] * np.ones((1, work, 1))
    top, mid, bot = np.array((246, 168, 90)), np.array((212, 118, 46)), np.array((148, 70, 27))
    col = np.where(ys < 0.55, top + (mid - top) * (ys / 0.55), mid + (bot - mid) * ((ys - 0.55) / 0.45))
    col = np.clip(col, 0, 255)
    shell_img = Image.fromarray(np.dstack([col[..., 0], col[..., 1], col[..., 2], np.full((work, work), 255)]).astype(np.uint8), "RGBA")
    canvas.alpha_composite(Image.composite(shell_img, Image.new("RGBA", canvas.size), mask))
    d.line([(x * u, y * u) for x, y in shell + shell[:1]], fill=(58, 22, 6, 255), width=round((14 if size < 20 else 10) * u), joint="curve")
    badge(canvas, 128 * u, 134 * u, (74 if size < 20 else 70) * u, stroke=20 if size < 20 else 16)
    return canvas.resize((size, size), Image.LANCZOS)


def bmp_entry(img: Image.Image) -> bytes:
    """A 32-bit DIB as an icon image: BITMAPINFOHEADER (height doubled), BGRA rows bottom-up, 1-bit AND mask."""
    w, h = img.size
    bgra = np.asarray(img.convert("RGBA"))[::-1, :, [2, 1, 0, 3]].tobytes()
    andmask = b"\x00" * (((w + 31) // 32) * 4 * h)
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


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--preview", type=Path, help="also write every size as a PNG to this directory")
    args = ap.parse_args()
    images = {s: emblem(s) for s in EMBLEM_SIZES} | {s: glyph(s) for s in GLYPH_SIZES}
    write_ico(images, ICO)
    images[256].save(PNG, optimize=True)
    print(f"wrote {ICO.relative_to(ROOT)} ({ICO.stat().st_size} bytes, sizes {', '.join(map(str, sorted(images)))}) "
          f"and {PNG.relative_to(ROOT)}")
    if args.preview:
        args.preview.mkdir(parents=True, exist_ok=True)
        for s, img in images.items():
            img.save(args.preview / f"icon-{s}.png")


if __name__ == "__main__":
    main()
