"""The promo's storyboard: every shot Termurion renders (shots.json, for the ignored test
render_promo_shots) and where each one sits in the video (cut.py reads SCENES). 60 fps, on the
music's grid (grid.json: 120 BPM, a bar is 2 s = 120 frames)."""
import json
import pathlib
import sys

FPS = 60
BAR = 120
HERE = pathlib.Path(__file__).resolve().parent

# Each effect's cursor color (its plugin.toml) and a one-line caption.
FX = {
    "lightning": ("#9fd0fe", "arcs of current crawl around the cursor"),
    "cyber": ("#50e080", "glyph rain in netrunner green"),
    "glitch": ("#f85452", "a corrupted signal that tears"),
    "neon": ("#ff5ed2", "a pink neon tube that breathes"),
    "mecha": ("#bcc8dc", "armor plates and energy blades"),
    "embers": ("#ff8a3c", "glowing embers rise from the lines"),
    "katana": ("#b8c4d4", "a blade cuts the lines in two"),
    "phosphor": ("#6be894", "an old green screen beam"),
    "shockwave": ("#b69cfe", "a pulse runs along the lines"),
    "teleport": ("#3fd8c8", "the lines sparkle and beam out"),
    "fireworks": ("#ffc856", "a fuse burns and rockets launch"),
    "aurora": ("#7fe0c0", "curtains of northern light"),
    "hologram": ("#4cc8f0", "a scan and a wireframe grid"),
}

HISTORY = ["\x1b[38;5;252m●\x1b[0m Updated the effects and the README.", "  └ 2 files changed, 41 insertions(+)"]
SHOTS = []
SCENES = []   # (video frame, shot id, frames, kind, effect, caption)


def shot(id, effect, frames, cols=80, rows=9, px=32, status=True, history=HISTORY, events=(), switches=None):
    SHOTS.append(dict(id=id, effect=effect, frames=frames, cols=cols, rows=rows, px=px, status=status,
                      history=list(history), events=list(events), switches=switches or {}))
    return id


def place(at, id, frames, kind, effect=None, caption=None):
    SCENES.append(dict(at=at, shot=id, frames=frames, kind=kind, effect=effect, caption=caption))


# Intro, 0-8 s: four living cursors, close up.
for k, fx in enumerate(["lightning", "aurora", "neon", "hologram"]):
    events = [dict(f=50, t="type", text="hi", every=14)] if fx in ("lightning", "neon") else []
    place(k * BAR, shot(f"p_{fx}", fx, BAR, cols=13, rows=3, px=84, status=False, history=[], events=events), BAR, "portrait", fx)

# 8 s: the drop and the title over a Lightning Enter.
place(4 * BAR, shot("title", "lightning", BAR, events=[dict(f=0, t="set", text="make the cursor effects awesome"), dict(f=0, t="enter")]),
      BAR, "title", "lightning")

# 10-22 s: three heroes, two bars each: typing, Enter on the downbeat, done a bar and a half later.
HEROES = [("lightning", "fix the login bug", "Fixed: the session cookie expired early."),
          ("cyber", "decrypt the logs", "Found 3 warnings, no errors."),
          ("glitch", "why is the build flaky", "A race in the GPU setup. Fixed.")]
for k, (fx, prompt, answer) in enumerate(HEROES):
    n = 2 * BAR
    start = BAR - 8 - 5 * len(prompt)
    ev = [dict(f=start, t="type", text=prompt, every=5), dict(f=BAR, t="enter"), dict(f=BAR + 90, t="done", answer=answer)]
    place(5 * BAR + k * n, shot(f"h_{fx}", fx, n, events=ev), n, "hero", fx)

# 22-24 s: Neon's Enter on the bar, into the break.
place(11 * BAR, shot("neon_drop", "neon", BAR, events=[dict(f=0, t="set", text="make the logo glow"), dict(f=0, t="enter")]), BAR, "montage", "neon")

# 24-30 s: the break. A long prompt grows the input box; Mecha's Enter lands on the second drop.
LONG = ("clean up the old branches, then write a short note for the team about which ones went and "
        "why, and keep anything that has an open pull request")
ev = [dict(f=12, t="type", text=LONG, every=1), dict(f=2 * BAR, t="enter"), dict(f=2 * BAR + 90, t="done", answer="Cleaned up 14 merged branches.")]
place(12 * BAR, shot("grow", "mecha", 3 * BAR, cols=60, rows=10, events=ev, switches={"LINE_EFFECT": True}), 3 * BAR, "grow", "mecha")

# 30-46 s: the other eight, a bar each, Enter on beat two.
MONTAGE = ["embers", "katana", "phosphor", "shockwave", "teleport", "fireworks", "aurora", "hologram"]
PROMPTS = ["warm up the cache", "cut the dead code", "show the old terminal", "ping every host",
           "beam me up", "ship it", "make it calm", "draw the architecture"]
for k, (fx, prompt) in enumerate(zip(MONTAGE, PROMPTS)):
    ev = [dict(f=0, t="set", text=prompt[:-4]), dict(f=2, t="type", text=prompt[-4:], every=6), dict(f=30, t="enter")]
    place(15 * BAR + k * BAR, shot(f"m_{fx}", fx, BAR, events=ev), BAR, "montage", fx)

# 46-50 s: all thirteen at once. Tiles arrive on the 16ths, all send on the next bar, done after.
ALL = ["cyber", "lightning", "glitch", "neon", "phosphor", "embers", "shockwave", "katana", "teleport",
       "fireworks", "aurora", "mecha", "hologram"]
for k, fx in enumerate(ALL):
    ev = [dict(f=0, t="set", text="ship it"), dict(f=BAR, t="enter"), dict(f=BAR + 90, t="done", answer="Shipped.")]
    place(23 * BAR, shot(f"t_{fx}", fx, 2 * BAR, cols=44, rows=6, px=14, events=ev), 2 * BAR, "tile", fx)

# 50-58 s: what it is, then the end card, over an idle Aurora.
place(25 * BAR, shot("outro", "aurora", 4 * BAR, cols=60, rows=5, px=30, history=[], status=False,
                     events=[dict(f=40, t="type", text="claude", every=8)]), 4 * BAR, "outro", "aurora")

END = 29 * BAR

if __name__ == "__main__":
    out = sys.argv[1]
    (pathlib.Path(sys.argv[2])).write_text(json.dumps(dict(out=out, shots=SHOTS), indent=1))
    print(len(SHOTS), "shots,", sum(s["frames"] for s in SHOTS), "frames")
