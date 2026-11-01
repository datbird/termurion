#!/usr/bin/env python3
"""The promo's music: an original synthwave track (A minor, 120 BPM), synthesized from scratch,
written to the video's grid (grid.json) with its sound effects placed from the cut's cues
(out/cues-video.json, written by cut.py). Everything is generated (no samples, no loops), so the
track can be published anywhere.

Adapted from ghostty-cursor-fx's promo/music/compose.py by AONIR (MIT license, see
../resources/render_plugins/LICENSE): the synthesis, the sound design and the mix are theirs;
the key, chords, melody, form and cue mapping are this video's.

  uv run --with numpy,scipy,pedalboard,soundfile,pyloudnorm python promo/music.py <out dir>
  -> <out dir>/music.wav (48 kHz, 24-bit, -14 LUFS, true peak under -1 dBTP)

Form (bars of 2 s): intro 0-3, drop 4-11 (Am F C G twice), break 12-13 (F G), drop 14-24 with
the lead (Am F C G twice, Am F G), outro 25-28 (Am F G Am).
"""
import json
import pathlib
import re
import sys

import numpy as np
import pedalboard as pb
import pyloudnorm as pyln
import soundfile as sf
from scipy import signal

HERE = pathlib.Path(__file__).resolve().parent
OUT = pathlib.Path(sys.argv[1])
SR = 48000
rng = np.random.default_rng(20261031)

GRID = json.loads((HERE / "grid.json").read_text())
BEAT = 60 / GRID["bpm"]
BAR = BEAT * GRID["beatsPerBar"]
S16 = BEAT / 4
DROP1, BRK, DROP2, OUTRO, END = GRID["drop1"], GRID["break"], GRID["drop2"], GRID["outro"], GRID["end"]
LEN = int((END + 0.5) * SR)
CUES = json.loads((OUT / "cues-video.json").read_text())


def bar(n, beats=0.0):
    return n * BAR + beats * BEAT


def mtof(m):
    return 440.0 * 2 ** ((m - 69) / 12)


def tt(n):
    return np.arange(n) / SR


# ---------------------------------------------------------------- oscillators, envelopes, filters
def _phase(freq, n, ph0=0.0):
    f = np.broadcast_to(np.asarray(freq, dtype=float), (n,))
    dt = f / SR
    return (ph0 + np.cumsum(dt)) % 1.0, dt


def _blep(ph, dt):
    y = np.zeros_like(ph)
    m = ph < dt
    x = ph[m] / dt[m]
    y[m] = x + x - x * x - 1
    m = ph > 1 - dt
    x = (ph[m] - 1) / dt[m]
    y[m] = x * x + x + x + 1
    return y


def saw(freq, n, ph0=0.0):          # band-limited (polyBLEP)
    ph, dt = _phase(freq, n, ph0)
    return 2 * ph - 1 - _blep(ph, dt)


def pulse(freq, n, pw=0.5, ph0=0.0):
    return 0.5 * (saw(freq, n, ph0) - saw(freq, n, (ph0 + pw) % 1.0))


def sine(freq, n, ph0=0.0):
    ph, _ = _phase(freq, n, ph0)
    return np.sin(2 * np.pi * ph)


def noise(n):
    return rng.standard_normal(n)


def adsr(gate, a=0.005, d=0.2, s=0.7, r=0.2, n=None):
    """Attack, exponential decay to s, release after gate; n samples long (zeros past the release)."""
    n = int((gate + r) * SR) if n is None else n
    t = tt(n)
    e = np.minimum(t / a, 1.0) if a > 0 else np.ones(n)
    e = e * (s + (1 - s) * np.exp(-np.maximum(t - a, 0) / d))
    return e * np.where(t > gate, np.exp(-(t - gate) / (r / 4)), 1.0) * (t < gate + r)


def decay(n, tau, a=0.001):
    t = tt(n)
    return np.minimum(t / a, 1.0) * np.exp(-t / tau)


def filt(y, kind, f, order=2):
    if kind == "bp":
        sos = signal.butter(order, [f[0], min(f[1], SR * 0.45)], "bandpass", fs=SR, output="sos")
    else:
        sos = signal.butter(order, min(f, SR * 0.45), kind, fs=SR, output="sos")
    return signal.sosfilt(sos, y, axis=-1)


def swept(y, kind, f_of_t, block=256, width=1.6, order=2):
    """A filter whose frequency follows f_of_t(seconds into y), per block."""
    out = np.zeros_like(y)
    zi = None
    for s in range(0, len(y), block):
        f = float(f_of_t(s / SR))
        if kind == "bp":
            sos = signal.butter(order, [max(20, f / width), min(f * width, SR * 0.45)], "bandpass", fs=SR, output="sos")
        else:
            sos = signal.butter(order, min(max(f, 20), SR * 0.45), kind, fs=SR, output="sos")
        if zi is None:
            zi = np.zeros((sos.shape[0], 2))
        out[s:s + block], zi = signal.sosfilt(sos, y[s:s + block], zi=zi)
    return out


