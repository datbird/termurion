"""The promo's music: an original chiptune track in D minor at 150 BPM, synthesized here from
plain waveforms (pulse, triangle, noise). No samples and no loops, so it can go anywhere.

The form follows the storyboard's bars (a bar is 1.6 s): an intro on the arpeggio, the first
drop at bar 4, a half-time break at bar 16, the second drop with the lead at bar 20, the finale
at bar 30 and the outro at bar 34. Every keystroke, Enter and done in the video plays a sound
from the cue list cut.py writes, so the effects and the track hit together.

  uv run --with numpy,scipy,soundfile,pyloudnorm python promo/music.py <out dir>
  reads <out dir>/cues-video.json, writes <out dir>/music.wav (48 kHz, 24-bit, -14 LUFS)
"""
import json
import pathlib
import sys

import numpy as np
import pyloudnorm as pyln
import soundfile as sf
from scipy import signal

OUT = pathlib.Path(sys.argv[1])
SR = 48000
BPM = 150
BEAT = 60 / BPM
BAR = 4 * BEAT
S16 = BEAT / 4
BARS = 40
LEN = int((BARS * BAR + 1.0) * SR)
rng = np.random.default_rng(1101)
CUES = json.loads((OUT / "cues-video.json").read_text())

INTRO, DROP1, BREAK, DROP2, FINALE, OUTRO = 0, 4, 16, 20, 30, 34


def section(b):
    if b < DROP1: return "intro"
    if b < BREAK: return "drop1"
    if b < DROP2: return "break"
    if b < FINALE: return "drop2"
    if b < OUTRO: return "finale"
    return "outro"


def at(bar, step=0.0):
    """Seconds at a bar and a sixteenth step in it."""
    return bar * BAR + step * S16


def hz(m):
    return 440.0 * 2 ** ((m - 69) / 12)


def n_of(sec):
    return max(1, int(sec * SR))


def t_of(n):
    return np.arange(n) / SR


# ---------------------------------------------------------------- waveforms
def phase(freq, n):
    f = np.broadcast_to(np.asarray(freq, float), (n,))
    return np.cumsum(f / SR) % 1.0


def pulse(freq, sec, duty=0.5):
    n = n_of(sec)
    return np.where(phase(freq, n) < duty, 1.0, -1.0)


def tri(freq, sec):
    n = n_of(sec)
    p = phase(freq, n)
    return 4 * np.abs(p - 0.5) - 1


def sine(freq, sec):
    n = n_of(sec)
    return np.sin(2 * np.pi * phase(freq, n))


def noise(sec):
    return rng.uniform(-1, 1, n_of(sec))


def env(n, a=0.002, d=0.1, s=0.6, r=0.05, hold=None):
    """Attack, decay to sustain, hold, release (seconds)."""
    t = t_of(n)
    hold = (n / SR) - r if hold is None else hold
    e = np.where(t < a, t / max(a, 1e-6), s + (1 - s) * np.exp(-(t - a) / max(d, 1e-6)))
    rel = np.clip(1 - (t - hold) / max(r, 1e-6), 0, 1)
    return e * np.where(t > hold, rel, 1.0)


def decay(n, tau):
    return np.exp(-t_of(n) / tau)


def lp(y, f):
    b, a = signal.butter(2, min(f, SR * 0.45) / (SR / 2), "low")
    return signal.lfilter(b, a, y)


def hp(y, f):
    b, a = signal.butter(2, f / (SR / 2), "high")
    return signal.lfilter(b, a, y)


def bp(y, lo, hi):
    b, a = signal.butter(2, [lo / (SR / 2), min(hi, SR * 0.45) / (SR / 2)], "band")
    return signal.lfilter(b, a, y)


def crush(y, bits=4, hold=6):
    y = np.repeat(y[::hold], hold)[:len(y)]
    q = 2 ** bits
    return np.round(y * q) / q


class Bus:
    def __init__(self):
        self.x = np.zeros((2, LEN))

    def add(self, t, y, gain=1.0, pan=0.0):
        i = int(round(t * SR))
        if i >= LEN or len(y) == 0:
            return
        y = y[:LEN - i]
        a = (pan + 1) * np.pi / 4
        self.x[0, i:i + len(y)] += gain * y * np.cos(a) * np.sqrt(2)
        self.x[1, i:i + len(y)] += gain * y * np.sin(a) * np.sqrt(2)


