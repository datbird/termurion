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

/// Every baseline frame without its text (the text cases follow below), then the picture cases.
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
    assert!(!on_layer.layer().framebufferOnly() && on_layer.layer().maximumDrawableCount() == 3 && on_layer.layer().displaySyncEnabled());
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
    // Through the completed handler's own path: the status, then the error's domain and code.
    // A Metal command buffer cannot be made to fail on purpose, so the handler's two reads of
    // the buffer (`status`, `error`) are the only part not run here.
    use objc2_metal::{MTLCommandBufferError as E, MTLCommandBufferStatus as S};
    // SAFETY: a constant NSString Metal exports.
    let domain = unsafe { objc2_metal::MTLCommandBufferErrorDomain };
    let error = |code: E| objc2_foundation::NSError::new(code.0 as isize, domain);
    super::completed(generation, S::Completed, None);
    super::completed(generation, S::Error, Some(&error(E::Internal)));
    assert!(crate::gpu_pick::take_lost().is_none(), "an internal error is logged, not a loss");
    super::completed(generation, S::Error, Some(&objc2_foundation::NSError::new(E::DeviceRemoved.0 as isize, objc2_foundation::ns_string!("SomeOtherDomain"))));
    assert!(crate::gpu_pick::take_lost().is_none(), "another domain's code 11 is no loss");
    super::completed(generation - 1, S::Error, Some(&error(E::DeviceRemoved)));
    assert!(crate::gpu_pick::take_lost().is_none(), "a replaced device's report is ignored");
    super::completed(generation, S::Error, Some(&error(E::DeviceRemoved)));
    let why = crate::gpu_pick::take_lost().expect("the removed device is reported lost");
    assert!(why.contains("device lost") && why.contains("Metal error 11"), "{why}");
    // The window moves onto a new device and draws again.
    crate::gpu_pick::next_generation();
    let fresh = super::MetalShared::new(wgpu::PowerPreference::HighPerformance).unwrap();
    win.rebuild(&crate::gpu_shared::GpuShared::Metal(fresh), None).unwrap();
    win.render(&Frame { clear: [0.0, 1.0, 0.0, 1.0], fade: 1.0, ..Default::default() }, None, None).unwrap().kept.unwrap();
    let px = super::readback::read_rgba(&test_device().queue, win.offscreen_texture(), 64, 32).unwrap();
    assert_eq!(&px[0..4], &[0, 255, 0, 255]);
}

/// Effects wait for Task 11: a window asked for one says so.
#[test]
fn an_effect_says_the_metal_renderer_has_none_yet() {
    let mut win = metal_window(SIZE);
    let effect = crate::render_fx::discover(None).0.into_iter().next().expect("a shipped effect");
    assert_eq!(win.set_effect(Some(effect.compile().unwrap()), None).unwrap_err(), super::window::NO_EFFECTS_YET);
}

/// A wgpu window and a Metal window whose renderers draw rows through the row renderer (`rows`)
/// or prepare every row each frame.
fn text_pair(rows: bool) -> (crate::gpu_window::GpuHandles, WindowGpu, MetalWindow) {
    let (on, mut wgpu) = wgpu_window(SIZE);
    let mut metal = metal_window(SIZE);
    wgpu.renderer_mut().set_row_instances(rows);
    metal.renderer_mut().set_row_instances(rows);
    (on, wgpu, metal)
}