def ladder(x, cutoff, res=0.15, drive=1.0, block=512):
    """Moog-style 24 dB low-pass on a stereo buffer; cutoff(seconds) is automated per block."""
    f = pb.LadderFilter(mode=pb.LadderFilter.Mode.LPF24, cutoff_hz=1000.0, resonance=res, drive=drive)
    x = x.astype(np.float32)
    out = np.zeros_like(x)
    for s in range(0, x.shape[1], block):
        f.cutoff_hz = float(min(cutoff(s / SR), 18000.0))
        out[:, s:s + block] = f.process(x[:, s:s + block], SR, reset=False)
    return out


def norm(y, p=1.0):
    """Scale to a peak of p, so a sound's gain below is its peak level."""
    return y * (p / (np.max(np.abs(y)) + 1e-9))


def smooth(x):
    x = np.clip(x, 0, 1)
    return x * x * (3 - 2 * x)


# ---------------------------------------------------------------- tracks
class Track:
    def __init__(self):
        self.x = np.zeros((2, LEN))

    def add(self, t, y, gain=1.0, pan=0.0):
        if y.ndim == 1:
            a = (pan + 1) * np.pi / 4
            y = np.stack([y * np.cos(a), y * np.sin(a)]) * np.sqrt(2)
        i = int(round(t * SR))
        j0, j1 = max(0, -i), min(y.shape[1], LEN - i)
        if j1 > j0:
            self.x[:, i + j0:i + j1] += gain * y[:, j0:j1]


