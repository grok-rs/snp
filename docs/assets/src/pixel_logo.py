#!/usr/bin/env python3
"""Render the snp README banner as pixel art.

Writes docs/assets/logo.png (static) and docs/assets/logo.gif (animated): a 240x130 canvas scaled 4x with
nearest-neighbour, so every art pixel is a crisp 4x4 block. Everything is drawn from the sprite maps and
procedures below: no external images.

Scene (minimal): the wizard holds the line with a glowing ">_" staff. From the left, alerts, failing lints,
bugs and broken commits fly at him and shatter on his light; behind him a stable server hums along, green.

    python3 docs/assets/src/pixel_logo.py
"""

from __future__ import annotations

import math
import random
from pathlib import Path

from PIL import Image

W, H, SCALE = 232, 104, 4
FRAMES = 8  # animation loop
OUT = Path(__file__).resolve().parent.parent


def hexc(s: str) -> tuple[int, int, int]:
    s = s.lstrip("#")
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16))


BAYER = ((0, 8, 2, 10), (12, 4, 14, 6), (3, 11, 1, 9), (15, 7, 13, 5))


def bayer(x: int, y: int) -> float:
    return (BAYER[y & 3][x & 3] + 0.5) / 16.0


# ---------------------------------------------------------------------------------------------- canvas


class Canvas:
    def __init__(self) -> None:
        self.px = [[(0, 0, 0)] * W for _ in range(H)]

    def put(self, x: int, y: int, c) -> None:
        if 0 <= x < W and 0 <= y < H:
            self.px[y][x] = c

    def get(self, x: int, y: int):
        return self.px[y][x]

    def add(self, x: int, y: int, c, a: float) -> None:
        if 0 <= x < W and 0 <= y < H:
            r, g, b = self.px[y][x]
            self.px[y][x] = (min(255, int(r + c[0] * a)), min(255, int(g + c[1] * a)), min(255, int(b + c[2] * a)))

    def mix(self, x: int, y: int, c, a: float) -> None:
        if 0 <= x < W and 0 <= y < H:
            r, g, b = self.px[y][x]
            self.px[y][x] = (int(r + (c[0] - r) * a), int(g + (c[1] - g) * a), int(b + (c[2] - b) * a))

    def rect(self, x0: int, y0: int, x1: int, y1: int, c) -> None:
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                self.put(x, y, c)

    def sprite(self, x0: int, y0: int, rows: list[str], cmap: dict) -> None:
        for dy, row in enumerate(rows):
            for dx, ch in enumerate(row):
                if ch != "." and ch in cmap:
                    self.put(x0 + dx, y0 + dy, cmap[ch])

    def image(self) -> Image.Image:
        img = Image.new("RGB", (W, H))
        img.putdata([p for row in self.px for p in row])
        return img.resize((W * SCALE, H * SCALE), Image.NEAREST)


def glow(c: Canvas, cx: float, cy: float, radius: float, color, strength: float, steps: int = 4) -> None:
    """Additive light, quantised to `steps` levels with ordered dithering (pixel-art falloff)."""
    for y in range(int(cy - radius), int(cy + radius) + 1):
        for x in range(int(cx - radius), int(cx + radius) + 1):
            d = math.hypot(x - cx, y - cy)
            if d > radius:
                continue
            t = (1.0 - d / radius) ** 2 * strength * steps
            q = int(t) + (1 if (t - int(t)) > bayer(x, y) else 0)
            if q > 0:
                c.add(x, y, color, q / steps)


# ---------------------------------------------------------------------------------------------- font

FONT5x7 = {
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
    "Y": ["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."],
    "-": [".....", ".....", ".....", ".###.", ".....", ".....", "....."],
    "*": [".....", ".....", "..#..", ".###.", "..#..", ".....", "....."],  # separator diamond
    " ": [".....", ".....", ".....", ".....", ".....", ".....", "....."],
}
MINI = {">": ["#..", ".#.", "..#", ".#.", "#.."], "_": ["...", "...", "...", "...", "###"]}


def text_width(s: str, scale: int = 1) -> int:
    return (len(s) * 6 - 1) * scale


def draw_text(c: Canvas, x: int, y: int, s: str, color, scale: int = 1, shadow=None) -> int:
    for pass_, col in (((1, 1), shadow), ((0, 0), color)):
        if col is None:
            continue
        cx = x + pass_[0]
        for ch in s:
            for gy, row in enumerate(FONT5x7[ch]):
                for gx, bit in enumerate(row):
                    if bit == "#":
                        c.rect(cx + gx * scale, y + pass_[1] + gy * scale,
                               cx + gx * scale + scale - 1, y + pass_[1] + gy * scale + scale - 1, col)
            cx += 6 * scale
    return x + len(s) * 6 * scale


