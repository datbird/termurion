"""The promo's storyboard: one Termurion window, full screen, for the whole video. Every shot is
the real window (tab bar, split panes, Claude Code or a shell) rendered by the ignored test
render_promo_shots at 2560x1440 and 60 fps; cut.py moves a camera over it and adds the captions.

The music (music.py) is 150 BPM: a beat is 24 frames, a bar 96, a sixteenth 6. Keys land on
sixteenths and every Enter on a downbeat, so the sound and the picture share one grid.

  python promo/storyboard.py <out dir on the build machine> <shots.json>
"""
import json
import sys

FPS = 60
BEAT = 24
BAR = 96
S16 = 6
W, H, SCALE, PX = 2560, 1440, 2.0, 44

# Each effect's cursor color (its plugin.toml) and a short note for the caption.
FX = {
    "lightning": ("#9fd0fe", "arcs of current"),
    "cyber": ("#50e080", "glyph rain"),
    "glitch": ("#f85452", "a corrupted signal"),
    "neon": ("#ff5ed2", "a breathing tube"),
    "mecha": ("#bcc8dc", "armor and blades"),
    "embers": ("#ff8a3c", "rising embers"),
    "katana": ("#b8c4d4", "a clean cut"),
    "phosphor": ("#6be894", "an old green screen"),
    "shockwave": ("#b69cfe", "a pulse"),
    "teleport": ("#3fd8c8", "beam out"),
    "fireworks": ("#ffc856", "a fuse and rockets"),
    "aurora": ("#7fe0c0", "northern light"),
    "hologram": ("#4cc8f0", "a wireframe scan"),
}

DIM, RESET = "\x1b[38;5;245m", "\x1b[0m"
DOT = "\x1b[38;5;252m●\x1b[0m"


def work(file, old, new, test):
    """A few lines of an earlier exchange, so each screen looks like real work."""
    return [
        f"{DOT} Read {file}",
        f"{DOT} Update({file})",
        f"  {DIM}⎿{RESET}  Updated {file} with 1 addition and 1 removal",
        f"       \x1b[48;5;52m 41 -  {old}\x1b[K{RESET}",
        f"       \x1b[48;5;22m 41 +  {new}\x1b[K{RESET}",
        f"{DOT} Bash({test})",
        f"  {DIM}⎿{RESET}  test result: ok. 48 passed; 0 failed",
        "",
    ]


WELCOME = [f"\x1b[38;5;209m✻{RESET} Welcome to Claude Code", f"  {DIM}/help for help, /status for your current setup{RESET}", ""]
H_AUTH = work("src/auth/session.rs", "let ttl = Duration::minutes(5);", "let ttl = Duration::hours(8);", "cargo test auth")
H_LOGS = work("tools/logs.py", "level = 'debug'", "level = 'info'", "pytest tools")
H_CI = work(".github/workflows/ci.yml", "runs-on: ubuntu-20.04", "runs-on: ubuntu-24.04", "act -j build")
H_UI = work("web/theme.css", "--accent: #888;", "--accent: #ff5ed2;", "npm test")
H_DOCS = work("docs/architecture.md", "## Overview", "## How it fits together", "mdbook build")
H_GIT = work("scripts/prune.sh", "git branch -d", "git branch -D", "bash -n scripts/prune.sh")

# A screen as full as a real session's: the welcome, two earlier changes, then the newest.
FULL = lambda h: WELCOME + H_GIT + H_LOGS + h

SHOTS, SCENES = [], []


def shot(id, frames, panes, events, tabs=("claude",), active=None, focus=0, px=PX):
    tabs = list(tabs)
    SHOTS.append(dict(id=id, frames=frames, px=px, tabs=tabs, active=len(tabs) - 1 if active is None else active,
                      panes=panes, focus=focus, events=events))
    return id


def claude(effect, history, status=True, typed=""):
    return dict(effect=effect, kind="claude", history=history, status=status, typed=typed)


def place(bar, id, frames, captions=(), camera=None, start=0):
    """Puts `frames` of shot `id` (from its frame `start`) at `bar`. Captions are (frame, effect
    or None, text); the camera is (frame, zoom, focus y 0-1) keys."""
    SCENES.append(dict(at=round(bar * BAR), shot=id, frames=frames, start=start, captions=list(captions), camera=camera or [(0, 1.0, 0.5)]))


