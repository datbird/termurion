#!/usr/bin/env python3
"""Termurion Acid: a terminal render test you can watch, and record as a GIF.

Four scenes, about 40 seconds, in the logo's colors:
  1. The wordmark, typed in with a moving 24-bit gradient over a star field.
  2. A plasma drawn with half blocks, two pixels per cell, behind a rounded panel.
  3. The test card: 256 colors, 24-bit color, every SGR style and underline kind,
     box drawing, block elements, a braille oscilloscope, wide and combining
     characters, and an OSC 8 link.
  4. A spinning torus, shaded in 24-bit color, and the score.

Run it in a window of about 120x40 or larger:  python scripts/acid.py
Options: --loop (repeat until Ctrl+C), --scene N (play one scene, 1 to 4).
No packages needed. Works in any terminal that speaks xterm sequences.
"""

import math
import os
import random
import shutil
import sys
import time

# The logo's colors (docs/assets/termurion/*.svg).
NAVY = (26, 35, 72)
CYAN = (47, 212, 255)
ICE = (127, 230, 255)
VIOLET = (138, 92, 255)
LAVENDER = (183, 155, 255)
AMBER = (255, 194, 74)
ORANGE = (255, 138, 42)
CREAM = (255, 243, 196)
WHITE = (232, 241, 255)
BG = (10, 13, 28)

ESC = "\x1b["
FPS = 24


def setup_console():
    """Windows: turn on VT output and UTF-8 for this console. Elsewhere nothing to do."""
    if os.name == "nt":
        import ctypes
        k32 = ctypes.windll.kernel32
        h = k32.GetStdHandle(-11)
        mode = ctypes.c_uint32()
        if k32.GetConsoleMode(h, ctypes.byref(mode)):
            k32.SetConsoleMode(h, mode.value | 0x0004)  # ENABLE_VIRTUAL_TERMINAL_PROCESSING
        k32.SetConsoleOutputCP(65001)
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except (AttributeError, ValueError):
        pass


def fg(c):
    return f"{ESC}38;2;{c[0]};{c[1]};{c[2]}m"


def bg(c):
    return f"{ESC}48;2;{c[0]};{c[1]};{c[2]}m"


def mix(a, b, t):
    t = 0.0 if t < 0 else 1.0 if t > 1 else t
    return (int(a[0] + (b[0] - a[0]) * t), int(a[1] + (b[1] - a[1]) * t), int(a[2] + (b[2] - a[2]) * t))


def ramp(stops, t):
    """A color along a looping ramp of stops, t in any range."""
    t = t % 1.0
    n = len(stops)
    f = t * n
    i = int(f)
    return mix(stops[i % n], stops[(i + 1) % n], f - i)


BRAND = [CYAN, ICE, LAVENDER, VIOLET, ORANGE, AMBER, CREAM]


def size():
    s = shutil.get_terminal_size((120, 40))
    return max(40, s.columns), max(16, s.lines)


class Frame:
    """One screen of text: a list of rows, each a list of (text, width) pieces."""

    def __init__(self, w, h):
        self.w, self.h = w, h
        self.cells = [[None] * w for _ in range(h)]  # (char, style) or None

    def put(self, x, y, ch, style="", wide=False):
        if 0 <= y < self.h and 0 <= x < self.w:
            if wide and x + 1 >= self.w:
                return
            self.cells[y][x] = (ch, style)
            if wide:
                self.cells[y][x + 1] = ("", style)

    def text(self, x, y, s, style=""):
        for i, ch in enumerate(s):
            self.put(x + i, y, ch, style)

    def render(self):
        out = [ESC + "H"]
        for y, row in enumerate(self.cells):
            out.append(f"{ESC}{y + 1};1H")
            last = None
            for cell in row:
                if cell is None:
                    if last != "":
                        out.append(f"{ESC}0m" + bg(BG))
                        last = ""
                    out.append(" ")
                    continue
                ch, style = cell
                if ch == "":
                    continue  # the right half of a wide character
                if style != last:
                    out.append(f"{ESC}0m" + bg(BG) + style)
                    last = style
                out.append(ch)
        out.append(f"{ESC}0m")
        return "".join(out)