drums, bass, arp, lead, pad, sfx = Bus(), Bus(), Bus(), Bus(), Bus(), Bus()

# ---------------------------------------------------------------- instruments
def kick():
    n = n_of(0.28)
    f = 45 + 140 * np.exp(-t_of(n) / 0.03)
    return sine(f, 0.28) * decay(n, 0.09) + 0.3 * tri(f * 0.5, 0.28) * decay(n, 0.02)


def snare():
    n = n_of(0.2)
    return 0.8 * hp(noise(0.2), 900) * decay(n, 0.06) + 0.4 * tri(185, 0.2) * decay(n, 0.04)


def hat(open_=False):
    sec = 0.18 if open_ else 0.035
    n = n_of(sec)
    return hp(noise(sec), 7000) * decay(n, sec / 3)


def bass_note(m, sec):
    n = n_of(sec)
    return tri(hz(m), sec) * env(n, 0.002, 0.08, 0.8, 0.02)


def arp_note(m, sec):
    n = n_of(sec)
    return pulse(hz(m), sec, 0.125) * env(n, 0.001, 0.05, 0.35, 0.02)


def lead_note(m, sec):
    n = n_of(sec)
    t = t_of(n)
    vib = 1 + 0.006 * np.sin(2 * np.pi * 5.5 * t) * np.clip((t - 0.12) / 0.1, 0, 1)
    return pulse(hz(m) * vib, sec, 0.25) * env(n, 0.004, 0.15, 0.7, 0.04)


def pad_chord(ms, sec):
    n = n_of(sec)
    y = sum(lp(pulse(hz(m) * (1 + d), sec, 0.5), 1400) for m in ms for d in (-0.003, 0.003))
    return y / (2 * len(ms)) * env(n, 0.25, 0.6, 0.8, 0.4)


# D minor: i iv VI V. Each chord: (bass root, arp tones, pad tones).
DM = (38, [62, 65, 69, 74, 77, 81], [62, 65, 69])
GM = (43, [62, 67, 70, 74, 79, 82], [62, 67, 70])
BB = (34, [62, 65, 70, 74, 77, 82], [62, 65, 70])
A = (33, [61, 64, 69, 73, 76, 81], [61, 64, 69])
PROG = [DM, GM, BB, A]
chord = lambda b: PROG[b % 4] if b < OUTRO else [DM, BB, A, DM][(b - OUTRO) % 4]

# The lead: (sixteenth, length in sixteenths, note), one list a bar.
PHRASE = [
    [(0, 4, 74), (4, 2, 77), (6, 4, 81), (10, 2, 79), (12, 2, 77), (14, 2, 76)],
    [(0, 4, 74), (4, 4, 79), (8, 4, 82), (12, 4, 81)],
    [(0, 6, 77), (6, 2, 74), (8, 2, 77), (10, 2, 82), (12, 4, 81)],
    [(0, 4, 76), (4, 4, 73), (8, 2, 76), (10, 6, 81)],
]
ANSWER = PHRASE[:3] + [[(0, 4, 76), (4, 4, 79), (8, 8, 81)]]
LAST = [[(0, 16, 86)], [], [], []]
GAPS = [(at(DROP1) - S16 * 2, at(DROP1)), (at(DROP2) - S16 * 2, at(DROP2)), (at(FINALE) - S16 * 2, at(FINALE))]
in_gap = lambda t: any(a <= t < z for a, z in GAPS)

