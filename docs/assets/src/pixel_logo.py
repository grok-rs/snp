#!/usr/bin/env python3
"""Render the snp README banner: a looping pixel-art short about what commit hooks do for a codebase.

    python3 docs/assets/src/pixel_logo.py                # docs/assets/logo.gif (animated) and logo.png (still)
    python3 docs/assets/src/pixel_logo.py --frames DIR   # also every distinct frame as a PNG, for review

The short (14.4 s, seamless loop). A developer types `git commit`; each commit flies out of the terminal to the
wizard who guards the bridge to `main`. He reads it with his ">_" staff while the hooks run, one chip each:

1. badly formatted code: the formatter re-indents it on the spot, every hook passes, the commit crosses to main;
2. a lint error and a failing test, with a bug riding along: he raises the staff and slams it down (you shell
   not pass) and the commit is thrown back to the terminal;
3. the fixed commit: every hook passes, it lands on main and the branch graph grows, still green.

Motion rules it follows:

* staging: one focal action at a time; everything else idles quietly on slow cycles;
* anticipation before the big action, follow-through and overlap after it (hat tip, chips, title settle late);
* eased timing, never linear: ease-out to arrive, ease-in to leave, overshoot ("back") for pops, gravity arcs;
* impact = smear frame, hit-stop (held frames) and a 2-pixel decaying shake; the flash stays local and nothing
  flashes more than three times a second (WCAG 2.3.1);
* pixel-art integrity: one pixel size everywhere (the title is Scale2x'd, never block-doubled), integer
  positions, nearest-neighbour scaling, hand-drawn squash/tumble/shrink frames instead of rotation or resampling,
  ordered dithering for light and fades, one global palette (no per-frame palette flicker);
* every idle cycle divides the loop, so the loop has no seam; holds come from merging identical frames;
* the still (logo.png, the README's prefers-reduced-motion fallback) is a full composition, and so is the GIF's
  first frame.

Requires Pillow and numpy. The GIF encoder below writes per-frame dirty rectangles with transparent unchanged
pixels. Everything is drawn from the sprite maps and procedures in this file: no external images.
"""

from __future__ import annotations

import argparse
import math
import random
import struct
from pathlib import Path

import numpy as np
from PIL import Image

W, H, SCALE = 320, 146, 4
TICK_MS = 40       # one animation tick (25 fps); holds are merged into longer GIF frames
T = 360            # ticks per loop (14.4 s); every idle period below divides it
GROUND = 96        # top of the bridge deck
STILL_TICK = 206   # the frame used for logo.png
OUT = Path(__file__).resolve().parent.parent


def hexc(s: str) -> tuple[int, int, int]:
    s = s.lstrip("#")
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16))


# ---------------------------------------------------------------------------------------------- palette

P = {k: hexc(v) for k, v in {
    "ink": "#0b0a18", "outline": "#1a1730", "white": "#ecebf7", "lav": "#aaa3d8", "dim": "#5d5a99",
    # night sky, moon, mountains
    "sky0": "#0b0a1c", "sky1": "#100e26", "sky2": "#16133a", "sky3": "#1e1a47", "sky4": "#272252",
    "star0": "#45427c", "star1": "#8582c8", "star2": "#d8d6ff", "star3": "#ffffff",
    "moon": "#f3ead0", "moon_sh": "#d9cca4", "moon_cr": "#c4b68c", "halo0": "#1f1c48", "halo1": "#29255c",
    "mtn_far": "#1b1838", "mtn_far_hi": "#2a2652", "mtn_near": "#14112b", "mtn_near_hi": "#221d45",
    "mist": "#2a2657",
    "abyss0": "#1a1736", "abyss1": "#121029", "abyss2": "#0c0b1d", "abyss3": "#090814", "abyss4": "#150a16",
    "ember": "#ff9a3a", "ember_hi": "#ffd27a", "ember_lo": "#c8402a",
    # rock, grass, bridge stone
    "rock0": "#1a1430", "rock1": "#261d3d", "rock2": "#33284f", "rock3": "#45376a",
    "grass0": "#1f4a37", "grass1": "#2f6e46", "grass2": "#4f9a55", "grass3": "#7cc46a",
    "stone0": "#1b1730", "stone1": "#2c2647", "stone2": "#3d355f", "stone3": "#524a7a", "stone4": "#6f679a",
    "stone5": "#8f88b8",
    # light and status colours
    "gold0": "#fffbe6", "gold1": "#ffe58a", "gold2": "#ffd060", "gold3": "#e09a2a", "gold_ink": "#3a2a00",
    "warm": "#ffd98a",
    "green0": "#b8ffc8", "green1": "#5cf07a", "green2": "#2fbf5f", "green3": "#1d7a45", "green4": "#0f3f2a",
    "red0": "#ffd0c8", "red1": "#ff5a5a", "red2": "#d63a3a", "red3": "#8a1f2a", "red4": "#3f0f1a",
    "yel0": "#fff2a8", "yel1": "#ffd34e", "yel2": "#c99a1e", "yel3": "#4a3510",
    "cyan0": "#e0fdff", "cyan1": "#8ff0ff", "cyan2": "#3fc8e8", "cyan3": "#1f7fa8",
    # wood
    "wood0": "#a0703f", "wood1": "#7b5434", "wood2": "#5e3e26",
    # terminal and code
    "term_bar": "#3a3670", "term_edge": "#2d2a55", "term_txt": "#e8e6ff",
    "card": "#252249", "card_hd": "#34306b", "card_sep": "#1d1a3c", "card_back": "#1d1a3c",
    "kw": "#c678dd", "fn": "#61afef", "str": "#98c379", "num": "#e5c07b", "var": "#e06c75", "pu": "#abb2bf",
    "ws": "#b0344a",
}.items()}


# ---------------------------------------------------------------------------------------------- easing

def clamp01(v: float) -> float:
    return 0.0 if v < 0 else 1.0 if v > 1 else v


def prog(t: float, a: float, b: float) -> float:
    """Progress of t through [a, b], clamped to 0..1."""
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
    """Small deterministic hash (stable across runs, unlike hash())."""
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
    """Bresenham line, endpoints included."""
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


def twinkle(cv: Canvas, x: int, y: int, k: int, core, arm) -> None:
    """A four-point star of arm length k (0..2)."""
    cv.put(x, y, core)
    for s in range(1, k + 1):
        for ddx, ddy in ((s, 0), (-s, 0), (0, s), (0, -s)):
            cv.put(x + ddx, y + ddy, arm if s == k else core)


# ---------------------------------------------------------------------------------------------- fonts