# ---------------------------------------------------------------------------------------------- palette

C = {k: hexc(v) for k, v in {
    "outline": "#1a1730", "white": "#ecebf7", "lav": "#aaa3d8", "dim": "#7d76bd", "shadow": "#28244e",
    "yellow": "#f2c14e", "orange": "#ec5b2c", "blue": "#4fa8ea",
    "star": "#9ad7e8", "star_dim": "#4f5a92", "cyan": "#5fd7e7", "cyan_hi": "#c8f8ff", "cyan_dk": "#2b7f93",
    "moon": "#efe4bf", "moon_sh": "#d6c89d", "moon_cr": "#c9bb90",
    "cloud": "#211f45", "cloud_hi": "#2b2858",
    "grass": "#4b8f53", "grass_hi": "#6cbf63", "grass_dk": "#356b40",
    "dirt": "#5c4150", "rock1": "#4a3446", "rock2": "#3a2a40", "rock3": "#30223a", "rock_edge": "#22182a",
    "rock_spk": "#56405c",
    "stone_top": "#9a92ae", "stone": "#6e6684", "stone2": "#59526d", "stone3": "#4b455e", "stone_edge": "#2a2438",
    "water": "#2f6f9a", "water_hi": "#5fb3d6", "water_fx": "#a8e4f4",
    "rain": "#8a94c8", "ember": "#ffb347", "ember2": "#ff7a2a",
    "fire_y": "#ffe27a", "fire_o": "#ff9a30", "fire_r": "#e8452d", "log": "#8a5a34", "log_dk": "#5a3a24",
    "lamp": "#ffe9a0", "pole": "#2b2b3a", "warm": "#ffb35c", "staff_light": "#ffd98a",
}.items()}

# ---------------------------------------------------------------------------------------------- sprites

WIZARD = [  # 15 wide, faces left; feet on the last row; the staff is drawn at column 0
    "..........OO...",
    ".........OhO...",
    "........OhdO...",
    "........OhdO...",
    ".......OHhdO...",
    ".......OHhhdO..",
    "......OHhhhdO..",
    "......OHhhhhdO.",
    ".....OHhhhhhdO.",
    "..OOOHbbbbbbbOO",
    ".OHHHHhhhhhhhdO",
    "..OOOOOOOOOOOO.",
    "...OsessWWWO...",
    "..OssSsWWWWO...",
    "..OwwwwwwWWO...",
    ".OrwwwwwwWrrO..",
    ".srrrwwwwWrrRO.",
    ".srrrwwwwWrrRO.",
    "..OrrwwwWrrrRO.",
    "..OrrrwwWrrrRO.",
    "..OBBBBwBBBBBO.",
    "..OrrrrwrrrrRO.",
    "..OLrrrrrrrrRO.",
    ".OLrrrrrrrrrRRO",
    ".OLrrrrrrrrrRRO",
    ".OLrrrrrrrrrRRO",
    "OLLrrrrrrrrrRRO",
    "OLrrrrrrrrrrRRO",
    ".OffOOOOOOOffO.",
]
WIZARD_C = {
    "O": C["outline"], "h": hexc("#7d7c99"), "H": hexc("#a3a2bd"), "d": hexc("#5c5b78"), "b": hexc("#4a4964"),
    "s": hexc("#f2c9a0"), "S": hexc("#cf9f78"), "e": C["outline"], "w": hexc("#eceaf5"), "W": hexc("#bdbbd2"),
    "r": hexc("#b9b8cf"), "R": hexc("#8c8ba6"), "L": hexc("#dcdbea"), "B": hexc("#6b5a8a"), "f": hexc("#3a2f3f"),
}

FERRIS = [  # 14 wide, claws up
    "CC..........CC",
    "CC..WK..WK..CC",
    ".C...D..D...C.",
    "..OOOOOOOOOO..",
    ".OLLOOOOOOOOO.",
    ".OOOMOOOOMOOO.",
    ".OOOOMMMMOOOO.",
    "..DOOOOOOOOD..",
    ".D.D.D..D.D.D.",
]
FERRIS_C = {"C": hexc("#e8552d"), "O": hexc("#e8552d"), "L": hexc("#ff8a55"), "D": hexc("#a8381a"),
            "W": C["white"], "K": C["outline"], "M": hexc("#5a1a10")}

