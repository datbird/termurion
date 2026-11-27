# Performance

How fast Termurion draws, how much CPU and memory it uses, and how that compares with other
terminals on the same machine. Every number here comes from the bench scripts in `scripts/`
(`draw-bench` on macOS, `draw-bench-win.ps1` on Windows, `draw-bench-linux` on Linux). No
hardware is named: read each table against the other rows of the same table, not against
another OS or another machine.

The goal of 1.1.12 was simple: an effect should be cheap enough to leave on just for fun.

## How we measure

**The window.** 220 columns by 63 rows (or as many rows as the screen allows), the same font
and size for every terminal (Monaco 12 on macOS), at the same place where the system allows it.

**The workloads.** A small program inside the terminal writes straight to its tty for 20
seconds:

- **idle**: a prompt with a still cursor. Nothing is written.
- **stream20**: 20 new lines a second, each a different line of 60 to 110 characters.
- **claude**: Claude Code's bottom area, redrawn 10 times a second: a spinner row and a 3-row
  input box, with a new line of answer above them every other tick.
- **flood**: a 1 MB block of colored text, written over and over as fast as the terminal reads
  it.

**Passes.** Every run is done twice (2 passes) and the tables show the mean of the two. On
macOS a run is repeated when the terminal lost the front or other processes used more than a
quarter of a core; on Windows and Linux the noise limit is set per machine and stated in each
section.

**The columns.**

- **CPU %**: CPU time of the terminal's processes (the app plus its session host) per second
  of wall time, as a percent of one core. A busy terminal uses several threads, so the flood
  goes above 100%.
- **frames/s**: frames the terminal presented to the screen. Termurion draws only when
  something changed, so plain Termurion shows 0 at idle and about 20 for stream20. A terminal
  running an animated effect draws every frame. On Windows this column needs PresentMon, which
  was not used, so it reads "-".
- **MB/s**: bytes the workload got into the tty per second. Only the flood is limited by the
  terminal; the other workloads write a fixed, small amount.
- **memory MB**: resident memory of the terminal's processes at the end of the run.

**Configs.** "Termurion" is plain Termurion. "Termurion + Lightning" turns on the Lightning
effect at the default "Effects animation" choice (Smooth, up to 60 frames a second). "Ghostty +
Lightning" is Ghostty with the same Lightning shader as a Ghostty custom shader
(`custom-shader-animation = always`), so both terminals run the same effect.

**Noise.** Two runs on the same machine on different days can differ by about 20% (Ghostty's
own idle row went from 5.5% to 4.3% between two sessions on macOS, and its flood from 90 to
122 MB/s). Compare rows from the same table; the history table at the end mixes sessions and
says so.

## macOS

Termurion 1.1.12 draws with native Metal. Measured on a macOS virtual machine (its
paravirtual GPU reports no GPU load, so GPU time is not measured). Source:
`target/draw-bench/final-mac-full` and `target/draw-bench/final-mac-idle`, 2 passes each.

### idle

| Terminal | CPU % | frames/s | MB/s | memory MB |
|---|---|---|---|---|
| Ghostty + Lightning | 4.3 | 60.0 | 0.00 | 133 |
| Termurion | 0.9 | 0.0 | 0.00 | 117 |
| Termurion + Lightning | 3.7 | 59.9 | 0.00 | 123 |

With an effect on, Termurion draws the same 60 frames a second as Ghostty for about 15% less
CPU. The system compositor (WindowServer) also does less work: about 12% of a core for
Termurion + Lightning against about 15% for Ghostty + Lightning.

### stream20

| Terminal | CPU % | frames/s | MB/s | memory MB |
|---|---|---|---|---|
| Ghostty + Lightning | 9.5 | 60.0 | 0.00 | 137 |
| Termurion | 7.0 | 19.8 | 0.00 | 153 |
| Termurion + Lightning | 7.1 | 60.0 | 0.00 | 158 |

### claude

| Terminal | CPU % | frames/s | MB/s | memory MB |
|---|---|---|---|---|
| Ghostty + Lightning | 6.1 | 59.9 | 0.01 | 139 |
| Termurion | 6.4 | 9.9 | 0.01 | 157 |
| Termurion + Lightning | 5.7 | 59.9 | 0.01 | 161 |

Plain Termurion draws only the 10 frames a second that change. With Lightning it draws 60, and
because an animation frame reuses the cells it already built, that row costs less than the
plain one in this run (the two are within noise of each other).

### flood

| Terminal | CPU % | frames/s | MB/s | memory MB |
|---|---|---|---|---|
| Ghostty + Lightning | 244.8 | 60.0 | 122.4 | 190 |
| Termurion | 258.5 | 62.6 | 85.6 | 178 |
| Termurion + Lightning | 258.9 | 63.2 | 85.6 | 182 |

