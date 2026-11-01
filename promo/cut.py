"""The promo's cut: puts Termurion's rendered shots (render_promo_shots) into 1920x1080 frames at
60 fps with the window, captions, camera and transitions, writes the cues for the music, then
encodes with ffmpeg and joins the music.

  python promo/cut.py <shots dir> cues      -> <shots dir>/cues-video.json (for music.py)
  python promo/cut.py <shots dir> video     -> <shots dir>/termurion-promo-1080p.mp4
  python promo/cut.py <shots dir> still 26.5 -> <shots dir>/still-26.50.png

Needs Pillow and numpy, ffmpeg on PATH or in FFMPEG, and Windows' Segoe UI and Cascadia Mono.
"""
import json
import math
import os
import pathlib
import subprocess
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import storyboard as sb  # noqa: E402

W, H, FPS = 1920, 1080, sb.FPS
SHOTS = pathlib.Path(sys.argv[1])
CUES = json.loads((SHOTS / "cues.json").read_text())
SIZES = CUES["sizes"]
FONTS = pathlib.Path(os.environ.get("WINDIR", "C:/Windows")) / "Fonts"
_fonts = {}


def font(name, size):
    key = (name, size)
    if key not in _fonts:
        _fonts[key] = ImageFont.truetype(str(FONTS / name), size)
    return _fonts[key]


BLACK, BOLD, REG, MONO = "seguibl.ttf", "segoeuib.ttf", "segoeui.ttf", "CascadiaMono.ttf"


