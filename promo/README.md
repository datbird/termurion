# Promo video

A 58-second, 1080p60 promo of the 13 built-in cursor effects, with music.
The terminal frames come from Termurion itself: its scene, its renderer and the real effect
shaders. Python adds only the window frame, the captions, the camera and the cuts.

Run these on the Windows build machine (they need its GPU, Python with `uv`, ffmpeg, Segoe UI
and Cascadia Mono). `OUT` is any empty folder.

```sh
python promo/storyboard.py OUT OUT/shots.json         # the shot list
TERMURION_PROMO=OUT/shots.json cargo test --release --lib render_promo_shots -- --ignored
uv run --with pillow,numpy python promo/cut.py OUT cues                      # cues for the music
uv run --with numpy,scipy,pedalboard,soundfile,pyloudnorm python promo/music.py OUT
uv run --with pillow,numpy python promo/cut.py OUT still 13.0 27.0           # check moments
FFMPEG=ffmpeg uv run --with pillow,numpy python promo/cut.py OUT video       # OUT/termurion-promo-1080p.mp4
```

- `storyboard.py`: every shot and where it sits on the music's grid (`grid.json`, 120 BPM).
- `cut.py`: the 1920x1080 frames, the cues (keystrokes, Enter, done, cuts), the encode.
- `music.py`: the soundtrack, generated (no samples). Adapted from
  [ghostty-cursor-fx](https://github.com/Avarce/ghostty-cursor-fx)'s `promo/music/compose.py`
  by AONIR (MIT). Its synthesis, sound design and mix are theirs; the key, chords, melody, form
  and cue mapping are this video's. This video's look also follows their showcase cut.