# ---------------------------------------------------------------- the song's map
AM = dict(pad=[57, 60, 64, 69], root=33, arp=[69, 72, 76, 81, 84, 88])
F = dict(pad=[57, 60, 65, 69], root=29, arp=[69, 72, 77, 81, 84, 89])
C = dict(pad=[55, 60, 64, 67], root=36, arp=[67, 72, 76, 79, 84, 88])
G = dict(pad=[55, 59, 62, 67], root=31, arp=[67, 71, 74, 79, 83, 86])
CHORDS = [AM, F, C, G] + [AM, F, C, G] * 2 + [F, G] + [AM, F, C, G] * 2 + [AM, F, G] + [AM, F, G, AM]
assert len(CHORDS) == round(END / BAR)
chord_at = lambda t: CHORDS[min(len(CHORDS) - 1, int(t // BAR))]


def section(t):
    if t < DROP1: return "intro"
    if t < BRK: return "drop1"
    if t < DROP2: return "break"
    if t < OUTRO: return "drop2"
    return "outro"


GAPS = [(DROP1 - S16 * 2, DROP1), (DROP2 - S16 * 2, DROP2), (OUTRO - S16 * 2, OUTRO)]   # silence before each hit

# ---------------------------------------------------------------- drums
def kick(tail=0.32):
    n = int(0.6 * SR)
    t = tt(n)
    f = 46 + 130 * np.exp(-t / 0.032)
    body = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / tail)
    click = filt(noise(n), "highpass", 1800) * np.exp(-t / 0.002) * 0.35
    y = np.tanh(1.6 * (body + click))
    return y * np.minimum(1, (n - np.arange(n)) / (0.02 * SR))


def clap():
    n = int(0.45 * SR)
    t = tt(n)
    env = sum(np.where(t >= o, np.exp(-(t - o) / 0.005), 0) for o in (0.0, 0.010, 0.020)) * 0.6
    env = env + np.where(t >= 0.03, np.exp(-(t - 0.03) / 0.13), 0)
    y = filt(noise(n), "bp", (850, 2800)) * env * 1.6
    y += np.sin(2 * np.pi * 185 * t) * np.exp(-t / 0.045) * 0.35
    return y


def snare(bright=1.0):
    n = int(0.25 * SR)
    t = tt(n)
    y = filt(noise(n), "bp", (1200, 7000 * bright)) * np.exp(-t / 0.09)
    return y + np.sin(2 * np.pi * 205 * t) * np.exp(-t / 0.04) * 0.5


METAL = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0]


def hat(open_=False, crush=False):
    n = int((0.45 if open_ else 0.12) * SR)
    t = tt(n)
    m = sum(np.sign(np.sin(2 * np.pi * f * 1.7 * t + rng.random() * 6.28)) for f in METAL)
    y = filt(0.55 * m + noise(n), "bp", (7000, 15000)) * decay(n, 0.13 if open_ else 0.028)
    if crush:
        y = np.round(y * 6) / 6
        y = np.repeat(y[::6], 6)[:n]
    return y


def crash(length=2.4):
    n = int(length * SR)
    t = tt(n)
    m = sum(np.sign(np.sin(2 * np.pi * f * 2.3 * t + rng.random() * 6.28)) for f in METAL)
    y = filt(0.4 * m + noise(n), "highpass", 4200) * decay(n, length / 4.5, 0.002)
    return np.stack([y, np.roll(y, 211)])


# ---------------------------------------------------------------- instruments
def supersaw(notes, dur, a=0.4, r=1.2, detune=0.2, voices=7):
    n = int((dur + r) * SR)
    env = adsr(dur, a=a, d=1.2, s=0.85, r=r, n=n)
    L = np.zeros(n)
    R = np.zeros(n)
    for m in notes:
        for o in np.linspace(-1, 1, voices):
            v = saw(mtof(m) * 2 ** (o * detune / 12), n, rng.random())
            ang = (o * 0.85 + 1) * np.pi / 4
            L += v * np.cos(ang)
            R += v * np.sin(ang)
    k = 1 / (len(notes) * np.sqrt(voices))
    return np.stack([L, R]) * env * k


def pluck(m, gate):
    n = int((gate + 0.12) * SR)
    f = mtof(m)
    v = 0.6 * pulse(f, n, 0.5, rng.random()) + 0.4 * saw(f * 1.004, n, rng.random())
    return v * adsr(gate, a=0.002, d=0.07, s=0.2, r=0.12, n=n)


def lead_note(m, dur, prev=None, glide=0.045):
    n = int((dur + 0.3) * SR)
    t = tt(n)
    f1 = mtof(m)
    f = f1 if prev is None else mtof(prev) * (f1 / mtof(prev)) ** np.minimum(t / glide, 1)
    f = f * (1 + 0.005 * np.sin(2 * np.pi * 5.6 * t) * np.clip((t - 0.2) / 0.25, 0, 1))
    v = saw(f, n, rng.random()) + saw(f * 1.007, n, rng.random()) * 0.8 + pulse(f / 2, n) * 0.5
    return v * adsr(dur, a=0.006, d=0.35, s=0.7, r=0.25, n=n)


# ---------------------------------------------------------------- write the parts
kick_t, drums, sub, reese, pad, arp, lead, fx, sfx = [], Track(), Track(), Track(), Track(), Track(), Track(), Track(), Track()
send_verb, send_delay = Track(), Track()
K = kick()
KH = kick(0.42)
CL, SN = clap(), snare()


def add_kick(t, g=1.0, k=None):
    drums.add(t, k if k is not None else K, 0.85 * g)
    kick_t.append(t)


for b in range(round(END / BAR)):
    t0 = bar(b)
    sec = section(t0 + 0.01)
    ch = CHORDS[b]
    full = sec in ("drop1", "drop2")
    # --- drums
    if full:
        for q in range(4):
            tq = bar(b, q)
            if any(a <= tq < z for a, z in GAPS): continue
            if BRK - 1.0 <= tq < BRK or OUTRO - 1.0 <= tq < OUTRO: continue           # the stutter, the last roll
            add_kick(tq, k=KH if q == 0 else None)
            if q in (1, 3): drums.add(tq, CL, 0.5); send_verb.add(tq, CL, 0.14)
        crush = GRID["glitch"] <= t0 < GRID["glitch"] + 4.0                  # the glitch chapter
        for s in range(16):
            ts = bar(b, s / 4)
            if any(a <= ts < z for a, z in GAPS) or BRK - 1.0 <= ts < BRK or OUTRO - 1.0 <= ts < OUTRO: continue
            if s % 4 == 2:
                drums.add(ts, hat(True, crush), 0.1, pan=0.15)
            else:
                drums.add(ts, hat(False, crush), (0.11 if s % 2 else 0.065) * (1.0 + 0.15 * rng.random()), pan=-0.2)
    elif sec == "intro" and b >= 2:
        beats = [0, 1, 2, 3] if b == 2 else [0, 1]
        for q in beats:
            add_kick(bar(b, q), 0.8)
            if q in (1, 3): drums.add(bar(b, q), CL, 0.3)
        for s in range(16 if b == 2 else 8):
            drums.add(bar(b, s / 4), hat(False), 0.045 + 0.02 * (s % 2), pan=-0.2)
    elif sec == "outro" and b < round(END / BAR) - 2:
        for q in (0, 2):
            add_kick(bar(b, q), 0.75)
        drums.add(bar(b, 2), CL, 0.28); send_verb.add(bar(b, 2), CL, 0.2)
    elif sec == "break" and b == 13:
        for s in range(8):
            drums.add(bar(b, s / 2), hat(False), 0.04, pan=-0.2)
    # --- bass
    root = ch["root"]
    if full:
        gaps = [(a, z) for a, z in GAPS] + [(BRK - 1.0, BRK), (OUTRO - 1.0, OUTRO)]
        n_sub = int(BAR * SR)
        if not any(a <= t0 < z for a, z in gaps):
            sub.add(t0, sine(mtof(root), n_sub) * adsr(BAR - 0.02, a=0.003, d=1, s=1, r=0.02, n=n_sub), 0.45)
        for s in range(16):
            if s % 4 == 0: continue                                         # the kick's 16th
            ts = bar(b, s / 4)
            if any(a <= ts < z for a, z in gaps): continue
            m = root + 12 + (12 if s in (7, 15) else 0)
            n = int(S16 * 1.1 * SR)
            v = (saw(mtof(m) * 0.996, n, rng.random()) + saw(mtof(m) * 1.004, n, rng.random())) * adsr(S16 * 0.85, a=0.002, d=0.06, s=0.6, r=0.03, n=n)
            reese.add(ts, v, 0.3)
    elif sec in ("intro", "break", "outro"):
        if not (sec == "intro" and b < 1):
            n = int((BAR + 0.3) * SR)
            g = 0.22 if sec == "intro" else 0.3 if sec == "break" else 0.38
            sub.add(t0, sine(mtof(root), n) * adsr(BAR, a=0.08, d=1, s=1, r=0.3, n=n), g)
    # --- pad (one chord a bar)
    pad.add(t0, supersaw(ch["pad"], BAR, a=0.5 if sec in ("intro", "break") else 0.05), 1.0)
    # --- arp: 16ths over the chord's tones
    order = [0, 2, 1, 3, 2, 4, 3, 5, 4, 2, 3, 1, 2, 0, 1, 3]
    for s in range(16):
        ts = bar(b, s / 4)
        if any(a <= ts < z for a, z in GAPS) or BRK - 1.0 <= ts < BRK: continue
        if sec == "intro" and b == 0 and s % 2: continue                     # the first bar on 8ths
        if sec == "outro" and b == round(END / BAR) - 1 and s > 8: continue
        m = ch["arp"][order[s]]
        arp.add(ts, pluck(m, S16 * 0.8), 1.0, pan=(-0.35 if s % 2 else 0.35))

# --- the lead in the second drop: a two-bar call and answer over Em C G D, sung twice
PHRASE = [
    [(0, 3, 76), (3, 3, 72), (6, 4, 81), (10, 2, 79), (12, 4, 76)],                # Am
    [(0, 3, 77), (3, 3, 76), (6, 4, 72), (10, 2, 74), (12, 4, 76)],                # F
    [(0, 2, 79), (2, 2, 76), (4, 4, 84), (8, 4, 83), (12, 4, 79)],                 # C
    [(0, 6, 79), (6, 6, 83), (12, 4, 86)],                                         # G
]
LAST = [(0, 4, 74), (4, 4, 79), (8, 4, 83), (12, 4, 86)]                           # G, rising
MELODY = PHRASE + PHRASE[:3] + [LAST] + [[(0, 12, 81)], [], []]                    # bars 14-24: ends on a high A
prev = None
for k, notes in enumerate(MELODY):
    for s, ln, m in notes:
        t = bar(14 + k, s / 4)
        dur = ln * S16 * 0.92
        v = lead_note(m, dur, prev)
        lead.add(t, v, 1.0)
        prev = m

# ---------------------------------------------------------------- effects: build-ups, hits
def riser(dur, f0=350, f1=9000):
    n = int(dur * SR)
    y = swept(noise(n), "bp", lambda s: f0 * (f1 / f0) ** (s / dur), width=1.5) * (tt(n) / dur) ** 2
    tone = saw(mtof(52) * 2 ** (2 * tt(n) / dur), n) * (tt(n) / dur) ** 3
    return norm(y * 1.4 + filt(tone, "lowpass", 3000) * 0.25)


def impact():
    n = int(2.5 * SR)
    t = tt(n)
    boom = np.sin(2 * np.pi * np.cumsum(32 + 30 * np.exp(-t / 0.12)) / SR) * np.exp(-t / 0.9)
    thud = filt(noise(n), "lowpass", 400) * np.exp(-t / 0.08) * 0.8
    return norm(np.tanh(1.3 * (boom + thud)))


def whoosh(dur=0.42):
    n = int(dur * SR)
    u = tt(n) / dur
    y = swept(noise(n), "bp", lambda s: 500 * (5000 / 500) ** (np.sin(np.pi * s / dur)), width=1.8)
    return norm(y * np.sin(np.pi * u) ** 2)


for t0, dur in [(4.0, DROP1 - S16 * 2 - 4.0), (bar(13), DROP2 - S16 * 2 - bar(13)), (bar(23, 2), OUTRO - S16 * 2 - bar(23, 2))]:
    r = riser(dur)
    fx.add(t0, r, 0.3)
    send_verb.add(t0, r, 0.15)
for t in (DROP1, DROP2, OUTRO):
    fx.add(t, impact(), 0.7)
    send_verb.add(t, impact(), 0.25)
for t, g in [(DROP1, 0.32), (bar(8), 0.24), (DROP2, 0.32), (bar(18), 0.24), (bar(19), 0.2), (bar(22), 0.24)]:
    drums.add(t, crash(), g)
    send_verb.add(t, crash()[0], g * 0.4)
for t in (DROP1, DROP2):                                                   # reverse cymbal into each drop
    c = crash(1.2)[:, ::-1]
    fx.add(t - c.shape[1] / SR, c, 0.22)
# snare rolls into the drops and the outro: 8ths, 16ths, 32nds, louder and brighter
for a, z in [(bar(3), DROP1 - S16 * 2), (bar(13, 2), DROP2 - S16 * 2), (bar(23, 2), OUTRO - S16 * 2)]:
    t = a
    while t < z - 1e-6:
        u = (t - a) / (z - a)
        step = S16 * 2 if u < 0.4 else S16 if u < 0.8 else S16 / 2
        drums.add(t, snare(0.6 + 0.6 * u), 0.12 + 0.25 * u * u, pan=0.1)
        send_verb.add(t, snare(), 0.06)
        t += step
# fills at the end of phrases
for a in (bar(7, 3), bar(17, 3), bar(21, 3)):
    for k in range(4):
        drums.add(a + k * S16, snare(1.0), 0.16 + 0.05 * k)

# ---------------------------------------------------------------- sound design from the cues
def key_click():
    n = int(0.03 * SR)
    t = tt(n)
    y = filt(noise(n), "bp", (2200, 9000)) * np.exp(-t / 0.0035)
    y += np.sin(2 * np.pi * (2900 + 600 * rng.random()) * t) * np.exp(-t / 0.004) * 0.4
    return norm(y)


def zap():                                    # lightning: a falling zap and crackle
    n = int(0.7 * SR)
    t = tt(n)
    z = saw(2400 * np.exp(-t / 0.05) + 140, n) * np.exp(-t / 0.1)
    crack = norm(filt((rng.random(n) > 0.994) * noise(n), "bp", (1500, 7000))) * np.exp(-t / 0.3)
    return norm(norm(filt(z, "lowpass", 6000)) * 0.7 + crack * 0.5)


def chirps(dur=1.2):                          # cyber: data chirps
    n = int(dur * SR)
    y = np.zeros(n)
    scale = [76, 79, 81, 83, 86, 88, 91, 93, 95]
    step = S16 / 2
    for k in range(int(dur / step)):
        if rng.random() < 0.35: continue
        m = scale[rng.integers(len(scale))]
        ln = int(0.028 * SR)
        i = int(k * step * SR)
        seg = pulse(mtof(m), ln, 0.25) * decay(ln, 0.01) * (1 - k * step / dur)
        y[i:i + ln] += seg[:max(0, min(ln, n - i))]
    return norm(filt(y, "lowpass", 7000))


def glitch_burst(dur=0.45):                   # glitch: crushed noise and blips, gated on 32nds
    n = int(dur * SR)
    t = tt(n)
    g = (np.floor(t / (S16 / 2)) % 2 == 0) * np.exp(-t / 0.2)
    y = np.round(noise(n) * 3) / 3 * 0.3 + pulse(1200 + 800 * np.floor(t / (S16 / 2) % 3), n, 0.3) * 0.4
    y = np.repeat(y[::8], 8)[:n]
    return norm(filt(y * g, "bp", (300, 9000)))


def ignite(dur=1.0):                          # mecha: an energy blade igniting, then its hum
    n = int(dur * SR)
    t = tt(n)
    f = 85 + 70 * smooth(t / 0.25)
    hum = (saw(f, n) + 0.6 * pulse(f * 1.5, n)) * (1 + 0.25 * np.sin(2 * np.pi * 9 * t))
    hum = filt(hum, "lowpass", 1400) * np.minimum(t / 0.03, 1) * np.exp(-t / 0.7)
    sizzle = filt(noise(n), "highpass", 5000) * np.exp(-t / 0.15) * 0.3
    return norm(hum * 0.6 + sizzle)


def clash():                                  # mecha done: the blades meet
    n = int(0.9 * SR)
    t = tt(n)
    y = sum(np.sin(2 * np.pi * f * t) * np.exp(-t / d) for f, d in [(620, 0.5), (1390, 0.35), (2210, 0.25), (3170, 0.15)])
    return norm(y * 0.25 + filt(noise(n), "bp", (2000, 9000)) * np.exp(-t / 0.05))


def buzz(dur=0.9):                            # neon: a tube's buzz flickering on
    n = int(dur * SR)
    t = tt(n)
    on = np.where(t < 0.04, 1, np.where(t < 0.08, 0.1, np.where(t < 0.12, 1, np.where(t < 0.16, 0.3, 1))))
    y = filt(pulse(120, n, 0.2), "bp", (200, 3000)) * on * np.exp(-t / 0.5)
    return norm(y * 0.6)


def thrust(dur=0.16):                         # mecha typing: a short thruster puff
    n = int(dur * SR)
    return norm(filt(noise(n), "bp", (400, 3500)) * decay(n, dur / 3, 0.004))


def crackle(dur=0.9):                         # embers
    n = int(dur * SR)
    t = tt(n)
    y = filt(noise(n), "lowpass", 600) * np.exp(-t / 0.4) * 0.3
    y = norm(y) * 0.5 + norm(filt((rng.random(n) > 0.996) * noise(n), "bp", (2000, 8000))) * np.exp(-t / 0.5)
    return norm(y)


def stab(ch, dur=0.35):                       # the montage's chord stabs
    s = supersaw([m + 12 for m in ch["pad"]], dur, a=0.002, r=0.25, detune=0.25, voices=5)
    return ladder(s, lambda u: 6000 * np.exp(-u / 0.1) + 600, res=0.1)


def converge(dur=0.35):                       # done: in from both edges (L and R swell to the middle)
    n = int(dur * SR)
    u = tt(n) / dur
    y = swept(noise(n), "bp", lambda s: 1200 * (6000 / 1200) ** (s / dur), width=1.7) * u ** 2
    pan = 1 - u
    return norm(np.stack([y * (0.5 + 0.5 * pan), np.roll(y, 37) * (0.5 + 0.5 * pan)]))


SWEET = {"lightning": (zap, 0.38), "cyber": (chirps, 0.24), "glitch": (glitch_burst, 0.3), "mecha": (ignite, 0.4),
         "neon": (buzz, 0.3), "embers": (crackle, 0.3), "panel": (chirps, 0.12)}
blip_scale = [64, 67, 71, 74, 76, 79, 83, 86, 88, 91, 95, 98, 100]
tile_k = 0
for c in CUES:
    t, ty, fx_ = c["t"], c["type"], c["fx"]
    if ty == "key":
        g = 0.12 if fx_ != "panel" else 0.08
        sfx.add(t + 0.012, key_click(), g * (0.8 + 0.4 * rng.random()), pan=rng.uniform(-0.15, 0.15))
        if fx_ == "mecha":
            sfx.add(t + 0.04, thrust(), 0.16, pan=-0.5)
    elif ty == "back":
        sfx.add(t + 0.012, key_click(), 0.12)
        sfx.add(t + 0.04, thrust(), 0.2, pan=0.6)
    elif ty == "switch":
        sfx.add(t, key_click(), 0.16)
        sfx.add(t + 0.03, key_click(), 0.1)
    elif ty == "enter":
        on = t + 0.04                                   # the effect starts SETTLE after the signal
        if fx_ in SWEET:
            f, g = SWEET[fx_]
            y = f()
            sfx.add(on, y, g)
            send_verb.add(on, y if y.ndim == 1 else y[0], g * 0.4)
        else:                                           # the montage: a chord stab on its beat
            y = stab(chord_at(on))
            sfx.add(on, y, 0.5)
            send_verb.add(on, y[0], 0.2)
    elif ty == "done":
        on = round((t + 0.025) / S16) * S16
        y = converge()
        sfx.add(on - y.shape[1] / SR, y, 0.22)
        if fx_ == "mecha":
            sfx.add(on + 0.35, clash(), 0.25)
            send_verb.add(on + 0.35, clash(), 0.15)
        elif fx_ in SWEET and fx_ != "cyber":
            f, g = SWEET[fx_]
            sfx.add(on, f(), g * 0.7)
    elif ty == "clear":
        y = zap()
        sfx.add(t + 0.12, y, 0.35)
        sfx.add(t + 0.12, whoosh(0.3), 0.2, pan=0.3)
    elif ty == "trans":
        w = whoosh()
        sfx.add(t - 0.21, w, 0.2, pan=-0.2)
        send_verb.add(t - 0.21, w, 0.1)
    elif ty == "tile":
        m = blip_scale[tile_k % len(blip_scale)]
        tile_k += 1
        n = int(0.09 * SR)
        sfx.add(t + 0.04, pulse(mtof(m), n, 0.3) * decay(n, 0.03), 0.12, pan=-0.6 + 1.2 * (tile_k / 13))
        send_delay.add(t + 0.04, pulse(mtof(m), n, 0.3) * decay(n, 0.03), 0.06)
    elif ty == "cut" and t < DROP1 - 1:                                      # the intro portraits
        sfx.add(t, glitch_burst(0.12), 0.18)
        n = int(0.6 * SR)
        sfx.add(t, sine(46, n) * decay(n, 0.25, 0.004), 0.4)

# ---------------------------------------------------------------- mix
kick_t = np.array(sorted(set(round(k, 5) for k in kick_t)))


def duck(depth, rel=0.22):
    t = tt(LEN)
    i = np.searchsorted(kick_t, t, side="right") - 1
    dt = np.where(i >= 0, t - kick_t[np.maximum(i, 0)], 1e3)
    att = 0.004
    return np.where(dt < att, 1 - depth * dt / att, 1 - depth * (1 - smooth((dt - att) / rel)))


def gate():
    g = np.ones(LEN)
    t = tt(LEN)
    for a, z in GAPS:
        g *= 1 - np.clip(np.minimum((t - a) / 0.004, (z - t) / 0.004), 0, 1) * ((t > a) & (t < z))
    return g


TIME = tt(LEN)


def curve(points):
    """A level that follows (seconds, value) points, linear in between."""
    ts, vs = zip(*points)
    return np.interp(TIME, ts, vs)


# the form in levels: a quiet intro that builds, the drops full, a dip in the break, a fading outro
PAD_LEVEL = curve([(0, 0.3), (4, 0.5), (7.75, 0.8), (8, 1.0), (24, 1.0), (24.01, 0.42), (26, 0.48), (27.75, 0.75), (28, 1.0),
                   (OUTRO, 1.0), (OUTRO + 0.01, 0.8), (END, 0.22)])
ARP_LEVEL = curve([(0, 0.32), (7.75, 0.55), (8, 0.33), (24, 0.33), (24.01, 0.42), (28, 0.5), (28.01, 0.4), (OUTRO, 0.4), (OUTRO + 0.01, 0.45), (END, 0.1)])
# pad: opens through the intro, darker in the break, closes in the outro; pumped by the kick
pad_cut = lambda s: (500 * (3200 / 500) ** min(1, s / DROP1) if s < DROP1 else 3200 if s < BRK else 1800 if s < DROP2 else 3600 if s < OUTRO
                     else 3600 * (400 / 3600) ** min(1, (s - OUTRO) / (END - OUTRO)))
padx = ladder(pad.x, pad_cut, res=0.1)
padx = pb.Chorus(rate_hz=0.4, depth=0.25, mix=0.4)(padx.astype(np.float32), SR)
padx = filt(padx, "highpass", 140) * duck(0.55)
padx = padx * PAD_LEVEL
arp_cut = lambda s: (900 * (4200 / 900) ** min(1, s / DROP1) if s < DROP1 else 2600 if s < BRK else 5000 if s < DROP2 else 2400 if s < OUTRO
                     else 3000 * (700 / 3000) ** min(1, (s - OUTRO) / (END - OUTRO)))
arpx = ladder(arp.x, arp_cut, res=0.25) * duck(0.35)
arpx = arpx * ARP_LEVEL
subx = filt(sub.x, "lowpass", 120) * duck(0.8, 0.18)
reesex = ladder(reese.x, lambda s: 900, res=0.2, drive=2.0)
reesex = filt(reesex, "highpass", 70) * duck(0.6, 0.16)
leadx = ladder(lead.x, lambda s: 4200, res=0.2, drive=1.5)
leadx = filt(leadx, "highpass", 200) * 0.2

send_verb.x += padx * 0.25 + arpx * 0.5 + leadx * 0.6
send_delay.x += arpx * 0.35 + leadx * 0.7


def make_ir(sec=2.8, pre=0.02):
    n = int(sec * SR)
    t = tt(n)
    fc = 9000 * (1400 / 9000) ** (t / sec)
    a = np.exp(-2 * np.pi * fc / SR)
    ir = np.zeros((2, n))
    for chn in range(2):
        z = noise(n)
        y = np.empty(n)
        acc = 0.0
        for i in range(n):
            acc = (1 - a[i]) * z[i] + a[i] * acc
            y[i] = acc
        ir[chn] = y * np.exp(-t * 6.9 / sec)
    ir = np.pad(ir, ((0, 0), (int(pre * SR), 0)))
    return ir / np.sqrt(np.sum(ir ** 2) / 2)


IR = make_ir()
verb = np.stack([signal.fftconvolve(send_verb.x[c], IR[c])[:LEN] for c in range(2)])
verb = filt(verb, "highpass", 250) * 0.22


def pingpong(x, d=0.375, fb=0.42, taps=7):
    x = filt(filt(x, "highpass", 400), "lowpass", 5000)
    y = np.zeros_like(x)
    m = 0.5 * (x[0] + x[1])
    for k in range(1, taps + 1):
        i = int(k * d * SR)
        y[(k + 1) % 2, i:] += m[:LEN - i] * fb ** k
        m = filt(m, "lowpass", 4500)
    return y


delay = pingpong(send_delay.x) * 0.3

drumx = pb.Compressor(threshold_db=-14, ratio=2.5, attack_ms=8, release_ms=120)(drums.x.astype(np.float32), SR)
music = padx * 0.55 + arpx * 0.42 + subx * 1.0 + reesex * 0.5 + leadx * 1.0 + verb + delay
mix = (drumx + music) * gate() + fx.x + sfx.x * 1.0

# the glitch chapter's end: a stutter of 32nds, then a tape stop into the break
i0, seg = int((BRK - 1.0) * SR), int(S16 / 2 * SR)
piece = mix[:, i0:i0 + seg].copy()
fade = np.minimum(1, np.minimum(np.arange(seg), seg - 1 - np.arange(seg)) / 48)
for k in range(8):
    j = i0 + k * seg
    mix[:, j:j + seg] = piece * fade * (1 - 0.06 * k)
a, b_ = int((BRK - 0.5) * SR), int(BRK * SR)
src = mix[:, a:a + int(1.0 * SR)].copy()
u = tt(b_ - a) / ((b_ - a) / SR)
pos = (u - u * u / 2) * ((b_ - a) / SR) * SR                                   # rate 1 -> 0
for c in range(2):
    mix[c, a:b_] = np.interp(pos, np.arange(src.shape[1]), src[c]) * (1 - u ** 3)

# ---------------------------------------------------------------- master: glue, limit, -14 LUFS
mix = filt(mix, "highpass", 28)
mix = pb.HighShelfFilter(cutoff_frequency_hz=5000, gain_db=2.5, q=0.7)(mix.astype(np.float32), SR)   # air
mix = pb.Compressor(threshold_db=-10, ratio=1.8, attack_ms=15, release_ms=180)(mix.astype(np.float32), SR)
meter = pyln.Meter(SR)


def brickwall(x, ceiling_db=-1.4, look=0.004, release=0.08):
    """A look-ahead peak limiter: gain falls before a peak arrives (never above the ceiling), recovers slowly."""
    c = 10 ** (ceiling_db / 20)
    L = int(look * SR)
    up = signal.resample_poly(x, 4, 1, axis=1)                 # true peak: the peaks between samples too
    pk = np.max(np.abs(up).reshape(2, -1, 4).max(axis=2), axis=0)[:x.shape[1]]
    pk = np.maximum(pk, np.max(np.abs(x), axis=0))
    need = np.minimum(1.0, c / np.maximum(pk, 1e-9))
    from scipy.ndimage import minimum_filter1d, uniform_filter1d
    g = minimum_filter1d(need, 2 * L + 1, origin=-L)          # the lowest gain needed in the next 2L samples
    g = uniform_filter1d(g, L + 1, origin=-(L // 2))           # a smooth fall that is complete by the peak
    a = np.exp(-1 / (release * SR))
    out = np.empty_like(g)
    acc = 1.0
    for i in range(len(g)):                                    # recover at the release rate, fall at once
        acc = g[i] if g[i] < acc else a * acc + (1 - a) * g[i]
        out[i] = acc
    return x * out


def master(x, gain_db):
    y = x * 10 ** (gain_db / 20)
    return brickwall(y)[:, :int(END * SR)].astype(np.float32)


g = 0.0
for _ in range(3):
    out = master(mix, g)
    lufs = meter.integrated_loudness(out.T.astype(np.float64))
    g += -14.0 - lufs
out = master(mix, g)
lufs = meter.integrated_loudness(out.T.astype(np.float64))
tp = 20 * np.log10(np.max(np.abs(signal.resample_poly(out, 4, 1, axis=1))) + 1e-12)
pre = 20 * np.log10(np.max(np.abs(mix[:, :int(END * SR)])) * 10 ** (g / 20))
limited = np.mean(np.max(np.abs(mix[:, :int(END * SR)]), axis=0) * 10 ** (g / 20) > 10 ** (-1.5 / 20)) * 100
print(f"peak before the limiter {pre:+.1f} dBFS, {limited:.3f} % of samples limited")
if "--debug" in sys.argv:
    pk = np.max(np.abs(mix[:, :int(END * SR)]), axis=0) * 10 ** (g / 20)
    for name, x in [("drums", drumx), ("music", music), ("fx", fx.x), ("sfx", sfx.x)]:
        print(f"  {name}: peak {20 * np.log10(np.max(np.abs(x)) + 1e-9):+.1f} dB at {np.argmax(np.max(np.abs(x), axis=0)) / SR:.3f} s")
    top = np.argsort(pk)[::-1]
    seen = []
    for i in top:
        if all(abs(i - j) > SR * 0.2 for j in seen): seen.append(i)
        if len(seen) == 8: break
    print("  loudest moments:", ", ".join(f"{i / SR:.2f}s {20 * np.log10(pk[i]):+.1f}" for i in seen))
sf.write(OUT / "music.wav", out.T, SR, subtype="PCM_24")
print(f"wrote music.wav: {out.shape[1] / SR:.2f} s, {lufs:.2f} LUFS, true peak {tp:.2f} dBTP, gain {g:+.1f} dB")
# short-term loudness per section (a check that the form reads: drops loudest, break calmer)
for name, a_, z_ in [("intro", 0, DROP1), ("drop1", DROP1, BRK), ("break", BRK, DROP2), ("drop2", DROP2, OUTRO), ("outro", OUTRO, END - 1)]:
    seg = out[:, int(a_ * SR):int(z_ * SR)].T.astype(np.float64)
    print(f"  {name:6s} {meter.integrated_loudness(seg):6.1f} LUFS")
if "--stems" in sys.argv:
    for nm, x in [("drums", drumx), ("pad", padx), ("arp", arpx), ("sub", subx), ("reese", reesex), ("lead", leadx), ("fx", fx.x), ("sfx", sfx.x)]:
        sf.write(HERE / f"stem_{nm}.wav", (x[:, :int(END * SR)] * 0.5).T, SR, subtype="PCM_16")