The flood is where Termurion still trails: about 86 MB/s against Ghostty's 122 MB/s. The effect
costs nothing here, and the frame rate holds at the display's pace, so the gap is in reading
and parsing output, not in drawing. It is the next performance item in the
[roadmap](ROADMAP.md).

### Every effect at idle

All 13 built-in effects at an idle prompt, Effects animation Smooth. Same session as above
(`target/draw-bench/final-mac-idle`), 2 passes. Teleport's first pass ran while other processes
used 23.6% of a core and read 13.5% at 49 frames/s; its row is pass 2 only.

| Config | CPU % | frames/s | memory MB |
|---|---|---|---|
| Ghostty + Lightning | 4.4 | 60.0 | 133 |
| Termurion | 0.9 | 0.0 | 115 |
| Termurion + Aurora | 3.5 | 60.0 | 120 |
| Termurion + Cyber | 3.6 | 60.0 | 125 |
| Termurion + Embers | 4.0 | 59.9 | 122 |
| Termurion + Fireworks | 3.5 | 60.0 | 123 |
| Termurion + Glitch | 3.6 | 60.0 | 123 |
| Termurion + Hologram | 3.3 | 60.0 | 123 |
| Termurion + Katana | 3.5 | 59.9 | 123 |
| Termurion + Lightning | 3.5 | 59.9 | 123 |
| Termurion + Mecha | 3.5 | 60.0 | 123 |
| Termurion + Neon | 3.5 | 60.0 | 123 |
| Termurion + Phosphor | 3.6 | 60.0 | 122 |
| Termurion + Shockwave | 3.8 | 60.0 | 122 |
| Termurion + Teleport (pass 2) | 3.6 | 59.9 | 122 |

Every effect costs 3.3 to 4.0% of a core at 60 frames a second, which is about 0.6 ms of CPU
per animation frame. Ghostty's frame costs about 0.7 ms. In 1.1.11 a Termurion animation frame
cost about 3.2 ms.

### How 1.1.12 got there (effect at idle, macOS)

Each step measured on the same machine with the same bench. The steps are from different
sessions, so allow for the noise above.

| Build | Renderer | Effects animation | Effect idle CPU % | frames/s | Ghostty + Lightning, same session |
|---|---|---|---|---|---|
| 1.1.11 | wgpu | Balanced (30 cap) | 8.4 to 9.6 (13 effects) | 27.4 to 27.9 | 5.5 at 60 |
| 1.1.11-era renderer, new Smooth pacing (Phase A) | wgpu | Smooth (60 cap) | 9.9 to 12.1 (3 effects) | 60.0 | 4.3 at 60 |
| Native Metal, drawing through the layer's drawables | Metal | Smooth | 6.3 to 6.8 (5 effects, 1 pass) | 59.9 to 60.0 | 4.2 at 60 |
| 1.1.12: Metal drawing into its own surfaces, paced by the display | Metal | Smooth | 3.3 to 4.0 (13 effects) | 59.9 to 60.0 | 4.3 to 4.4 at 60 |

Sources: the spec's 1.1.11 table (`docs/superpowers/specs/2026-11-23-cheap-effects-metal-design.md`),
`target/draw-bench/phase-a/table.md`, the first Metal pass
(`.superpowers/sdd/2026-11-23-cheap-effects-metal/task15-mac-pass1.tsv`), and the final runs above.

## Windows

Windows draws with wgpu on DirectX 12. The test machine ran other work during every session:
other processes used 36 to 152% of a core (mean 60%) during the like-for-like runs, and 55 to
110% in the other sessions. That load moves the
absolute numbers by several points from one session to the next, so the Windows rows below are
same-session comparisons of two Termurion builds, with the builds run in turn and the order
reversed on the second pass. Do not compare them with the macOS or Linux tables.

**Windows Terminal is not measured.** A second Windows Terminal window joins the Windows
Terminal process that is already running, so the bench cannot tell its CPU apart from the
window that was open before (it read 0%). No other Windows terminal was measured in this round.

### idle and claude, like for like

1.1.12 (build cfff08cc) at Balanced (30 frames a second, the old default) against 1.1.11 at its own default
(Balanced). The last column is 1.1.12 at its new default, Smooth (60 frames a second), for idle
only. CPU %, mean of 2 passes. Source: `.superpowers/sdd/2026-11-23-cheap-effects-metal/task15-win-balanced.md`.

| Config | Workload | 1.1.12 Balanced | 1.1.11 Balanced | 1.1.12 Smooth |
|---|---|---|---|---|
| Termurion | idle | 2.0 | 2.1 | not run |
| Termurion + Lightning | idle | 4.2 | 4.8 | 4.7 |
| Termurion + Cyber | idle | 4.4 | 5.2 | 6.7 |
| Termurion + Aurora | idle | 4.8 | 4.8 | 8.2 |
| Termurion | claude | 22.8 | 18.1 | not run |
| Termurion + Lightning | claude | 25.3 | 21.2 | not run |
| Termurion + Cyber | claude | 23.8 | 22.2 | not run |
| Termurion + Aurora | claude | 27.0 | 21.8 | not run |

