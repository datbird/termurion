# Beat Ghostty: spec addendum for 1.1.11

Date: 2026-11-20. Status: approved work list (owner), this addendum states the targets and the
one design decision the list left open. It extends `docs/superpowers/specs/2026-11-18-draw-speed.md`,
whose "What must not change" section stays binding.

## Goal

Termurion uses no more CPU than Ghostty in every `scripts/draw-bench` workload, with and without
the Lightning effect, and in a flood it draws at least as many frames and takes at least as many
bytes as Ghostty. Not one rendered pixel changes, and nothing gets less stable.

## Where it stands (funtimes, 1.1.10, CPU % of one core)

| workload | termurion | termurion + Lightning | ghostty | ghostty + Lightning (1.1.9 run) |
|---|---|---|---|---|
| idle | 0.8 | 9.6 | 0.6 | 4.3 |
| stream20 | 13.6 | 15.4 | 11.1 | 9.9 |
| claude | 8.6 | 11.8 | 7.1 | 6.5 |
| flood | 252.5, 26 fps, 68.5 MB/s | 253.4 | 234.6, 59 fps, 110.8 MB/s | 235.1 |

Where the time goes is in `.superpowers/sdd/2026-11-18-draw-speed/task-17-report.md` ("New
main-thread split" and "Where the rest is").

## Targets (binding)

Measured with `scripts/draw-bench --passes 2 termurion termurion-fx ghostty ghostty-fx` on
funtimes, Ghostty measured in the same run, mean of the 2 passes:

1. Every `termurion` row at or below the matching `ghostty` row (idle, stream20, claude, flood CPU).
2. Every `termurion-fx` row at or below the matching `ghostty-fx` row.
3. Flood: frames per second at or above Ghostty's, MB/s at or above Ghostty's.
4. Flood: CPU per MB/s no higher than 1.1.10's (252.5 / 68.5 = 3.69 points per MB/s).

A target that is missed is reported with its profile; it is not met by changing the bench.

## What must not change

- Every rule of the draw-speed spec's "What must not change": the look, pixel for pixel, every
  drawing feature, every render plugin and FX profile, the OpenGL path and Windows.
- `Frame`'s `Debug` output, byte for byte (the golden pane frames, `tests/fixtures` untouched).
- Every frame an effect shows equals today's frame for the same scene and the same uniforms. A
  frame may be left out only when it would equal the one on screen.
- The menus: every check mark, enabled state and list in the macOS menu bar, the Windows and
  Linux bars and the parked windows' bars shows what it shows today after any output.
- Instant Replay records the same frames at the same cadence; triggers, coprocesses, tmux
  integration, SSH integration, the API and session ends behave as today.
- The GLES 3.0 rule: no storage buffers in vertex shaders, no `first_instance`; vertex buffer
  slices, instance-rate vertex attributes and uniform buffers with dynamic offsets are fine. A
  uniform block stays at or under 16384 bytes.

## Two facts that shape the targets

1. **Lightning animates every frame.** Its cursor and prompt looks follow `iTime` without steps
   (`fract(t * 9.0)`, `fract(t / 2.2)` in `lightning-cursor.glsl`), so no two idle frames are
   equal and "draw less often" cannot skip frames at an idle prompt without changing what is
   shown. The idle target (9.6 to 4.3) has to come from the CPU cost of each animation frame,
   about 3.3 ms today against Ghostty's 0.6 ms. Frames are left out only where their output is
   provably the same (same scene and same uniform bytes).
2. **The flood frame rate is capped at 30 by design.** `cadence::interval` uses iTerm2's slow rate
   (`metalSlowFrameRate`, 30 fps) above 10 KB/s while Maximize Throughput is on (the default),
   because iTerm2 gives the parser the main thread. Ghostty draws 59. Target 3 cannot be met
   while that cap holds, so with parsing moved off the drawing path (below) a flood draws at
   `MaximumFrameRate` (60). Owner decision recorded here; the old cap comes back with the switch.

## The flood design decision

The approved item was "parse output on its own thread, or a safer design". Moving each pane's
`Screen` to a parse thread behind a lock (Alacritty's design) was rejected: the main thread reads
`Screen` in about 1050 places, the parse path also needs most of `Pane` (SSH and tmux hooks,
logs, the coprocess tee, replies to the pty), and a lock or a lease over that much state would
risk deadlocks or unsound access. Moving the drawing to a render thread was rejected too: the
surface, the Linux menu bar strip and the GL context would move off the main thread on three
OSes.

Chosen: **parse while drawing.** After a frame is built, the main thread draws and presents it
while a scoped worker thread (`std::thread::scope`) parses the panes' waiting output with
`Pane::pump_until`, the same code the slices run today. The two borrow disjoint fields of `App`
(`panes` against `gpu` and the frame), so the compiler proves they share nothing; no lock and no
`unsafe` is added. When the frame is presented the worker stops after its current chunk (64 KB at
most), and everything that follows output (`after_output`, triggers' requests, Instant Replay,
session ends, the API) runs on the main thread as today, before the next frame is built. Between
frames the main thread parses in slices as it does now. Only plain local and hosted panes are
parsed by the worker; tmux, SSH integration, conductor and coprocess panes stay on the main
thread. `Pane` and `Screen` are `Send` today (checked with a compile probe on Linux at 05ce84ac;
Windows is checked in the task).

A new Advanced setting `ParseWhileDrawing` (default on) turns it off, which restores 1.1.10's
path exactly, the 30 fps flood cap included.

The worker alone cannot reach 110.8 MB/s: with nothing drawn at all (1.1.8, 0.1 fps) one core
parsed 69.5 MB/s. So the flood work also cuts the per-byte cost of the output path (the hook
scans, copies and passes over each chunk) before the worker lands.
