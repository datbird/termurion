# Promo video

A 64-second, 1080p60 promo of the cursor effects, with its own music.

The whole video is one Termurion window: the real tab bar, split panes, and Claude Code or a
shell in each pane, rendered by Termurion's scene, renderer and effect shaders. Python moves a
camera over those frames, adds small captions and encodes the result. Nothing else is drawn.

The music is an original chiptune track (D minor, 150 BPM), synthesized from plain waveforms
in `music.py`. Keystrokes fall on sixteenths and every Enter on a downbeat, and each keystroke,
Enter and done plays its own sound from the cue list, so the picture and the track hit together.

Run these on the Windows build machine (they need its GPU, Python with `uv`, ffmpeg and
Cascadia Mono). `OUT` is any empty folder.

```sh
python promo/storyboard.py OUT OUT/shots.json         # the shot list
TERMURION_PROMO=OUT/shots.json cargo test --release --lib render_promo_shots -- --ignored
uv run --with pillow,numpy python promo/cut.py OUT cues                     # cues for the music
uv run --with numpy,scipy,soundfile,pyloudnorm python promo/music.py OUT    # OUT/music.wav
uv run --with pillow,numpy python promo/cut.py OUT still 13.0 33.0          # check moments
FFMPEG=ffmpeg uv run --with pillow,numpy python promo/cut.py OUT video      # OUT/termurion-promo-1080p.mp4
uv run --with pillow,numpy python promo/cut.py OUT thumb 58.5               # OUT/thumbnail.png
```

- `storyboard.py`: every shot (2560x1440) and where it sits on the 150 BPM grid.
- `cut.py`: the camera, the captions, the cues and the encode.
- `music.py`: the soundtrack.

The cursor effects themselves come from
[ghostty-cursor-fx](https://github.com/Avarce/ghostty-cursor-fx) by AONIR (MIT), and the video's
end card credits them.