def typing(text, at, pane=None, every=S16):
    e = dict(f=at, t="type", text=text, every=every)
    if pane is not None:
        e["pane"] = pane
    return e


# 1. Bars 0-3: a shell. "claude" is typed on the eighths, Claude Code starts on bar 2.
id = shot("intro", 4 * BAR, [dict(effect="aurora", kind="shell", prompt="\x1b[38;5;110m~/app\x1b[0m $ ", history=[])],
          [typing("claude", BAR, every=12), dict(f=2 * BAR, t="enter", claude=True, welcome=WELCOME)], tabs=["zsh"])
place(0, id, 4 * BAR, [(24, None, "termurion"), (2 * BAR, "aurora", "cursor effects, built in")],
      camera=[(0, 1.33, 0.12), (2 * BAR - 1, 1.2, 0.2), (2 * BAR, 1.0, 0.5)])

# 2. Bars 4-7, the drop: Lightning. Typed on the sixteenths, Enter on bar 6, done on bar 7.5.
id = shot("drop", 4 * BAR, [claude("lightning", FULL(H_AUTH))],
          [typing("fix the session timeout", 6), dict(f=2 * BAR, t="enter"), dict(f=3 * BAR + 2 * BEAT, t="done", answer="Fixed: sessions last 8 hours now.")])
place(4, id, 4 * BAR, [(0, "lightning", "typing"), (2 * BAR, "lightning", "enter: the prompt is sent"),
                       (3 * BAR + 2 * BEAT, "lightning", "done: the answer is in")],
      camera=[(0, 1.25, 1.0), (2 * BAR - 1, 1.12, 1.0), (2 * BAR, 1.0, 0.7), (4 * BAR, 1.06, 0.8)])

# 3. Bars 8-15: a new tab every two bars, each its own profile and effect.
TABS = [("cyber", "decrypt the error logs", H_LOGS), ("glitch", "why is the build flaky", H_CI),
        ("neon", "make the accent color pop", H_UI), ("hologram", "draw the architecture", H_DOCS)]
ev, tabs = [], ["claude"]
for k, (fx, prompt, hist) in enumerate(TABS):
    f = k * 2 * BAR
    ev += [dict(f=f, t="addtab", title=fx), dict(f=f, t="effect", effect=fx), dict(f=f, t="history", lines=FULL(hist)),
           typing(prompt, f + 12), dict(f=f + BAR, t="enter")]
id = shot("tabs", 8 * BAR, [claude("cyber", [])], ev, tabs=["claude"])
place(8, id, 8 * BAR, [(k * 2 * BAR, fx, FX[fx][1]) for k, (fx, _, _) in enumerate(TABS)],
      camera=sum(([(k * 2 * BAR, 1.15, 1.0), (k * 2 * BAR + BAR - 1, 1.08, 1.0), (k * 2 * BAR + BAR, 1.0, 0.7)] for k in range(4)), []))

# 4. Bars 16-20, the break: a long prompt grows the input box; Enter lands on the second drop.
LONG = ("prune the merged branches, then write a short note for the team about which ones went "
        "and why, and keep anything with an open pull request")
id = shot("grow", 5 * BAR, [claude("mecha", WELCOME + H_CI + H_LOGS + H_GIT)],
          [dict(f=0, t="addtab", title="mecha"), typing(LONG, 12, every=2), dict(f=4 * BAR, t="enter")],
          tabs=["claude", "cyber", "glitch", "neon", "hologram"])
place(16, id, 5 * BAR, [(0, "mecha", "long prompt? the box grows"), (2 * BAR, "mecha", "the effects follow both lines")],
      camera=[(0, 1.0, 0.6), (4 * BAR - 1, 1.3, 1.0), (4 * BAR, 1.0, 0.7)])

# 5. Bars 21-26: three panes, three hosts. Focus moves every two bars and the window's effect
# follows the focused pane's profile.
HOSTS = [("embers", "\x1b[38;5;110m$\x1b[0m ssh devbox", "tune the cache"),
         ("katana", "\x1b[38;5;110m$\x1b[0m wsl", "drop dead code"),
         ("teleport", "\x1b[38;5;110m$\x1b[0m tmux attach", "deploy staging")]