At the same frame rate an effect at idle costs the same or less than in 1.1.11. Smooth draws
twice the frames, so it costs more than Balanced on Windows (Lightning's Smooth pass 2 read 2.4,
against 7.0 on pass 1, so treat that cell as noisy). Pick Balanced in Settings > General > GPU
Renderer > Effects animation for 1.1.11's rate.

The claude rows above read higher in every config, in this session and in an earlier one
(plain claude 23.9 against 19.0). A bisect found the cause before release: a text buffer kept
between frames was freed and grown again on every frame once a frame passed 8 MiB. 1.1.12 keeps
every such buffer at the size recent frames needed and gives memory back only after the load
stays lower for 300 frames. Plain claude, median of 3 interleaved rounds in one session:

| Build | CPU % |
|---|---|
| The build before the slowdown | 20.4 |
| 1.1.12 before the fix | 24.4 |
| 1.1.12 | 21.2 |

The fix wins back 3.2 of the 4.0 points; 0.8 remains and was not traced further. It costs about
40 MB more memory while this workload runs.

### stream20

Plain Termurion, CPU %, same session. Source: `.superpowers/sdd/2026-11-23-cheap-effects-metal/win-regression.md`.

| Build | CPU % |
|---|---|
| 1.1.11, built from source | 8.3 |
| 1.1.11, the released build | 7.4 |
| 1.1.12 work in progress (440c6559) | 5.5 |

### flood

Plain Termurion from the full Windows run (`.superpowers/sdd/2026-11-23-cheap-effects-metal/task15-win-bench.md`, build 440c6559): 160 MB/s, the same
throughput as 1.1.11 in that run (159 MB/s). The 1.1.11 flood CPU in that run came from a
quieter machine state and was not repeated, so no flood CPU comparison is claimed here.

## Linux

Linux draws with wgpu on Vulkan, with OpenGL as the fallback. Measured on a GNOME desktop on
Wayland, idle and claude workloads, 2 passes (median). Source:
`.superpowers/sdd/2026-11-23-cheap-effects-metal/task15-linux-bench-2.md`. stream20 and flood
were not run in this round, and Ptyxis was not measured in it.

An earlier Linux run was thrown away: a "Tips of the Day" question opened over every bench
window and stopped its animation, so every effect read the same as plain Termurion.

**Different grid.** The screen is too small for 220 x 63, so the compositor shrinks each
window: Termurion gets 173 x 45 cells and Ghostty 176 x 48. Termurion against Termurion is like
for like. The Ghostty rows draw a few more cells and are a guide, not a like-for-like comparison.

**Frame rate.** frames/s comes from a separate run that counts the window's Wayland commits
(that debug log costs CPU, so the CPU columns come from runs without it). At Smooth this machine
reached about 37 to 42 frames a second for most effects, not 60. Embers reached 33.5, Shockwave
36.8 and Cyber 29.6. At Balanced every effect drew 30.3 frames a second (Cyber 25.3), and 1.1.11
drew 29.8 (Cyber 25.1). Plain Termurion draws 2 frames a second at idle, for the cursor blink.

**Ghostty is still well ahead on Linux.** Ghostty with the same Lightning shader idles at 1.4%
of a core (plain Ghostty 0.3%), against about 7 to 9% for Termurion with an effect. Linux does not
match Ghostty yet. What 1.1.12 does on Linux is cost less than 1.1.11 at the same frame rate.

### idle

| Terminal | CPU % | frames/s | MB/s | memory MB |
|---|---|---|---|---|
| Ghostty (different grid) | 0.3 | - | 0.00 | - |
| Ghostty + Lightning (different grid) | 1.4 | - | 0.00 | - |
| Termurion 1.1.11 (Balanced) | 2.6 | 2 | 0.00 | 139 |
| Termurion 1.1.11 + Lightning (Balanced) | 8.9 | 29.8 | 0.00 | 148 |
| Termurion 1.1.12 (Smooth) | 1.8 | 2 | 0.00 | 139 |
| Termurion 1.1.12 + Lightning (Balanced) | 8.0 | 30.3 | 0.00 | 150 |
| Termurion 1.1.12 + Lightning (Smooth) | 8.0 | 37 to 42 | 0.00 | 150 |

### claude

No frames/s were taken for claude on Linux.