for b in range(BARS):
    sec = section(b)
    root, tones, padm = chord(b)
    full = sec in ("drop1", "drop2", "finale")
    # drums
    for s in range(16):
        t = at(b, s)
        if in_gap(t):
            continue
        if full:
            if s % 4 == 0: drums.add(t, kick(), 0.9)
            if s in (4, 12): drums.add(t, snare(), 0.55)
            if s % 2 == 1: drums.add(t, hat(), 0.18, pan=0.3)
            if s in (6, 14): drums.add(t, hat(True), 0.12, pan=-0.3)
            if b in (15, 29) and s >= 8: drums.add(t, snare(), 0.25 + 0.03 * (s - 8))  # fills
        elif sec == "break":
            if s in (0, 10): drums.add(t, kick(), 0.7)
            if s == 8: drums.add(t, snare(), 0.45)
            if b == BREAK + 3 and s >= 4: drums.add(t, snare(), 0.12 + 0.04 * (s - 4))  # the roll in
        elif sec == "intro" and b >= 2:
            if s % 4 == 0: drums.add(t, kick(), 0.6)
            if s % 2 == 1: drums.add(t, hat(), 0.1, pan=0.3)
        elif sec == "outro" and b < BARS - 2:
            if s in (0, 8): drums.add(t, kick(), 0.5)
            if s == 12: drums.add(t, hat(True), 0.08)
    if b in (DROP1, DROP2, FINALE):
        drums.add(at(b), hp(noise(1.6), 3000) * decay(n_of(1.6), 0.5), 0.28)  # the crash
    # bass: eighths, root and octave
    if sec != "intro" or b >= 2:
        for e in range(8):
            t = at(b, e * 2)
            if in_gap(t):
                continue
            if sec == "break" and e % 2:
                continue
            m = root + (12 if e % 2 else 0) if full else root
            bass.add(t, bass_note(m, S16 * (1.8 if full else 3.6)), 0.5)
    # arpeggio: sixteenths up and down the chord
    order = [0, 1, 2, 3, 4, 5, 4, 3, 2, 1, 2, 3, 4, 5, 4, 3]
    for s in range(16):
        t = at(b, s)
        if in_gap(t) or (sec == "break" and s % 2):
            continue
        if sec == "intro" and b < 2 and s % 2:
            continue
        arp.add(t, arp_note(tones[order[s]], S16 * 0.9), 1.0, pan=-0.4 if s % 2 else 0.4)
    # pad in the calm parts
    if sec in ("intro", "break", "outro", "finale"):
        pad.add(at(b), pad_chord(padm, BAR + 0.3), 1.0)

# the lead melody in the second drop and the finale
melody = PHRASE + ANSWER + PHRASE[:2]  # bars 20-29
melody += PHRASE[2:] + LAST[:2]        # bars 30-33
for k, notes in enumerate(melody):
    for s, ln, m in notes:
        t = at(DROP2 + k, s)
        if not in_gap(t):
            lead.add(t, lead_note(m, ln * S16 * 0.92), 1.0)
# the outro: the opening line again, softer, ending on D
for k, notes in enumerate([PHRASE[0], PHRASE[2], [(0, 8, 76), (8, 8, 73)], [(0, 16, 74)]]):
    for s, ln, m in notes:
        lead.add(at(OUTRO + 1 + k, s), lead_note(m, ln * S16 * 0.92), 0.55)

# ---------------------------------------------------------------- the effects' sounds
def sweep(f0, f1, sec, wave=sine):
    n = n_of(sec)
    f = f0 * (f1 / f0) ** (t_of(n) / sec)
    return wave(f, sec)


def mixa(*ys):
    """Sums sounds of different lengths."""
    y = np.zeros(max(len(v) for v in ys))
    for v in ys:
        y[:len(v)] += v
    return y


def chime(base):
    y = np.zeros(n_of(0.6))
    for k, m in enumerate([base, base + 7, base + 12]):
        i = n_of(k * S16 / 2)
        note = tri(hz(m), 0.6)[:len(y) - i] * decay(len(y) - i, 0.18)
        y[i:] += note
    return y / 2


def stab():
    root, tones, _ = DM
    y = sum(pulse(hz(m), 0.3, 0.25) for m in tones[:3]) / 3
    return y * decay(n_of(0.3), 0.09)