panes = [claude(fx, [cmd, ""] + WELCOME + H_AUTH + H_DOCS, status=False) for fx, cmd, _ in HOSTS]
ev = []
for k, (fx, _, prompt) in enumerate(HOSTS):
    f = k * 2 * BAR
    ev += [dict(f=f, t="focus", pane=k), typing(prompt, f + 6, pane=k), dict(f=f + BAR, t="enter", pane=k),
           dict(f=f + BAR + 90, t="done", pane=k, answer="Done.")]
id = shot("split", 6 * BAR, panes, ev, tabs=["claude", "cyber", "glitch", "neon", "hologram", "mecha", "ssh / wsl / tmux"])
place(21, id, 6 * BAR, [(0, "embers", "ssh"), (2 * BAR, "katana", "wsl"), (4 * BAR, "teleport", "tmux"),
                        (BAR, None, "nothing to install on the host")])

# 6. Bars 27-28: four more, half a bar each. Bar 29: all thirteen tabs on the sixteenths.
ALL = ["lightning", "cyber", "glitch", "neon", "hologram", "mecha", "embers", "katana", "teleport",
       "phosphor", "shockwave", "fireworks", "aurora"]
for k, fx in enumerate(["phosphor", "shockwave", "fireworks", "aurora"]):
    id = shot(f"quick_{fx}", BAR // 2, [claude(fx, FULL(H_DOCS), typed="ship it")], [dict(f=0, t="enter")],
              tabs=ALL[:10 + k], active=9 + k)
    place(27 + k / 2, id, BAR // 2, [(0, fx, FX[fx][1])], camera=[(0, 1.12, 1.0)])
for k, fx in enumerate(ALL):
    id = shot(f"roll_{fx}", 40, [claude(fx, FULL(H_AUTH), typed="go")], [dict(f=0, t="enter")], tabs=ALL, active=k)
    place(29 + k * S16 / BAR, id, S16, camera=[(0, 1.18, 1.0)], start=6)

# 7. Bars 30-33: one window, thirteen effects, a last Enter.
id = shot("finale", 4 * BAR, [claude("lightning", FULL(H_AUTH), typed="ship it")],
          [dict(f=0, t="enter"), dict(f=BAR + 2 * BEAT, t="done", answer="Shipped.")], tabs=ALL, active=0)
place(30, id, 4 * BAR, [(0, None, "13 effects, built in"), (2 * BAR, None, "settings > profiles > window > effects")],
      camera=[(0, 1.0, 0.7), (4 * BAR, 1.1, 0.9)])

# 8. Bars 34-39: the end card is the shell's own output.
LOGO = [
    "█████ ████ ████  █   █ █  █ ████  █  ███  █   █",
    "  █   █    █   █ ██ ██ █  █ █   █ █ █   █ ██  █",
    "  █   ███  ████  █ █ █ █  █ ████  █ █   █ █ █ █",
    "  █   █    █  █  █   █ █  █ █  █  █ █   █ █  ██",
    "  █   ████ █   █ █   █  ██  █   █ █  ███  █   █",
]
OUT = [""] + ["  \x1b[38;5;153m" + l + RESET for l in LOGO] + [
    "",
    "  iTerm2 for Windows. Free and open source.",
    "  \x1b[38;5;117mgithub.com/datbird/termurion\x1b[0m",
    "",
    f"  {DIM}cursor effects: ghostty-cursor-fx by AONIR (MIT){RESET}",
    f"  {DIM}music: made for this video{RESET}",
    "",
]
id = shot("end", 6 * BAR, [dict(effect="aurora", kind="shell", prompt="\x1b[38;5;110m~/app\x1b[0m $ ", history=[])],
          [typing("termurion --about", 6), dict(f=BAR + 2 * BEAT, t="enter", out=[]),
           dict(f=BAR + 2 * BEAT + 2, t="print", lines=OUT, every=3)], tabs=["zsh"], px=56)
place(34, id, 6 * BAR, camera=[(0, 1.2, 0.0), (BAR + 2 * BEAT, 1.0, 0.0)])

END = 40 * BAR

if __name__ == "__main__":
    shots_json = dict(out=sys.argv[1], width=W, height=H, scale=SCALE, shots=SHOTS)
    pathlib_path = sys.argv[2]
    with open(pathlib_path, "w") as fh:
        json.dump(shots_json, fh, indent=1)
    print(len(SHOTS), "shots,", sum(s["frames"] for s in SHOTS), "frames,", END / FPS, "s")
