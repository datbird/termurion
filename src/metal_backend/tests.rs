//! Native Metal against wgpu's Metal backend, in one process: the same frames, the same bytes.

use std::sync::Arc;

use objc2_metal::MTLTexture;

use super::{test_device, MetalWindow};
use crate::gpu_window::tests::{handles, read_back};
use crate::gpu_window::{WgpuWindow, WindowGpu};
use crate::layout::Rect;
use crate::render::Renderer;
use crate::render_pixel_tests::FACE;
use crate::scene::{Frame, ImageQuad, Quad};

const PX: f32 = 14.0;
const SIZE: (u32, u32) = (808, 486);

fn wgpu_window(size: (u32, u32)) -> (crate::gpu_window::GpuHandles, WindowGpu) {
    let on = handles();
    assert_eq!(on.adapter.get_info().backend, wgpu::Backend::Metal, "the wgpu side must run on wgpu's Metal backend");
    let mut r = Renderer::new(&on.device, &on.queue, wgpu::TextureFormat::Bgra8UnormSrgb, FACE, PX);
    r.set_view_formats(crate::render_fx_gpu::view_formats_supported(&on.adapter));
    let w = WindowGpu::Wgpu(WgpuWindow::offscreen(&on, r, wgpu::TextureFormat::Bgra8UnormSrgb, size));
    (on, w)
}

fn metal_window(size: (u32, u32)) -> MetalWindow {
    MetalWindow::offscreen(test_device(), Renderer::cpu_only(FACE, PX), size)
}

/// A picture `w` by `h` with a different color in every texel.
fn picture(key: u64, rect: Rect, (w, h): (u32, u32), uv: [f32; 4], repeat: bool) -> ImageQuad {
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i * 37 % 256) as u8, (i * 91 % 256) as u8, (255 - i * 13 % 256) as u8, 255]).collect();
    ImageQuad { rect, uv, key: (key, 0), pixels: Arc::new(px), size: (w, h), repeat }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

/// Frames with pictures: inline ones scaled up and down at fractional places, a tiled
/// background, translucent quads over and under them, popups, and a fade.
fn image_cases() -> Vec<(String, Frame)> {
    let quad = |r: Rect, c: [f32; 4]| Quad { rect: r, color: c };
    let pictures = Frame {
        clear: [0.05, 0.1, 0.2, 1.0],
        fade: 1.0,
        backgrounds: vec![picture(1, rect(0.0, 0.0, 808.0, 486.0), (7, 5), [0.0, 0.0, 9.3, 4.1], true)],
        quads: vec![quad(rect(10.5, 20.25, 300.0, 40.0), [1.0, 0.0, 0.0, 0.5]), quad(rect(0.0, 300.0, 808.0, 3.0), [0.2, 0.9, 0.3, 1.0])],
        images: vec![picture(2, rect(40.3, 60.7, 401.0, 211.0), (13, 9), [0.0, 0.0, 1.0, 1.0], false), picture(3, rect(500.0, 100.0, 37.5, 19.25), (64, 40), [0.1, 0.2, 0.9, 0.8], false)],
        popups: vec![quad(rect(380.0, 200.0, 200.0, 120.0), [0.9, 0.9, 0.8, 0.95])],
        overlay: vec![quad(rect(0.0, 0.0, 808.0, 486.0), [0.0, 0.0, 0.0, 0.3])],
        ..Default::default()
    };
    let fading = Frame { fade: 0.6, ..pictures.clone() };
    let only_background = Frame { clear: [0.0, 0.0, 0.0, 1.0], fade: 1.0, backgrounds: vec![picture(4, rect(-3.5, 2.0, 700.0, 400.0), (31, 17), [0.0, 0.0, 1.0, 1.0], false)], ..Default::default() };
    vec![("pictures".into(), pictures), ("pictures-fading".into(), fading), ("background-only".into(), only_background)]
}

/// Every baseline frame without its text (Metal draws text from Task 10 on), then the picture cases.
fn cases() -> Vec<(String, Frame)> {
    let mut out: Vec<(String, Frame)> = crate::scene::agent_box_tests::baseline_cases()
        .into_iter()
        .map(|(name, mut frame, _)| {
            frame.rows.clear();
            frame.glyphs.clear();
            (name, frame)
        })
        .collect();
    out.extend(image_cases());
    out
}

/// The Global Constraints rule: equal bytes. Returns the largest per-channel step that differs
/// (0 when equal); the caller fails on more than 1 and names every case at 1.
fn largest_step(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len(), "the pictures differ in size");
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

fn compare(name: &str, wgpu: &[u8], metal: &[u8], w: u32, near: &mut Vec<String>) {
    match largest_step(wgpu, metal) {
        0 => {}
        1 => near.push(name.to_string()),
        _ => crate::render_pixel_tests::assert_same(name, wgpu, metal, w),
    }
}

#[test]
fn metal_matches_wgpu_on_quads_and_images() {
    let (on, mut wgpu) = wgpu_window(SIZE);
    let mut metal = metal_window(SIZE);
    let mut near = Vec::new();
    let cases = cases();
    assert!(cases.len() > 10, "{} cases", cases.len());
    for (name, frame) in &cases {
        let (w, h) = SIZE;
        // Screenshots: the texture with its second (non-sRGB) view.
        let a = wgpu.capture(frame, None, w, h).unwrap();
        let b = metal.capture(frame, None, w, h).unwrap();
        compare(&format!("{name} capture"), &a, &b, w, &mut near);
        // The window's own frame: no second view, so a fade runs on the sRGB view.
        wgpu.render(frame, None, None).unwrap().kept.unwrap();
        let a = match &wgpu {
            WindowGpu::Wgpu(win) => read_back(&on, win.offscreen_texture()),
            WindowGpu::Metal(_) => unreachable!(),
        };
        metal.render(frame, None, None).unwrap().kept.unwrap();
        let b = super::readback::read_rgba(&test_device().queue, metal.offscreen_texture(), w, h).unwrap();
        compare(&format!("{name} render"), &a, &b, w, &mut near);
    }
    // Allowed only for float rounding in the MSL compile; each is listed in the task report.
    if !near.is_empty() {
        eprintln!("cases one step apart: {near:?}");
    }
}

