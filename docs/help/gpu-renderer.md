# GPU Renderer

Termurion draws every window on the GPU through DirectX 12, the native Windows graphics API that Windows Terminal also uses. The see-through (transparent) windows use it too, so all windows run on one API and one GPU.

## Which API

Termurion tries DirectX 12 first. Only if DirectX 12 cannot make an adapter, a surface or a device does it try Vulkan, and then OpenGL. The log file host.log says which one drew and why a fallback happened:

- `renderer: Dx12 on <GPU> (...)` is the normal line.
- `renderer: DirectX 12 unavailable: ...` and `renderer: fell back to Vulkan because ...` appear only after a failure.
- `adapters: ...` lists every GPU Windows offers, once per run.
- `renderer: device lost (...)`, then `renderer: recovered on <API> <GPU>`, appear when the driver resets or the GPU goes away.

To test another API, set the environment variable `TERMURION_RENDERER` to `dx12`, `vulkan` or `gl` before starting Termurion. That API is tried first; the others still follow as fallbacks.

## Which GPU

Settings > General > GPU Renderer has "Prefer integrated to discrete GPU", as in iTerm2. It is on by default. The row is enabled only on a computer that has both an integrated and a discrete GPU, and a change applies after you restart Termurion.

- On: Termurion asks Windows for the low-power GPU (the integrated one). A terminal needs little GPU power, and this keeps a discrete card idle.
- Off: Termurion asks for the high-performance GPU (the discrete one).

host.log records the choice, for example `renderer: Dx12 on <GPU> (Prefer integrated to discrete GPU is on: asked for the low-power GPU; got IntegratedGpu; honored)`. If Windows hands out a different kind of GPU than the setting asks for, the line says `NOT honored`.

## If the GPU is lost

A driver update or reset, a timeout, a removed GPU or a laptop switching GPUs can take the graphics device away. Termurion makes a new one for every terminal window, using the same API order and GPU setting as at start, and redraws. Your sessions are not affected. host.log says `renderer: recovered on <API> <GPU>`. If a try fails, Termurion waits a moment and tries again, then moves down the API order. If every API fails, a banner asks you to restart Termurion. Settings and prompt windows that were open at the time may stop drawing; close and reopen them.