# A 5-row block font, just the letters of the name.
FONT = {
    "T": ["█████", "  █  ", "  █  ", "  █  ", "  █  "],
    "E": ["█████", "█    ", "████ ", "█    ", "█████"],
    "R": ["████ ", "█   █", "████ ", "█  █ ", "█   █"],
    "M": ["█   █", "██ ██", "█ █ █", "█   █", "█   █"],
    "U": ["█   █", "█   █", "█   █", "█   █", " ███ "],
    "I": ["█████", "  █  ", "  █  ", "  █  ", "█████"],
    "O": [" ███ ", "█   █", "█   █", "█   █", " ███ "],
    "N": ["█   █", "██  █", "█ █ █", "█  ██", "█   █"],
}


def wordmark_pixels(word="TERMURION"):
    rows = ["", "", "", "", ""]
    for ch in word:
        for r in range(5):
            rows[r] += FONT[ch][r] + " "
    return [r[:-1] for r in rows]


def draw_wordmark(f, cx, cy, t, reveal=1.0, scale=2):
    rows = wordmark_pixels()
    width = len(rows[0]) * scale
    x0 = cx - width // 2
    shown = int(width * reveal)
    for r, line in enumerate(rows):
        for i, px in enumerate(line):
            for s in range(scale):
                x = i * scale + s
                if x >= shown:
                    continue
                if px == "█":
                    c = ramp(BRAND, x / (width * 1.4) - t * 0.25 + r * 0.02)
                    f.put(x0 + x, cy + r, "█", fg(c))
    # A soft shadow under the letters, drawn where no letter cell is.
    for r, line in enumerate(rows):
        for i, px in enumerate(line):
            below = rows[r + 1][i] if r + 1 < 5 else " "
            if px == "█" and below != "█":
                for s in range(scale):
                    x = i * scale + s + 1
                    if x < shown and 0 <= cy + r + 1 < f.h and 0 <= x0 + x < f.w and f.cells[cy + r + 1][x0 + x] is None:
                        f.put(x0 + x, cy + r + 1, "▀", fg(mix(BG, VIOLET, 0.45)))
    if shown < width and shown > 0:
        for r in range(5):
            f.put(x0 + shown, cy + r, "▌", fg(WHITE))
    return x0, width