SOUNDS = {
    "lightning": lambda: mixa(bp(noise(0.5), 1500, 9000) * decay(n_of(0.5), 0.12), 0.4 * sweep(2400, 200, 0.25, tri) * decay(n_of(0.25), 0.08)),
    "cyber": lambda: np.concatenate([pulse(hz(60 + rng.integers(0, 36)), S16 / 2, 0.25) for _ in range(10)]) * 0.6,
    "glitch": lambda: crush(mixa(np.concatenate([noise(0.03) * (k % 2) for k in range(12)]), 0.5 * pulse(110, 0.36)), 3, 12),
    "neon": lambda: pulse(120, 0.8, 0.5) * (0.6 + 0.4 * np.sin(2 * np.pi * 9 * t_of(n_of(0.8)))) * env(n_of(0.8), 0.04, 0.3, 0.5, 0.3) * 0.5,
    "hologram": lambda: sweep(300, 2400, 0.5) * env(n_of(0.5), 0.05, 0.2, 0.6, 0.15),
    "mecha": lambda: lp(sweep(80, 640, 0.6, lambda f, s: pulse(f, s, 0.5)), 2500) * env(n_of(0.6), 0.01, 0.3, 0.7, 0.2),
    "embers": lambda: np.concatenate([hp(noise(0.012), 2000) * rng.uniform(0.2, 1) if rng.random() < 0.35 else np.zeros(n_of(0.012)) for _ in range(55)]),
    "katana": lambda: mixa(hp(noise(0.4), 5000) * decay(n_of(0.4), 0.05), 0.3 * sine(3520, 0.6) * decay(n_of(0.6), 0.2)),
    "teleport": lambda: np.concatenate([sine(hz(m), S16 / 2) * decay(n_of(S16 / 2), 0.03) for m in (74, 78, 81, 86, 90, 93, 98)]),
    "phosphor": lambda: mixa(0.6 * lp(pulse(60, 0.6, 0.5), 300) * env(n_of(0.6), 0.02, 0.3, 0.6, 0.2), 0.5 * hp(noise(0.02), 3000)),
    "shockwave": lambda: sweep(160, 38, 0.6) * decay(n_of(0.6), 0.25),
    "fireworks": lambda: np.concatenate([sweep(900, 2600, 0.25) * 0.4, hp(noise(0.3), 1500) * decay(n_of(0.3), 0.07)]),
    "aurora": lambda: sum(sine(hz(m), 1.2) for m in (74, 77, 81, 86)) / 4 * env(n_of(1.2), 0.2, 0.5, 0.6, 0.5),
}

for c in CUES:
    t, ty, fx = c["t"], c["type"], c["fx"]
    if ty == "key":
        sfx.add(t, pulse(2093, 0.012, 0.5) * decay(n_of(0.012), 0.004), 0.06, pan=rng.uniform(-0.2, 0.2))
    elif ty == "enter":
        sfx.add(t, stab(), 0.22)
        if fx in SOUNDS:
            sfx.add(t, SOUNDS[fx](), 0.35)
    elif ty == "done":
        sfx.add(t, chime(86), 0.3, pan=0.25)
    elif ty == "roll":
        sfx.add(t, pulse(hz(74 + int(c.get("k", 0)) % 12), S16 * 0.8, 0.125) * decay(n_of(S16 * 0.8), 0.03), 0.18)

# ---------------------------------------------------------------- mix and master
def bus(b, gain):
    return b.x * gain


# a little room: a short stereo delay on the arpeggio and the lead
def delay(x, sec=BEAT * 0.75, fb=0.35, taps=4):
    y = np.zeros_like(x)
    d = n_of(sec)
    for k in range(1, taps + 1):
        y[(k % 2), k * d:] += lp(x[(k + 1) % 2, :LEN - k * d], 4000) * fb ** k
    return y


# the arpeggio opens up through the intro
cut = np.interp(np.arange(LEN) / SR, [0, at(DROP1), at(OUTRO), at(BARS)], [700, 6000, 6000, 1200])
arpx = arp.x.copy()
for c in range(2):
    blocks = np.array_split(np.arange(LEN), 200)
    out = np.zeros(LEN)
    for idx in blocks:
        out[idx] = lp(arpx[c, max(0, idx[0] - 2000):idx[-1] + 1], cut[idx[0]])[-len(idx):]
    arpx[c] = out
leadx = lead.x
mix = (bus(drums, 1.0) + bus(bass, 0.9) + arpx * 0.22 + leadx * 0.26 + bus(pad, 0.35) + bus(sfx, 1.0)
       + delay(arpx * 0.22 + leadx * 0.26) * 0.8)
mix = hp(mix, 30)
# fade the very end
end = int(BARS * BAR * SR)
mix[:, end - n_of(1.2):end] *= np.linspace(1, 0, n_of(1.2))
mix = mix[:, :end]

meter = pyln.Meter(SR)
gain = 1.0
for _ in range(4):
    y = np.tanh(mix * gain * 1.1) / 1.1
    lufs = meter.integrated_loudness(y.T)
    gain *= 10 ** ((-14.0 - lufs) / 20)
y = np.tanh(mix * gain * 1.1) / 1.1
peak = 20 * np.log10(np.max(np.abs(signal.resample_poly(y, 4, 1, axis=1))))
if peak > -1.0:
    y *= 10 ** ((-1.0 - peak) / 20)
sf.write(OUT / "music.wav", y.T.astype(np.float32), SR, subtype="PCM_24")
print(f"wrote music.wav: {y.shape[1] / SR:.1f} s, {meter.integrated_loudness(y.T):.1f} LUFS, peak {peak:.1f} dBFS")