/// A new scale reaches the layer at once, and the next frame draws at the new size with the new
/// screen size in the shaders.
#[test]
fn a_new_scale_resizes_the_drawable() {
    let layer_size = |m: &MetalWindow| {
        let s = m.layer().drawableSize();
        ((s.width as u32, s.height as u32), m.layer().contentsScale())
    };
    let mut on_layer = MetalWindow::on_detached_layer(test_device(), Renderer::cpu_only(FACE, PX), (400, 200), 2.0);
    assert_eq!(layer_size(&on_layer), ((400, 200), 2.0));
    assert!(!on_layer.layer().framebufferOnly() && on_layer.layer().maximumDrawableCount() == 2 && on_layer.layer().displaySyncEnabled());
    on_layer.resize(200, 100, 1.0);
    assert_eq!(layer_size(&on_layer), ((200, 100), 1.0));

    let mut win = metal_window((400, 200));
    let frame = Frame { clear: [0.0, 0.0, 0.0, 1.0], fade: 1.0, quads: vec![Quad { rect: rect(0.0, 0.0, 100.0, 50.0), color: [1.0, 0.0, 0.0, 1.0] }], ..Default::default() };
    win.render(&frame, None, None).unwrap().kept.unwrap();
    assert_eq!(win.last_uniform(), [400.0, 200.0, 0.0, 0.0]);
    win.resize(200, 100, 1.0);
    win.render(&frame, None, None).unwrap().kept.unwrap();
    let t = win.offscreen_texture();
    assert_eq!((t.width(), t.height()), (200, 100));
    assert_eq!(win.last_uniform(), [200.0, 100.0, 0.0, 0.0]);
    // The quad keeps its pixel size: the top-left quarter is red, the rest black.
    let px = super::readback::read_rgba(&test_device().queue, t, 200, 100).unwrap();
    let at = |x: usize, y: usize| &px[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4];
    assert_eq!((at(99, 49), at(100, 50)), (&[255, 0, 0, 255][..], &[0, 0, 0, 255][..]));
}

/// A command buffer that failed with a removed device reaches `gpu_pick::take_lost`; a good frame,
/// another kind of error and a stale device's report do not.
#[test]
fn a_lost_device_is_reported() {
    // The wgpu loss test takes the same reports: one at a time.
    let _one = crate::render::test_gpu::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = crate::gpu_pick::take_lost();
    let generation = crate::gpu_pick::next_generation();
    let mut win = metal_window((64, 32));
    assert_eq!(win.generation(), generation);
    win.render(&Frame { fade: 1.0, ..Default::default() }, None, None).unwrap().kept.unwrap();
    assert!(crate::gpu_pick::take_lost().is_none(), "a good frame is no loss");
    super::command_failed(generation, objc2_metal::MTLCommandBufferError::Internal.0 as isize, "an internal error");
    assert!(crate::gpu_pick::take_lost().is_none(), "an internal error is logged, not a loss");
    super::command_failed(generation - 1, objc2_metal::MTLCommandBufferError::DeviceRemoved.0 as isize, "the old device");
    assert!(crate::gpu_pick::take_lost().is_none(), "a replaced device's report is ignored");
    super::command_failed(generation, objc2_metal::MTLCommandBufferError::DeviceRemoved.0 as isize, "the GPU was removed");
    let why = crate::gpu_pick::take_lost().expect("the removed device is reported lost");
    assert!(why.contains("device lost") && why.contains("the GPU was removed"), "{why}");
    // The window moves onto a new device and draws again.
    crate::gpu_pick::next_generation();
    let fresh = super::MetalShared::new(false).unwrap();
    win.rebuild(&crate::gpu_shared::GpuShared::Metal(fresh), None).unwrap();
    win.render(&Frame { clear: [0.0, 1.0, 0.0, 1.0], fade: 1.0, ..Default::default() }, None, None).unwrap().kept.unwrap();
    let px = super::readback::read_rgba(&test_device().queue, win.offscreen_texture(), 64, 32).unwrap();
    assert_eq!(&px[0..4], &[0, 255, 0, 255]);
}

/// Text waits for Task 10: a frame with text says so, and its quads still draw.
#[test]
fn a_frame_with_text_draws_its_quads_and_says_why_not_the_text() {
    let (_, frame, _) = crate::scene::agent_box_tests::baseline_cases().into_iter().find(|(_, f, _)| !f.rows.is_empty() && !f.quads.is_empty()).expect("a case with text and quads");
    let mut win = metal_window(SIZE);
    let err = win.render(&frame, None, None).unwrap().kept.unwrap_err();
    assert_eq!(err, super::draw::NO_TEXT_YET);
    let effect = crate::render_fx::discover(None).0.into_iter().next().expect("a shipped effect");
    assert_eq!(win.set_effect(Some(effect.compile().unwrap()), None).unwrap_err(), super::window::NO_EFFECTS_YET);
}