# Scene 1: the wordmark over a star field.
def scene_logo(f, t, stars):
    w, h = f.w, f.h
    cy = h // 2 - 4
    for s in stars:
        x = int((s[0] - t * s[2] * 6) % w)
        y = int(s[1] * h)
        if cy - 1 <= y <= cy + 10:
            continue  # keep the wordmark and its lines clear
        tw = 0.5 + 0.5 * math.sin(t * 3 + s[3])
        ch = "✦" if s[2] > 0.8 else "·" if s[2] < 0.4 else "•"
        f.put(x, y, ch, fg(mix(BG, ICE if s[2] > 0.6 else LAVENDER, 0.25 + 0.6 * tw)))
    scale = 2 if w >= 112 else 1
    reveal = min(1.0, t / 2.2)
    draw_wordmark(f, w // 2, cy, t, reveal, scale)
    tag = "iTerm2, on Windows."
    n = int(max(0.0, t - 2.4) * 14)
    if n:
        s = tag[:n]
        x = w // 2 - len(tag) // 2
        f.text(x, cy + 7, s, fg(CREAM) + f"{ESC}3m")
        if n < len(tag) and int(t * 4) % 2 == 0:
            f.put(x + len(s), cy + 7, "▏", fg(AMBER))
    if t > 4.2:
        sub = "24-bit color · Unicode · GPU text"
        a = min(1.0, (t - 4.2) / 1.0)
        f.text(w // 2 - len(sub) // 2, cy + 9, sub, fg(mix(BG, LAVENDER, a)))


# Scene 2: plasma in half blocks behind a rounded panel.
def plasma_palette():
    stops = [NAVY, VIOLET, CYAN, ICE, CREAM, AMBER, ORANGE, VIOLET, NAVY]
    pal = []
    for i in range(256):
        c = ramp(stops, i / 256)
        pal.append((c[0] & ~7, c[1] & ~7, c[2] & ~7))
    return pal


PAL = plasma_palette()


def scene_plasma(f, t):
    w, h = f.w, f.h
    ph = h * 2
    cx, cy = w / 2, ph / 2
    colx = [math.sin(x / 9.0 + t) for x in range(w)]
    rowy = [math.sin(y / 7.0 - t * 0.7) for y in range(ph)]
    rows = []
    for y in range(ph):
        ry = rowy[y]
        row = []
        dy = (y - cy) * 2.0
        for x in range(w):
            dx = x - cx
            v = colx[x] + ry + math.sin((x + y + t * 6) / 13.0) + math.sin(math.sqrt(dx * dx + dy * dy) / 7.0 - t * 1.6)
            row.append(PAL[int((v + 4) * 32 + t * 20) & 255])
        rows.append(row)
    for y in range(h):
        top, bot = rows[2 * y], rows[2 * y + 1]
        for x in range(w):
            f.cells[y][x] = ("▀", fg(top[x]) + bg(bot[x]))
    # the panel
    pw, phh = min(w - 8, 64), 9
    px, py = (w - pw) // 2, (h - phh) // 2
    border = fg(CYAN) + bg(BG)
    for yy in range(phh):
        for xx in range(pw):
            ch = " "
            if yy == 0:
                ch = "╭" if xx == 0 else "╮" if xx == pw - 1 else "─"
            elif yy == phh - 1:
                ch = "╰" if xx == 0 else "╯" if xx == pw - 1 else "─"
            elif xx in (0, pw - 1):
                ch = "│"
            f.cells[py + yy][px + xx] = (ch, border)
    title = "T E R M U R I O N"
    f.text(px + (pw - len(title)) // 2, py + 2, title, f"{ESC}1m" + fg(ramp(BRAND, t * 0.2)) + bg(BG))
    lines = [
        "half blocks: two pixels a cell",
        f"{w}x{h * 2} pixels, 24-bit, {FPS} fps",
    ]
    for i, s in enumerate(lines):
        f.text(px + (pw - len(s)) // 2, py + 4 + i, s, fg(ICE) + bg(BG))
    fps = "rendered on the GPU, one frame per refresh"
    f.text(px + (pw - len(fps)) // 2, py + 7, fps[: pw - 4], fg(mix(BG, LAVENDER, 0.8)) + bg(BG))


# Scene 3: the test card.
def box(f, x, y, w, h, title, color):
    st = fg(color)
    for xx in range(w):
        f.put(x + xx, y, "─", st)
        f.put(x + xx, y + h - 1, "─", st)
    for yy in range(h):
        f.put(x, y + yy, "│", st)
        f.put(x + w - 1, y + yy, "│", st)
    f.put(x, y, "╭", st)
    f.put(x + w - 1, y, "╮", st)
    f.put(x, y + h - 1, "╰", st)
    f.put(x + w - 1, y + h - 1, "╯", st)
    f.text(x + 2, y, f" {title} ", f"{ESC}1m" + fg(color))


BRAILLE = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]]


def card_colors(f, x, y, t):
    box(f, x, y, 40, 10, "256 colors", CYAN)
    for i in range(16):
        f.put(x + 2 + i * 2, y + 1, " ", f"{ESC}48;5;{i}m")
        f.put(x + 3 + i * 2, y + 1, " ", f"{ESC}48;5;{i}m")
    for row in range(6):
        for col in range(36):
            n = 16 + (col // 6) * 36 + row * 6 + col % 6
            f.put(x + 2 + col, y + 2 + row, " ", f"{ESC}48;5;{n}m")
    for i in range(24):
        f.put(x + 2 + i, y + 8, " ", f"{ESC}48;5;{232 + i}m")
    f.text(x + 27, y + 8, "16+216+24", fg(mix(BG, ICE, 0.7)))


def card_truecolor(f, x, y, w, t):
    box(f, x, y, w, 5, "24-bit color", VIOLET)
    span = w - 4
    for i in range(span):
        c1 = ramp(BRAND, i / span - t * 0.15)
        c2 = mix(NAVY, ramp([CYAN, VIOLET, ORANGE], i / span + t * 0.1), 0.5 + 0.5 * math.sin(i / 6 + t * 2))
        f.put(x + 2 + i, y + 1, "▀", fg(c1) + bg(c2))
        h_ = 0.5 + 0.5 * math.sin(i / span * math.tau * 2 - t * 3)
        f.put(x + 2 + i, y + 2, " ", bg(mix(BG, AMBER, h_)))
        g = int(255 * i / max(1, span - 1))
        f.put(x + 2 + i, y + 3, " ", bg((g, g, g)))


def card_styles(f, x, y, t):
    box(f, x, y, 40, 10, "SGR styles", AMBER)
    items = [
        ("bold", "1"), ("dim", "2"), ("italic", "3"), ("inverse", "7"),
        ("strike", "9"), ("overline", "53"), ("blink", "5"), ("faint+it", "2;3"),
    ]
    cx, cy = x + 2, y + 1
    for i, (name, code) in enumerate(items):
        col, row = i % 4, i // 4
        f.text(cx + col * 9, cy + row, name, f"{ESC}{code}m" + fg(WHITE))
    unders = [("single", "4:1"), ("double", "4:2"), ("curly", "4:3"), ("dotted", "4:4"), ("dashed", "4:5")]
    for i, (name, code) in enumerate(unders):
        uc = ramp(BRAND, i / 5 + t * 0.1)
        f.text(cx + (i % 3) * 12, cy + 3 + i // 3, name, f"{ESC}{code}m{ESC}58:2::{uc[0]}:{uc[1]}:{uc[2]}m" + fg(ICE))
    f.text(cx, cy + 6, "colored underline, any of 16M", f"{ESC}4:3m{ESC}58:2::255:138:42m" + fg(CREAM))
    f.text(cx, cy + 7, "bold+italic+underline", f"{ESC}1;3;4m" + fg(CYAN))


def card_boxes(f, x, y, t):
    box(f, x, y, 40, 10, "lines and blocks", ICE)
    lines = [
        "┌─┬─┐ ┏━┳━┓ ╔═╦═╗ ╭─┬─╮",
        "├─┼─┤ ┣━╋━┫ ╠═╬═╣ │ │ │",
        "└─┴─┘ ┗━┻━┛ ╚═╩═╝ ╰─┴─╯",
        "░░▒▒▓▓██  ▖▗▘▝▚▞▙▛▜▟",
    ]
    for i, s in enumerate(lines):
        f.text(x + 2, y + 1 + i, s, fg(mix(ICE, LAVENDER, i / 4)))
    bars = "▁▂▃▄▅▆▇█"
    for i in range(36):
        v = 0.5 + 0.45 * math.sin(i / 3.0 + t * 3) * math.cos(i / 7.0 - t)
        lvl = v * 3 * 8
        for r in range(3):
            fill = lvl - (2 - r) * 8
            ch = "█" if fill >= 8 else bars[int(fill)] if fill > 0 else " "
            f.put(x + 2 + i, y + 5 + r, ch, fg(ramp([CYAN, VIOLET, ORANGE, AMBER], i / 36 + v * 0.2)))
    f.text(x + 2, y + 8, "eighth blocks, 3 rows, live", fg(mix(BG, ICE, 0.7)))


def card_scope(f, x, y, w, t):
    box(f, x, y, w, 10, "braille oscilloscope", CYAN)
    cw, ch = w - 4, 8
    dots = [[0] * cw for _ in range(ch)]
    pw, ph = cw * 2, ch * 4
    for k in range(pw * 3):
        a = k / (pw * 3) * math.tau
        px = int((math.sin(a * 3 + t) * 0.5 + 0.5) * (pw - 1))
        py = int((math.sin(a * 4 + t * 1.3) * 0.5 + 0.5) * (ph - 1))
        dots[py // 4][px // 2] |= BRAILLE[py % 4][px % 2]
    for xx in range(pw):
        py = int((math.sin(xx / pw * math.tau * 2 - t * 2) * 0.35 + 0.5) * (ph - 1))
        dots[py // 4][xx // 2] |= BRAILLE[py % 4][xx % 2]
    for r in range(ch):
        for c in range(cw):
            if dots[r][c]:
                f.put(x + 2 + c, y + 1 + r, chr(0x2800 + dots[r][c]), fg(ramp([CYAN, ICE, LAVENDER], c / cw + r / 30)))


def card_unicode(f, x, y, w, t):
    box(f, x, y, w, 10, "Unicode", LAVENDER)
    cx, cy = x + 2, y + 1
    f.text(cx, cy, "wide:", fg(mix(BG, ICE, 0.7)))
    wide = "端末 터미널 終端"
    xx = cx + 6
    for chh in wide:
        if chh == " ":
            xx += 1
            continue
        f.put(xx, cy, chh, fg(CREAM), wide=True)
        xx += 2
    f.text(cx, cy + 1, "emoji:", fg(mix(BG, ICE, 0.7)))
    xx = cx + 7
    for e in ["🚀", "🐙", "🌈", "⚡", "🧪"]:
        f.put(xx, cy + 1, e, "", wide=True)
        xx += 3
    f.text(cx, cy + 2, "combining:", fg(mix(BG, ICE, 0.7)))
    for i, s in enumerate(["é", "ñ", "ä", "ô", "ç"]):
        f.put(cx + 11 + i * 2, cy + 2, s, fg(AMBER))
    f.text(cx, cy + 3, "math: ∑ ∫ √ ∞ ≈ ≠ ≤ ≥ ∂ ∇ π λ", fg(ICE))
    f.text(cx, cy + 4, "arrows: ← ↑ → ↓ ↔ ⇄ ⟳ ↻ ⏎", fg(ICE))
    spin = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"[int(t * 12) % 10]
    f.text(cx, cy + 5, f"spinners: {spin}", fg(CYAN))
    f.put(cx + 12, cy + 5, "◐◓◑◒"[int(t * 6) % 4], fg(AMBER))
    f.put(cx + 14, cy + 5, "▖▘▝▗"[int(t * 8) % 4], fg(LAVENDER))
    link = "github.com/datbird/termurion"
    f.text(cx, cy + 7, "link:", fg(mix(BG, ICE, 0.7)))
    # OSC 8 around the text, written as one piece so the link spans every cell.
    osc = "\x1b]8;;https://github.com/datbird/termurion\x1b\\"
    end = "\x1b]8;;\x1b\\"
    f.put(cx + 6, cy + 7, osc + link[0], f"{ESC}4m" + fg(CYAN))
    for i, chh in enumerate(link[1:-1], 1):
        f.put(cx + 6 + i, cy + 7, chh, f"{ESC}4m" + fg(CYAN))
    f.put(cx + 6 + len(link) - 1, cy + 7, link[-1] + end, f"{ESC}4m" + fg(CYAN))


def scene_card(f, t):
    w, h = f.w, f.h
    title = " TERMURION RENDER TEST "
    for x in range(w):
        f.put(x, 0, " ", bg(ramp(BRAND, x / w * 0.6 - t * 0.1)))
    f.text((w - len(title)) // 2, 0, title, f"{ESC}1m" + fg(BG) + bg(CREAM))
    x0 = max(0, (w - 122) // 2)
    oy = max(0, (h - 32) // 2)  # the card is 30 rows; center it
    full = w >= 122 and h >= 32
    if full:
        card_colors(f, x0, oy + 2, t)
        card_styles(f, x0 + 41, oy + 2, t)
        card_boxes(f, x0 + 82, oy + 2, t)
        card_truecolor(f, x0, oy + 12, 122, t)
        card_scope(f, x0, oy + 17, 60, t)
        card_unicode(f, x0 + 61, oy + 17, 61, t)
    else:
        card_colors(f, x0, oy + 2, t)
        if w >= 81:
            card_styles(f, x0 + 41, oy + 2, t)
        card_truecolor(f, x0, oy + 12, min(w, 81), t)
        card_unicode(f, x0, oy + 17, min(w, 61), t)
    stamp_row = oy + 27
    n = min(100, int(t / 8.5 * 100))
    bar_w = 40
    filled = int(bar_w * n / 100)
    y = min(h - 2, stamp_row + 2)
    xb = (w - bar_w - 10) // 2
    for i in range(bar_w):
        f.put(xb + i, y, "█" if i < filled else "░", fg(ramp([CYAN, VIOLET, AMBER], i / bar_w) if i < filled else mix(BG, ICE, 0.25)))
    f.text(xb + bar_w + 2, y, f"{n:3d}/100", f"{ESC}1m" + fg(AMBER if n == 100 else ICE))


# Scene 4: the torus.
def scene_torus(f, t):
    w, h = f.w, f.h
    A, B = t * 1.1, t * 0.6
    cA, sA, cB, sB = math.cos(A), math.sin(A), math.cos(B), math.sin(B)
    zbuf = [[0.0] * w for _ in range(h)]
    lum = [[-1.0] * w for _ in range(h)]
    K2 = 5.0
    K1 = min(w, h * 2) * K2 * 3 / (8 * 3)
    th = 0.0
    while th < math.tau:
        ct, st = math.cos(th), math.sin(th)
        ph = 0.0
        while ph < math.tau:
            cp, sp = math.cos(ph), math.sin(ph)
            cx = 2 + 1 * ct
            cy = 1 * st
            x = cx * (cB * cp + sA * sB * sp) - cy * cA * sB
            y = cx * (sB * cp - sA * cB * sp) + cy * cA * cB
            z = K2 + cA * cx * sp + cy * sA
            ooz = 1 / z
            xp = int(w / 2 + K1 * ooz * x)
            yp = int(h / 2 - K1 * ooz * y / 2)
            L = cp * ct * sB - cA * ct * sp - sA * st + cB * (cA * st - ct * sA * sp)
            if 0 <= xp < w and 0 <= yp < h and ooz > zbuf[yp][xp]:
                zbuf[yp][xp] = ooz
                lum[yp][xp] = L
            ph += 0.025
        th += 0.08
    shades = ".,-~:;=!*#$@"
    for y in range(h):
        for x in range(w):
            L = lum[y][x]
            if L >= -0.99:
                k = max(0.0, (L + 1.0) / 2.42)
                c = mix(mix(NAVY, VIOLET, k * 1.6), ramp([CYAN, ICE, CREAM, AMBER], x / w * 0.5 + t * 0.08), k)
                f.put(x, y, shades[min(11, int(k * 12))], fg(c))
    word = "TERMURION"
    f.text((w - len(word) * 2) // 2, h - 3, " ".join(word), f"{ESC}1m" + fg(ramp(BRAND, t * 0.3)))
    if t > 6:
        s = "ACID: 100/100  PASS"
        f.text((w - len(s)) // 2, h - 2, s, f"{ESC}1m" + fg(AMBER))


SCENES = [(scene_logo, 7.0), (scene_plasma, 9.0), (scene_card, 12.0), (scene_torus, 10.0)]


def main():
    args = sys.argv[1:]
    loop = "--loop" in args
    only = None
    if "--scene" in args:
        i = args.index("--scene")
        only = int(args[i + 1]) - 1
    setup_console()
    out = sys.stdout
    random.seed(7)
    stars = [(random.random() * 400, random.random(), random.random(), random.random() * 6) for _ in range(160)]
    out.write(f"{ESC}?1049h{ESC}?25l{ESC}0m" + bg(BG) + f"{ESC}2J")
    out.flush()
    try:
        while True:
            for idx, (scene, length) in enumerate(SCENES):
                if only is not None and idx != only:
                    continue
                start = time.monotonic()
                last_size = None
                while True:
                    t = time.monotonic() - start
                    if t > length:
                        break
                    w, h = size()
                    if (w, h) != last_size:
                        out.write(f"{ESC}0m" + bg(BG) + f"{ESC}2J")
                        last_size = (w, h)
                    fr = Frame(w, h)
                    if scene is scene_logo:
                        scene(fr, t, stars)
                    else:
                        scene(fr, t)
                    # Synchronized output (DEC mode 2026): the terminal shows the frame whole.
                    out.write(f"{ESC}?2026h" + fr.render() + f"{ESC}?2026l")
                    out.flush()
                    spent = time.monotonic() - start - t
                    time.sleep(max(0.0, 1.0 / FPS - spent))
            if not loop:
                break
    except KeyboardInterrupt:
        pass
    finally:
        out.write(f"{ESC}0m{ESC}?25h{ESC}?1049l")
        out.flush()


if __name__ == "__main__":
    main()