UMBRELLA = [
    "....RRRRRRR....",
    "..RLLRRRRRRRR..",
    ".RLRRRRRRRRRRR.",
    "RRRRRRRRRRRRRRR",
    "R.dR..dRd..Rd.R",
]
UMBRELLA_C = {"R": hexc("#d94040"), "L": hexc("#f07070"), "d": hexc("#9a2a2a")}

FIRE = [  # three flicker frames, 7 wide
    ["...Y...", "..YOY..", "..OYO..", ".OORYO.", ".RORRO.", "LLLLLLL", ".B...B."],
    ["..Y....", "..OY...", ".YOYO..", ".OYROO.", ".RORRO.", "LLLLLLL", ".B...B."],
    ["....Y..", "...YO..", "..OYOY.", ".OORYO.", ".ROORR.", "LLLLLLL", ".B...B."],
]
FIRE_C = {"Y": C["fire_y"], "O": C["fire_o"], "R": C["fire_r"], "L": C["log"], "B": C["log_dk"]}

BALROG = [  # the production incident, lurking in the chasm
    "K.............K",
    "KK...........KK",
    ".KK.........KK.",
    "..KKK.....KKK..",
    "...KKKKKKKKK...",
    "..KKKKKKKKKKK..",
    "..KKYYKKKYYKK..",
    ".KKKKKKKKKKKKK.",
    ".KKKKKMMMKKKKK.",
    "KKKKKKKKKKKKKKK",
    "KKKKKKKKKKKKKKK",
    "KKKKKKKKKKKKKKK",
    "KKKKKKKKKKKKKKK",
    "KKKKKKKKKKKKKKK",
]
BALROG_C = {"K": hexc("#1a0d18"), "Y": hexc("#ffdd55"), "M": hexc("#ff6a2a")}


def cube(c: Canvas, x: int, y: int, face, hi, sh, symbol: str) -> None:
    """A 7x7 commit block standing on y+6."""
    c.rect(x, y, x + 6, y + 6, C["outline"])
    c.rect(x + 1, y + 1, x + 5, y + 5, face)
    c.rect(x + 1, y + 1, x + 5, y + 1, hi)
    c.rect(x + 5, y + 2, x + 5, y + 5, sh)
    white = C["white"]
    pts = {
        "x": [(2, 2), (4, 2), (3, 3), (2, 4), (4, 4)],
        "check": [(1, 3), (2, 4), (3, 3), (4, 2)],
        "dots": [(2, 4), (3, 4), (4, 4)],
    }[symbol]
    for dx, dy in pts:
        c.put(x + dx, y + dy, white)


# ---------------------------------------------------------------------------------------------- icons

WARN = [  # 9x8 warning triangle
    "....R....",
    "...RYR...",
    "...RKR...",
    "..RYKYR..",
    "..RYKYR..",
    ".RYYYYYR.",
    ".RYYKYYR.",
    "RRRRRRRRR",
]
WARN_C = {"R": hexc("#e8452d"), "Y": hexc("#f2c14e"), "K": hexc("#2a1a10")}

XBADGE = [  # 7x7 red error badge
    ".RRRRR.",
    "RWRRRWR",
    "RRWRWRR",
    "RRRWRRR",
    "RRWRWRR",
    "RWRRRWR",
    ".RRRRR.",
]
XBADGE_C = {"R": hexc("#d93a3a"), "W": hexc("#ffe0e0")}

BUG = [  # 9x8 bug
    ".K.....K.",
    "..K...K..",
    "...RRR...",
    "KKRRDRRKK",
    "..RRDRR..",
    "KKRRDRRKK",
    "..RRDRR..",
    "...RRR...",
]
BUG_C = {"R": hexc("#ec5b2c"), "D": hexc("#7a1f10"), "K": hexc("#c9c2e8")}

LINT = [  # 13x9 code card with a red squiggle
    "OOOOOOOOOOOOO",
    "OggggggggpppO",
    "OppppgggggggO",
    "OgggggppppppO",
    "OrrrrrrrrrrrO",
    "OppgggggggggO",
    "OggppppgggggO",
    "O.r.r.r.r.r.O",
    "OOOOOOOOOOOOO",
]
LINT_C = {"O": hexc("#3a3566"), "g": hexc("#7d76bd"), "p": hexc("#4c4782"), "r": hexc("#ff5a5a"),
          ".": hexc("#221f44")}

