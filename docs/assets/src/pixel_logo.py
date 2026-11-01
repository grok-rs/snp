#!/usr/bin/env python3
"""Render the snp README banner as pixel art.

Writes docs/assets/logo.png (static) and docs/assets/logo.gif (animated): a 240x130 canvas scaled 4x with
nearest-neighbour, so every art pixel is a crisp 4x4 block. Everything is drawn from the sprite maps and
procedures below: no external images.

Scene: the wizard holds the bridge between the repository island (server rack, queued commits) and the
production island (Ferris under an umbrella, campfire, deployed commit). A failing commit is stopped in
front of the glowing ">_" staff; the production incident lurks in the fire below.

    python3 docs/assets/src/pixel_logo.py
"""

from __future__ import annotations

import math
import random
from pathlib import Path

from PIL import Image

W, H, SCALE = 240, 130, 4
FRAMES = 10  # animation loop; rain falls 13 px per frame so 10 frames tile the 130 px height
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


# ---------------------------------------------------------------------------------------------- scene


def render(frame: int) -> Canvas:
    c = Canvas()
    rng = random.Random(1337)  # static layout; `frame` only drives the animated details
    phase = frame / FRAMES

    # sky: banded gradient, dithered only around band edges
    sky = [hexc(h) for h in ("#0f0f26", "#13132d", "#181735", "#1e1b3f", "#251f49", "#2c2452")]
    for y in range(H):
        t = y / (H - 1) * (len(sky) - 1)
        i = int(t)
        f = min(1.0, max(0.0, ((t - i) - 0.35) / 0.3))
        for x in range(W):
            c.put(x, y, sky[min(i + 1, len(sky) - 1)] if f > bayer(x, y) else sky[i])

    # text boxes (keep stars out of them)
    title = "YOU SHELL NOT PASS"
    sub = "SNP * THE GIT HOOK GUARD"
    tag = [("PRE-COMMIT DROP-IN", C["yellow"]), (" * ", C["dim"]), ("RUST", C["orange"]), (" * ", C["dim"]),
           ("DEVOPS", C["blue"])]
    tag_len = sum(len(s) for s, _ in tag)
    tx, ty = (W - text_width(title, 2)) // 2, 7
    sx, sy = (W - text_width(sub)) // 2, 27
    gx, gy = (W - (tag_len * 6 - 1)) // 2, 37
    boxes = [(tx - 3, ty - 3, tx + text_width(title, 2) + 3, ty + 17),
             (sx - 3, sy - 2, sx + text_width(sub) + 3, sy + 9),
             (gx - 3, gy - 2, gx + tag_len * 6 + 3, gy + 9)]

    def free(x: int, y: int) -> bool:
        return not any(a <= x <= b and c0 <= y <= d for a, c0, b, d in boxes)

    # distant clouds
    for cx0, cy0, ln in ((-4, 62, 40), (14, 70, 28), (178, 74, 46), (150, 66, 22), (96, 52, 18)):
        for i, (dy, cut) in enumerate(((0, 6), (1, 0), (2, 3))):
            c.rect(cx0 + cut, cy0 + dy, cx0 + ln - cut - i * 2, cy0 + dy, C["cloud"] if i else C["cloud_hi"])

    # stars: dim dots, bright dots, twinkling crosses, little cyan "bits" like data packets
    for _ in range(70):
        x, y = rng.randrange(W), rng.randrange(0, 84)
        if free(x, y):
            c.put(x, y, C["star_dim"] if rng.random() < 0.6 else C["star"])
    for i in range(9):
        x, y = rng.randrange(6, W - 6), rng.randrange(4, 78)
        if not free(x, y):
            continue
        bright = (i + frame) % 3 != 0
        col = C["cyan_hi"] if bright else C["star"]
        c.put(x, y, col)
        if bright:
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                c.put(x + dx, y + dy, C["star"])
    for _ in range(7):
        x, y = rng.randrange(4, W - 6), rng.randrange(8, 88)
        if free(x, y) and free(x + 2, y + 2):
            c.rect(x - 1, y - 1, x + 2, y + 2, C["shadow"])
            c.rect(x, y, x + 1, y + 1, C["cyan"])

    # moon with a dotted halo
    mx, my, mr = 224, 58, 7.5
    for y in range(int(my - 13), int(my + 14)):
        for x in range(int(mx - 13), int(mx + 14)):
            d = math.hypot(x - mx, y - my)
            if d <= mr:
                col = C["moon_sh"] if (x - mx) + (y - my) > mr * 0.75 else C["moon"]
                c.put(x, y, col)
            elif d <= 12.5 and (x + y) % 2 == 0 and bayer(x, y) < (12.5 - d) / 4.5:
                c.put(x, y, hexc("#3c3a72") if d < 10 else hexc("#2e2c5e"))
    for x, y in ((221, 55), (222, 55), (226, 60), (223, 61), (227, 56)):
        c.put(x, y, C["moon_cr"])

    # chasm fire glow between the islands
    fire_pal = [hexc(h) for h in ("#3a1d44", "#5e2240", "#8f2c38", "#c43f2c", "#ee6a2a", "#ffa040")]
    for y in range(80, H):
        for x in range(60, 181):
            v = 1 - math.hypot((x - 120) / 48, (y - 142) / 44)
            if v <= 0:
                continue
            t = v * (len(fire_pal) + 0.4)
            i = int(t)
            f = min(1.0, max(0.0, ((t - i) - 0.25) / 0.5))
            idx = i + (1 if f > bayer(x, y) else 0)
            if idx >= 1:
                c.put(x, y, fire_pal[min(idx - 1, len(fire_pal) - 1)])
    c.sprite(113, 112, BALROG, BALROG_C)
    for i in range(26):
        x = rng.randrange(92, 150)
        y0 = rng.randrange(0, 30)
        y = 98 + (y0 - frame * 3) % 30
        c.put(x, y, C["ember"] if i % 3 else C["ember2"])

    # floating islands
    mask: set[tuple[int, int]] = set()

    def island(x0: int, x1: int, top: int, maxd: int, seed: int) -> None:
        r = random.Random(seed)
        cx, hw = (x0 + x1) / 2, (x1 - x0) / 2
        jag = [r.choice((-1, 0, 0, 1)) for _ in range(x1 - x0 + 2)]
        for x in range(x0, x1 + 1):
            u = (x - cx) / hw
            d = max(3, int(maxd * (1 - abs(u) ** 1.7)) + jag[(x - x0) // 2])
            for y in range(top, top + d + 1):
                k = y - top
                if y == top + d:
                    col = C["rock_edge"]
                elif k <= 1:
                    col = C["dirt"]
                elif k <= 4:
                    col = C["rock1"]
                elif u > 0.3 and (x + y) % 2 == 0:
                    col = C["rock3"]
                else:
                    col = C["rock2"]
                c.put(x, y, col)
                mask.add((x, y))
            # roots and drips under the edge
            if r.random() < 0.12:
                ln = r.randrange(2, 6)
                for yy in range(top + d + 1, top + d + 1 + ln):
                    c.put(x, yy, C["rock_edge"])
                if r.random() < 0.4:
                    c.put(x, top + d + ln + 2 + (frame % 3), C["cyan_dk"])
        for _ in range(int((x1 - x0) * 1.4)):  # rock speckles
            x, y = r.randrange(x0, x1), r.randrange(top + 3, top + maxd)
            if (x, y) in mask and (x + 1, y) in mask and (x, y + 1) in mask:
                c.put(x, y, C["rock_spk"])
                c.put(x + 1, y, C["rock_spk"])
                c.put(x, y + 1, C["rock_edge"])
        for x in range(x0 + 1, x1):  # grass
            c.put(x, top, C["grass_hi"] if r.random() < 0.25 else C["grass"])
            c.put(x, top + 1, C["grass_dk"] if r.random() < 0.6 else C["dirt"])
            if r.random() < 0.18:
                c.put(x, top - 1, C["grass_hi"])
                if r.random() < 0.3:
                    c.put(x, top - 2, C["grass"])

    island(6, 88, 93, 30, 11)
    island(152, 234, 93, 32, 23)

    crystals = [(28, 104), (46, 113), (66, 101), (18, 99), (74, 108), (168, 106), (190, 117), (212, 101),
                (200, 99), (224, 107)]
    for x, y in crystals:
        if (x, y) in mask:
            c.put(x, y, C["cyan_hi"])
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                c.put(x + dx, y + dy, C["cyan"])
            c.put(x + 1, y + 1, C["cyan_dk"])

    # pond and waterfall on the repository island
    for x in range(8, 18):
        c.put(x, 93, C["water_hi"] if (x + frame) % 4 == 0 else C["water"])
        c.put(x, 94, C["water"])
    for y in range(93, H):
        for x, base in ((5, C["water"]), (6, C["water_hi"])):
            col = C["water_fx"] if (y + frame * 2 + x * 3) % 7 == 0 else base
            c.put(x, y, col)
        if (y + frame) % 5 == 0:
            c.put(4 - (y % 2), y, C["water_hi"])

    # the bridge of Khazad-dum: a thin arch, thick at the abutments, lit along its lower rim by the fire
    for x in range(85, 156):
        u = (x - 120) / 35
        thick = 2 + int(round(13 * u ** 4 + 3 * u * u))
        for y in range(93, 93 + thick + 1):
            if y == 93:
                col = C["stone_top"]
            elif y == 93 + thick:
                col = hexc("#b0563a") if abs(u) < 0.8 else C["stone_edge"]  # firelit rim
            elif y == 93 + thick - 1:
                col = C["stone_edge"]
            elif y == 94:
                col = C["stone3"] if x % 6 == 0 else C["stone"]
            elif y == 95 and x % 6 == 3:
                col = C["stone3"]
            else:
                col = C["stone2"] if y < 97 else C["stone3"]
            c.put(x, y, col)

    # repository island: server rack, lamp, queued commits
    rx, ry = 21, 69
    c.rect(rx, ry, rx + 11, ry + 23, C["outline"])
    c.rect(rx + 1, ry + 1, rx + 10, ry + 22, hexc("#2c2c42"))
    leds = (hexc("#5cf07a"), hexc("#ffc24a"), hexc("#4fa8ea"))
    for i in range(5):
        uy = ry + 2 + i * 4
        c.rect(rx + 2, uy, rx + 9, uy + 2, hexc("#3b3b57"))
        on = (i * 3 + frame) % 4 != 0
        c.put(rx + 3, uy + 1, leds[0] if on else hexc("#24603a"))
        c.put(rx + 5, uy + 1, leds[(i + 1) % 3] if (i + frame) % 2 else hexc("#3b3b57"))
        for vx in range(rx + 7, rx + 9):
            c.put(vx, uy + 1, hexc("#24243a"))
    c.put(rx + 9, ry - 1, C["pole"])
    c.put(rx + 9, ry - 2, C["pole"])
    c.put(rx + 9, ry - 3, hexc("#ff5050") if frame % 2 == 0 else hexc("#7a2a2a"))

    lx = 41
    c.rect(lx, 76, lx, 92, C["pole"])
    c.rect(lx - 1, 73, lx + 1, 75, C["outline"])
    c.put(lx, 74, C["lamp"])

    for qx in (50, 61, 72):
        cube(c, qx, 86, hexc("#7b7fa8"), hexc("#a3a7cc"), hexc("#5a5d85"), "dots")

    # production island: deployed commit, campfire, Ferris under an umbrella, prod server
    cube(c, 160, 86, hexc("#43c463"), hexc("#7ae08a"), hexc("#2b8a45"), "check")
    c.sprite(176, 86, FIRE[frame % 3], FIRE_C)
    for i in range(3):  # sparks
        c.put(178 + (i * 2 + frame) % 4, 82 - (frame + i * 3) % 6, C["ember"])
    c.sprite(189, 84, FERRIS, FERRIS_C)
    c.rect(202, 72, 202, 83, hexc("#3a3a4a"))
    c.sprite(195, 67, UMBRELLA, UMBRELLA_C)
    px0, py0 = 217, 79
    c.rect(px0, py0, px0 + 9, py0 + 13, C["outline"])
    c.rect(px0 + 1, py0 + 1, px0 + 8, py0 + 12, hexc("#2c2c42"))
    for i in range(3):
        uy = py0 + 2 + i * 4
        c.rect(px0 + 2, uy, px0 + 7, uy + 2, hexc("#3b3b57"))
        c.put(px0 + 3, uy + 1, hexc("#5cf07a") if (i + frame) % 3 else hexc("#24603a"))
        c.put(px0 + 5, uy + 1, hexc("#5cf07a"))

    # the wizard holds the bridge, staff raised against the failing commit
    wx, wy = 114, 64
    c.sprite(wx, wy, WIZARD, WIZARD_C)
    rim = hexc("#f4f2ff")
    for dy, row in enumerate(WIZARD):
        last = max((i for i, ch in enumerate(row) if ch not in ".O"), default=None)
        if last is not None and dy % 2 == 0 and row[last] in "hrRdW":
            c.put(wx + last, wy + dy, rim if row[last] in "rR" else hexc("#c4c3dc"))
    for y in range(wy - 6, wy + 29):
        c.put(wx, y, hexc("#7b5434") if y % 5 else hexc("#5e3e26"))
    cube(c, 99, 86, hexc("#d94040"), hexc("#f06a5a"), hexc("#8a2028"), "x")

    # lights
    pulse = 1.0 + 0.12 * math.sin(2 * math.pi * phase)
    ox, oy = wx, wy - 9
    glow(c, ox, oy, 19, C["staff_light"], 0.5 * pulse, steps=6)
    glow(c, lx, 74, 8, C["lamp"], 0.45, steps=5)
    glow(c, 179, 88, 12, C["warm"], 0.42 * (1.0 + 0.15 * ((frame * 7) % 3 - 1)), steps=5)
    for x, y in crystals:
        if (x, y) in mask:
            glow(c, x, y, 4, C["cyan"], 0.25)
    for y in range(int(oy - 5), int(oy + 6)):  # the bright orb of the staff
        for x in range(int(ox - 5), int(ox + 6)):
            d = math.hypot(x - ox, y - oy)
            if d <= 4.6:
                c.put(x, y, hexc("#fffbe6") if d <= 3.2 else hexc("#ffe58a"))
    for gi, ch in enumerate(">_"):
        for yy, row in enumerate(MINI[ch]):
            for xx, bit in enumerate(row):
                if bit == "#":
                    c.put(int(ox) - 3 + gi * 4 + xx, int(oy) - 2 + yy, hexc("#3a2a00"))
    for dx, dy in ((7, -1), (8, 1), (7, 3), (9, -2)):  # the commit is refused: sparks off the block
        if (dx + dy + frame) % 3:
            c.put(99 + dx, 86 + dy, C["white"] if dy % 2 else hexc("#ff8a8a"))

    # fireflies
    for i in range(6):
        x = rng.randrange(20, 225)
        y = rng.randrange(60, 90) + int(2 * math.sin(2 * math.pi * (phase + i / 6)))
        if free(x, y):
            c.put(x, y, C["yellow"] if (i + frame) % 4 else C["fire_y"])

    # rain
    drops = [(rng.randrange(W), rng.randrange(H), rng.choice((2, 3, 3, 4))) for _ in range(150)]
    for x, y0, ln in drops:
        y = (y0 + frame * 13) % H
        for k in range(ln):
            c.mix(x, (y + k) % H, C["rain"], 0.28)

    # text
    draw_text(c, tx, ty, title, C["white"], 2, shadow=C["shadow"])
    draw_text(c, sx, sy, sub, C["lav"], 1, shadow=C["shadow"])
    x = gx
    for s, col in tag:
        x = draw_text(c, x, gy, s, col, 1, shadow=C["shadow"])
    return c


def main() -> None:
    frames = [render(f).image() for f in range(FRAMES)]
    frames[0].save(OUT / "logo.png", optimize=True)
    pal = [fr.quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for fr in frames]
    pal[0].save(OUT / "logo.gif", save_all=True, append_images=pal[1:], duration=110, loop=0, optimize=True,
                disposal=1)
    print(f"wrote {OUT / 'logo.png'} and {OUT / 'logo.gif'}")


if __name__ == "__main__":
    main()