FONT5 = {  # 5x7 caps (variable width)
    "A": [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "B": ["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."],
    "C": [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
    "D": ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."],
    "E": ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
    "F": ["#####", "#....", "#....", "####.", "#....", "#....", "#...."],
    "G": [".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".####"],
    "H": ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "I": [".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
    "K": ["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"],
    "L": ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
    "M": ["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"],
    "N": ["#...#", "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#"],
    "O": [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "P": ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."],
    "R": ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"],
    "S": [".####", "#....", "#....", ".###.", "....#", "....#", "####."],
    "T": ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
    "U": ["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "V": ["#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."],
    "W": ["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "#.#.#", ".#.#."],
    "X": ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"],
    "Y": ["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."],
    "?": [".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#.."],
    "&": [".##..", "#..#.", "#.#..", ".#...", "#.#.#", "#..#.", ".##.#"],
    ".": [".", ".", ".", ".", ".", ".", "#"],
    ":": [".", ".", "#", ".", ".", "#", "."],
    "*": [".....", ".....", "..#..", ".###.", "..#..", ".....", "....."],
    " ": ["...", "...", "...", "...", "...", "...", "..."],
}

FONT3 = {  # 3x5 caps, monospaced (terminal, chips, labels)
    "A": [".#.", "#.#", "###", "#.#", "#.#"], "B": ["##.", "#.#", "##.", "#.#", "##."],
    "C": [".##", "#..", "#..", "#..", ".##"], "D": ["##.", "#.#", "#.#", "#.#", "##."],
    "E": ["###", "#..", "##.", "#..", "###"], "F": ["###", "#..", "##.", "#..", "#.."],
    "G": [".##", "#..", "#.#", "#.#", ".##"], "H": ["#.#", "#.#", "###", "#.#", "#.#"],
    "I": ["###", ".#.", ".#.", ".#.", "###"], "K": ["#.#", "#.#", "##.", "#.#", "#.#"],
    "L": ["#..", "#..", "#..", "#..", "###"], "M": ["#.#", "###", "###", "#.#", "#.#"],
    "N": ["##.", "#.#", "#.#", "#.#", "#.#"], "O": [".#.", "#.#", "#.#", "#.#", ".#."],
    "P": ["##.", "#.#", "##.", "#..", "#.."], "R": ["##.", "#.#", "##.", "#.#", "#.#"],
    "S": [".##", "#..", ".#.", "..#", "##."], "T": ["###", ".#.", ".#.", ".#.", ".#."],
    "U": ["#.#", "#.#", "#.#", "#.#", "###"], "X": ["#.#", "#.#", ".#.", "#.#", "#.#"],
    "0": ["###", "#.#", "#.#", "#.#", "###"], "1": [".#.", "##.", ".#.", ".#.", "###"],
    "2": ["##.", "..#", ".#.", "#..", "###"], "3": ["##.", "..#", ".#.", "..#", "##."],
    "4": ["#.#", "#.#", "###", "..#", "..#"], "5": ["###", "#..", "##.", "..#", "##."],
    "6": [".##", "#..", "###", "#.#", "###"], "7": ["###", "..#", ".#.", ".#.", ".#."],
    "8": ["###", "#.#", "###", "#.#", "###"], "9": ["###", "#.#", "###", "..#", "##."],
    ">": ["#..", ".#.", "..#", ".#.", "#.."], "_": ["...", "...", "...", "...", "###"],
    ".": ["...", "...", "...", "...", ".#."], "-": ["...", "...", "###", "...", "..."],
    "[": ["##.", "#..", "#..", "#..", "##."], "]": [".##", "..#", "..#", "..#", ".##"],
    " ": ["...", "...", "...", "...", "..."],
}


def text3(cv: Canvas, x: int, y: int, s: str, color, a: float = 1.0, clip: tuple[int, int] | None = None) -> int:
    for ch in s:
        for gy, row in enumerate(FONT3[ch]):
            yy = y + gy
            if clip and not clip[0] <= yy <= clip[1]:
                continue
            for gx, bit in enumerate(row):
                if bit == "#":
                    cv.put(x + gx, yy, color, a)
        x += 4
    return x


def width5(s: str) -> int:
    return sum(len(FONT5[ch][0]) + 1 for ch in s) - 1


def text5(cv: Canvas, x: int, y: int, s: str, color, a: float = 1.0) -> int:
    for ch in s:
        g = FONT5[ch]
        for gy, row in enumerate(g):
            for gx, bit in enumerate(row):
                if bit == "#":
                    cv.put(x + gx, y + gy, color, a)
        x += len(g[0]) + 1
    return x


def scale2x(rows: list[str]) -> list[str]:
    """Scale2x (EPX): doubles a 1-bit glyph and rounds its diagonals, so the title keeps one pixel size."""
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


# ---------------------------------------------------------------------------------------------- sprites

WIZ = [  # 17x40, faces left; the arm, hand and staff are drawn per pose
    "..........OO.....",
    ".........OhO.....",
    "........OhhO.....",
    "........OhdO.....",
    ".......OHhdO.....",
    ".......OHhhdO....",
    "......OHhhhdO....",
    "......OHhhhhdO...",
    ".....OHhhhhhdO...",
    ".....OHhhhhhhdO..",
    "....OHhhhhhhhdO..",
    "..OOObbbbbbbbbOOO",
    ".OHHHHHhhhhhhhhdO",
    "..OOOOOOOOOOOOOO.",
    "....OwwsWWWWWO...",
    "....OsesWWWWWWO..",
    "...OsssSWWWWWWO..",
    "....OSwwWWWWWWO..",
    "....OwwwwwwWWWO..",
    "...OrwwwwwwWrrO..",
    "..OrrwwwwwwWrrRO.",
    "..OrrrwwwwwWrrRO.",
    "..OrrrwwwwWWrrRO.",
    "..OrrrrwwwWrrrRO.",
    "..OBBBkwwWBBBBBO.",
    "..OrrrrrwWrrrrRO.",
    "..OLrrrrrwrrrrRO.",
    "..OLrrrrrrrrrrRO.",
    "..OLrrrrrrrrrRRO.",
    "..OLrrrrrrrrRrRO.",
    "..OLrrrrrrrrRrRO.",
    ".OLLrrrrrrrrRrRRO",
    ".OLLrrrrrrrrRrRRO",
    ".OLLrrrrrrrrRrRRO",
    ".OLLrrrrrrrrrRRRO",
    ".OLLrrrrrrrrrRRRO",
    "OLLLrrrrrrrrrRRRO",
    "OLLrrrrrrrrrrRRRO",
    "OOOOOOOOOOOOOOOOO",
    "..OffO.....OffO..",
]
assert all(len(r) == 17 for r in WIZ)
WIZ_C = {
    "O": P["outline"], "h": hexc("#7d7c99"), "H": hexc("#a3a2bd"), "d": hexc("#5c5b78"), "b": hexc("#6b5a8a"),
    "s": hexc("#f2c9a0"), "S": hexc("#cf9f78"), "e": P["outline"], "w": hexc("#f4f2ff"), "W": hexc("#c4c2db"),
    "r": hexc("#b9b8cf"), "R": hexc("#8c8ba6"), "L": hexc("#dcdbea"), "B": hexc("#6b5a8a"), "k": P["gold2"],
    "f": hexc("#3a2f3f"),
}
SLEEVE, SLEEVE_HI, SLEEVE_LO = hexc("#b9b8cf"), hexc("#dcdbea"), hexc("#6e6d8c")

SERVER = [  # 20x26 rack; 'k' is the commit bay, lit when a commit arrives; 1-9, A-C are activity LEDs
    "OOOOOOOOOOOOOOOOOOOO",
    "OHHHHHHHHHHHHHHHHHdO",
    "OcccccccccccccccccdO",
    "OcuuuuuuuuuuuuuuucdO",
    "Ocu1u2u3vvvvvvvvucdO",
    "OcuuuuuuuuuuuuuuucdO",
    "OcccccccccccccccccdO",
    "OcuuuuuuuuuuuuuuucdO",
    "Ocu4u5u6vvvvvvvvucdO",
    "OcuuuuuuuuuuuuuuucdO",
    "OcccccccccccccccccdO",
    "OcuuuuuuuuuuuuuuucdO",
    "Ocu7u8u9vvvvvvvvucdO",
    "OcuuuuuuuuuuuuuuucdO",
    "OcccccccccccccccccdO",
    "OcbbbbbbbbbbbbbbbcdO",
    "OcbkkkkkkkkkkkkkbcdO",
    "OcbbbbbbbbbbbbbbbcdO",
    "OcccccccccccccccccdO",
    "OcuuuuuuuuuuuuuuucdO",
    "OcuAuBuCvvvvvvvvucdO",
    "OcuuuuuuuuuuuuuuucdO",
    "OcccccccccccccccccdO",
    "OcccccccccccccccccdO",
    "OOOOOOOOOOOOOOOOOOOO",
    ".Oo..............oO.",
]
assert all(len(r) == 20 for r in SERVER)
SERVER_C = {"O": hexc("#0e0d1f"), "H": hexc("#4a4878"), "c": hexc("#2e2c50"), "d": hexc("#1f1d3a"),
            "u": hexc("#3b3966"), "v": hexc("#262447"), "b": hexc("#57548a"), "k": P["ink"], "o": hexc("#1a1830")}
LEDS = [(r, row.index(ch)) for ch in "123456789ABC" for r, row in enumerate(SERVER) if ch in row]

BUG = [  # two walk frames, 7x5
    [".K...K.", "..RRR..", "KRRDRRK", ".RRDRR.", "K.RRR.K"],
    ["K.....K", ".KRRRK.", ".RRDRR.", "KRRDRRK", "..RRR.."],
]
BUG_C = {"R": hexc("#ec5b2c"), "D": hexc("#7a1f10"), "K": hexc("#d8d2f2")}

CHECK = ["....#", "...##", "#.##.", "###..", ".#..."]
CROSS = ["#...#", ".#.#.", "..#..", ".#.#.", "#...#"]
SPARK = [["..#..", "..#..", "##.##", "..#..", "..#.."], ["#...#", ".#.#.", ".....", ".#.#.", "#...#"]]
RING = [(1, 0), (2, 0), (3, 0), (4, 1), (4, 2), (4, 3), (3, 4), (2, 4), (1, 4), (0, 3), (0, 2), (0, 1)]

CODE_A = [  # (indent, [(length, colour)]): the badly formatted commit
    (0, [(2, "kw"), (1, None), (6, "fn"), (1, "pu")]),
    (3, [(3, "kw"), (1, None), (2, "var"), (1, None), (5, "str")]),
    (3, [(6, "fn"), (1, "pu"), (2, "num"), (1, "pu")]),
    (0, [(1, "pu")]),
]
MESSY_DX = [2, -3, 4, 3]       # where each line of CODE_A starts before the formatter runs
TRAILING_WS = {0: 2, 2: 3}     # line -> trailing whitespace pixels (shown red, like an editor does)
CODE_B = [  # the commit with a lint error (line 1) and a failing test (the bug)
    (0, [(2, "kw"), (1, None), (4, "fn"), (1, "pu")]),
    (3, [(3, "kw"), (1, None), (3, "var"), (1, None), (4, "num")]),
    (3, [(2, "kw"), (1, None), (5, "fn"), (1, "pu")]),
    (0, [(1, "pu")]),
]
CW, CH = 22, 20                # commit card size
LINE_ROWS = [5, 9, 13, 17]

# ---------------------------------------------------------------------------------------------- layout

TX0, TY0, TX1, TY1 = 6, 12, 74, 58          # terminal window
TEXT_X, TEXT_Y, LH, ROWS = TX0 + 4, TY0 + 9, 7, 5
SPOT = (104, 70)                             # where a commit hovers while the hooks run
WX, WY = 146, GROUND - len(WIZ)              # wizard sprite origin
SX, SY = 278, GROUND - len(SERVER)           # server origin
BAY = (SX + 10, SY + 16)                     # commit bay centre
GRAPH_Y, GRAPH_X1, GRAPH_X0 = 40, 306, 236   # branch graph: newest node at GRAPH_X1
MOON = (206, 16, 7.5)
CHIP_X, CHIP_Y = SPOT[0] + CW // 2 - 14, 31
HOOKS = ["FMT", "LINT", "TEST"]

# ---------------------------------------------------------------------------------------------- timeline
# Ticks of 40 ms. Three commits: A (bad formatting, fixed), B (lint error + failing test, blocked), C (clean).

A = dict(spawn=35, out=39, hop=51, land=55, scan=(57, 69), pops=(59, 62, 65), fix=72, results=(92, 96, 100),
         leave=106, arrive=128, gone=134, kind="messy")
B = dict(spawn=159, out=163, hop=175, land=179, scan=(181, 193), pops=(183, 186, 189), results=(196, 200, 204),
         raise_=208, charge=213, slam=225, impact=226, knock=229, back=250, kind="bad")
C = dict(spawn=263, out=266, hop=276, land=280, scan=(281, 289), pops=(282, 284, 286), results=(290, 292, 294),
         leave=298, arrive=318, gone=324, kind="clean")
COMMITS = (A, B, C)
# 225 is the smear frame, 226 the impact, held through 228 (hit-stop); the shake decays over 229-233
SHAKE = {B["knock"] + i: d for i, d in enumerate([(2, 0), (-2, 1), (2, -1), (-1, 1), (1, 0)])}
HITSTOP = range(B["impact"] + 1, B["knock"])

POSES = {  # relative to the sprite origin: hand, staff base, staff top, body offset (+ = squash)
    "idle": dict(hand=(0, 23), base=(0, 39), top=(0, -4), body=0),
    "point": dict(hand=(-2, 20), base=(3, 39), top=(-8, -2), body=0),
    "raise": dict(hand=(0, 11), base=(0, 29), top=(0, -15), body=-1),
    "slam": dict(hand=(0, 23), base=(0, 41), top=(0, -3), body=2),
}
POSE_KEYS = [  # (tick, pose, easing into it)
    (0, "idle", ease_io), (55, "idle", ease_io), (59, "point", ease_io), (102, "point", ease_io),
    (106, "idle", ease_io), (179, "idle", ease_io), (183, "point", ease_io), (B["raise_"], "point", ease_io),
    (B["raise_"] + 5, "raise", ease_out), (B["slam"] - 1, "raise", ease_io), (B["impact"], "slam", ease_in),
    (B["impact"] + 10, "slam", ease_io), (B["impact"] + 19, "idle", ease_back), (279, "idle", ease_io),
    (282, "point", ease_io), (296, "point", ease_io), (300, "idle", ease_io), (T, "idle", ease_io),
]

CAPTIONS = [  # (from tick, [(text, colour)]); each leaves during the 4 ticks before the next one
    (0, [("SNP", "gold2"), (" * THE GIT HOOK GUARD", "lav")]),
    (A["fix"], [("BAD FORMATTING? ", "lav"), ("FIXED", "yel1"), (" ON COMMIT.", "lav")]),
    (B["results"][1], [("LINT ERRORS & FAILING TESTS: ", "lav"), ("BLOCKED.", "red1")]),
    (C["results"][0], [("CLEAN CODE PASSES. MAIN STAYS ", "lav"), ("GREEN.", "green1")]),
    (344, [("SNP", "gold2"), (" * THE GIT HOOK GUARD", "lav")]),
]


def pose_at(t: float) -> dict:
    for (t0, p0, _), (t1, p1, ease) in zip(POSE_KEYS, POSE_KEYS[1:]):
        if t0 <= t <= t1:
            v = ease(prog(t, t0, t1))
            a, b = POSES[p0], POSES[p1]
            return {k: (lerp(a[k][0], b[k][0], v), lerp(a[k][1], b[k][1], v)) if isinstance(a[k], tuple)
                    else lerp(a[k], b[k], v) for k in a}
    return dict(POSES["idle"])


def is_idle(t: int) -> bool:
    for (t0, p0, _), (t1, p1, _) in zip(POSE_KEYS, POSE_KEYS[1:]):
        if t0 <= t < t1:
            return p0 == "idle" and p1 == "idle"
    return True


def orb_of(pose: dict) -> tuple[float, float]:
    """The orb sits just past the staff top, along the staff."""
    (bx, by), (tx, ty) = pose["base"], pose["top"]
    ln = math.hypot(tx - bx, ty - by) or 1
    return WX + tx + (tx - bx) / ln * 4, WY + ty + (ty - by) / ln * 4


# ---------------------------------------------------------------------------------------------- background

def vgrad(a: np.ndarray, stops: list[tuple[int, str]], seam: int = 3) -> None:
    """Flat colour bands with ordered-dither seams (pixel-art gradient)."""
    for y in range(H):
        k = max(i for i, (sy, _) in enumerate(stops) if sy <= y) if y >= stops[0][0] else 0
        a[y] = P[stops[k][1]]
        for j in range(1, len(stops)):
            sy = stops[j][0]
            if sy - seam <= y < sy + seam:
                f = (y - (sy - seam) + 0.5) / (2 * seam)
                a[y] = P[stops[j - 1][1]]
                a[y][BAYER[y] < f] = P[stops[j][1]]


def value_noise(x: float, y: float, cell: float, seed: int) -> float:
    """Smooth hashed lattice noise in 0..1."""
    gx, gy = x / cell, y / cell
    x0, y0 = math.floor(gx), math.floor(gy)
    fx, fy = gx - x0, gy - y0
    fx, fy = fx * fx * (3 - 2 * fx), fy * fy * (3 - 2 * fy)
    v = [[(h32(x0 + i, y0 + j, seed) % 1000) / 1000 for i in (0, 1)] for j in (0, 1)]
    return lerp(lerp(v[0][0], v[0][1], fx), lerp(v[1][0], v[1][1], fx), fy)


def milky_way(a: np.ndarray) -> None:
    """A faint diagonal band of dust and dense small stars, rising from the left mountains to the top right."""
    cv = Canvas(a)
    (x0, y0), (x1, y1) = (20, 96), (330, -14)
    ln = math.hypot(x1 - x0, y1 - y0)
    nx, ny = -(y1 - y0) / ln, (x1 - x0) / ln
    rng = random.Random(23)
    for y in range(0, 92):
        for x in range(W):
            d = (x - x0) * nx + (y - y0) * ny
            band = math.exp(-(d / 15) ** 2)
            cloud = 0.65 * value_noise(x, y, 11, 1) + 0.35 * value_noise(x, y, 5, 2)
            v = band * (0.35 + 0.9 * cloud) - 0.18
            if v > 0.42 * bayer(x, y) + 0.08:
                cv.add(x, y, (24, 20, 58), 0.55 if v > 0.45 else 0.35)
            if band > 0.25 and rng.random() < band * cloud * 0.05:
                cv.put(x, y, P["star0"] if rng.random() < 0.7 else P["star1"])


STARS: list[tuple[int, int, int, int]] = []  # twinkling stars: (x, y, brightness, period)


def build_far() -> np.ndarray:
    a = np.zeros((H, W, 3), np.uint8)
    vgrad(a, [(0, "sky0"), (20, "sky1"), (42, "sky2"), (62, "sky3"), (80, "sky4"), (100, "abyss0"),
              (110, "abyss1"), (121, "abyss2"), (132, "abyss3"), (142, "abyss4")])
    milky_way(a)
    cv = Canvas(a)
    mx, my, mr = MOON
    for y in range(int(my - mr - 8), int(my + mr + 9)):  # halo
        for x in range(int(mx - mr - 8), int(mx + mr + 9)):
            d = math.hypot(x - mx, y - my)
            if mr < d <= mr + 8:
                f = 1 - (d - mr) / 8
                if f * 0.75 > bayer(x, y):
                    cv.put(x, y, P["halo1"] if f > 0.6 else P["halo0"])
    for y in range(int(my - mr - 1), int(my + mr + 2)):  # a crescent; the dark side keeps a faint earthshine
        for x in range(int(mx - mr - 1), int(mx + mr + 2)):
            d = math.hypot(x - mx, y - my)
            if d <= mr:
                shadow = math.hypot(x - mx - 3.2, y - my + 2.4) <= mr - 0.3
                if shadow:
                    cv.put(x, y, P["halo1"])
                else:
                    edge = math.hypot(x - mx - 3.2, y - my + 2.4) <= mr + 1.0
                    cv.put(x, y, P["moon_sh"] if edge else P["moon"])
    rng = random.Random(11)
    for i in range(110):  # stars; one in six twinkles (drawn per frame)
        x, y = rng.randrange(2, W - 2), rng.randrange(2, 84)
        if math.hypot(x - mx, y - my) < mr + 8:
            continue
        kind = rng.choices([0, 1, 2, 3], weights=[55, 30, 12, 3])[0]
        if i % 6 == 0 and kind < 3:
            STARS.append((x, y, kind, rng.choice([72, 90, 120, 180])))
            continue
        if kind == 3:
            twinkle(cv, x, y, 1, P["star3"], P["star1"])
        else:
            cv.put(x, y, P[f"star{kind}"])

    def ridge(x: int, base: float, amp: list[tuple[float, float, float]]) -> int:
        return rnd(base + sum(m * math.sin(x * f + ph) for m, f, ph in amp))

    far = [ridge(x, 74, [(7, 0.031, 1.0), (4, 0.083, 2.0), (2, 0.19, 0.4)]) for x in range(W + 1)]
    near = [ridge(x, 85, [(5, 0.047, 4.0), (3, 0.121, 1.0), (1.5, 0.27, 2.2)]) for x in range(W + 1)]
    for x in range(W):  # far ridge, moonlit edge
        a[far[x]:101, x] = P["mtn_far"]
        cv.put(x, far[x], P["mtn_far_hi"])
        if far[x + 1] > far[x]:
            cv.put(x, far[x] + 1, P["mtn_far_hi"])
    for y in range(80, 101):  # mist in the valley
        f = (y - 80) / 20 * 0.55
        for x in range(W):
            if f > bayer(x, y) and y >= far[x]:
                cv.put(x, y, P["mist"])
    for x in range(W):  # near ridge
        a[near[x]:101, x] = P["mtn_near"]
        cv.put(x, near[x], P["mtn_near_hi"])
        if near[x + 1] > near[x]:
            cv.put(x, near[x] + 1, P["mtn_near_hi"])
    return a


def build_near() -> tuple[np.ndarray, np.ndarray]:
    """Cliffs and the bridge, as a layer with a mask (embers rise behind it)."""
    a = np.zeros((H, W, 3), np.uint8)
    mask = np.zeros((H, W), bool)
    rng = random.Random(5)

    def put(x: int, y: int, c) -> None:
        if 0 <= x < W and 0 <= y < H:
            a[y, x] = c
            mask[y, x] = True

    def depth_ok(x: int, y: int, start: int, span: int) -> bool:
        return 1 - (y - start) / span > bayer(x, y)

    jl = jr = 0
    for y in range(GROUND, H):  # cliffs, dissolving into the dark
        jl = max(-2, min(2, jl + rng.choice((-1, 0, 0, 1))))
        jr = max(-2, min(2, jr + rng.choice((-1, 0, 0, 1))))
        el = rnd(44 - 0.5 * (y - GROUND)) + jl
        er = rnd(236 + 0.6 * (y - GROUND)) + jr
        for x in list(range(0, el)) + list(range(er, W)):
            if not depth_ok(x, y, GROUND + 10, 26):
                continue
            sy = y + rnd(2 * math.sin(x * 0.23) + 1.5 * math.sin(x * 0.071 + 1))  # tilted strata
            layer, pos = sy // 4, sy % 4
            c = P["rock1"] if layer % 2 else P["rock2"]
            if pos == 0 and h32(x // 2, layer) % 3:
                c = P["rock3"]  # lit lip of a ledge
            elif pos == 3 or (pos in (1, 2) and h32(x // 2, layer, 9) % 9 == 0):
                c = P["rock0"]  # shadow under a ledge, cracks
            if x in (el - 1, er):
                c = P["rock3"]  # the faces towards the chasm catch the light
            put(x, y, c)
    for x in list(range(0, 46)) + list(range(234, W)):  # grass on the cliff tops
        put(x, GROUND, P["grass1"])
        put(x, GROUND + 1, P["grass0"])
        hgt = h32(x, 3) % 4
        if hgt:
            put(x, GROUND - 1, P["grass2"])
        if hgt == 3:
            put(x, GROUND - 2, P["grass3"] if x % 2 else P["grass2"])
    for x in range(38, 244):  # bridge deck
        put(x, GROUND, P["stone5"] if h32(x, 9) % 7 == 0 else P["stone4"])
        put(x, GROUND + 1, P["stone3"])
        for y in range(GROUND + 2, GROUND + 5):
            put(x, y, P["stone2"])
        put(x, GROUND + 5, P["stone0"])
        if x % 9 == 0:
            for y in range(GROUND + 2, GROUND + 5):
                put(x, y, P["stone0"])
            put(x + 1, GROUND + 2, P["stone3"])
    for px0 in (92, 186):  # pillars
        for y in range(GROUND + 6, H):
            for dx in range(8):
                x = px0 + dx
                if not depth_ok(x, y, GROUND + 8, 22):
                    continue
                c = P["stone3"] if dx == 0 else P["stone1"] if dx >= 6 else P["stone2"]
                put(x, y, P["stone0"] if (y - GROUND) % 6 == 0 else c)
    return a, mask


FAR = build_far()
NEAR, NEAR_MASK = build_near()


# ---------------------------------------------------------------------------------------------- title

def build_title() -> list[tuple[int, np.ndarray, np.ndarray]]:
    """Per-letter tiles (x, rgb, mask) of the Scale2x title with outline, drop shadow and gradient."""
    word = "YOU SHELL NOT PASS"
    glyphs = [scale2x(FONT5[ch]) for ch in word]
    total = sum(len(g[0]) + 2 for g in glyphs) - 2
    x = (W - total) // 2
    grad = [hexc(c) for c in ("#ffffff", "#fbfaff", "#f4f2ff", "#e6e3f8", "#d6d2f1", "#c7c2ea", "#b9b3e3",
                              "#aca5dc", "#a098d5", "#958dce", "#8b83c7", "#827ac0", "#7a72b9", "#726bb1")]
    tiles = []
    for ch, g in zip(word, glyphs):
        gw, gh = len(g[0]), len(g)
        if ch != " ":
            rgb = np.zeros((gh + 5, gw + 2, 3), np.uint8)
            msk = np.zeros((gh + 5, gw + 2), bool)
            on = {(gx, gy) for gy, row in enumerate(g) for gx, b in enumerate(row) if b == "#"}
            for gx, gy in on:  # drop shadow, two pixels deep
                for sy in (2, 3):
                    rgb[gy + 1 + sy, gx + 1] = hexc("#2b2658")
                    msk[gy + 1 + sy, gx + 1] = True
            for gx, gy in on:  # outline
                for ddx, ddy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    if (gx + ddx, gy + ddy) not in on:
                        rgb[gy + 1 + ddy, gx + 1 + ddx] = P["ink"]
                        msk[gy + 1 + ddy, gx + 1 + ddx] = True
            for gx, gy in on:
                rgb[gy + 1, gx + 1] = grad[gy]
                msk[gy + 1, gx + 1] = True
            tiles.append((x - 1, rgb, msk))
        x += gw + 2
    return tiles


TITLE_TILES = build_title()
TITLE_Y = 105
CAPTION_Y = 130


# ---------------------------------------------------------------------------------------------- scene parts

def draw_stars_embers(cv: Canvas, t: int) -> None:
    for x, y, kind, per in STARS:
        v = 0.5 + 0.5 * math.sin(2 * math.pi * (t / per + (x * 7 + y) % 13 / 13))
        lvl = min(3, max(0, kind + (1 if v > 0.8 else -1 if v < 0.25 else 0)))
        cv.put(x, y, P[f"star{lvl}"])
    for i in range(16):  # embers rising out of the abyss; their periods divide T
        per = (72, 90, 120, 180)[i % 4]
        ph = (h32(i, 1) % 1000) / 1000
        f = ((t / per) + ph) % 1.0
        x = 20 + (h32(i, 2) % 280) + rnd(2 * math.sin(2 * math.pi * (f * 2 + ph)))
        y = rnd(H + 2 - f * 52)
        if 1 - f > bayer(x, y) * 1.1:
            cv.put(x, y, P["ember_hi"] if f < 0.25 else P["ember"] if f < 0.6 else P["ember_lo"])


LANTERN = [".OOO.", "OOOOO", "OyYyO", "OYwYO", "OyYyO", "OOOOO", "..O.."]
LANTERNS = (42, 238)


def draw_lanterns(cv: Canvas, t: int) -> None:
    """Two lanterns mark the ends of the bridge; their flames flicker on a 6-tick step."""
    for i, lx in enumerate(LANTERNS):
        cv.rect(lx + 2, GROUND - 12, lx + 2, GROUND - 1, hexc("#14122a"))
        cv.put(lx + 1, GROUND - 1, hexc("#14122a"))
        cv.put(lx + 3, GROUND - 1, hexc("#14122a"))
        fl = h32(i, (t // 6) % (T // 6), 3) % 3
        glow(cv, lx + 2, GROUND - 16, 15 + fl, P["warm"], 0.30 + 0.04 * fl, steps=4)
        cv.sprite(lx, GROUND - 19, LANTERN, {"O": hexc("#14122a"), "y": P["gold3"], "Y": P["gold1"],
                                             "w": P["gold0"] if fl else P["gold1"]})


def draw_meteor(cv: Canvas, t: int) -> None:
    """A shooting star crosses the sky once, in the quiet hold at the end of the loop."""
    a = t - 347
    if not 0 <= a < 9:
        return
    hx, hy = 304 - 8 * a, 3 + 2.5 * a
    fade = min(1.0, (9 - a) / 3)
    for k in range(10):
        cv.put(rnd(hx + 1.6 * k), rnd(hy - 0.5 * k), P["star3"] if k < 2 else P["star2"] if k < 5 else P["star1"],
               fade * (1 - k / 10))


def draw_motes(cv: Canvas, t: int, ox: float, oy: float) -> None:
    """Specks of dust drifting up through the staff light (period 180 ticks, divides the loop)."""
    for i in range(9):
        ph = (h32(i, 11) % 1000) / 1000
        f = (t / 180 + ph) % 1.0
        x = rnd(ox - 14 + (h32(i, 12) % 28) + 3 * math.sin(2 * math.pi * (f * 2 + ph)))
        y = rnd(oy + 18 - f * 36)
        d = math.hypot(x - ox, y - oy)
        if d < 20:
            cv.put(x, y, P["gold1"] if d < 10 else P["gold3"], (1 - d / 20) * min(1.0, 4 * f, 4 * (1 - f)) * 1.4)


def arrival(t: int) -> tuple[float, int]:
    """(intensity 0..1, tick) of the latest commit landing on main, for the server's reaction."""
    for c in (A, C):
        g = c["gone"]
        if g - 4 <= t < g + 30:
            return 1 - prog(t, g, g + 30), g
    return 0.0, -999


def draw_server(cv: Canvas, t: int) -> None:
    k, g = arrival(t)
    glow(cv, SX + 10, SY + 12, 26 + 6 * k, P["green2"], 0.20 + 0.35 * k, steps=4)
    cv.sprite(SX, SY, SERVER, SERVER_C)
    for i, (row, col) in enumerate(LEDS):
        on = h32(i, (t // 6) % (T // 6)) % 3 != 0
        if g <= t < g + 12:  # cascade on arrival
            on = (t - g) >= i // 3 * 2
        cv.put(SX + col, SY + row, P["green1"] if on else P["green3"])
    if k > 0 and t >= g:  # the bay lights up and cools down
        c = P["green0"] if t - g < 4 else P["green1"] if t - g < 14 else P["green2"]
        for x in range(SX + 3, SX + 16):
            cv.put(x, BAY[1], c, 1.0 if t - g < 14 else k * 2)
    bx, by = SX + 9, SY - 3  # beacon
    cv.sprite(bx - 1, by, [".OO.", "OggO", "OOOO"], {"O": hexc("#0e0d1f"), "g": P["green1"]})
    pulse = (0, 1, 2, 1)[(t // 9) % 4]
    glow(cv, bx + 1, by + 1, 7 + pulse + 6 * k, P["green1"], 0.35 + 0.3 * k, steps=3)


def draw_graph(cv: Canvas, t: int) -> None:
    """`main` as a branch graph: each commit that lands adds a node and the history scrolls left."""
    shift, pops = 0.0, []
    for c in (A, C):
        t0 = c["gone"] + 8
        shift += 10 * ease_out(prog(t, t0, t0 + 6))
        pops.append(t0 + 6)
    shift %= 10
    text3(cv, GRAPH_X0 - 2, GRAPH_Y - 9, "MAIN", P["green1"])
    for x in range(GRAPH_X0, GRAPH_X1 + 1):
        cv.put(x, GRAPH_Y, P["green3"], prog(x, GRAPH_X0, GRAPH_X0 + 14))
    node = [".###.", "#####", "#####", "#####", ".###."]
    for j in range(9):
        cx = rnd(GRAPH_X1 + 10 - shift - 10 * j)
        if not GRAPH_X0 <= cx <= GRAPH_X1:
            continue
        a = prog(cx, GRAPH_X0, GRAPH_X0 + 18)
        newest = cx == GRAPH_X1
        cv.sprite(cx - 2, GRAPH_Y - 2, node, {"#": P["green2"] if newest else P["green3"]}, a)
        cv.put(cx, GRAPH_Y, P["green0"] if newest else P["green2"], a)
    for t0 in pops:  # the new node arrives with a ring
        if t0 <= t < t0 + 10:
            v = ease_out(prog(t, t0, t0 + 10))
            r = 3 + v * 6
            for ang in range(0, 360, 15):
                cv.put(rnd(GRAPH_X1 + r * math.cos(math.radians(ang))),
                       rnd(GRAPH_Y + r * math.sin(math.radians(ang))), P["green0"], 1 - v)
    glow(cv, GRAPH_X1, GRAPH_Y, 9, P["green1"], 0.35, steps=3)
    for c in (A, C):  # a spark carries the commit from the server up into the graph
        t0 = c["gone"]
        if t0 <= t < t0 + 8:
            v = ease_io(prog(t, t0, t0 + 8))
            x = rnd(lerp(SX + 10, GRAPH_X1, v))
            y = rnd(lerp(SY - 4, GRAPH_Y, v) - 10 * math.sin(math.pi * v))
            glow(cv, x, y, 6, P["green1"], 0.5, steps=3)
            cv.put(x, y, P["green0"])
            cv.put(x, y + 1, P["green1"])


# ------------------------------------------------------------------------------- terminal

def term_events() -> list[tuple[int, str, list[tuple[str, str]]]]:
    """(tick, kind, segments): 'line' appends a line, 'prompt' starts a prompt line, 'clear' is ^L."""
    def hook(name: str, ok: bool) -> list[tuple[str, str]]:
        return [(name, "term_txt"), ("." * (7 - len(name)), "dim"),
                ("PASSED" if ok else "FAILED", "green1" if ok else "red1")]
    ev = [(0, "prompt", [])]
    for i, n in enumerate(HOOKS):
        ev.append((A["results"][i], "line", hook(n, True)))
        if i == 0:
            ev.append((A["results"][0] + 2, "line", [("- AUTO-FIXED", "yel1")]))
    ev += [(A["gone"] + 6, "line", [("[MAIN 3F2A1C]", "lav")]), (A["gone"] + 12, "prompt", [])]
    ev += [(B["results"][i], "line", hook(n, i == 0)) for i, n in enumerate(HOOKS)]
    ev += [(B["back"] + 2, "prompt", [])]
    ev += [(C["results"][i], "line", hook(n, True)) for i, n in enumerate(HOOKS)]
    ev += [(C["gone"] + 6, "line", [("[MAIN 4D5E6F]", "lav")]), (C["gone"] + 12, "prompt", []),
           (342, "clear", [])]
    return sorted(ev, key=lambda e: e[0])


TERM_EVENTS = term_events()
COMMAND = "GIT COMMIT"
# per prompt: (tick the command appears, ticks per typed character; 0 = recalled from history with the up key)
TYPING = {0: (12, 2), 1: (A["gone"] + 18, 0), 2: (B["back"] + 6, 0)}


def draw_terminal(cv: Canvas, t: int) -> None:
    shake = (1, -1, 1, -1, 1, 0)[t - B["back"]] if B["back"] <= t < B["back"] + 6 else 0
    x0, x1 = TX0 + shake, TX1 + shake
    for y in range(TY0 + 2, TY1 + 3):  # drop shadow
        for x in range(x0 + 2, x1 + 3):
            if x > x1 or y > TY1:
                cv.put(x, y, P["ink"], 0.5)
    body = cv.a[TY0 + 6:TY1, x0 + 1:x1].astype(np.float32)  # smoked glass: the stars show through, faintly
    cv.a[TY0 + 6:TY1, x0 + 1:x1] = (body * 0.22 + np.array([6, 5, 16]) * 0.78).astype(np.uint8)
    for y0_, y1_, xa, xb in ((TY0, TY0, x0, x1), (TY1, TY1, x0, x1), (TY0, TY1, x0, x0), (TY0, TY1, x1, x1)):
        cv.rect(xa, y0_, xb, y1_, P["ink"])
    cv.rect(x0 + 1, TY0 + 1, x1 - 1, TY0 + 5, P["term_bar"])
    cv.rect(x0 + 1, TY0 + 1, x1 - 1, TY0 + 1, hexc("#4a4688"))
    for i, c in enumerate(("#ff5f56", "#ffbd2e", "#27c93f")):
        cv.rect(x0 + 3 + i * 4, TY0 + 3, x0 + 4 + i * 4, TY0 + 4, hexc(c))
    edge = P["red1"] if B["back"] <= t < B["back"] + 8 and (t - B["back"]) % 4 < 2 else P["term_edge"]
    cv.rect(x0 + 1, TY0 + 6, x0 + 1, TY1 - 1, edge)
    cv.rect(x1 - 1, TY0 + 6, x1 - 1, TY1 - 1, edge)
    cv.rect(x0 + 1, TY1 - 1, x1 - 1, TY1 - 1, edge)

    # the log: lines, a scroll offset eased per appended line, the active prompt with typing and cursor
    lines: list[list[tuple[str, str]]] = []
    prompt_idx, prompt_no, offset, target = -1, -1, 0.0, 0
    for te, kind, segs in TERM_EVENTS:
        if te > t:
            break
        if kind == "clear":
            new_target = prompt_idx * LH
            offset += (new_target - target) * ease_io(prog(t, te, te + 6))
            target = new_target
            continue
        lines.append(segs)
        if kind == "prompt":
            prompt_idx, prompt_no = len(lines) - 1, prompt_no + 1
        new_target = max(target, (len(lines) - ROWS) * LH)
        offset += (new_target - target) * ease_out(prog(t, te, te + 3))
        target = new_target
    if t >= 348:  # after ^L the screen is the bare prompt again, exactly as at tick 0
        lines, prompt_idx, offset, prompt_no = [[]], 0, 0.0, -1
    start, per = TYPING.get(prompt_no, (10 ** 6, 0))
    typed = ""
    if t >= start:
        typed = COMMAND if per == 0 else COMMAND[:min(len(COMMAND), (t - start) // per + 1)]
    typing = per > 0 and start <= t < start + len(COMMAND) * per
    clip = (TY0 + 7, TY1 - 2)
    for i, segs in enumerate(lines):
        y = rnd(TEXT_Y + i * LH - offset)
        if y > clip[1] or y + 4 < clip[0]:
            continue
        x = TEXT_X + shake
        if not segs and i != prompt_idx:  # an earlier prompt: its command ran
            text3(cv, text3(cv, x, y, "> ", P["green1"], clip=clip), y, COMMAND, P["term_txt"], clip=clip)
            continue
        if i == prompt_idx:
            x = text3(cv, x, y, "> ", P["green1"], clip=clip)
            x = text3(cv, x, y, typed, P["term_txt"], clip=clip)
            if i == len(lines) - 1 and ((t % 24) < 12 or typing):  # cursor until the hooks start printing
                for yy in range(max(y, clip[0]), min(y + 4, clip[1]) + 1):
                    cv.rect(x, yy, x + 2, yy, P["term_txt"])
            continue
        for s, col in segs:
            x = text3(cv, x, y, s, P[col], clip=clip)


# ------------------------------------------------------------------------------- commit card

def card_lines(kind: str, t: int) -> list[tuple[int, list, int]]:
    """(x offset, segments, trailing whitespace) per code line; the formatter animates kind 'messy'."""
    code = CODE_A if kind == "messy" else CODE_B
    out = []
    for i, (indent, segs) in enumerate(code):
        dx, ws = 0, 0
        if kind == "messy":
            t0 = A["fix"] + 2 + 3 * i
            dx = rnd(MESSY_DX[i] * (1 - ease_back(prog(t, t0, t0 + 7))))
            ws = TRAILING_WS.get(i, 0) if t < t0 + 2 else 0
        out.append((indent + dx, segs, ws))
    return out


def draw_card(cv: Canvas, x: int, y: int, kind: str, t: int, outline=None, w: int = CW, h: int = CH,
              back: bool = False) -> None:
    """A commit: a little code file. Narrower or shorter cards are the hand-drawn tumble and shrink frames."""
    ol = outline or P["outline"]
    if w < CW or h < CH:
        x += (CW - w) // 2
        y += (CH - h) // 2
        cv.rect(x, y, x + w - 1, y + h - 1, ol)
        if w <= 2 or h <= 2:
            return
        cv.rect(x + 1, y + 1, x + w - 2, y + h - 2, P["card_back"] if back else P["card"])
        if back:
            cv.put(x + 1, y + 1, P["card_hd"])
            return
        if h >= 8:
            cv.rect(x + 1, y + 1, x + w - 2, y + 2, P["card_hd"])
        for n, r in enumerate(range(4 if h >= 8 else 2, h - 2, 3 if h >= 8 else 2)):
            cv.rect(x + 2, y + r, x + max(2, w - 3 - (n % 2) * (w // 3)), y + r, P["fn"] if n % 2 else P["kw"])
        return
    cv.rect(x, y, x + w - 1, y + h - 1, ol)
    cv.rect(x + 1, y + 1, x + w - 2, y + h - 2, P["card"])
    cv.rect(x + 1, y + 1, x + w - 2, y + 2, P["card_hd"])
    cv.rect(x + 1, y + 3, x + w - 2, y + 3, P["card_sep"])
    for i, c in enumerate(("#ff5f56", "#ffbd2e", "#27c93f")):
        cv.put(x + 2 + i * 2, y + 1, hexc(c))
    for i, (dx, segs, ws) in enumerate(card_lines(kind, t)):
        lx0 = lx = x + 2 + dx
        ly = y + LINE_ROWS[i]
        for n, col in segs:
            if col:
                cv.rect(lx, ly, lx + n - 1, ly, P[col])
            lx += n
        for k in range(ws):
            cv.put(lx + 1 + k, ly, P["ws"])
        if kind == "bad" and i == 1:  # the lint error: a red squiggle under line 1
            for sx in range(lx0, lx):
                cv.put(sx, ly + 1 + (sx % 2), P["red1"])


def card_state(c: dict, t: int):
    """Where commit c is at tick t: (x, y, form, outline colour, layer) or None."""
    if t < c["spawn"]:
        return None
    sx, sy = SPOT
    outline = None
    if c["kind"] != "bad" and t >= c["results"][2]:
        outline = P["green1"]
    if c["kind"] == "bad" and t >= c["results"][1]:
        outline = P["red1"]
    bob = 0 if (t // 12) % 2 == 0 else -1
    if t < c["out"]:  # slides out from behind the terminal
        v = ease_out(prog(t, c["spawn"], c["out"]))
        return rnd(lerp(TX1 - CW - 2, TX1 + 3, v)), 34, ("full",), outline, "under_term"
    if t < c["hop"]:  # tossed onto the bridge: linear x, parabolic y (gravity)
        v = prog(t, c["out"], c["hop"])
        x = lerp(TX1 + 3, sx, v)
        y = lerp(34, sy, v) - 16 * 4 * v * (1 - v)
        return rnd(x), rnd(y), ("full",), outline, "front"
    if t < c["land"]:  # lands: squash, rebound, settle
        k = t - c["hop"]
        return sx, sy + (2, 1, -1, 0)[min(k, 3)], ("squash",) if k == 0 else ("full",), outline, "front"
    if c["kind"] == "bad":
        if t < c["knock"]:
            return sx, sy + (0 if t >= c["raise_"] else bob), ("full",), outline, "front"
        home = ((TX0 + TX1) // 2 - CW // 2, (TY0 + TY1) // 2 - CH // 2 + 2)
        if t < c["back"] - 6:  # thrown back to the terminal, tumbling end over end
            v = prog(t, c["knock"], c["back"] - 6)
            x = lerp(sx, home[0], ease_out(v, 2))
            y = lerp(sy, home[1], v) - 30 * 4 * v * (1 - v)
            k = (t - c["knock"]) // 2 % 8
            w = (CW, 16, 8, 2, 8, 16, CW, 16)[k]
            return rnd(x), rnd(y), ("tumble", w, k in (4, 5, 6)), outline, "front"
        if t < c["back"]:  # back into the working tree it came from
            w, h = ((16, 14), (11, 9), (7, 6), (4, 3), (2, 2), (1, 1))[t - (c["back"] - 6)]
            return home[0], home[1], ("shrink", w, h), outline, "front"
        return None
    if t < c["leave"]:
        return sx, sy + bob, ("full",), outline, "front"
    if t < c["arrive"]:  # crosses to main: an eased quadratic Bezier over the wizard's head
        v = ease_io(prog(t, c["leave"], c["arrive"]))
        p0, p1, p2 = (sx, sy), (186, -8), (BAY[0] - CW // 2, BAY[1] - CH // 2)
        x = (1 - v) ** 2 * p0[0] + 2 * v * (1 - v) * p1[0] + v * v * p2[0]
        y = (1 - v) ** 2 * p0[1] + 2 * v * (1 - v) * p1[1] + v * v * p2[1]
        return rnd(x), rnd(y), ("full",), outline, "under_wiz" if 120 < x < 180 else "front"
    if t < c["gone"]:  # slots into the server bay, shrinking through hand-drawn sizes
        w, h = ((16, 12), (11, 8), (7, 5), (4, 3), (2, 2), (1, 1))[min(t - c["arrive"], 5)]
        return BAY[0] - CW // 2, BAY[1] - CH // 2, ("shrink", w, h), outline, "front"
    return None


def draw_commit(cv: Canvas, c: dict, t: int, layer: str) -> None:
    st = card_state(c, t)
    if st is None or st[4] != layer:
        return
    x, y, form, outline, _ = st
    if form[0] == "full":
        if outline is not None:  # a second, darker ring: a crisp glow
            ring = P["green3"] if outline == P["green1"] else P["red3"]
            for xa, ya, xb, yb in ((x - 1, y - 1, x + CW, y - 1), (x - 1, y + CH, x + CW, y + CH),
                                   (x - 1, y, x - 1, y + CH - 1), (x + CW, y, x + CW, y + CH - 1)):
                cv.rect(xa, ya, xb, yb, ring)
        draw_card(cv, x, y, c["kind"], t, outline)
    elif form[0] == "squash":  # one frame: a pixel lower and a pixel wider on each side
        draw_card(cv, x, y + 1, c["kind"], t, outline)
        cv.rect(x - 1, y + 3, x - 1, y + CH - 1, outline or P["outline"])
        cv.rect(x + CW, y + 3, x + CW, y + CH - 1, outline or P["outline"])
    elif form[0] == "tumble":
        draw_card(cv, x, y, c["kind"], t, outline, w=form[1], back=form[2])
    else:
        draw_card(cv, x, y, c["kind"], t, outline, w=form[1], h=form[2])
    if c["kind"] == "bad":
        draw_bug(cv, c, t, x, y)


def draw_bug(cv: Canvas, c: dict, t: int, x: int, y: int) -> None:
    """The failing test, made visible: a bug climbs out during the scan and is zapped by the slam."""
    s0 = c["scan"][0] + 5
    if t < s0 or t > c["knock"]:
        return
    if t < c["results"][2]:  # peeks over the top edge
        n = rnd(4 * ease_out(prog(t, s0, s0 + 5)))
        if n:
            cv.sprite(x + 14, y - n, BUG[0][:n], BUG_C)
        return
    v = prog(t, c["results"][2], c["raise_"] + 2)
    bx, by = rnd(x + 14 - 9 * v), y - 5
    frame = (t // 2) % 2 if v < 1 else 0
    if c["raise_"] <= t < c["slam"]:
        bx += (t % 2) * 2 - 1  # trembles under the raised staff
    cv.sprite(bx, by, BUG[frame], {k: P["white"] for k in BUG_C} if t == c["knock"] else BUG_C)


# ------------------------------------------------------------------------------- hooks (chips)

def chip_state(c: dict, i: int, t: int) -> tuple[str, int] | None:
    """(state, ticks since it began) of hook chip i for commit c."""
    if t < c["pops"][i]:
        return None
    res = c["results"][i]
    if c["kind"] == "messy" and i == 0 and c["fix"] <= t < res:
        return "fix", t - c["fix"]
    if t >= res:
        return ("fail" if c["kind"] == "bad" and i > 0 else "pass"), t - res
    return "pending", t - c["pops"][i]


CHIP_COL = {  # fill, edge, text
    "pending": ("card_sep", "term_bar", "star1"), "fix": ("yel3", "yel2", "yel1"),
    "pass": ("green4", "green2", "green0"), "fail": ("red4", "red2", "red0"),
}


def draw_chips(cv: Canvas, c: dict, t: int) -> None:
    out0 = c["leave"] if "leave" in c else c["knock"] + 7
    if t >= out0 + 6:
        return
    fade = 1 - prog(t, out0, out0 + 6)
    for i, name in enumerate(HOOKS):
        st = chip_state(c, i, t)
        if st is None:
            continue
        state, age = st
        x0, y0 = CHIP_X, CHIP_Y + i * 11 - rnd(3 * (1 - fade))
        fill, edge, txt = (P[k] for k in CHIP_COL[state])
        since_pop = t - c["pops"][i]
        if since_pop < 3:  # pops in: grows from a sliver, overshoots, settles
            hh = (3, 7, 11)[since_pop]
            yc = y0 + 4
            cv.rect(x0 + 1, yc - hh // 2, x0 + 26, yc + hh // 2, edge)
            if hh > 3:
                cv.rect(x0 + 2, yc - hh // 2 + 1, x0 + 25, yc + hh // 2 - 1, fill)
            continue
        if state == "fail" and age < 8:
            x0 += (1, -1, 1, -1, 1, -1, 0, 0)[age]
        if state in ("pass", "fail") and age < 3:
            y0 -= (1, 2, 1)[age]
        flash = state in ("pass", "fail", "fix") and age == 0
        if flash:
            fill, edge, txt = P["white"], P["white"], P["outline"]
        cv.rect(x0 + 1, y0, x0 + 26, y0, edge, fade)
        cv.rect(x0 + 1, y0 + 8, x0 + 26, y0 + 8, edge, fade)
        cv.rect(x0, y0 + 1, x0, y0 + 7, edge, fade)
        cv.rect(x0 + 27, y0 + 1, x0 + 27, y0 + 7, edge, fade)
        cv.rect(x0 + 1, y0 + 1, x0 + 26, y0 + 7, fill, fade)
        ix, iy = x0 + 3, y0 + 2
        if state == "pending":  # a spinner
            for k, (rx, ry) in enumerate(RING):
                cv.put(ix + rx, iy + ry, P["cyan1"] if (k - t) % 12 < 3 else P["cyan3"], fade)
        elif state == "fix":
            cv.sprite(ix, iy, SPARK[(t // 3) % 2], {"#": P["yel0"]}, fade)
        else:
            col = txt if flash else P["green1"] if state == "pass" else P["red1"]
            cv.sprite(ix, iy, CHECK if state == "pass" else CROSS, {"#": col}, fade)
        text3(cv, x0 + 10, y0 + 2, name, txt, fade)


# ------------------------------------------------------------------------------- wizard

def draw_wizard(cv: Canvas, t: int, pose: dict, power: float) -> None:
    body = rnd(pose["body"])
    if is_idle(t):
        body = 1 if (t % 60) >= 30 else 0  # breathing
    lag = pose_at(t - 2)["body"] - pose["body"]
    sway = max(-2, min(2, rnd(lag * 1.2)))  # the hat tip follows through
    blink = (t % 180) in (100, 101, 102)
    split = 27  # rows above move with the body (head and torso), rows below stay planted
    for dy in range(split, len(WIZ)):
        for dx, ch in enumerate(WIZ[dy]):
            if ch in WIZ_C:
                cv.put(WX + dx, WY + dy, WIZ_C[ch])
    rows = list(range(split)) + [split - 1] * max(0, -body)
    for n, dy in enumerate(rows):
        yy = WY + dy + body if n < split else WY + split - 1 + body + (n - split + 1)
        xs = rnd(sway * (7 - dy) / 7) if dy < 7 else 0
        for dx, ch in enumerate(WIZ[dy]):
            if ch == "e" and blink:
                ch = "s"
            if ch in WIZ_C:
                cv.put(WX + dx + xs, yy, WIZ_C[ch])
    for dy, row in enumerate(WIZ):  # rim light: moon from behind (cool), staff from the front (warm)
        yy = WY + dy + (body if dy < split else 0)
        inner = [i for i, ch in enumerate(row) if ch not in ".O"]
        if not inner:
            continue
        if dy % 2 == 0 and row[inner[-1]] in "hrRdW":
            cv.put(WX + inner[-1], yy, hexc("#e6e4fa") if row[inner[-1]] in "rR" else hexc("#c4c3dc"))
        if row[inner[0]] in "hHrLws":
            cv.add(WX + inner[0], yy, P["warm"], 0.18 + 0.25 * power)
    bx, by = WX + rnd(pose["base"][0]), WY + rnd(pose["base"][1])
    tx, ty = WX + rnd(pose["top"][0]), WY + rnd(pose["top"][1])
    for i, (x, y) in enumerate(line_pts(bx, by, tx, ty)):  # staff, two pixels wide, with knots
        knot = i % 7 == 3
        cv.put(x, y, P["wood2"] if knot else P["wood0"])
        cv.put(x + 1, y, P["wood2"] if knot else P["wood1"])
    hx, hy = WX + rnd(pose["hand"][0]), WY + rnd(pose["hand"][1])
    pts = line_pts(WX + 6, WY + 20 + body, hx + 1, hy)  # sleeve, then the hand on the staff
    for x, y in pts:
        cv.rect(x - 1, y - 1, x + 1, y + 1, SLEEVE_LO)
    for x, y in pts:
        cv.put(x, y, SLEEVE)
        cv.put(x, y - 1, SLEEVE_HI)
    cv.rect(hx, hy, hx + 1, hy + 1, WIZ_C["s"])
    cv.put(hx + 1, hy + 1, WIZ_C["S"])


def orb_state(t: int) -> dict:
    """Light of the staff orb: colour, radius, power, core colours."""
    pulse = (0, 1, 2, 1)[(t // 9) % 4]
    st = dict(color=P["warm"], r=17 + pulse, power=0.45, core=P["gold0"], mid=P["gold1"])
    for c in (A, C):
        okt = c["results"][2]
        if okt <= t < okt + 12:  # approves: flashes green
            k = 1 - prog(t, okt, okt + 12)
            st.update(color=P["green1"], r=18 + 6 * k, power=0.45 + 0.3 * k, mid=P["green0"])
    ch, sl = B["charge"], B["slam"]
    if ch <= t < sl:  # charging
        k = ease_in(prog(t, ch, sl))
        st.update(r=18 + 12 * k, power=0.5 + 0.5 * k, core=P["white"])
    if sl <= t < sl + 16:
        k = 1 - prog(t, sl, sl + 16)
        st.update(r=18 + 16 * k, power=0.5 + 0.6 * k, core=P["white"])
    return st


def draw_orb(cv: Canvas, t: int, ox: int, oy: int, st: dict) -> None:
    glow(cv, ox, oy, st["r"], st["color"], st["power"], steps=6)
    for y in range(oy - 5, oy + 6):
        for x in range(ox - 5, ox + 6):
            d = math.hypot(x - ox, y - oy)
            if d <= 4.6:
                cv.put(x, y, st["core"] if d <= 3.2 else st["mid"])
    cv.put(ox - 4, oy + 3, P["wood1"])
    cv.put(ox + 4, oy + 3, P["wood1"])
    for yy, row in enumerate(FONT3[">"]):
        for xx, bit in enumerate(row):
            if bit == "#":
                cv.put(ox - 3 + xx, oy - 2 + yy, P["gold_ink"])
    if (t % 24) < 12:  # the prompt's cursor blinks in step with the terminal's
        cv.rect(ox + 1, oy + 2, ox + 3, oy + 2, P["gold_ink"])


# ------------------------------------------------------------------------------- magic

def arc_points(cx: float, cy: float, r: float, a0: float, a1: float) -> list[tuple[int, int]]:
    """Pixel arc from angle a0 to a1 (degrees, y down), one pixel per step, above the deck."""
    pts: list[tuple[int, int]] = []
    for a20 in range(int(a0 * 20), int(a1 * 20) + 1):
        ang = math.radians(a20 / 20)
        p = (rnd(cx + r * math.cos(ang)), rnd(cy + r * math.sin(ang)))
        if p[1] < GROUND and (not pts or p != pts[-1]):
            pts.append(p)
    return pts


SHIELD = (WX + 9, GROUND - 22, 25)
SHIELD_PTS = arc_points(*SHIELD, 105, 255)


def draw_shield(cv: Canvas, t: int) -> None:
    """A crescent of light in front of the wizard: gold at rest, green to let a commit pass, white-hot to block."""
    cx, cy, r = SHIELD
    tint, base, run = P["warm"], 0.24, P["gold1"]
    for c in (A, C):
        okt = c["results"][2]
        if okt <= t < c["gone"]:
            v = prog(t, okt, okt + 4) * (1 - prog(t, c["arrive"] - 4, c["gone"]))
            if v > 0:
                tint, base, run = P["green1"], 0.24 + 0.4 * v, P["green0"]
    sl = B["impact"]
    if sl <= t < sl + 18:
        flare = 1 - prog(t, sl + 4, sl + 18)
        color, thick = (P["white"], 3) if flare > 0.5 else (P["gold0"], 2)
        glow(cv, cx - r + 3, cy, 34, P["warm"], 0.7 * max(flare, 0.15), steps=5)
        for k in range(thick):
            for x, y in arc_points(cx, cy, r + k - thick // 2, 105, 255):
                cv.put(x, y, color, 1.0 if flare > 0.25 else 0.6)
        for k in range(3):  # ripples pushing out to the left
            t0 = sl + 1 + k * 3
            if t0 <= t < t0 + 12:
                v = ease_out(prog(t, t0, t0 + 12))
                rr = r + 4 + v * 26
                for a10 in range(1200, 2401, 25):
                    ang = math.radians(a10 / 10)
                    x, y = rnd(cx + rr * math.cos(ang)), rnd(cy + rr * 0.9 * math.sin(ang))
                    if y < GROUND:
                        cv.put(x, y, P["gold1"], 1 - v)
        return
    n = len(SHIELD_PTS)
    for x, y in SHIELD_PTS:  # a faint, unbroken line of light
        cv.add(x, y, tint, base)
    for k in range(2):  # two runes climbing the arc (period 45 ticks divides the loop)
        head = ((t / 45 + k / 2) % 1) * (n + 8)
        for j in range(6):
            i = int(head) - j
            if 0 <= i < n:
                cv.add(*SHIELD_PTS[i], run, 0.9 - j * 0.14)


def draw_beam(cv: Canvas, c: dict, t: int, ox: int, oy: int) -> None:
    s0, s1 = c["scan"]
    st = card_state(c, t)
    if not (s0 - 1 <= t < s1 + 2) or st is None:
        return
    x, y = st[0], st[1]
    v = ease_io(prog(t, s0, s1))
    sy = rnd(y + 1 + (CH - 3) * v)
    a = 0.30 * min(prog(t, s0 - 1, s0 + 1), 1 - prog(t, s1, s1 + 2))
    xa, xb = x - 1, x + CW
    for yy in range(min(oy, sy), max(oy, sy) + 1):  # a cone of light from the orb to the scanline
        f = (yy - oy) / (sy - oy) if sy != oy else 1
        lx, rx = lerp(ox, xa, f), lerp(ox, xb, f)
        for xx in range(rnd(min(lx, rx)), rnd(max(lx, rx)) + 1):
            if 0 <= xx < W and 0 <= yy < H and a > BAYER[yy, xx]:
                cv.add(xx, yy, P["cyan2"], 0.55)
    if t < s1:
        cv.rect(xa, sy, xb, sy, P["cyan1"])
        for k, al in ((1, 0.6), (2, 0.3)):
            for xx in range(xa, xb + 1):
                cv.put(xx, sy - k, P["cyan2"], al)


def particles(cv: Canvas, t: int) -> None:
    """Deterministic bursts: each is a function of (tick - emission tick), so every frame renders on its own."""
    ox_r, oy_r = orb_of(POSES["raise"])
    ch, sl, hit, imp = B["charge"], B["slam"], B["knock"], B["impact"]
    for i in range(14):  # charge: sparks drawn in toward the raised orb
        t0 = ch + i * 0.8
        if t0 <= t < min(t0 + 7, sl):
            v = ease_in(prog(t, t0, t0 + 7))
            ang = math.radians(h32(i, 4) % 360)
            rr = 24 * (1 - v)
            cv.put(rnd(ox_r + rr * math.cos(ang)), rnd(oy_r + rr * math.sin(ang)), P["gold0"] if i % 2 else P["white"])
    if t == sl:  # smear frame: a streak of light from the raised orb down to the deck
        for y in range(rnd(oy_r), GROUND):
            for x in range(WX - 2, WX + 4):
                if 0.6 * (1 - abs(x - WX - 0.5) / 3) > bayer(x, y):
                    cv.add(x, y, P["gold1"], 0.7)
    for i in range(18):  # sparks off the shield
        life = 10 + h32(i, 5) % 8
        a = t - hit
        if 0 <= a < life:
            ang = math.radians(120 + h32(i, 6) % 120)
            sp = 1.6 + (h32(i, 7) % 100) / 45
            dist = sp * (1 - 0.86 ** a) / (1 - 0.86)
            f = a / life
            col = P["white"] if f < 0.2 else P["gold1"] if f < 0.45 else P["gold3"] if f < 0.7 else P["red2"]
            cv.put(rnd(WX - 12 + dist * math.cos(ang)), rnd(GROUND - 22 + dist * math.sin(ang) + 0.09 * a * a), col)
    a = t - imp
    if 0 <= a < 12:  # dust kicked up at the staff's foot
        v = ease_out(prog(a, 0, 12))
        for i, d in enumerate((-1, 1, -1, 1)):
            px = rnd(WX + 1 + d * (3 + 10 * v + i * 3))
            rad = 1 + v * 2.5
            for yy in range(rnd(GROUND - 1 - rad), GROUND):
                for xx in range(rnd(px - rad), rnd(px + rad) + 1):
                    if math.hypot(xx - px, yy - GROUND + 1) <= rad:
                        cv.put(xx, yy, P["stone5"], 0.7 * (1 - v))
    if 0 <= a < 60:  # cracks in the deck: white-hot, then embers, then stone, then gone
        for cx_, cy_ in ((-1, 0), (-2, 1), (-3, 1), (-4, 2), (-5, 2), (-6, 3), (2, 0), (3, 1), (4, 1), (5, 2),
                         (6, 2), (7, 3), (-3, 3), (-4, 4), (4, 3), (5, 4)):
            col = P["gold1"] if a < 5 else P["ember"] if a < 14 else P["stone0"]
            cv.put(WX + 1 + cx_, GROUND + cy_, col, 1 - prog(a, 40, 60))
    if 0 <= a < 14:  # shockwave along the deck
        v = ease_out(prog(a, 0, 14))
        rx = 4 + 70 * v
        for xx in range(rnd(WX + 1 - rx), rnd(WX + 1 + rx) + 1):
            if rx - 3 <= abs(xx - WX - 1) <= rx:
                cv.put(xx, GROUND, P["gold0"], 1 - v)
                cv.put(xx, GROUND - 1, P["gold1"], 0.6 * (1 - v))
    a = t - (hit + 1)
    if 0 <= a < 12:  # the bug bursts into pixels
        bug_x, bug_y = SPOT[0] + 8, SPOT[1] - 3
        for i in range(12):
            ang = math.radians(i * 30 + 15)
            dist = 7 * ease_out(prog(a, 0, 12))
            cv.put(rnd(bug_x + dist * math.cos(ang)), rnd(bug_y + dist * math.sin(ang) + 0.05 * a * a),
                   P["ember_hi"] if i % 3 else P["red1"], 1 - prog(a, 4, 12))
    fx = A["fix"]
    ox_p, oy_p = orb_of(POSES["point"])
    for i in range(8):  # the formatter's magic: sparks stream from the orb onto the card
        t0 = fx + i * 2
        if t0 <= t < t0 + 8:
            v = ease_io(prog(t, t0, t0 + 8))
            tx, ty = SPOT[0] + 4 + (i * 5) % 14, SPOT[1] + LINE_ROWS[i % 4]
            x = (1 - v) ** 2 * ox_p + 2 * v * (1 - v) * (ox_p - 18) + v * v * tx
            y = (1 - v) ** 2 * oy_p + 2 * v * (1 - v) * (oy_p - 14) + v * v * ty
            cv.put(rnd(x), rnd(y), P["yel0"])
            cv.put(rnd(x) + 1, rnd(y), P["yel1"], 0.5)
    for i in range(4):  # a twinkle where each line snaps into place
        t0 = fx + 2 + 3 * i + 5
        if t0 <= t < t0 + 6:
            twinkle(cv, SPOT[0] + CW - 3, SPOT[1] + LINE_ROWS[i], (0, 1, 2, 2, 1, 0)[t - t0], P["yel0"], P["yel1"])
    for c in (A, C):  # twinkles around the server when a commit lands
        for i, (dx, dy) in enumerate(((-6, 4), (23, 0), (-3, 18), (25, 14), (10, -8))):
            t0 = c["gone"] + i * 2
            if t0 <= t < t0 + 6:
                twinkle(cv, SX + dx, SY + dy, (0, 1, 2, 2, 1, 0)[t - t0], P["green0"], P["green1"])


def draw_title(cv: Canvas, t: int) -> None:
    sl = B["impact"]
    for x, rgb, msk in TITLE_TILES:
        cx = x + rgb.shape[1] / 2
        a = t - sl - 1 - abs(cx - (WX + 1)) / 14  # the shockwave reaches each letter in turn
        dy = -(1, 2, 2, 1, 0)[int(a)] if 0 <= a < 5 else 0
        y = TITLE_Y + dy
        h, w = msk.shape
        region = cv.a[y:y + h, x:x + w]
        region[msk] = rgb[msk]
        if 0 <= a < 3:
            glow(cv, cx, TITLE_Y + 7, 9, P["warm"], 0.25, steps=3)


def draw_caption(cv: Canvas, t: int) -> None:
    for j, (t0, segs) in enumerate(CAPTIONS):
        t1 = CAPTIONS[j + 1][0] if j + 1 < len(CAPTIONS) else T + 99
        if not t0 <= t < t1:
            continue
        a, dy = 1.0, 0
        if t >= t1 - 4:  # leaving: slides down and dissolves
            v = prog(t, t1 - 4, t1)
            a, dy = 1 - v, rnd(2 * v)
        elif t0 > 0 and t < t0 + 6:  # entering: rises with overshoot, resolving out of the dither
            v = prog(t, t0, t0 + 6)
            a, dy = min(1.0, v * 1.6), rnd(4 * (1 - ease_back(v)))
        text = "".join(s for s, _ in segs)
        x = (W - width5(text)) // 2
        for s, col in segs:
            x = text5(cv, x, CAPTION_Y + dy, s, P[col], a)


# ---------------------------------------------------------------------------------------------- frame

def render(t: int) -> np.ndarray:
    t %= T
    if t in HITSTOP:  # hit-stop: hold the impact frame
        t = B["impact"]
    a = FAR.copy()
    cv = Canvas(a)
    draw_stars_embers(cv, t)
    draw_meteor(cv, t)
    a[NEAR_MASK] = NEAR[NEAR_MASK]
    draw_lanterns(cv, t)
    draw_graph(cv, t)
    draw_server(cv, t)
    for c in COMMITS:
        draw_commit(cv, c, t, "under_term")
    draw_terminal(cv, t)
    draw_shield(cv, t)
    pose = pose_at(t)
    ost = orb_state(t)
    for c in COMMITS:
        draw_commit(cv, c, t, "under_wiz")
    draw_wizard(cv, t, pose, ost["power"])
    ox, oy = orb_of(pose)
    draw_motes(cv, t, ox, oy)
    draw_orb(cv, t, rnd(ox), rnd(oy), ost)
    for c in COMMITS:
        draw_commit(cv, c, t, "front")
        draw_beam(cv, c, t, rnd(ox), rnd(oy))
        draw_chips(cv, c, t)
    particles(cv, t)
    draw_title(cv, t)
    draw_caption(cv, t)
    if t in SHAKE:
        dx, dy = SHAKE[t]
        a = np.roll(a, (dy, dx), axis=(0, 1))
        if dy > 0:
            a[:dy] = a[dy]
        elif dy < 0:
            a[dy:] = a[dy - 1]
        if dx > 0:
            a[:, :dx] = a[:, dx:dx + 1]
        elif dx < 0:
            a[:, dx:] = a[:, dx - 1:dx]
    return a


# ---------------------------------------------------------------------------------------------- GIF writer

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
        flags = 1 << 2  # disposal: leave in place
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
    """One global palette for every frame (no per-frame palette flicker).

    Exact when the art uses at most 255 colours. Otherwise every hand-picked colour (each hex literal in this
    file) keeps its slot, and the remaining slots are fitted to the light mixes by weighted k-means.
    """
    codes = [(f[..., 0].astype(np.int32) << 16) | (f[..., 1].astype(np.int32) << 8) | f[..., 2] for f in frames]
    uniq, counts = np.unique(np.concatenate([c.ravel() for c in codes]), return_counts=True)
    rgb = np.stack([(uniq >> 16) & 255, (uniq >> 8) & 255, uniq & 255], axis=1).astype(float)
    if len(uniq) <= 255:
        palette, lut = rgb, np.arange(len(uniq))
    else:
        import re
        lits = {int(h, 16) for h in re.findall(r"#([0-9a-fA-F]{6})\b", Path(__file__).read_text())}
        fixed = rgb[np.isin(uniq, list(lits))]
        free_n = 255 - len(fixed)

        def dist2(a: np.ndarray, b: np.ndarray) -> np.ndarray:
            return ((a[:, None, :] - b[None, :, :]) ** 2 * np.array([0.30, 0.59, 0.11])).sum(-1)

        centers = fixed.copy()
        for _ in range(free_n):  # deterministic k-means++ seeding: most pixels farthest from any centre
            centers = np.vstack([centers, rgb[np.argmax(counts * dist2(rgb, centers).min(1))]])
        for _ in range(20):  # Lloyd iterations; hand-picked colours stay put
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
            delays[-1] += TICK_MS // 10
            continue
        frames.append(f)
        delays.append(TICK_MS // 10)
        ticks.append(t)
    write_gif(OUT / "logo.gif", frames, delays, palette)
    still = Image.fromarray(np.repeat(np.repeat(idx[STILL_TICK], SCALE, axis=0), SCALE, axis=1), mode="P")
    still.putpalette(palette.tobytes())
    still.save(OUT / "logo.png", optimize=True)
    if args.frames:
        args.frames.mkdir(parents=True, exist_ok=True)
        for t, f in zip(ticks, frames):
            Image.fromarray(palette[f]).resize((W * SCALE, H * SCALE), Image.NEAREST).save(
                args.frames / f"t{t:03d}.png")
    size = (OUT / "logo.gif").stat().st_size
    print(f"wrote {OUT / 'logo.gif'} ({len(frames)} frames, {len(palette)} colours, {size / 1e6:.2f} MB) "
          f"and {OUT / 'logo.png'}")


if __name__ == "__main__":
    main()
