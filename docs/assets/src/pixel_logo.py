#!/usr/bin/env python3
"""Render the snp README banner: a 10-second pixel-art causal loop.

    python3 docs/assets/src/pixel_logo.py                # docs/assets/logo.webp, logo.gif and logo.png (still)
    python3 docs/assets/src/pixel_logo.py --frames DIR   # also every distinct frame as a PNG, for review

A wizard guards a stone bridge over a glowing digital abyss; behind him stands the repository (a gate marked SNP,
a terminal with `git log --graph` and flowing code). The loop is one causal circle:

    staff strike -> corrupted commit rejected (x lint, x test, x fmt) -> it tumbles into the abyss -> it trips the
    git hook lever -> gears turn, a pulse runs along the commit graph in the bridge -> the pulse charges the staff
    -> the wizard raises it while the next corrupted commit sneaks up -> staff strike (frame 0)

The last frames are the downswing whose impact is frame 0: no fade, no cut, no repeated frame.

Motion rules: one focal action at a time; anticipation before the strike and follow-through after it; eased
timing (ease-out to arrive, ease-in to fall, overshoot for pops, gravity arcs); impact = smear frame and hit-stop
(held frames), no camera shake; one pixel size everywhere (titles are Scale2x'd, never block-doubled), integer
positions, nearest-neighbour scaling, quarter-turn tumbles instead of resampled rotation, ordered dithering for
light and fades; one global palette; every idle cycle divides the loop, so nothing pops at the seam.

Requires Pillow (with WebP) and numpy. Everything is drawn from the sprite maps and procedures in this file.
"""

from __future__ import annotations

import argparse
import math
import random
import re
import struct
from pathlib import Path

import numpy as np
from PIL import Image

W, H, SCALE = 300, 100, 4
TICK_MS = 50           # 20 fps; holds are merged into longer frames
T = 200                # ticks per loop (10 s)
DECK = 70              # top of the bridge deck
GAP0, GAP1 = 206, 229  # the broken span of the bridge
STILL_TICK = 16
OUT = Path(__file__).resolve().parent.parent


def hexc(s: str) -> tuple[int, int, int]:
    s = s.lstrip("#")
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16))


P = {k: hexc(v) for k, v in {
    "ink": "#07060f", "outline": "#1a1730", "white": "#f2f0ff", "lav": "#aaa3d8", "dim": "#4f4c86",
    "bg0": "#07060f", "bg1": "#0b0a19", "bg2": "#100e24", "bg3": "#15132e",
    "arch": "#13112a", "arch_hi": "#1c1938", "trace": "#123326", "trace_hi": "#1f5a3c",
    "ab0": "#060a10", "ab1": "#08151a", "ab2": "#0b2421", "ab3": "#0f3a2c", "ab4": "#17583c",
    "stone0": "#151227", "stone1": "#231e3c", "stone2": "#322b52", "stone3": "#463e6c", "stone4": "#625a8e",
    "stone5": "#857db0",
    "rock0": "#100d1e", "rock1": "#1c1731", "rock2": "#2a2346", "rock3": "#3b3360",
    "gold0": "#fffbe6", "gold1": "#ffe58a", "gold2": "#ffd060", "gold3": "#e09a2a", "gold_ink": "#3a2a00",
    "warm": "#ffcf7a",
    "green0": "#c8ffd6", "green1": "#5cf07a", "green2": "#2fbf5f", "green3": "#1d7a45", "green4": "#0f3f2a",
    "red0": "#ffd0c8", "red1": "#ff4d63", "red2": "#d63a4a", "red3": "#8a1f2a", "red4": "#3f0f1a",
    "wood0": "#a0703f", "wood1": "#7b5434", "wood2": "#5e3e26",
    "screen": "#06120d", "code1": "#5cf07a", "code2": "#2fbf5f", "code3": "#1d7a45", "code_y": "#e5c07b",
    "code_b": "#61afef",
    "iron0": "#2a2a40", "iron1": "#41415e", "iron2": "#62628a",
}.items()}


# ---------------------------------------------------------------------------------------------- easing

def clamp01(v: float) -> float:
    return 0.0 if v < 0 else 1.0 if v > 1 else v


def prog(t: float, a: float, b: float) -> float:
    return clamp01((t - a) / (b - a)) if b > a else float(t >= a)


def ease_out(v: float, p: float = 3) -> float:
    return 1 - (1 - v) ** p


def ease_in(v: float, p: float = 2) -> float:
    return v ** p


def ease_io(v: float) -> float:
    return 4 * v ** 3 if v < 0.5 else 1 - (-2 * v + 2) ** 3 / 2


def ease_back(v: float, s: float = 1.9) -> float:
    v -= 1
    return 1 + (s + 1) * v ** 3 + s * v ** 2


def lerp(a: float, b: float, v: float) -> float:
    return a + (b - a) * v


def rnd(v: float) -> int:
    return int(math.floor(v + 0.5))