| Terminal | CPU % | frames/s | MB/s | memory MB |
|---|---|---|---|---|
| Termurion 1.1.11 (Balanced) | 8.4 | - | 0.01 | 149 |
| Termurion 1.1.11 + Lightning (Balanced) | 8.4 | - | 0.01 | 158 |
| Termurion 1.1.12 (Smooth) | 6.8 | - | 0.01 | 149 |
| Termurion 1.1.12 + Lightning (Balanced) | 8.8 | - | 0.01 | 160 |
| Termurion 1.1.12 + Lightning (Smooth) | 8.2 | - | 0.01 | 160 |

Plain Termurion streams Claude Code for 6.8% against 1.1.11's 8.4%. With Lightning the three rows
are within noise of each other.

### Every effect at idle

CPU %. 1.1.11 has no Smooth choice; its default is Balanced, so the Balanced columns are like
for like. frames/s as in the note above.

| Effect | 1.1.12 Smooth | 1.1.12 Balanced | 1.1.11 Balanced |
|---|---|---|---|
| none | 1.8 | 1.6 | 2.6 |
| Aurora | 7.9 | 7.8 | 9.1 |
| Cyber | 7.2 | 6.9 | 7.2 |
| Embers | 7.5 | 7.8 | 10.1 |
| Fireworks | 9.9 | 8.4 | 10.2 |
| Glitch | 7.9 | 8.1 | 9.0 |
| Hologram | 8.4 | 7.3 | 9.4 |
| Katana | 8.9 | 7.7 | 8.9 |
| Lightning | 8.0 | 8.0 | 8.9 |
| Mecha | 8.6 | 7.7 | 9.1 |
| Neon | 9.3 | 7.0 | 9.9 |
| Phosphor | 8.8 | 8.0 | 10.3 |
| Shockwave | 9.2 | 8.1 | 8.3 |
| Teleport | 8.7 | 7.3 | 8.2 |

At Balanced, the same 30 frames a second, every effect costs less in 1.1.12 than in 1.1.11 (6.9
to 8.4% against 7.2 to 10.3%). At Smooth, 1.1.12 draws 37 to 42 frames a second for 7.2 to 9.9%,
about what 1.1.11 spent on 30.

## History

Termurion's own numbers, release by release, from [`FIXES.md`](FIXES.md) entries 26 to 37 and
the benches above. All on macOS, the same machine and bench, 220 columns, Monaco 12, but in
different sessions, so small steps are within noise. "-" means the fix was not about speed or
no number was taken.

| Release | What changed | Measured |
|---|---|---|
| 1.1.3 | A point is one pixel on macOS, as in iTerm2 (FIXES 26) | Text the same size as iTerm2's (it was a third larger) |
| 1.1.4 | The Effects animation setting (Balanced by default: 30 frames a second, pauses after 2 minutes idle) | - |
| 1.1.5 | A point is 4/3 of a pixel on Windows and Linux (FIXES 27) | Text the same size as Windows Terminal's and Ptyxis's |
| 1.1.6 | An animation frame reuses the frame it built and runs only the shader passes (FIXES 28) | Lightning at an idle prompt used 30 to 45% before |
| 1.1.7 | Only rows that changed are built again (FIXES 29); a row partly under the announcement bar keeps its visible part (FIXES 31) | Idle tmux status bar and Claude Code used 3 to 30% before, 27 to 37% while streaming |
| 1.1.8 | Row hashes use FxHash; a cursor blink no longer rebuilds every row (FIXES 32) | Claude streaming used 18 to 27% before |
| 1.1.9 | Same frames are not drawn again, kept bind groups and glyphs, persistent buffers (FIXES 33); output parses in slices between frames (FIXES 34) | claude 15.2 to 10.8% plain, 17.4 to 13.4% with Lightning; stream20 19.9 to 14.9% plain; flood 0.1 to 25 frames/s at 69 MB/s |
| 1.1.10 | One staging belt for GPU uploads, cheaper Instant Replay (FIXES 35) | claude 10.8 to 8.6% plain (Ghostty 7.1%), 11.8% with Lightning |
| 1.1.11 | Flow control, ASCII parsed in runs, 16 KiB host relay, cached shaped words (FIXES 36) | flood memory 970 to 175 MB, 26 to 55 frames/s, 64 to 69 to 97 MB/s; claude 8.6 to 7.5% |
| 1.1.12 | Native Metal on macOS, cheaper effect frames on every OS, Smooth (60 frames a second) by default (FIXES 37) | effect idle 8.4 to 9.6% at 28 frames/s to 3.3 to 4.0% at 60 frames/s; claude 6.4% plain, 5.7% with Lightning; flood 86 MB/s at 178 MB |

The flood rate moves a lot from one session to the next: Ghostty itself took 90 MB/s in one
session (`target/draw-bench/phase-a/table.md`) and 122 MB/s in the 1.1.12 run. The 1.1.11-era
renderer took 88 MB/s in that first session and 1.1.12 takes 86 MB/s in its own. 1.1.12 did not
change the flood, parse or text paths.