def hexrgb(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def color(fx):
    return hexrgb(sb.FX[fx][0])


def ease(u):
    u = min(1.0, max(0.0, u))
    return 1 - (1 - u) ** 3


# ---------------------------------------------------------------- the stage
_bg = {}


def background(fx):
    """Near black, a soft glow of the effect's color behind the window, darker corners."""
    if fx not in _bg:
        y, x = np.mgrid[0:H, 0:W].astype(np.float32)
        base = np.array([11, 12, 16], np.float32)
        d = np.sqrt(((x - W / 2) / (W * 0.55)) ** 2 + ((y - H * 0.45) / (H * 0.5)) ** 2)
        glow = np.exp(-d * d * 2.2)[..., None] * np.array(color(fx) if fx else (90, 90, 110), np.float32) * 0.10
        vignette = np.clip(1.15 - 0.45 * d, 0.55, 1.0)[..., None]
        img = np.clip((base + glow) * vignette, 0, 255).astype(np.uint8)
        _bg[fx] = Image.fromarray(img, "RGB")
    return _bg[fx].copy()


TITLE_BAR = 40
_chrome = {}


def window(shot_img, title="claude"):
    """The shot in a Windows-style window: a title bar with the caption buttons, rounded corners
    and a shadow. Returns (RGBA image, shadow RGBA, the shadow's offset)."""
    w, h = shot_img.size
    key = (w, h, title)
    if key not in _chrome:
        fw, fh = w, h + TITLE_BAR
        frame = Image.new("RGBA", (fw, fh), (0, 0, 0, 0))
        d = ImageDraw.Draw(frame)
        d.rounded_rectangle((0, 0, fw - 1, fh - 1), 12, fill=(22, 24, 30, 255), outline=(62, 66, 78, 255))
        d.rounded_rectangle((1, 1, fw - 2, TITLE_BAR + 10), 11, fill=(32, 34, 42, 255))
        d.rectangle((1, TITLE_BAR - 2, fw - 2, TITLE_BAR + 10), fill=(22, 24, 30, 255))
        d.line((1, TITLE_BAR - 1, fw - 2, TITLE_BAR - 1), fill=(50, 53, 64, 255))
        f = font(REG, 15)
        d.text((16, TITLE_BAR / 2), "Termurion", font=font(BOLD, 15), fill=(200, 204, 214, 255), anchor="lm")
        d.text((fw / 2, TITLE_BAR / 2), title, font=f, fill=(150, 154, 166, 255), anchor="mm")
        cx, cy, g = fw - 24, TITLE_BAR / 2, (170, 174, 186, 255)
        d.line((cx - 6, cy - 6, cx + 6, cy + 6), fill=g, width=1)
        d.line((cx - 6, cy + 6, cx + 6, cy - 6), fill=g, width=1)
        d.rectangle((cx - 52, cy - 6, cx - 40, cy + 6), outline=g)
        d.line((cx - 92, cy, cx - 80, cy), fill=g)
        mask = Image.new("L", (fw, fh), 0)
        ImageDraw.Draw(mask).rounded_rectangle((0, 0, fw - 1, fh - 1), 12, fill=255)
        pad = 60
        shadow = Image.new("RGBA", (fw + 2 * pad, fh + 2 * pad), (0, 0, 0, 0))
        ImageDraw.Draw(shadow).rounded_rectangle((pad, pad + 14, pad + fw, pad + fh + 14), 14, fill=(0, 0, 0, 170))
        shadow = shadow.filter(ImageFilter.GaussianBlur(26))
        _chrome[key] = (frame, mask, shadow, pad)
    frame, mask, shadow, pad = _chrome[key]
    out = frame.copy()
    out.paste(shot_img, (0, TITLE_BAR))
    out.putalpha(mask)
    return out, shadow, pad


def place_window(canvas, shot_img, cx, cy, scale=1.0, alpha=1.0, dx=0, dy=0, title="claude"):
    win, shadow, pad = window(shot_img, title)
    if scale != 1.0:
        win = win.resize((round(win.width * scale), round(win.height * scale)), Image.BILINEAR)
        shadow = shadow.resize((round(shadow.width * scale), round(shadow.height * scale)), Image.BILINEAR)
        pad = round(pad * scale)
    if alpha < 1.0:
        a = win.getchannel("A").point(lambda v: int(v * alpha))
        win.putalpha(a)
        shadow.putalpha(shadow.getchannel("A").point(lambda v: int(v * alpha)))
    x, y = round(cx - win.width / 2 + dx), round(cy - win.height / 2 + dy)
    canvas.alpha_composite(shadow, (x - pad, y - pad))
    canvas.alpha_composite(win, (x, y))
    return x, y, win.width, win.height


def glow_text(canvas, xy, text, fnt, fill, anchor="ls", glow=0.8, radius=18, alpha=1.0):
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.text(xy, text, font=fnt, fill=fill + (int(255 * alpha),), anchor=anchor)
    if glow > 0:
        g = layer.filter(ImageFilter.GaussianBlur(radius))
        g.putalpha(g.getchannel("A").point(lambda v: min(255, int(v * glow * 1.6))))
        canvas.alpha_composite(g)
    canvas.alpha_composite(layer)


def text(canvas, xy, s, fnt, fill, anchor="ls", alpha=1.0):
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ImageDraw.Draw(layer).text(xy, s, font=fnt, fill=fill + (int(255 * alpha),), anchor=anchor)
    canvas.alpha_composite(layer)


def chip(canvas, x, y, label, note, fx, alpha=1.0):
    """A small label over the window: a dot in the effect's color, the moment, a note."""
    d = ImageDraw.Draw(canvas)
    c = color(fx)
    d.ellipse((x, y - 5, x + 9, y + 4), fill=c + (int(255 * alpha),))
    text(canvas, (x + 18, y), label, font(BLACK, 17), (235, 236, 240), "lm", alpha)
    lw = font(BLACK, 17).getlength(label)
    text(canvas, (x + 30 + lw, y), note, font(REG, 17), (150, 154, 166), "lm", alpha)


def wordmark(canvas, alpha=0.75):
    text(canvas, (W - 48, 52), "TERMURION", font(BLACK, 22), (220, 222, 230), "rm", alpha)


def caption(canvas, fx, x, y, size=104, alpha=1.0, slide=0.0):
    name = fx.upper()
    glow_text(canvas, (x + slide, y), name, font(BLACK, size), color(fx), "ls", 0.9, 20, alpha)
    text(canvas, (x + 4 + slide * 0.6, y + 42), sb.FX[fx][1], font(REG, 30), (196, 200, 210), "ls", alpha)


# ---------------------------------------------------------------- shots and cues
def frame_of(shot, f):
    f = max(0, min(f, next(s["frames"] for s in sb.SHOTS if s["id"] == shot) - 1))
    return Image.open(SHOTS / shot / f"f{f:05d}.png").convert("RGB")


def shot_cues(shot):
    return [c for c in CUES["cues"] if c["shot"] == shot]


def video_cues():
    out = []
    for sc in sb.SCENES:
        t0 = sc["at"] / FPS
        if sc["kind"] == "tile":
            continue
        cs = shot_cues(sc["shot"])
        if sc["kind"] == "grow":
            keys = [c for c in cs if c["type"] == "key"][::3]
            cs = keys + [c for c in cs if c["type"] != "key"]
        for c in cs:
            out.append(dict(t=round(t0 + c["f"] / FPS, 4), type=c["type"], fx=c["fx"]))
        if sc["kind"] in ("hero", "montage", "grow"):
            out.append(dict(t=t0, type="trans", fx=sc["effect"]))
        if sc["kind"] == "portrait":
            out.append(dict(t=t0, type="cut", fx=sc["effect"]))
    tiles = [sc for sc in sb.SCENES if sc["kind"] == "tile"]
    t0 = tiles[0]["at"] / FPS
    for k, sc in enumerate(tiles):
        out.append(dict(t=round(t0 + k * 8 / FPS, 4), type="tile", fx=sc["effect"]))
    for c in shot_cues(tiles[0]["shot"]):
        if c["type"] in ("enter", "done"):
            out.append(dict(t=round(t0 + c["f"] / FPS, 4), type=c["type"], fx="tiles"))
    return sorted(out, key=lambda c: c["t"])


def since(shot, kind, f):
    """Frames since the shot's last cue of `kind` at or before f (None before the first)."""
    fs = [c["f"] for c in shot_cues(shot) if c["type"] == kind and c["f"] <= f]
    return f - max(fs) if fs else None


def punch(shot, f, fx):
    """The camera on Enter and done: a short zoom, and a shake for the loud effects."""
    s, dx, dy = 1.0, 0.0, 0.0
    for kind, amount in (("enter", 0.035), ("done", 0.018)):
        n = since(shot, kind, f)
        if n is not None and n < 40:
            e = math.exp(-n / 9)
            s += amount * e
            if fx in ("lightning", "glitch", "mecha", "shockwave", "fireworks") and kind == "enter":
                dx += math.sin(n * 2.1) * 9 * e
                dy += math.cos(n * 2.9) * 5 * e
    return s, dx, dy


# ---------------------------------------------------------------- scenes
def render(vf):
    sc = next(s for s in reversed(sb.SCENES) if s["at"] <= vf)
    f = vf - sc["at"]
    kind, fx, shot = sc["kind"], sc["effect"], sc["shot"]
    canvas = background(fx).convert("RGBA")
    enter_in = ease(f / 8)
    if kind == "portrait":
        place_window(canvas, frame_of(shot, f), W / 2, H * 0.40, 1.0 + 0.02 * f / sb.BAR, enter_in, dx=(1 - enter_in) * 90)
        caption(canvas, fx, 150, 870, 150, enter_in, (1 - enter_in) * -60)
        wordmark(canvas)
    elif kind == "title":
        s, dx, dy = punch(shot, f, fx)
        place_window(canvas, frame_of(shot, f), W / 2, H * 0.62, s, 1.0, dx, dy)
        u = ease(f / 14)
        glow_text(canvas, (W / 2, 250), "TERMURION", font(BLACK, int(150 + 30 * (1 - u))), (240, 242, 248), "ms", 0.6, 24, u)
        text(canvas, (W / 2, 320), "cursor effects, built in", font(REG, 44), (190, 196, 210), "ms", u)
    elif kind in ("hero", "montage", "grow"):
        s, dx, dy = punch(shot, f, fx)
        cy = H * 0.44 if kind != "grow" else H * 0.50
        x, y, ww, hh = place_window(canvas, frame_of(shot, f), W / 2, cy, s, enter_in, dx + (1 - enter_in) * 140, dy,
                                    "claude" if kind != "grow" else "claude  (a long prompt)")
        e, d = since(shot, "enter", f), since(shot, "done", f)
        if kind == "grow":
            glow_text(canvas, (W / 2, 150), "Long prompt? The input box grows.", font(BLACK, 56), (240, 242, 248), "ms", 0.4, 18, ease(f / 12))
            text(canvas, (W / 2, 205), "The effects follow both of its lines.", font(REG, 34), (190, 196, 210), "ms", ease((f - 10) / 12))
            caption(canvas, fx, x, y + hh + 120, 96, ease((f - 20) / 10))
        else:
            label = ("DONE", "the answer is in, from the edges") if d is not None else ("ENTER", "the prompt is sent") if e is not None else ("TYPING", "every key plays")
            chip(canvas, x + 4, y - 26, label[0], label[1], fx, enter_in)
            caption(canvas, fx, x, y + hh + 120, 104 if kind == "hero" else 96, enter_in, (1 - enter_in) * -50)
        wordmark(canvas)
    elif kind == "tile":
        tiles = [s for s in sb.SCENES if s["kind"] == "tile" and s["at"] == sc["at"]]
        glow_text(canvas, (W / 2, 132), "Thirteen effects, built in", font(BLACK, 56), (240, 242, 248), "ms", 0.4, 18, ease(f / 10))
        text(canvas, (W / 2, 184), "Settings > Profiles > Window > Effects", font(MONO, 30), (159, 208, 254), "ms", ease((f - 6) / 10))
        gap, rows = 26, [tiles[0:5], tiles[5:10], tiles[10:13]]
        sw, sh = SIZES[tiles[0]["shot"]]
        fit = min(1.0, (W - 120 - 4 * gap) / (5 * sw))
        tw, th = round(sw * fit), round(sh * fit)
        for r, row in enumerate(rows):
            total = len(row) * tw + (len(row) - 1) * gap
            for k, t in enumerate(row):
                idx = r * 5 + k
                u = ease((f - idx * 8) / 10)
                if u <= 0:
                    continue
                s, _, _ = punch(t["shot"], f, t["effect"])
                cx = (W - total) / 2 + k * (tw + gap) + tw / 2
                cy = 290 + r * (th + 92) + th / 2
                img = frame_of(t["shot"], f).convert("RGBA")
                sz = (round(tw * (0.9 + 0.1 * u) * s), round(th * (0.9 + 0.1 * u) * s))
                img = img.resize(sz, Image.LANCZOS)
                img.putalpha(int(255 * u))
                canvas.alpha_composite(img, (round(cx - sz[0] / 2), round(cy - sz[1] / 2)))
                ImageDraw.Draw(canvas).rectangle((round(cx - sz[0] / 2), round(cy - sz[1] / 2), round(cx + sz[0] / 2), round(cy + sz[1] / 2)), outline=(60, 64, 76, int(255 * u)))
                text(canvas, (cx - tw / 2, cy + th / 2 + 30), t["effect"].capitalize(), font(BOLD, 22), color(t["effect"]), "ls", u)
        wordmark(canvas)
    elif kind == "outro":
        if f < 2 * sb.BAR:
            place_window(canvas, frame_of(shot, f), W / 2, H * 0.78, 0.9, ease(f / 10))
            glow_text(canvas, (W / 2, 190), "Nothing to install on the host", font(BLACK, 60), (240, 242, 248), "ms", 0.4, 18, ease(f / 10))
            lines = ["Enter and done looks for Claude Code", "Termurion makes the signals itself",
                     "Works over ssh, in WSL and in tmux", "Write your own in GLSL or WGSL"]
            for k, l in enumerate(lines):
                u = ease((f - 20 - k * 30) / 10)
                text(canvas, (W / 2, 290 + k * 62), l, font(REG, 38), (210, 214, 224), "ms", u)
        else:
            g = f - 2 * sb.BAR
            u = ease(g / 14)
            glow_text(canvas, (W / 2, 430), "TERMURION", font(BLACK, 170), (240, 242, 248), "ms", 0.6, 26, u)
            text(canvas, (W / 2, 500), "iTerm2 for Windows. Free and open source.", font(REG, 42), (190, 196, 210), "ms", u)
            text(canvas, (W / 2, 600), "github.com/datbird/termurion", font(MONO, 44), (159, 208, 254), "ms", ease((g - 12) / 12))
            text(canvas, (W / 2, 900), "Cursor effects from ghostty-cursor-fx by AONIR (MIT). Music adapted from its promo.",
                 font(REG, 24), (130, 134, 146), "ms", ease((g - 24) / 12))
            text(canvas, (W / 2, 936), "Shader uniforms follow Ghostty's (MIT).", font(REG, 24), (130, 134, 146), "ms", ease((g - 24) / 12))
    out = canvas.convert("RGB")
    fade = min(1.0, vf / 12, (sb.END - 1 - vf) / 30)
    if fade < 1.0:
        out = Image.eval(out, lambda v: int(v * max(0.0, fade)))
    return out


def main():
    mode = sys.argv[2]
    if mode == "cues":
        (SHOTS / "cues-video.json").write_text(json.dumps(video_cues(), indent=1))
        print(len(video_cues()), "cues")
    elif mode == "thumb":
        render(round(float(sys.argv[3]) * FPS)).resize((1280, 720), Image.LANCZOS).save(SHOTS / "thumbnail.png")
    elif mode == "still":
        for t in sys.argv[3:]:
            render(round(float(t) * FPS)).save(SHOTS / f"still-{float(t):05.2f}.png")
    elif mode == "video":
        ff = os.environ.get("FFMPEG", "ffmpeg")
        pic = SHOTS / "picture.mp4"
        p = subprocess.Popen([ff, "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS),
                              "-i", "-", "-c:v", "libx264", "-preset", "slow", "-crf", "16", "-pix_fmt", "yuv420p", str(pic)], stdin=subprocess.PIPE)
        for vf in range(sb.END):
            p.stdin.write(render(vf).tobytes())
            if vf % 300 == 0:
                print(f"frame {vf}/{sb.END}", flush=True)
        p.stdin.close()
        p.wait()
        final = SHOTS / "termurion-promo-1080p.mp4"
        subprocess.check_call([ff, "-y", "-loglevel", "error", "-i", str(pic), "-i", str(SHOTS / "music.wav"), "-c:v", "copy",
                               "-c:a", "aac", "-b:a", "320k", "-shortest", "-movflags", "+faststart", str(final)])
        print("wrote", final)


if __name__ == "__main__":
    main()