SERVER = [  # 15x26 healthy server
    "OOOOOOOOOOOOOOO",
    "OsssssssssssssO",
    "OsPPPPPPPPPPPsO",
    "OsPGPgPvvvvvPsO",
    "OsPPPPPPPPPPPsO",
    "OsssssssssssssO",
    "OsPPPPPPPPPPPsO",
    "OsPGPGPvvvvvPsO",
    "OsPPPPPPPPPPPsO",
    "OsssssssssssssO",
    "OsPPPPPPPPPPPsO",
    "OsPGPgPvvvvvPsO",
    "OsPPPPPPPPPPPsO",
    "OsssssssssssssO",
    "OsPPPPPPPPPPPsO",
    "OsPGPGPvvvvvPsO",
    "OsPPPPPPPPPPPsO",
    "OsssssssssssssO",
    "OsPPPPPPPPPPPsO",
    "OsPGPgPvvvvvPsO",
    "OsPPPPPPPPPPPsO",
    "OsssssssssssssO",
    "OsssssssssssssO",
    "OOOOOOOOOOOOOOO",
    ".OO.........OO.",
    ".OO.........OO.",
]
SERVER_C = {"O": hexc("#14142a"), "s": hexc("#2e2e4a"), "P": hexc("#3d3d60"), "G": hexc("#5cf07a"),
            "g": hexc("#2f8a49"), "v": hexc("#262642")}



WIZARD_BIG = [  # 17x40, slim, faces left; staff at column 0; feet on the last row
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
    "....OsssWWWWWO...",
    "....OsesWWWWWWO..",
    "...OsssSWWWWWWO..",
    "....OSsWWWWWWWO..",
    "....OwwwwwwWWWO..",
    "...OrwwwwwwWrrO..",
    ".ssrrwwwwwwWrrRO.",
    ".ssrrrwwwwwWrrRO.",
    "..OrrrwwwwWrrrRO.",
    "..OrrrrwwwWrrrRO.",
    "..OBBBBBwwBBBBBO.",
    "..OrrrrrrwrrrrRO.",
    "..OLrrrrrwrrrrRO.",
    "..OLrrrrrrrrrrRO.",
    "..OLrrrrrrrrrRRO.",
    "..OLrrrrrrrrrRRO.",
    "..OLrrrrrrrrrRRO.",
    "..OLrrrrrrrrrRRO.",
    ".OLLrrrrrrrrrRRO.",
    ".OLLrrrrrrrrrRRO.",
    ".OLLrrrrrrrrrRRO.",
    ".OLLrrrrrrrrrRRO.",
    ".OLLrrrrrrrrrRRO.",
    ".OLLrrrrrrrrrRRO.",
    ".OOOOOOOOOOOOOOO.",
    "..OffO.....OffO..",
]
assert all(len(r) == 17 for r in WIZARD_BIG)

# ---------------------------------------------------------------------------------------------- scene


