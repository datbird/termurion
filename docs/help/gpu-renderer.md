# GPU Renderer

Termurion draws every window on the GPU. On Windows it uses DirectX 12, the native graphics API that Windows Terminal also uses. The see-through (transparent) windows use it too, so all windows run on one API and one GPU. On macOS it draws with native Metal, and only Metal. On Linux it uses Vulkan, with OpenGL as the fallback.

## Which API

On Windows, Termurion tries DirectX 12 first. Only if DirectX 12 cannot make an adapter, a surface or a device does it try Vulkan, and then OpenGL. The log file host.log, in the settings folder (see [Installing](installing.md)), says which one drew and why a fallback happened:

- `renderer: Dx12 on <GPU> (...)` is the normal line.
- `renderer: DirectX 12 unavailable: ...` and `renderer: fell back to Vulkan because ...` appear only after a failure.
- `adapters: ...` lists every GPU Windows offers, once per run.
- `renderer: device lost (...)`, then `renderer: recovered on <API> <GPU>`, appear when the driver resets or the GPU goes away.

On Linux, Termurion tries Vulkan first, then OpenGL. The log lines name the API that was used. Effects work on all of them, OpenGL included.

On macOS, Termurion draws everything with Metal: text, cells, effects, the Settings and other windows, and screenshots. There is no other API to fall back to. host.log says `renderer: native Metal on <GPU> (...)`. If Metal cannot start, Termurion shows "Termurion could not start Metal:" with the reason, writes `renderer: Metal unavailable: ...` to host.log, and quits. It never switches to another API in silence.

To test another API on Windows or Linux, set the environment variable `TERMURION_RENDERER` to `dx12` (Windows only), `vulkan` or `gl` before starting Termurion. That API is tried first; the others still follow as fallbacks. On macOS the variable is ignored, whatever its value, and host.log says so: `renderer: TERMURION_RENDERER=<value> is ignored: macOS draws with Metal only`.

## Which GPU

Settings > General > GPU Renderer has "Prefer integrated to discrete GPU", as in iTerm2. It is on by default. The row is enabled only on a computer that has both an integrated and a discrete GPU, and a change applies after you restart Termurion.

- On: Termurion asks the system for the low-power GPU (the integrated one). A terminal needs little GPU power, and this keeps a discrete card idle.
- Off: Termurion asks for the high-performance GPU (the discrete one).

On Linux, with the setting on, a computer whose integrated GPU has no usable Vulkan driver draws on that GPU through OpenGL instead of falling back to the discrete card.

host.log records the choice, for example `renderer: Dx12 on <GPU> (Prefer integrated to discrete GPU is on: asked for the low-power GPU; got IntegratedGpu; honored)`. If the system hands out a different kind of GPU than the setting asks for, the line says `NOT honored`.

## Effects animation

Settings > General > GPU Renderer > "Effects animation" sets how hard animated effects (render plugins whose animation is `always`) may drive the GPU. Such an effect redraws the whole window, so on an older GPU it can keep the GPU busy with nobody at the machine, and slow other programs that use the same GPU (a video encoder, screen sharing). The change applies to the next frame.

- Smooth (the default): up to 60 frames a second, even on a faster screen. The effect pauses after 2 minutes with no typing, no output and no focus change.
- Full: as fast as the display refreshes, and it never stops (Termurion before 1.1.4).
- Balanced: up to 30 frames a second. The effect pauses after 2 minutes with no typing, no output and no focus change.
- Light (older hardware): up to 15 frames a second, and it pauses after 30 seconds.

A paused effect holds its last frame and starts again with the next key or output. Effects whose animation is `auto` or `off` are only capped, never paused.

## Redraw only the rows that changed

Settings > Advanced > Drawing > "Redraw only the rows that changed" is on by default. Termurion rebuilds only the rows whose text, colors or highlights changed, as other fast terminals do. Turn this off if text is drawn wrongly (leftover or stale characters) and report it.

## If the GPU is lost

A driver update or reset, a timeout, a removed GPU or a laptop switching GPUs can take the graphics device away. Termurion makes a new one for every terminal window, using the same API order and GPU setting as at start, and redraws. Your sessions are not affected. host.log says `renderer: recovered on <API> <GPU>`. If a try fails, Termurion waits a moment and tries again, then moves down the API order. If every API fails, a banner asks you to restart Termurion. Settings and prompt windows that were open at the time may stop drawing; close and reopen them.

On macOS the same recovery runs with Metal alone. Termurion makes a new Metal device and rebuilds every terminal window on it. A GPU memory allocation that fails counts as a lost device too, so the window is rebuilt rather than drawn half way. It tries up to 3 times, waiting a little longer after each failure. If all 3 fail, or the device is lost again within a minute of a recovery, the banner "The graphics device was lost and could not be recovered. Restart Termurion." shows. host.log says `renderer: recovering after: ...`, then `renderer: recovered on Metal <GPU>` or `renderer: recovery on Metal failed: ...`.