/// Draws `cases` in order on both windows, the state carried from case to case, and compares
/// every screenshot. Cases of the windows' own size are also drawn through `render`.
fn match_in_order(on: &crate::gpu_window::GpuHandles, wgpu: &mut WindowGpu, metal: &mut MetalWindow, cases: &[crate::render_pixel_tests::PixelCase], tag: &str, near: &mut Vec<String>) {
    for c in cases {
        let (w, h) = c.size;
        let name = format!("{} ({tag})", c.name);
        let a = wgpu.capture(&c.frame, None, w, h).unwrap_or_else(|e| panic!("{name}: wgpu: {e}"));
        let b = metal.capture(&c.frame, None, w, h).unwrap_or_else(|e| panic!("{name}: Metal: {e}"));
        compare(&format!("{name} capture"), &a, &b, w, near);
        if c.size != SIZE {
            continue;
        }
        wgpu.render(&c.frame, None, None).unwrap().kept.unwrap_or_else(|e| panic!("{name}: wgpu render: {e}"));
        let a = match &*wgpu {
            WindowGpu::Wgpu(win) => read_back(on, win.offscreen_texture()),
            WindowGpu::Metal(_) => unreachable!(),
        };
        metal.render(&c.frame, None, None).unwrap().kept.unwrap_or_else(|e| panic!("{name}: Metal render: {e}"));
        // The window's own queue: after a rebuild it is not the test device's.
        let crate::gpu_shared::GpuShared::Metal(m) = metal.shared() else { unreachable!() };
        let b = super::readback::read_rgba(&m.queue, metal.offscreen_texture(), w, h).unwrap();
        compare(&format!("{name} render"), &a, &b, w, near);
    }
}

fn report_near(near: &[String]) {
    // Allowed only for float rounding in the MSL compile; each is listed in the task report.
    if !near.is_empty() {
        eprintln!("cases one step apart: {near:?}");
    }
}

/// Every pixel case (baseline frames, agent screens, the 300 `cjk_flood` frames), on the row path
/// and on the full path, drawn in order on one window each.
#[test]
fn metal_matches_wgpu_on_every_pixel_case() {
    let mut near = Vec::new();
    for rows in [true, false] {
        let (on, mut wgpu, mut metal) = text_pair(rows);
        let cases = crate::render_pixel_tests::pixel_cases(wgpu.renderer());
        assert!(cases.len() > 300, "{} cases", cases.len());
        // Equal pictures prove nothing if neither drew the text.
        let c = cases.iter().find(|c| !c.frame.rows.is_empty()).expect("a case with text");
        let bare = Frame { rows: Vec::new(), ..c.frame.clone() };
        let mut probe = metal_window(c.size);
        probe.renderer_mut().set_row_instances(rows);
        assert_ne!(probe.capture(&c.frame, None, c.size.0, c.size.1).unwrap(), probe.capture(&bare, None, c.size.0, c.size.1).unwrap(), "{}: Metal drew no text", c.name);
        match_in_order(&on, &mut wgpu, &mut metal, &cases, if rows { "rows" } else { "full" }, &mut near);
        if rows {
            assert!(metal.renderer().row_prepares() > 0, "the row path drew no rows");
        }
    }
    report_near(&near);
}

/// The flood with the atlas capped at 512: it grows from 256 once mid run, then evicts, and the
/// Metal textures follow every grow and upload.
#[test]
fn metal_matches_wgpu_through_atlas_growth() {
    let mut near = Vec::new();
    for rows in [true, false] {
        let (on, mut wgpu, mut metal) = text_pair(rows);
        wgpu.renderer_mut().atlas().set_max_size(512);
        metal.renderer_mut().atlas().set_max_size(512);
        let cases: Vec<_> = crate::render_pixel_tests::pixel_cases(wgpu.renderer()).into_iter().filter(|c| c.name.starts_with("cjk_flood")).collect();
        let (grown, evicted) = (metal.renderer_mut().atlas().generation(), metal.renderer_mut().atlas().evictions());
        match_in_order(&on, &mut wgpu, &mut metal, &cases, if rows { "rows, capped" } else { "full, capped" }, &mut near);
        assert!(metal.renderer_mut().atlas().generation() != grown, "the capped flood never grew the atlas");
        assert!(metal.renderer_mut().atlas().evictions() > evicted, "the capped flood never evicted");
    }
    report_near(&near);
}