def render(frame: int) -> Canvas:
    c = Canvas()
    phase = frame / FRAMES
    bg = hexc("#14132b")
    for y in range(H):
        for x in range(W):
            c.put(x, y, bg)

    ground = 67
    wx, wy = 106, ground - 39  # wizard sprite origin (feet on the ground line)
    ox, oy = wx, wy - 9        # staff orb
    WZ = WIZARD_BIG

    # ground: a single quiet line, fading out at both ends
    for x in range(16, W - 16):
        edge = min(x - 16, W - 17 - x)
        if edge > 10 or bayer(x, ground + 1) < edge / 10:
            c.put(x, ground + 1, hexc("#2a2752"))

    # behind the wizard: the stable server, calm green glow and a check mark
    sx0, sy0 = 150, ground - 25
    glow(c, sx0 + 7, sy0 + 12, 20, hexc("#3ecf6a"), 0.22, steps=5)
    c.sprite(sx0, sy0, SERVER, SERVER_C)
    for i in range(5):  # blinking activity LEDs
        c.put(sx0 + 5, sy0 + 3 + i * 4, hexc("#2f8a49") if (i + frame) % 4 == 0 else hexc("#9cffb0"))
    cx0, cy0 = sx0 + 4, sy0 - 9
    for dx, dy in ((0, 2), (1, 3), (2, 4), (3, 3), (4, 2), (5, 1), (6, 0)):
        c.put(cx0 + dx, cy0 + dy, hexc("#5cf07a"))
        c.put(cx0 + dx, cy0 + dy + 1, hexc("#2f8a49"))

    # the shield: a crescent of light around the wizard's front
    scx, scy, sr = wx + 8, wy + 22, 24
    for a10 in range(1150, 2451, 12):
        a = math.radians(a10 / 10)
        x, y = int(round(scx + sr * math.cos(a))), int(round(scy + sr * math.sin(a)))
        if y > ground:
            continue
        shimmer = 0.55 + 0.45 * (((a10 // 12) + frame * 2) % 7 < 3)
        c.add(x, y, hexc("#ffe9a8"), 0.85 * shimmer)
        c.add(x - 1, y, hexc("#ffd98a"), 0.25 * shimmer)
        c.add(x + 1, y, hexc("#ffd98a"), 0.12)

    # incoming trouble, left to right, each with a fading motion trail
    bob = lambda k: int(round(1.2 * math.sin(2 * math.pi * (phase + k / 5))))
    trouble = [
        (14, 50 + bob(0), LINT, LINT_C),
        (34, 30 + bob(1), WARN, WARN_C),
        (50, 54 + bob(2), BUG, BUG_C),
        (60, 38 + bob(3), XBADGE, XBADGE_C),
        (72, 24 + bob(4), XBADGE, XBADGE_C),
        (74, 47 + bob(2), WARN, WARN_C),
    ]
    for x, y, spr, cmap in trouble:
        h = len(spr)
        for k in range(1, 6):  # motion trail
            if (k + frame) % 2 == 0 or k < 3:
                c.add(x - 2 * k, y + h // 2, hexc("#ff5a5a"), 0.30 - k * 0.05)
                if h > 7:
                    c.add(x - 2 * k - 1, y + h // 2 - 2, hexc("#ff5a5a"), 0.18 - k * 0.03)
        c.sprite(x, y, spr, cmap)

    # sparks where the trouble breaks on the shield
    for i, (px_, py_) in enumerate(((86, 30), (84, 44), (85, 37), (85, 55), (88, 24), (83, 50))):
        if (i + frame) % 3 != 0:
            c.put(px_, py_, hexc("#fff4c4"))
            c.put(px_ - 1, py_ - 1 + (frame % 2) * 2, hexc("#ff8a5a"))
            if (i + frame) % 2:
                c.put(px_ - 2, py_ + 1, hexc("#ff5a5a"))

    # the wizard, rim-lit, staff raised
    c.sprite(wx, wy, WZ, WIZARD_C)
    rim = hexc("#f4f2ff")
    for dy, row in enumerate(WZ):
        last = max((i for i, ch in enumerate(row) if ch not in ".O"), default=None)
        if last is not None and dy % 2 == 0 and row[last] in "hrRdW":
            c.put(wx + last, wy + dy, rim if row[last] in "rR" else hexc("#c4c3dc"))
    for y in range(wy - 6, wy + 40):
        c.put(wx, y, hexc("#7b5434") if y % 5 else hexc("#5e3e26"))
    pulse = 1.0 + 0.12 * math.sin(2 * math.pi * phase)
    glow(c, ox, oy, 17, C["staff_light"], 0.5 * pulse, steps=6)
    for y in range(int(oy - 5), int(oy + 6)):
        for x in range(int(ox - 5), int(ox + 6)):
            d = math.hypot(x - ox, y - oy)
            if d <= 4.6:
                c.put(x, y, hexc("#fffbe6") if d <= 3.2 else hexc("#ffe58a"))
    for gi, ch in enumerate(">_"):
        for yy, row in enumerate(MINI[ch]):
            for xx, bit in enumerate(row):
                if bit == "#":
                    c.put(int(ox) - 3 + gi * 4 + xx, int(oy) - 2 + yy, hexc("#3a2a00"))

    # wordmark
    title = "YOU SHELL NOT PASS"
    sub = "SNP * THE GIT HOOK GUARD"
    tx = (W - text_width(title, 2)) // 2
    draw_text(c, tx, 75, title, C["white"], 2, shadow=C["shadow"])
    sx = (W - text_width(sub)) // 2
    x = draw_text(c, sx, 94, "SNP", C["yellow"], 1, shadow=C["shadow"])
    draw_text(c, x, 94, sub[3:], C["lav"], 1, shadow=C["shadow"])
    return c


def main() -> None:
    frames = [render(f).image() for f in range(FRAMES)]
    frames[0].save(OUT / "logo.png", optimize=True)
    pal = [fr.quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for fr in frames]
    pal[0].save(OUT / "logo.gif", save_all=True, append_images=pal[1:], duration=140, loop=0, optimize=True,
                disposal=1)
    print(f"wrote {OUT / 'logo.png'} and {OUT / 'logo.gif'}")


if __name__ == "__main__":
    main()
