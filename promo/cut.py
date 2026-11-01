"""The promo's cut: a camera over Termurion's rendered window (2560x1440 shots from
render_promo_shots) down to 1920x1080 at 60 fps, small typed captions, the cues for the music,
then the encode with ffmpeg.

  python promo/cut.py <shots dir> cues        -> <shots dir>/cues-video.json (for music.py)
  python promo/cut.py <shots dir> still 12.5  -> <shots dir>/still-12.50.png
  python promo/cut.py <shots dir> video       -> <shots dir>/termurion-promo-1080p.mp4

Needs Pillow and numpy, ffmpeg (FFMPEG or PATH) and the Cascadia Mono font.
"""
import json
import math
import os
import pathlib
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import storyboard as sb  # noqa: E402

OW, OH, FPS = 1920, 1080, sb.FPS
SHOTS = pathlib.Path(sys.argv[1])
CUES = json.loads((SHOTS / "cues.json").read_text())["cues"]
FONTS = pathlib.Path(os.environ.get("WINDIR", "C:/Windows")) / "Fonts"
MONO = ImageFont.truetype(str(FONTS / "CascadiaMono.ttf"), 34)
LOUD = {"lightning", "glitch", "mecha", "shockwave", "fireworks"}


def hexrgb(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def scene_at(vf):
    return next(s for s in reversed(sb.SCENES) if s["at"] <= vf)


def shot_cues(sc):
    return [c for c in CUES if c["shot"] == sc["shot"] and sc["start"] <= c["f"] < sc["start"] + sc["frames"]]


def video_cues():
    out = []
    for sc in sb.SCENES:
        t0 = sc["at"] / FPS
        if sc["shot"].startswith("roll_"):
            k = sb.ALL.index(sc["shot"][5:])
            out.append(dict(t=round(t0, 4), type="roll", fx=sc["shot"][5:], k=k))
            continue
        for c in shot_cues(sc):
            out.append(dict(t=round(t0 + (c["f"] - sc["start"]) / FPS, 4), type=c["type"], fx=c["fx"]))
    return sorted(out, key=lambda c: c["t"])


def camera(sc, f):
    """Zoom and the crop's height (0 top, 1 bottom) from the scene's keys, eased between them,
    plus a short push and shake on Enter."""
    keys = sc["camera"]
    z, y = keys[0][1], keys[0][2]
    for (fa, za, ya), (fb, zb, yb) in zip(keys, keys[1:]):
        if fa <= f < fb:
            u = (f - fa) / max(1, fb - fa)
            u = u * u * (3 - 2 * u)
            z, y = za + (zb - za) * u, ya + (yb - ya) * u
            break
        if f >= fb:
            z, y = zb, yb
    dx = dy = 0.0
    for c in shot_cues(sc):
        n = f + sc["start"] - c["f"]
        if c["type"] == "enter" and 0 <= n < 40:
            e = math.exp(-n / 8)
            z *= 1 + 0.035 * e
            if c["fx"] in LOUD:
                dx += math.sin(n * 2.3) * 10 * e
                dy += math.cos(n * 3.1) * 6 * e
    return z, y, dx, dy


def caption_now(sc, f):
    live = [c for c in sc["captions"] if c[0] <= f]
    return (f - live[-1][0], live[-1][1], live[-1][2]) if live else None


def draw_caption(img, age, fx, text):
    """A small terminal-style label at the top right: the effect's name in its color, then a
    comment. It types itself on, a character a frame."""
    head = (fx + "  ") if fx else ""
    body = "// " + text
    full = head + body
    shown = full[:max(0, age)]
    if not shown:
        return
    pad, x1, y0 = 22, OW - 56, 92
    width = MONO.getlength(full)
    x0 = x1 - width - 2 * pad - 20
    a = min(1.0, age / 6)
    layer = Image.new("RGBA", (OW, OH), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.rounded_rectangle((x0, y0, x1, y0 + 64), 10, fill=(10, 11, 15, int(200 * a)), outline=(70, 74, 86, int(255 * a)))
    x = x0 + pad
    color = hexrgb(sb.FX[fx][0]) if fx else (159, 208, 254)
    if shown[:len(head)]:
        d.text((x, y0 + 32), shown[:len(head)], font=MONO, fill=color + (int(255 * a),), anchor="lm")
        x += MONO.getlength(shown[:len(head)])
    rest = shown[len(head):]
    if rest:
        d.text((x, y0 + 32), rest, font=MONO, fill=(214, 218, 228, int(255 * a)), anchor="lm")
        x += MONO.getlength(rest)
    if len(shown) < len(full) or (age // 20) % 2 == 0:
        d.rectangle((x + 2, y0 + 16, x + 18, y0 + 48), fill=color + (int(220 * a),))
    img.alpha_composite(layer)


def render(vf):
    sc = scene_at(vf)
    f = vf - sc["at"]
    last = next(s["frames"] for s in sb.SHOTS if s["id"] == sc["shot"]) - 1
    src = Image.open(SHOTS / sc["shot"] / f"f{min(sc['start'] + f, last):05d}.png").convert("RGB")
    z, fy, dx, dy = camera(sc, f)
    cw, ch = sb.W / z, sb.H / z
    x0 = dx  # terminal text starts at the left, so the camera keeps the left edge
    y0 = fy * (sb.H - ch) + dy
    x0, y0 = max(0, min(sb.W - cw, x0)), max(0, min(sb.H - ch, y0))
    img = src.resize((OW, OH), Image.BICUBIC, box=(x0, y0, x0 + cw, y0 + ch)).convert("RGBA")
    cap = caption_now(sc, f)
    if cap:
        draw_caption(img, *cap)
    out = img.convert("RGB")
    # a flash on the drops, a fade in and out
    if sc["at"] in (sb.BAR * 4, sb.BAR * 20, sb.BAR * 30) and f < 10:
        out = Image.blend(out, Image.new("RGB", out.size, (255, 255, 255)), 0.22 * (1 - f / 10))
    fade = min(1.0, vf / 12, (sb.END - 1 - vf) / 48)
    if fade < 1.0:
        out = Image.eval(out, lambda v: int(v * max(0.0, fade)))
    return out


def main():
    mode = sys.argv[2]
    if mode == "cues":
        cues = video_cues()
        (SHOTS / "cues-video.json").write_text(json.dumps(cues, indent=1))
        print(len(cues), "cues")
    elif mode == "still":
        for t in sys.argv[3:]:
            render(round(float(t) * FPS)).save(SHOTS / f"still-{float(t):05.2f}.png")
    elif mode == "thumb":
        render(round(float(sys.argv[3]) * FPS)).resize((1280, 720), Image.LANCZOS).save(SHOTS / "thumbnail.png")
    elif mode == "video":
        ff = os.environ.get("FFMPEG", "ffmpeg")
        pic = SHOTS / "picture.mp4"
        p = subprocess.Popen([ff, "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{OW}x{OH}", "-r", str(FPS),
                              "-i", "-", "-c:v", "libx264", "-preset", "slow", "-crf", "16", "-pix_fmt", "yuv420p", str(pic)], stdin=subprocess.PIPE)
        for vf in range(sb.END):
            p.stdin.write(render(vf).tobytes())
            if vf % 480 == 0:
                print(f"frame {vf}/{sb.END}", flush=True)
        p.stdin.close()
        p.wait()
        final = SHOTS / "termurion-promo-1080p.mp4"
        subprocess.check_call([ff, "-y", "-loglevel", "error", "-i", str(pic), "-i", str(SHOTS / "music.wav"), "-c:v", "copy",
                               "-c:a", "aac", "-b:a", "320k", "-shortest", "-movflags", "+faststart", str(final)])
        print("wrote", final)


if __name__ == "__main__":
    main()