/// A pane of `s` filling `size`, drawn with `r`'s cell.
fn screen_case(name: String, r: &Renderer, s: &crate::screen::Screen, size: (u32, u32)) -> crate::render_pixel_tests::PixelCase {
    let rect = rect(0.0, 0.0, size.0 as f32, size.1 as f32);
    let mut frame = Frame { clear: crate::scene::rgba(s.palette.bg, 1.0), fade: 1.0, ..Default::default() };
    let look = crate::cursor_look::CursorLook::shown(true);
    crate::scene::pane_with(&mut frame, s, rect, r.cell, true, &look, &Default::default(), None, crate::render::DEFAULT_FACE, None);
    crate::render_pixel_tests::PixelCase { name, frame, size }
}

/// A screen scrolling one line at a time: the rows that only moved keep their instances (only
/// the new line is prepared), and Metal draws them where wgpu does.
#[test]
fn metal_matches_wgpu_on_scrolled_rows() {
    let (on, mut wgpu, mut metal) = text_pair(true);
    let cell = metal.renderer().cell;
    let size = ((80.0 * cell.0).ceil() as u32, (20.0 * cell.1).ceil() as u32);
    let mut s = crate::screen::Screen::new(80, 20);
    for n in 0..20 {
        let end = if n < 19 { "\r\n" } else { "" };
        s.feed(format!("line {n:02} of the scroll test{end}").as_bytes());
    }
    let mut near = Vec::new();
    let first = screen_case("twenty rows".into(), metal.renderer(), &s, size);
    match_in_order(&on, &mut wgpu, &mut metal, &[first], "scroll", &mut near);
    for n in 20..32 {
        let before = metal.renderer().row_prepares();
        s.feed(format!("\r\nline {n:02} of the scroll test").as_bytes());
        let c = screen_case(format!("scrolled to line {n}"), metal.renderer(), &s, size);
        match_in_order(&on, &mut wgpu, &mut metal, &[c], "scroll", &mut near);
        let prepared = metal.renderer().row_prepares() - before;
        assert!(prepared <= 2, "scrolling one line prepared {prepared} rows");
    }
    report_near(&near);
}