B4 = np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]], dtype=float)
BAYER = (np.tile(B4, (H // 4 + 1, W // 4 + 1))[:H, :W] + 0.5) / 16.0


def bayer(x: int, y: int) -> float:
    return (B4[y & 3, x & 3] + 0.5) / 16.0


def h32(*v: int) -> int:
    n = 2166136261
    for k in v:
        n = ((n ^ (k & 0xFFFFFFFF)) * 16777619) & 0xFFFFFFFF
    return n


# ---------------------------------------------------------------------------------------------- canvas

class Canvas:
    def __init__(self, a: np.ndarray) -> None:
        self.a = a

    def put(self, x: int, y: int, c, a: float = 1.0) -> None:
        if 0 <= x < W and 0 <= y < H and (a >= 1 or a > bayer(x, y)):
            self.a[y, x] = c

    def add(self, x: int, y: int, c, k: float) -> None:
        if 0 <= x < W and 0 <= y < H:
            p = self.a[y, x]
            self.a[y, x] = (min(255, int(p[0] + c[0] * k)), min(255, int(p[1] + c[1] * k)),
                            min(255, int(p[2] + c[2] * k)))

    def rect(self, x0: int, y0: int, x1: int, y1: int, c, a: float = 1.0) -> None:
        if a >= 1:
            x0, x1, y0, y1 = max(x0, 0), min(x1, W - 1), max(y0, 0), min(y1, H - 1)
            if x0 <= x1 and y0 <= y1:
                self.a[y0:y1 + 1, x0:x1 + 1] = c
            return
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                self.put(x, y, c, a)

    def sprite(self, x0: int, y0: int, rows, cmap: dict, a: float = 1.0) -> None:
        for dy, row in enumerate(rows):
            for dx, ch in enumerate(row):
                col = cmap.get(ch)
                if col is not None:
                    self.put(x0 + dx, y0 + dy, col, a)


def glow(cv: Canvas, cx: float, cy: float, r: float, color, strength: float, steps: int = 4) -> None:
    """Additive light, quantised to `steps` levels with ordered dithering (pixel-art falloff)."""
    if r <= 0 or strength <= 0:
        return
    x0, x1 = max(int(cx - r), 0), min(int(cx + r) + 1, W)
    y0, y1 = max(int(cy - r), 0), min(int(cy + r) + 1, H)
    if x0 >= x1 or y0 >= y1:
        return
    ys, xs = np.mgrid[y0:y1, x0:x1]
    d = np.hypot(xs - cx, ys - cy)
    v = np.clip(1.0 - d / r, 0, None) ** 2 * strength * steps
    q = np.floor(v) + ((v - np.floor(v)) > BAYER[y0:y1, x0:x1])
    add = (q[..., None] / steps) * np.array(color, dtype=float)
    reg = cv.a[y0:y1, x0:x1].astype(np.int16) + add.astype(np.int16)
    cv.a[y0:y1, x0:x1] = np.clip(reg, 0, 255).astype(np.uint8)


def line_pts(x0: int, y0: int, x1: int, y1: int) -> list[tuple[int, int]]:
    pts = []
    dx, dy = abs(x1 - x0), -abs(y1 - y0)
    sx, sy = (1 if x0 < x1 else -1), (1 if y0 < y1 else -1)
    err = dx + dy
    while True:
        pts.append((x0, y0))
        if x0 == x1 and y0 == y1:
            return pts
        e2 = 2 * err
        if e2 >= dy:
            err += dy
            x0 += sx
        if e2 <= dx:
            err += dx
            y0 += sy


def rot90(rows: list[str], k: int) -> list[str]:
    """Quarter turns are pixel-exact, so tumbling needs no resampling."""
    for _ in range(k % 4):
        rows = ["".join(r[i] for r in reversed(rows)) for i in range(len(rows[0]))]
    return rows


# ---------------------------------------------------------------------------------------------- fonts

FONT5 = {
    "A": [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "E": ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
    "H": ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "L": ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
    "N": ["#...#", "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#"],
    "O": [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "P": ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."],
    "S": [".####", "#....", "#....", ".###.", "....#", "....#", "####."],
    "T": ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
    "U": ["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "Y": ["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."],
    " ": ["...", "...", "...", "...", "...", "...", "..."],
}
FONT3 = {
    "F": ["###", "#..", "##.", "#..", "#.."], "M": ["#.#", "###", "###", "#.#", "#.#"],
    "T": ["###", ".#.", ".#.", ".#.", ".#."], "L": ["#..", "#..", "#..", "#..", "###"],
    "I": ["###", ".#.", ".#.", ".#.", "###"], "N": ["##.", "#.#", "#.#", "#.#", "#.#"],
    "E": ["###", "#..", "##.", "#..", "###"], "S": [".##", "#..", ".#.", "..#", "##."],
    ">": ["#..", ".#.", "..#", ".#.", "#.."],
}
CROSS = ["#...#", ".#.#.", "..#..", ".#.#.", "#...#"]


def scale2x(rows: list[str]) -> list[str]:
    """Scale2x (EPX): doubles a 1-bit glyph and rounds its diagonals, keeping one pixel size."""
    h, w = len(rows), len(rows[0])

    def px(x: int, y: int) -> bool:
        return 0 <= x < w and 0 <= y < h and rows[y][x] == "#"

    out = [[False] * (w * 2) for _ in range(h * 2)]
    for y in range(h):
        for x in range(w):
            p, a, b, c, d = px(x, y), px(x, y - 1), px(x + 1, y), px(x - 1, y), px(x, y + 1)
            out[2 * y][2 * x] = a if (c == a and c != d and a != b) else p
            out[2 * y][2 * x + 1] = b if (a == b and a != c and b != d) else p
            out[2 * y + 1][2 * x] = c if (d == c and d != b and c != a) else p
            out[2 * y + 1][2 * x + 1] = d if (b == d and b != a and d != c) else p
    return ["".join("#" if v else "." for v in r) for r in out]


def text3(cv: Canvas, x: int, y: int, s: str, color, a: float = 1.0) -> int:
    for ch in s:
        for gy, row in enumerate(FONT3[ch]):
            for gx, bit in enumerate(row):
                if bit == "#":
                    cv.put(x + gx, y + gy, color, a)
        x += 4
    return x


def build_word(word: str, x0: int, grad: list, shadow) -> list[tuple[int, np.ndarray, np.ndarray]]:
    """Per-letter tiles (x, rgb, mask): Scale2x glyphs with outline, two-pixel drop shadow and a gradient."""
    tiles, x = [], x0
    for ch in word:
        g = scale2x(FONT5[ch])
        gw, gh = len(g[0]), len(g)
        if ch != " ":
            rgb = np.zeros((gh + 5, gw + 2, 3), np.uint8)
            msk = np.zeros((gh + 5, gw + 2), bool)
            on = {(gx, gy) for gy, row in enumerate(g) for gx, b in enumerate(row) if b == "#"}
            for gx, gy in on:
                for sy in (2, 3):
                    rgb[gy + 1 + sy, gx + 1], msk[gy + 1 + sy, gx + 1] = shadow, True
            for gx, gy in on:
                for ddx, ddy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    if (gx + ddx, gy + ddy) not in on:
                        rgb[gy + 1 + ddy, gx + 1 + ddx], msk[gy + 1 + ddy, gx + 1 + ddx] = P["ink"], True
            for gx, gy in on:
                rgb[gy + 1, gx + 1], msk[gy + 1, gx + 1] = grad[gy], True
            tiles.append((x - 1, rgb, msk))
        x += gw + 2
    return tiles


def word_width(word: str) -> int:
    return sum(len(FONT5[ch][0]) * 2 + 2 for ch in word) - 2


TITLE = "YOU SHELL NOT PASS"
TITLE_X, TITLE_Y = W - word_width(TITLE) - 5, 3
TITLE_TILES = build_word(TITLE, TITLE_X, [hexc(c) for c in (
    "#ffffff", "#fbfaff", "#f4f2ff", "#e6e3f8", "#d6d2f1", "#c7c2ea", "#b9b3e3", "#aca5dc", "#a098d5", "#958dce",
    "#8b83c7", "#827ac0", "#7a72b9", "#726bb1")], hexc("#241f4a"))
SNP_X, SNP_Y = 11, 8
SNP_TILES = build_word("SNP", SNP_X, [hexc(c) for c in (
    "#e8ffee", "#c8ffd6", "#a6ffbc", "#86f9a0", "#6ff28c", "#5cf07a", "#4fe06c", "#45cf62", "#3cbf58", "#34ae4f",
    "#2d9e47", "#278f40", "#22823a", "#1d7a45")], hexc("#0a1a12"))


# ---------------------------------------------------------------------------------------------- sprites

WIZ = [  # 17x40, faces right; arm, hand and staff are drawn per pose; gold trim on the hat band and hem
    ".....OO..........", "....OhO..........", "....OhhO.........", "....OdhO.........",
    "....OdhHO........", "...OdhhHO........", "...OdhhhHO.......", "...OdhhhhHO......",
    "...OdhhhhhHO.....", "..OdhhhhhhHO.....", "..OdhhhhhhhHO....", "OOObbgbbgbbbbOOO.",
    "OdhhhhhhhhHHHHHO.", ".OOOOOOOOOOOOOO..", "...OWWWWWsww0....", "..OWWWWWWseso....",
    "..OWWWWWWSsssO...", "..OWWWWWWwwSO....", "..OWWWwwwwwwO....", "..OrrWwwwwwwrO...",
    ".ORrrWwwwwwwrrO..", ".ORrrWwwwwwrrrO..", ".ORrrWWwwwwrrrO..", ".ORrrrWwwwrrrrO..",
    ".OBBBBBWwwkBBBO..", ".ORrrrrWwrrrrrO..", ".ORrrrrwrrrrrLO..", ".ORrrrrrrrrrrLO..",
    ".ORRrrrrrrrrrLO..", ".ORrRrrrrrrrrLO..", ".ORrRrrrrrrrrLO..", "ORRrRrrrrrrrrLLO.",
    "ORRrRrrrrrrrrLLO.", "ORRrRrrrrrrrrLLO.", "ORRRrrrrrrrrrLLO.", "ORRRrrrrrrrrrLLO.",
    "ORRRrrrrrrrrrLLLO", "ORRRgrrgrrgrrrLLO", "OOOOOOOOOOOOOOOOO", "..OffO.....OffO..",
]
WIZ = [r.replace("0", "O") for r in WIZ]
assert all(len(r) == 17 for r in WIZ) and len(WIZ) == 40
WIZ_C = {
    "O": P["outline"], "h": hexc("#7d7c99"), "H": hexc("#a3a2bd"), "d": hexc("#5c5b78"), "b": hexc("#4a3d6a"),
    "g": P["gold2"], "s": hexc("#f2c9a0"), "S": hexc("#cf9f78"), "e": P["outline"], "w": hexc("#f4f2ff"),
    "W": hexc("#c4c2db"), "r": hexc("#b9b8cf"), "R": hexc("#8c8ba6"), "L": hexc("#dcdbea"), "B": hexc("#4a3d6a"),
    "k": P["gold2"], "f": hexc("#3a2f3f"),
}
SLEEVE, SLEEVE_HI, SLEEVE_LO = hexc("#b9b8cf"), hexc("#dcdbea"), hexc("#6e6d8c")

CREEP = {  # the corrupted commit, facing left (12x11): broken code with an error glyph on its body
    "walk0": ["..a......a..", "...a....a...", "..RRRRRRRR..", ".RkkkkkkkkR.", "RkWWkkxkxkkR", "RkWEkkkxkkkR",
              "RkkkkkxkxkkR", "RkTkTkTkkkkR", ".RkkkkkkkkR.", "..RRRRRRRR..", "..L..L.L..L."],
    "walk1": ["..a......a..", "..a......a..", "..RRRRRRRR..", ".RkkkkkkkkR.", "RkWWkkxkxkkR", "RkWEkkkxkkkR",
              "RkkkkkxkxkkR", "RkTkTkTkkkkR", ".RkkkkkkkkR.", "..RRRRRRRR..", ".L..L...L..L"],
    "sneak": ["............", "..a......a..", "...a....a...", "..RRRRRRRR..", ".RkkkkkkkkR.", "RkWWkkxkxkkR",
              "RkEWkkkxkkkR", "RkTkTkxkxkkR", ".RkkkkkkkkR.", "..RRRRRRRR..", ".L.L....L.L."],
    "glee": ["L.a......a.L", "L..a....a..L", "LLRRRRRRRRLL", ".RkkkkkkkkR.", "RkWWkkxkxkkR", "RkWEkkkxkkkR",
             "RkkkkkxkxkkR", "RkTTTTTTkkkR", ".RkkkkkkkkR.", "..RRRRRRRR..", "..L..L.L..L."],
    "hurt": [".a........a.", "..a......a..", "..RRRRRRRR..", ".RkkkkkkkkR.", "RkWWWkxkxkkR", "RkWEWkkxkkkR",
             "RkWWWkxkxkkR", "RkkTTTTkkkkR", ".RkkkkkkkkR.", "..RRRRRRRR..", ".L...LL...L."],
}
CREEP_C = {"R": hexc("#ff3b5c"), "k": hexc("#1d0b1c"), "W": P["white"], "E": hexc("#ff3b5c"), "x": hexc("#ff7a8a"),
           "T": P["red0"], "a": hexc("#ff8a9a"), "L": hexc("#c8304a")}

# ---------------------------------------------------------------------------------------------- layout

WX, WY = 60, DECK - len(WIZ)        # wizard sprite origin
SHIELD = (WX + 8, DECK - 20, 31)    # barrier arc: centre and radius
STOP_X = 104                        # where a corrupted commit is stopped
EDGE_X = 194                        # where a rejected commit teeters (the gap starts at GAP0)
PIVOT = (222, 85)                   # git hook lever pivot
GEAR = (232, 92, 6)
GEAR2 = (213, 95, 4)
TRACE_Y = 75                        # the commit graph carved into the bridge face

# ---------------------------------------------------------------------------------------------- timeline
# Ticks of 50 ms. 0 = impact of the staff (the loop's last frames are its downswing).
HIT_HOLD = (1, 2)                                      # hit-stop: frames 1 and 2 repeat the impact
LABELS = [(5, "LINT"), (9, "TEST"), (13, "FMT")]
FALL = (52, 70)                                        # tumbling into the abyss
LEVER_HIT = 70
PULSE = [(76, 84), (84, 118), (118, 122), (122, 136)]  # gear->bridge, along the bridge, into the staff, up it
NEXT = dict(enter=92, edge=128, hop=(140, 152), sneak=(154, 193))
POSES = {
    "idle": dict(hand=(15, 23), base=(16, 39), top=(16, -4), body=0),
    "raise": dict(hand=(15, 10), base=(16, 26), top=(16, -17), body=-1),
    "impact": dict(hand=(16, 24), base=(18, 40), top=(15, -3), body=2),
}
POSE_KEYS = [(0, "impact", None), (6, "impact", None), (18, "idle", ease_back), (158, "idle", None),
             (174, "raise", ease_out), (195, "raise", None), (T, "impact", ease_in)]


def pose_at(t: float) -> dict:
    t %= T
    for (t0, p0, _), (t1, p1, ease) in zip(POSE_KEYS, POSE_KEYS[1:]):
        if t0 <= t <= t1:
            v = (ease or ease_io)(prog(t, t0, t1))
            a, b = POSES[p0], POSES[p1]
            return {k: (lerp(a[k][0], b[k][0], v), lerp(a[k][1], b[k][1], v)) if isinstance(a[k], tuple)
                    else lerp(a[k], b[k], v) for k in a}
    return dict(POSES["impact"])


def crystal_of(pose: dict) -> tuple[float, float]:
    (bx, by), (tx, ty) = pose["base"], pose["top"]
    ln = math.hypot(tx - bx, ty - by) or 1
    return WX + tx + (tx - bx) / ln * 4, WY + ty + (ty - by) / ln * 4


def charge(t: int) -> float:
    """How charged the staff is: drained by the strike, refilled by the pulse."""
    if t < 10:
        return 1 - ease_out(prog(t, 0, 10))
    if t < PULSE[3][0]:
        return 0.0
    return min(1.0, 0.35 * prog(t, *PULSE[3]) + 0.65 * ease_in(prog(t, PULSE[3][1], 196)))


# ---------------------------------------------------------------------------------------------- background

def vgrad(a: np.ndarray, stops: list[tuple[int, str]], seam: int = 3) -> None:
    for y in range(H):
        k = max(i for i, (sy, _) in enumerate(stops) if sy <= y) if y >= stops[0][0] else 0
        a[y] = P[stops[k][1]]
        for j in range(1, len(stops)):
            sy = stops[j][0]
            if sy - seam <= y < sy + seam:
                f = (y - (sy - seam) + 0.5) / (2 * seam)
                a[y] = P[stops[j - 1][1]]
                a[y][BAYER[y] < f] = P[stops[j][1]]


def build_far() -> np.ndarray:
    """The cavern: a vaulted hall of stone arches with old circuitry, and the abyss glowing below."""
    a = np.zeros((H, W, 3), np.uint8)
    vgrad(a, [(0, "bg0"), (22, "bg1"), (46, "bg2"), (64, "bg3"), (76, "ab0"), (84, "ab1"), (90, "ab2"),
              (95, "ab3"), (99, "ab4")])
    cv = Canvas(a)
    for px0 in (98, 156, 214, 272):  # pillars of the back wall, with old circuit traces
        for y in range(10, 77):
            for x in range(px0, px0 + 9):
                cv.put(x, y, P["arch_hi"] if x == px0 else P["arch"], 1 - prog(y, 64, 80))
        cv.rect(px0 + 4, 18, px0 + 4, 70, P["trace"])
        for ny in (24, 38, 52):
            cv.rect(px0 + 3, ny, px0 + 5, ny + 2, P["trace"])
            cv.put(px0 + 4, ny + 1, P["trace_hi"])
    for cx in (127, 185, 243):  # arches between them
        for x in range(cx - 21, cx + 22):
            yy = rnd(16 - math.sqrt(max(0, 21 ** 2 - (x - cx) ** 2)) * 0.45)
            cv.put(x, yy, P["arch_hi"])
            cv.put(x, yy + 1, P["arch"])
    for x in range(W):  # ceiling stalactites
        d = 2 + h32(x // 3, 5) % 5 if (x // 3) % 3 else 1
        cv.rect(x, 0, x, d, P["ink"])
    rng = random.Random(4)
    for _ in range(26):
        cv.put(rng.randrange(80, W), rng.randrange(24, 64), P["dim"])
    return a


def build_near() -> tuple[np.ndarray, np.ndarray]:
    """Ledges, the bridge (broken over the gap), the gate of the repository, the hook mechanism's ledge."""
    a = np.zeros((H, W, 3), np.uint8)
    mask = np.zeros((H, W), bool)

    def put(x: int, y: int, c, al: float = 1.0) -> None:
        if 0 <= x < W and 0 <= y < H and (al >= 1 or al > bayer(x, y)):
            a[y, x], mask[y, x] = c, True

    def rock(x0: int, x1: int, y0: int, fade: int) -> None:
        for y in range(y0, H):
            for x in range(x0, x1):
                sy = y + rnd(1.5 * math.sin(x * 0.27) + math.sin(x * 0.09 + 2))
                layer, pos = sy // 4, sy % 4
                c = P["rock1"] if layer % 2 else P["rock2"]
                if pos == 0 and h32(x // 2, layer) % 3:
                    c = P["rock3"]
                elif pos == 3:
                    c = P["rock0"]
                put(x, y, c, 1 - prog(y, y0 + 6, y0 + fade))

    rock(0, 62, DECK, 26)
    rock(GAP1 + 1, W, DECK, 24)
    for x in list(range(0, 62)) + list(range(GAP1 + 1, W)):
        put(x, DECK, P["stone4"])
        put(x, DECK + 1, P["stone3"])
    for x in range(56, GAP0):  # bridge deck; the commit graph runs in a channel along its face
        broken = x >= GAP0 - 3 and (x - (GAP0 - 3)) * 2 > (h32(x, 1) % 5)
        put(x, DECK, P["stone5"] if h32(x, 9) % 7 == 0 else P["stone4"])
        for y in range(DECK + 1, DECK + 8):
            if broken and y > DECK + 4:
                continue
            c = P["stone2"]
            if y == DECK + 1:
                c = P["stone3"]
            elif y == DECK + 7:
                c = P["stone0"]
            elif y == TRACE_Y:
                c = P["stone1"]
            elif x % 10 == 0:
                c = P["stone0"]
            put(x, y, c)
    for y in range(DECK + 8, H):  # pillar under the wizard
        for x in range(70, 80):
            put(x, y, P["stone3"] if x == 70 else P["stone1"] if x > 77 else P["stone2"], 1 - prog(y, 82, 98))
    for y in range(92, H):  # the mechanism's ledge in the abyss
        for x in range(206, 242):
            if abs(x - 224) < 18 - (y - 92) * 0.6:
                put(x, y, P["rock3"] if y == 92 else P["rock2"] if (y + x // 3) % 4 else P["rock1"])
    for x0 in (1, 51):  # the repository: gate columns...
        for y in range(24, DECK):
            for x in range(x0, x0 + 8):
                c = P["stone3"] if x == x0 else P["stone1"] if x == x0 + 7 else P["stone2"]
                put(x, y, P["stone0"] if (y - 24) % 7 == 6 else c)
    for y in range(4, 26):  # ...a lintel carrying the SNP sign...
        for x in range(0, 60):
            c = P["stone2"]
            if y in (4, 25) or x in (0, 59):
                c = P["stone0"]
            elif y == 5:
                c = P["stone4"]
            put(x, y, c)
    for y in range(26, DECK):  # ...and a dark arched chamber
        for x in range(9, 51):
            if y > 34 or math.hypot(x - 29.5, (y - 36) * 1.6) < 21:
                put(x, y, hexc("#05040c"))
    return a, mask


FAR = build_far()
NEAR, NEAR_MASK = build_near()

# ---------------------------------------------------------------------------------------------- scene parts

SCREEN = (13, 42, 46, 64)
GRAPH_ROWS = ["*.", "|*", "*|", "|.", "*."]  # `git log --graph` gutter, repeating every 5 lines


def draw_repo(cv: Canvas, t: int) -> None:
    """The protected repository: a terminal with `git log --graph` and code flowing up (period divides T)."""
    x0, y0, x1, y1 = SCREEN
    cv.rect(x0 - 1, y0 - 1, x1 + 1, y1 + 1, P["iron1"])
    cv.rect(x0, y0, x1, y1, P["screen"])
    scroll = (t // 4) % 50
    for n in range(-1, 12):
        k = (n + scroll // 5) % 10
        y = y0 + 2 + n * 5 - scroll % 5
        if y < y0 + 1 or y + 1 > y1 - 1:
            continue
        for i, ch in enumerate(GRAPH_ROWS[k % 5]):
            gx = x0 + 2 + i * 3
            if ch == "*":
                cv.rect(gx, y, gx + 1, y + 1, P["gold2"] if k == 0 else P["green1"])
            elif ch == "|":
                cv.rect(gx, max(y0 + 1, y - 2), gx, min(y1 - 1, y + 3), P["green3"])
        lx = x0 + 10
        for ln, col in ((3 + h32(k, 1) % 4, "code_b"), (2 + h32(k, 2) % 5, "code1"),
                        (2 + h32(k, 3) % 6, "code_y" if k % 3 == 0 else "code2")):
            if lx + ln > x1 - 2:
                break
            cv.rect(lx, y, lx + ln - 1, y, P[col])
            lx += ln + 1
    glow(cv, (x0 + x1) / 2, (y0 + y1) / 2, 30, P["green2"], 0.22, steps=4)
    glow(cv, SNP_X + 17, SNP_Y + 8, 24, P["green2"], 0.18 + 0.12 * (1 - prog(t, 4, 24)), steps=3)
    for x, rgb, msk in SNP_TILES:
        h, w = msk.shape
        cv.a[SNP_Y:SNP_Y + h, x:x + w][msk] = rgb[msk]


def draw_title(cv: Canvas, t: int) -> None:
    for i, (x, rgb, msk) in enumerate(TITLE_TILES):
        h, w = msk.shape
        cv.a[TITLE_Y:TITLE_Y + h, x:x + w][msk] = rgb[msk]
        hit = 8 + i * 0.7  # the strike's sparks reach each letter in turn
        if hit <= t < hit + 8:
            k = 1 - prog(t, hit, hit + 8)
            sub = cv.a[TITLE_Y:TITLE_Y + h, x:x + w]
            inner = msk & (rgb.astype(int).sum(-1) > 300)
            sub[inner] = np.clip(sub[inner].astype(int) * (1 - 0.5 * k) + np.array(P["green1"]) * 0.5 * k,
                                 0, 255).astype(np.uint8)
            glow(cv, x + w / 2, TITLE_Y + 8, 10, P["green1"], 0.35 * k, steps=3)


def draw_abyss(cv: Canvas, t: int) -> None:
    """Digital rain falling into the glow, and motes rising out of it (periods divide T)."""
    for i in range(22):
        per = (40, 50, 100, 200)[i % 4]
        f = ((t / per) + (h32(i, 1) % 100) / 100) % 1.0
        x = 84 + h32(i, 2) % 214
        y = rnd(74 + f * 30)
        cv.put(x, y, P["green2"] if i % 3 else P["green3"], 0.9)
        cv.put(x, y - 1, P["green3"], 0.5)
    for i in range(10):
        f = ((t / (100 if i % 2 else 200)) + (h32(i, 3) % 100) / 100) % 1.0
        x = 90 + h32(i, 4) % 200 + rnd(2 * math.sin(2 * math.pi * (f * 2 + i / 10)))
        cv.put(x, rnd(100 - f * 40), P["green0"] if f < 0.3 else P["green1"], 1 - f)


def trace_path() -> list[tuple[int, int]]:
    """Where the pulse runs: from the gear, up the cable, along the bridge face, up into the staff."""
    pts = [(x, GEAR[1]) for x in range(GEAR[0], GEAR[0] - 6, -1)]
    pts += [(GEAR[0] - 6, y) for y in range(GEAR[1], TRACE_Y - 1, -1)]
    pts += [(x, TRACE_Y) for x in range(GEAR[0] - 7, WX + 17, -1)]
    pts += [(WX + 17, y) for y in range(TRACE_Y - 1, DECK - 1, -1)]
    return pts


TRACE = trace_path()
I_BRIDGE = next(i for i, (x, _) in enumerate(TRACE) if x == GAP0 - 1)
I_STAFF = next(i for i, (_, y) in enumerate(TRACE) if y < TRACE_Y)
NODES = [196, 182, 168, 154, 140, 126, 112, 98]  # commits on the graph in the bridge face
BRANCH = (186, 158)                              # a feature branch splits off and merges back
BRANCH_NODES = [178, 166]


def pulse_index(t: int) -> float:
    (a0, a1), (b0, b1), (c0, c1) = PULSE[0], PULSE[1], PULSE[2]
    if t < a0:
        return -1
    if t < a1:
        return lerp(0, I_BRIDGE, prog(t, a0, a1))
    if t < b1:
        return lerp(I_BRIDGE, I_STAFF, ease_io(prog(t, b0, b1)))
    return lerp(I_STAFF, len(TRACE) - 1, prog(t, c0, c1))


def draw_trace(cv: Canvas, t: int) -> None:
    head = pulse_index(t)
    fade = 1 - prog(t, 150, 196)  # the lit graph cools down before the next cycle
    head_x = TRACE[min(int(head), len(TRACE) - 1)][0] if head >= I_BRIDGE else 999
    for i, (x, y) in enumerate(TRACE):
        lit = head >= i and fade > 0
        if lit and head - i < 6:
            cv.put(x, y, P["green0"])
        elif lit:
            cv.put(x, y, P["green2"], max(fade, 0.3))
            if fade < 1:
                cv.put(x, y, P["trace"], 1 - max(fade, 0.3))
        else:
            cv.put(x, y, P["trace"])
    bx0, bx1 = BRANCH
    for x in range(bx1, bx0 + 1):
        y = TRACE_Y - 2 if bx1 + 2 <= x <= bx0 - 2 else TRACE_Y - 1
        cv.put(x, y, P["green2"] if head_x <= x and fade > 0.3 else P["trace"])
    for nx, ny in [(n, TRACE_Y) for n in NODES] + [(n, TRACE_Y - 2) for n in BRANCH_NODES]:
        lit = head_x <= nx and fade > 0.3
        fresh = lit and head_x > nx - 12
        cv.rect(nx - 1, ny - 1, nx + 1, ny + 1, P["green0"] if fresh else P["green1"] if lit else P["trace_hi"])
        if fresh:
            glow(cv, nx, ny, 6, P["green1"], 0.5, steps=3)
    if 0 <= head < len(TRACE) - 1:
        hx, hy = TRACE[int(head)]
        glow(cv, hx, hy, 9, P["green1"], 0.7, steps=4)
        cv.put(hx, hy, P["white"])


def draw_mechanism(cv: Canvas, t: int) -> None:
    """The git hook: a hook-shaped lever on a pivot and two gears that turn when it is tripped."""
    spin = ease_out(prog(t, LEVER_HIT, LEVER_HIT + 50)) * 2 * math.pi * 1.5
    lit = 1 - prog(t, 120, 190) if t >= LEVER_HIT else 0.0
    for (gx, gy, r), dirn in ((GEAR, 1), (GEAR2, -1)):
        teeth = 8 if r > 4 else 6
        rim = P["green2"] if lit > 0.3 else P["iron1"]
        for y in range(gy - r - 1, gy + r + 2):
            for x in range(gx - r - 1, gx + r + 2):
                d = math.hypot(x - gx, y - gy)
                if d <= r - 0.5:
                    cv.put(x, y, P["iron0"] if d > 1.5 else P["iron2"])
                elif d <= r + 0.4:
                    cv.put(x, y, rim)
        for k in range(teeth):
            ang = spin * dirn * (6 / r) + k * 2 * math.pi / teeth
            cv.put(rnd(gx + (r + 1.2) * math.cos(ang)), rnd(gy + (r + 1.2) * math.sin(ang)), rim)
            cv.put(rnd(gx + (r - 2) * math.cos(ang)), rnd(gy + (r - 2) * math.sin(ang)), P["iron2"])
        if lit > 0:
            glow(cv, gx, gy, r + 6, P["green1"], 0.35 * lit, steps=3)
    px, py = PIVOT  # lever: up when armed, slammed down when tripped, springs back
    cv.rect(px, py, px + 1, 92, P["iron1"])
    down = 0.0
    if t >= LEVER_HIT - 1:
        down = ease_out(prog(t, LEVER_HIT - 1, LEVER_HIT + 2)) * (1 - ease_io(prog(t, 100, 130)))
    ang = math.radians(lerp(-150, 165, down))
    ex, ey = rnd(px + 11 * math.cos(ang)), rnd(py + 11 * math.sin(ang))
    for x, y in line_pts(px, py, ex, ey):
        cv.put(x, y, P["iron2"])
        cv.put(x, y + 1, P["iron0"])
    for dx, dy in ((-1, 1), (-1, 2), (0, 3), (1, 2)):  # the hook at its end
        cv.put(ex + dx, ey + dy, P["green1"] if lit > 0.3 else P["iron2"])
    cv.rect(px - 1, py - 1, px + 1, py + 1, P["green1"] if lit > 0.3 else P["iron2"])


def creep_state(t: int):
    """(sprite rows, x, y) of the corrupted commit on screen, or None."""
    y0 = DECK - 11
    if t < 3:  # struck (frame 0 and the hit-stop)
        return CREEP["hurt"], STOP_X + (2 if t >= 1 else 0), y0
    if t < 22:  # knocked back in two bouncing arcs, tumbling
        if t < 13:
            v, x0, x1, hgt = prog(t, 3, 13), STOP_X + 2, 152, 16
        else:
            v, x0, x1, hgt = prog(t, 13, 22), 152, 180, 7
        x = lerp(x0, x1, v)
        return rot90(CREEP["hurt"], t // 2 + 1), rnd(x), rnd(y0 - hgt * 4 * v * (1 - v))
    if t < 30:
        return CREEP["hurt"], rnd(lerp(180, EDGE_X, ease_out(prog(t, 22, 30)))), y0
    if t < FALL[0]:  # teeters on the broken edge, arms flailing
        lean = (0, 1, 2, 1, 0, 1, 2, 3)[((t - 30) // 3) % 8]
        return CREEP["glee"] if (t // 3) % 2 else CREEP["hurt"], EDGE_X + lean, y0 + (1 if lean == 3 else 0)
    if t < FALL[1]:  # falls, tumbling, onto the lever
        v = prog(t, *FALL)
        return (rot90(CREEP["hurt"], (t - FALL[0]) // 3 + 1), rnd(lerp(EDGE_X + 3, PIVOT[0] - 16, v)),
                rnd(lerp(y0, 70, ease_in(v))))
    e, edge, hop, sneak = NEXT["enter"], NEXT["edge"], NEXT["hop"], NEXT["sneak"]
    if t < e:
        return None
    if t < edge:  # the next one walks in...
        return CREEP["walk0" if (t // 3) % 2 else "walk1"], rnd(lerp(W + 2, GAP1 + 3, prog(t, e, edge))), y0
    if t < hop[0]:  # ...peers into the gap...
        return CREEP["sneak"], GAP1 + 3 - (1 if (t // 4) % 2 else 0), y0
    if t < hop[1]:  # ...hops it...
        v = prog(t, *hop)
        return CREEP["glee"], rnd(lerp(GAP1 + 3, EDGE_X - 2, v)), rnd(y0 - 15 * 4 * v * (1 - v))
    if t < sneak[0]:
        return CREEP["sneak"], EDGE_X - 2, y0
    if t < sneak[1]:  # ...sneaks up to the wizard...
        x = lerp(EDGE_X - 2, STOP_X, ease_io(prog(t, *sneak)))
        return CREEP["sneak" if (t // 2) % 2 else "walk0"], rnd(x), y0
    return CREEP["glee"], STOP_X, y0 - (1 if (t // 2) % 2 else 0)  # ...and gloats


def draw_creep(cv: Canvas, t: int) -> None:
    st = creep_state(t)
    if st is None:
        return
    rows, x, y = st
    glitch = h32(t // 3, 7) % 5 == 0
    gy = 3 + (t // 3) % 4
    for dy, row in enumerate(rows):
        off = (1 if (t // 3) % 2 else -1) if glitch and dy in (gy, gy + 1) else 0
        for dx, ch in enumerate(row):
            c = CREEP_C.get(ch)
            if c is not None:
                cv.put(x + dx + off, y + dy, c)
                if off and ch == "R":
                    cv.put(x + dx - off, y + dy, hexc("#4de0ff"))  # chromatic split of a glitch
    glow(cv, x + 6, y + 5, 10, P["red1"], 0.18, steps=3)


def draw_labels(cv: Canvas, t: int) -> None:
    """x lint, x test, x fmt: the failed hooks pop out of the rejected commit and float away."""
    for i, (t0, name) in enumerate(LABELS):
        a = t - t0
        if not 0 <= a < 40:
            continue
        x0 = (118, 142, 168)[i]
        y0 = (36, 28, 38)[i] - rnd(6 * ease_out(prog(a, 0, 40)))
        fade = 1 - prog(a, 28, 40)
        w = 13 + len(name) * 4
        if a < 3:  # pops: sliver, overshoot, settle
            hh = (3, 7, 11)[a]
            cv.rect(x0, y0 + 4 - hh // 2, x0 + w - 1, y0 + 4 + hh // 2, P["red2"])
            continue
        if a < 9:
            x0 += (1, -1, 1, -1, 1, 0)[a - 3]
        cv.rect(x0 + 1, y0, x0 + w - 2, y0 + 8, P["red2"], fade)
        cv.rect(x0, y0 + 1, x0 + w - 1, y0 + 7, P["red2"], fade)
        cv.rect(x0 + 1, y0 + 1, x0 + w - 2, y0 + 7, P["red4"], fade)
        cv.sprite(x0 + 3, y0 + 2, CROSS, {"#": P["red1"]}, fade)
        text3(cv, x0 + 10, y0 + 2, name, P["red0"], fade)


def draw_wizard(cv: Canvas, t: int, pose: dict) -> None:
    body = rnd(pose["body"])
    idle = 18 <= t < 158
    if idle:
        body = 1 if (t % 40) >= 20 else 0  # breathing
    lag = pose_at(t - 2)["body"] - pose["body"]
    sway = max(-2, min(2, rnd(lag * 1.2)))  # the hat tip follows through
    blink = idle and (t % 100) in (60, 61, 62)
    split = 26
    for dy in range(split, len(WIZ)):
        for dx, ch in enumerate(WIZ[dy]):
            if ch in WIZ_C:
                cv.put(WX + dx, WY + dy, WIZ_C[ch])
    rows = list(range(split)) + [split - 1] * max(0, -body)
    for n, dy in enumerate(rows):
        yy = WY + dy + body if n < split else WY + split - 1 + body + (n - split + 1)
        xs = -rnd(sway * (7 - dy) / 7) if dy < 7 else 0
        for dx, ch in enumerate(WIZ[dy]):
            if ch == "e" and blink:
                ch = "s"
            if ch in WIZ_C:
                cv.put(WX + dx + xs, yy, WIZ_C[ch])
    glow_k = 0.2 + 0.5 * charge(t)
    for dy, row in enumerate(WIZ):  # rim light: the staff in front (warm), the repository behind (green)
        yy = WY + dy + (body if dy < split else 0)
        inner = [i for i, ch in enumerate(row) if ch not in ".O"]
        if inner:
            cv.add(WX + inner[-1], yy, P["warm"], glow_k)
            if dy % 2 == 0:
                cv.add(WX + inner[0], yy, P["green2"], 0.35)
    bx, by = WX + rnd(pose["base"][0]), WY + rnd(pose["base"][1])
    tx, ty = WX + rnd(pose["top"][0]), WY + rnd(pose["top"][1])
    pts = line_pts(bx, by, tx, ty)
    climb = prog(t, *PULSE[3]) if t >= PULSE[3][0] else (1.0 if t < 10 else 0.0)
    for i, (x, y) in enumerate(pts):  # the staff's runes light up as the pulse climbs it
        rune = i % 6 == 3
        lit = rune and i / max(1, len(pts) - 1) <= climb and charge(t) > 0.05
        cv.put(x, y, P["green1"] if lit else P["wood2"] if rune else P["wood0"])
        cv.put(x + 1, y, P["green3"] if lit else P["wood2"] if rune else P["wood1"])
    hx, hy = WX + rnd(pose["hand"][0]), WY + rnd(pose["hand"][1])
    arm = line_pts(WX + 10, WY + 20 + body, hx, hy)
    for x, y in arm:
        cv.rect(x - 1, y - 1, x + 1, y + 1, SLEEVE_LO)
    for x, y in arm:
        cv.put(x, y, SLEEVE)
        cv.put(x, y - 1, SLEEVE_HI)
    cv.rect(hx, hy, hx + 1, hy + 1, WIZ_C["s"])
    cv.put(hx, hy + 1, WIZ_C["S"])


def draw_crystal(cv: Canvas, t: int, cx: int, cy: int) -> None:
    """The staff's head: a little terminal of amber light with a prompt and a blinking cursor."""
    k = charge(t)
    glow(cv, cx, cy, 12 + 16 * k, P["warm"], 0.35 + 0.55 * k, steps=5)
    if k > 0.3:
        glow(cv, cx, cy, 8 + 8 * k, P["green1"], 0.4 * k, steps=3)
    face = P["gold0"] if k > 0.6 else P["gold1"]
    cv.rect(cx - 4, cy - 3, cx + 4, cy + 3, P["gold3"])
    cv.rect(cx - 3, cy - 4, cx + 3, cy + 4, P["gold3"])
    cv.rect(cx - 3, cy - 2, cx + 3, cy + 2, face)
    cv.rect(cx - 2, cy - 3, cx + 2, cy + 3, face)
    for yy, row in enumerate(FONT3[">"]):
        for xx, bit in enumerate(row):
            if bit == "#":
                cv.put(cx - 3 + xx, cy - 2 + yy, P["gold_ink"])
    if (t % 20) < 10 or k > 0.9:
        cv.rect(cx + 1, cy + 2, cx + 3, cy + 2, P["gold_ink"])


def draw_shield(cv: Canvas, t: int) -> None:
    """The barrier commits cannot cross: a faint arc of light that flares green on the strike."""
    cx, cy, r = SHIELD
    flare = 1 - prog(t, 3, 16) if t < 16 else 0.0
    for a10 in range(-700, 701, 5):
        ang = math.radians(a10 / 10)
        for k in range(3 if flare > 0.5 else 2 if flare > 0 else 1):
            rr = r + k - (1 if flare > 0 else 0)
            x, y = rnd(cx + rr * math.cos(ang)), rnd(cy + rr * math.sin(ang))
            if y >= DECK:
                continue
            if flare > 0:
                cv.put(x, y, P["white"] if flare > 0.6 else P["green1"], 1.0 if k == 1 or flare > 0.6 else 0.6)
            else:
                run = ((a10 // 10) + t * 3) % 50 < 7
                cv.add(x, y, P["warm"], 0.85 if run else 0.22)
    if flare > 0:
        glow(cv, cx + r - 2, cy + 2, 30, P["green1"], 0.6 * flare, steps=5)


def draw_strike(cv: Canvas, t: int) -> None:
    """The strike: a smear on the downswing, then a green pulse across the deck, sparks and error debris."""
    if t >= 197:  # downswing smear (the impact itself is frame 0)
        _, oy = crystal_of(POSES["raise"])
        for y in range(rnd(oy) + (t - 197) * 10, DECK):
            for x in range(WX + 14, WX + 21):
                if 0.55 * (1 - abs(x - WX - 17) / 4) > bayer(x, y):
                    cv.add(x, y, P["gold1"], 0.6)
    if t < 14:
        v = ease_out(prog(t, 0, 14))
        bx = WX + 18
        for x in range(W):
            d = abs(x - bx)
            if 120 * v - 6 <= d <= 120 * v and not GAP0 <= x <= GAP1:
                cv.put(x, DECK, P["green0"], 1 - v)
                cv.put(x, DECK - 1, P["green1"], 0.7 * (1 - v))
        rr = 3 + 14 * v
        for a10 in range(1800, 3601, 40):
            ang = math.radians(a10 / 10)
            cv.put(rnd(bx + rr * math.cos(ang)), rnd(DECK + rr * 0.4 * math.sin(ang)), P["green1"], 1 - v)
        glow(cv, bx, DECK - 2, 18 * (1 - v) + 4, P["green1"], 0.9 * (1 - v), steps=4)
    for t0, ox, oy, n in ((3, STOP_X + 6, DECK - 6, 16), (13, 152, DECK - 3, 8), (22, 180, DECK - 3, 8)):
        for i in range(n):  # red error debris on the knock-back and each bounce
            life = 12 + h32(i, 5) % 10
            a = t - t0
            if 0 <= a < life:
                ang = math.radians(200 + h32(i, 6, t0) % 140)
                sp = 1.2 + (h32(i, 7) % 100) / 50
                dist = sp * (1 - 0.85 ** a) / 0.15
                col = P["red0"] if a < 3 else P["red1"] if a < life * 0.6 else P["red3"]
                cv.put(rnd(ox + dist * math.cos(ang)), rnd(oy + dist * math.sin(ang) + 0.08 * a * a), col)
    for i in range(24):  # the strike's energy flies up into the title, letter by letter
        a = t - (2 + i * 0.25)
        if 0 <= a < 8:
            tx = TITLE_X + 6 + (i * 9) % (len(TITLE) * 12 - 10)
            v = ease_in(prog(a, 0, 7))
            x = lerp(WX + 18, tx, v) + 10 * math.sin(math.pi * v) * (1 if i % 2 else -1)
            y = lerp(DECK - 4, TITLE_Y + 8, v)
            cv.put(rnd(x), rnd(y), P["green0"])
            cv.put(rnd(x), rnd(y) + 1, P["green1"], 0.6)


def draw_charge(cv: Canvas, t: int, cx: float, cy: float) -> None:
    """Sparks drawn into the crystal while it charges; the commit shattering on the hook."""
    k = charge(t)
    if k > 0.2 and t > 100:
        for i in range(12):
            f = ((t + i) % 12) / 12
            ang = math.radians(h32(i, 9, (t + i) // 12) % 360)
            rr = (18 + 6 * k) * (1 - ease_in(f))
            cv.put(rnd(cx + rr * math.cos(ang)), rnd(cy + rr * math.sin(ang)), P["gold0"] if i % 3 else P["green0"],
                   min(1.0, k * 1.5))
    a = t - LEVER_HIT
    if 0 <= a < 22:
        for i in range(18):
            ang = math.radians(h32(i, 10) % 360)
            sp = 0.6 + (h32(i, 11) % 100) / 70
            x = PIVOT[0] - 12 + sp * a * math.cos(ang)
            y = 80 + sp * a * math.sin(ang) * 0.6 + 0.12 * a * a
            cv.put(rnd(x), rnd(y), P["red1"] if i % 3 else P["red0"], 1 - prog(a, 10, 22))
        if a < 4:
            glow(cv, PIVOT[0] - 10, 80, 12, P["red1"], 0.6 * (1 - a / 4), steps=3)


# ---------------------------------------------------------------------------------------------- frame

def render(t: int) -> np.ndarray:
    t %= T
    if t in HIT_HOLD:
        t = 0
    a = FAR.copy()
    cv = Canvas(a)
    draw_abyss(cv, t)
    a[NEAR_MASK] = NEAR[NEAR_MASK]
    draw_repo(cv, t)
    draw_mechanism(cv, t)
    draw_trace(cv, t)
    draw_title(cv, t)
    draw_shield(cv, t)
    pose = pose_at(t)
    draw_wizard(cv, t, pose)
    cx, cy = crystal_of(pose)
    draw_crystal(cv, t, rnd(cx), rnd(cy))
    draw_charge(cv, t, cx, cy)
    draw_creep(cv, t)
    draw_strike(cv, t)
    draw_labels(cv, t)
    return a


# ---------------------------------------------------------------------------------------------- encoders

def lzw(data: bytes, min_size: int = 8) -> bytes:
    """GIF LZW image data (variable code width, clear at 4095, giflib's code-width schedule), in sub-blocks."""
    clear, eoi = 1 << min_size, (1 << min_size) + 1
    out = bytearray()
    buf = nbits = 0
    size, nxt = min_size + 1, eoi + 1
    limit = 1 << size
    table: dict[int, int] = {}

    def emit(code: int) -> None:
        nonlocal buf, nbits
        buf |= code << nbits
        nbits += size
        while nbits >= 8:
            out.append(buf & 0xFF)
            buf >>= 8
            nbits -= 8

    emit(clear)
    prefix = data[0]
    for b in data[1:]:
        key = (prefix << 8) | b
        code = table.get(key)
        if code is not None:
            prefix = code
            continue
        emit(prefix)
        if nxt >= limit and size < 12:
            size += 1
            limit <<= 1
        if nxt >= 4095:
            emit(clear)
            table.clear()
            size, nxt = min_size + 1, eoi + 1
            limit = 1 << size
        else:
            table[key] = nxt
            nxt += 1
        prefix = b
    emit(prefix)
    if nxt >= limit and size < 12:
        size += 1
    emit(eoi)
    if nbits:
        out.append(buf & 0xFF)
    blocks = bytearray([min_size])
    for i in range(0, len(out), 255):
        chunk = out[i:i + 255]
        blocks.append(len(chunk))
        blocks += chunk
    blocks.append(0)
    return bytes(blocks)


def write_gif(path: Path, frames: list[np.ndarray], delays_cs: list[int], palette: np.ndarray) -> None:
    """frames: palette-index arrays at art resolution; index 255 is reserved as 'unchanged' (transparent)."""
    def enc(reg: np.ndarray) -> bytes:
        return lzw(np.repeat(np.repeat(reg, SCALE, axis=0), SCALE, axis=1).astype(np.uint8).tobytes())

    pal = np.zeros((256, 3), np.uint8)
    pal[:len(palette)] = palette
    out = bytearray(b"GIF89a")
    out += struct.pack("<HHBBB", W * SCALE, H * SCALE, 0xF7, 0, 0)
    out += pal.tobytes()
    out += b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
    prev = None
    for f, d in zip(frames, delays_cs):
        flags = 1 << 2
        if prev is None:
            x0, y0, x1, y1 = 0, 0, W, H
            data = enc(f)
        else:
            diff = f != prev
            ys, xs = np.nonzero(diff)
            x0, y0, x1, y1 = int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1
            region = f[y0:y1, x0:x1]
            data = enc(region)
            alt = enc(np.where(diff[y0:y1, x0:x1], region, 255))
            if len(alt) < len(data):
                data, flags = alt, flags | 1
        out += struct.pack("<BBBBHBB", 0x21, 0xF9, 4, flags, d, 255, 0)
        out += struct.pack("<BHHHHB", 0x2C, x0 * SCALE, y0 * SCALE, (x1 - x0) * SCALE, (y1 - y0) * SCALE, 0)
        out += data
        prev = f
    out += b"\x3b"
    path.write_bytes(bytes(out))


def to_palette(frames: list[np.ndarray]) -> tuple[list[np.ndarray], np.ndarray]:
    """One global palette for every frame: exact up to 255 colours; otherwise every hand-picked colour (each hex
    literal in this file) keeps its slot and the remaining slots are fitted to the light mixes by weighted k-means."""
    codes = [(f[..., 0].astype(np.int32) << 16) | (f[..., 1].astype(np.int32) << 8) | f[..., 2] for f in frames]
    uniq, counts = np.unique(np.concatenate([c.ravel() for c in codes]), return_counts=True)
    rgb = np.stack([(uniq >> 16) & 255, (uniq >> 8) & 255, uniq & 255], axis=1).astype(float)
    if len(uniq) <= 255:
        palette, lut = rgb, np.arange(len(uniq))
    else:
        lits = {int(h, 16) for h in re.findall(r"#([0-9a-fA-F]{6})\b", Path(__file__).read_text())}
        fixed = rgb[np.isin(uniq, list(lits))]

        def dist2(a: np.ndarray, b: np.ndarray) -> np.ndarray:
            return ((a[:, None, :] - b[None, :, :]) ** 2 * np.array([0.30, 0.59, 0.11])).sum(-1)

        centers = fixed.copy()
        for _ in range(255 - len(fixed)):
            centers = np.vstack([centers, rgb[np.argmax(counts * dist2(rgb, centers).min(1))]])
        for _ in range(20):
            near = dist2(rgb, centers).argmin(1)
            for k in range(len(fixed), len(centers)):
                m = near == k
                if m.any():
                    centers[k] = (rgb[m] * counts[m, None]).sum(0) / counts[m].sum()
        palette = centers
        lut = dist2(rgb, palette).argmin(1)
    palette = np.clip(np.rint(palette), 0, 255).astype(np.uint8)
    return [lut[np.searchsorted(uniq, c)].astype(np.uint8) for c in codes], palette


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--frames", type=Path, help="also write every distinct frame as a PNG into this directory")
    args = ap.parse_args()
    idx, palette = to_palette([render(t) for t in range(T)])
    frames, delays, ticks = [], [], []
    for t, f in enumerate(idx):  # merge identical neighbours into one longer frame
        if frames and np.array_equal(frames[-1], f):
            delays[-1] += TICK_MS
            continue
        frames.append(f)
        delays.append(TICK_MS)
        ticks.append(t)
    write_gif(OUT / "logo.gif", frames, [d // 10 for d in delays], palette)
    imgs = [Image.fromarray(palette[np.repeat(np.repeat(f, SCALE, axis=0), SCALE, axis=1)]) for f in frames]
    imgs[0].save(OUT / "logo.webp", save_all=True, append_images=imgs[1:], duration=delays, loop=0,
                 lossless=True, method=6, quality=100)
    still = Image.fromarray(np.repeat(np.repeat(idx[STILL_TICK], SCALE, axis=0), SCALE, axis=1), mode="P")
    still.putpalette(palette.tobytes())
    still.save(OUT / "logo.png", optimize=True)
    if args.frames:
        args.frames.mkdir(parents=True, exist_ok=True)
        for t, im in zip(ticks, imgs):
            im.save(args.frames / f"t{t:03d}.png")
    for name in ("logo.webp", "logo.gif", "logo.png"):
        print(f"wrote {OUT / name} ({(OUT / name).stat().st_size / 1e6:.2f} MB)")
    print(f"{len(frames)} frames, {len(palette)} colours")


if __name__ == "__main__":
    main()