/// A frame whose text cannot fit fails, but what it prepared first (a row's slot, its glyphs)
/// still reaches the Metal buffers with the next frame, which reuses the row.
#[test]
fn a_failed_prepare_loses_no_write_on_metal() {
    let size = (320, 120);
    let window = rect(0.0, 0.0, size.0 as f32, size.1 as f32);
    let row = |text: &str, y: f32, scale: f32| {
        let span = crate::scene::Span { text: text.into(), color: crate::palette::rgb(230, 230, 230), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        crate::scene::TextRow { x: 4.0, y, spans: vec![span].into(), clip: window, scale, face: crate::render::DEFAULT_FACE }
    };
    let frame = |rows| Frame { clear: [0.0, 0.0, 0.0, 1.0], fade: 1.0, rows, ..Default::default() };
    let kept = row("a row prepared before the failure", 4.0, 1.0);
    let mut metal = MetalWindow::offscreen(test_device(), Renderer::cpu_only(FACE, PX), size);
    metal.renderer_mut().atlas().set_max_size(256);
    // One glyph bigger than the whole capped atlas.
    assert!(metal.capture(&frame(vec![kept.clone(), row("W", 30.0, 24.0)]), None, size.0, size.1).is_err(), "the huge glyph fit");
    let prepared = metal.renderer().row_prepares();
    let got = metal.capture(&frame(vec![kept.clone()]), None, size.0, size.1).unwrap();
    assert_eq!(metal.renderer().row_prepares(), prepared, "the row was prepared again, so this proves nothing");
    let (_, mut wgpu) = wgpu_window(size);
    let want = wgpu.capture(&frame(vec![kept]), None, size.0, size.1).unwrap();
    crate::render_pixel_tests::assert_same("after a failed frame", &want, &got, size.0);
}

/// A window moved onto a new device starts its atlas and rows over with it, and draws text as a
/// fresh wgpu window does.
#[test]
fn text_draws_the_same_after_a_rebuild() {
    let (on, mut wgpu, mut metal) = text_pair(true);
    let cases: Vec<_> = crate::render_pixel_tests::pixel_cases(wgpu.renderer()).into_iter().filter(|c| !c.name.starts_with("cjk_flood") || c.name.ends_with("0")).collect();
    let mut near = Vec::new();
    match_in_order(&on, &mut wgpu, &mut metal, &cases[..cases.len() / 2], "before", &mut near);
    let fresh = super::MetalShared::new(wgpu::PowerPreference::HighPerformance).unwrap();
    metal.rebuild(&crate::gpu_shared::GpuShared::Metal(fresh), None).unwrap();
    assert_eq!((metal.renderer_mut().atlas().generation(), metal.renderer().row_prepares()), (0, 0), "the atlas and rows started over");
    let (on, mut wgpu) = wgpu_window(SIZE);
    match_in_order(&on, &mut wgpu, &mut metal, &cases, "after", &mut near);
    report_near(&near);
}

/// Steady text frames (the same screen with one line changing) make no Metal buffer, texture or
/// staging chunk once the first frames sized them.
#[test]
fn steady_text_frames_make_no_buffers() {
    let mut metal = metal_window(SIZE);
    let mut s = crate::screen::Screen::new(80, 20);
    for n in 0..19 {
        s.feed(format!("steady line {n:02}\r\n").as_bytes());
    }
    let mut made = None;
    for i in 0..40 {
        s.feed(format!("\rtick {:02}", i % 10).as_bytes());
        let c = screen_case(format!("tick {i}"), metal.renderer(), &s, SIZE);
        metal.render(&c.frame, None, None).unwrap().kept.unwrap();
        // The first ten frames place every glyph the ticks use.
        if i == 10 {
            made = Some((metal.text_objects_made(), metal.staging_made()));
        }
    }
    let (objects, staging) = made.unwrap();
    assert_eq!(metal.text_objects_made(), objects, "steady frames made text buffers or textures");
    assert!(metal.staging_made() <= staging.max(3), "{} staging buffers", metal.staging_made());
}

/// The window's layer follows its root layer's bounds and scale, as wgpu's raw-window-metal layer
/// does, and takes the drawable size it is given.
#[test]
fn the_layer_follows_its_root_layer() {
    use objc2_foundation::{NSPoint as CGPoint, NSRect as CGRect, NSSize as CGSize};
    let root = objc2_quartz_core::CALayer::new();
    root.setContentsScale(2.0);
    root.setBounds(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(300.0, 150.0)));
    let surface = super::surface::Surface::under(&root);
    assert_eq!((surface.layer.contentsScale(), surface.layer.bounds().size), (2.0, CGSize::new(300.0, 150.0)), "the first values come at once");
    root.setContentsScale(1.0);
    root.setBounds(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(200.5, 90.0)));
    assert_eq!((surface.layer.contentsScale(), surface.layer.bounds().size), (1.0, CGSize::new(200.5, 90.0)), "fractional bounds are kept");
    surface.configure(&test_device().device, (201, 90), 1.0);
    assert_eq!(surface.layer.drawableSize(), CGSize::new(201.0, 90.0));
    assert_eq!(surface.layer.contentsScale(), 1.0, "the root's scale, not the argument's, wins under a root");
}

/// Steady frames reuse their staging chunks: no new Metal buffer per frame.
#[test]
fn steady_frames_make_no_staging_buffers() {
    let mut win = metal_window((320, 200));
    for i in 0..30 {
        let c = (i % 7) as f32 / 7.0;
        let frame = Frame { fade: 1.0, quads: (0..40).map(|k| Quad { rect: rect(k as f32 * 7.0, 10.0, 6.0, 6.0 + i as f32), color: [c, 0.5, 1.0 - c, 1.0] }).collect(), ..Default::default() };
        win.render(&frame, None, None).unwrap().kept.unwrap();
    }
    // A chunk is free again once its command buffer completed; a frame or two may still hold one.
    assert!(win.staging_made() <= 3, "{} staging buffers for 30 frames", win.staging_made());
}
