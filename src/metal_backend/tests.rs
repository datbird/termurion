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

/// Cases allowed to be one step apart: each only for float rounding in the MSL compile, and each
/// named in the task report. Empty: every case is byte-equal.
const ONE_STEP_ALLOWED: &[&str] = &[];

fn compare(name: &str, wgpu: &[u8], metal: &[u8], w: u32, near: &mut Vec<String>) {
    match largest_step(wgpu, metal) {
        0 => {}
        1 if ONE_STEP_ALLOWED.contains(&name) => near.push(name.to_string()),
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
    report_near(&near);
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
    let _one = loss_reports();
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

/// The loss reports are process-wide, so tests that take them run one at a time under
/// `test_gpu::LOCK`. The shared wgpu device is made first: its making takes the same lock, so a
/// test holding the lock while another makes the device would wait on it forever.
fn loss_reports() -> std::sync::MutexGuard<'static, ()> {
    handles();
    crate::render::test_gpu::LOCK.lock().unwrap_or_else(|e| e.into_inner())
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
    // Only cases on `ONE_STEP_ALLOWED` get here.
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
    let failed = metal.capture(&frame(vec![kept.clone(), row("W", 30.0, 24.0)]), None, size.0, size.1).expect_err("the huge glyph fit");
    assert!(failed.contains("AtlasFull"), "failed for another reason: {failed}");
    assert!(metal.stale().is_none(), "a failed prepare is no Metal failure");
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
    // Each offscreen frame waits for its command buffer, so the one chunk comes back every time.
    assert_eq!(metal.staging_made(), staging, "steady frames made staging chunks");
}

/// The atlas in Web color mode (color glyphs in RGBA8Unorm, not sRGB) on both sides.
#[test]
fn metal_matches_wgpu_in_web_color_mode() {
    let (on, mut wgpu, mut metal) = text_pair(true);
    wgpu.renderer_mut().set_color_mode(glyphon::ColorMode::Web);
    metal.set_color_mode(glyphon::ColorMode::Web);
    let cases: Vec<_> = crate::render_pixel_tests::pixel_cases(wgpu.renderer()).into_iter().filter(|c| !c.name.starts_with("cjk_flood") || c.name.ends_with('0')).collect();
    // The mode must change something: a flood frame (it has emoji) draws otherwise in Accurate.
    let flood = cases.iter().find(|c| c.name.starts_with("cjk_flood")).expect("a flood frame");
    let mut accurate = metal_window(flood.size);
    assert_ne!(metal.capture(&flood.frame, None, flood.size.0, flood.size.1).unwrap(), accurate.capture(&flood.frame, None, flood.size.0, flood.size.1).unwrap(), "Web mode drew as Accurate");
    metal.set_color_mode(glyphon::ColorMode::Web);
    let mut near = Vec::new();
    match_in_order(&on, &mut wgpu, &mut metal, &cases, "web", &mut near);
    report_near(&near);
}

/// A Metal allocation that fails mid-frame (a grown atlas or row buffer, a staging chunk, a quad
/// buffer, a picture) leaves the CPU side counting changes the GPU never got: the frame is
/// skipped and reported as a lost device, and once recovery rebuilds the window both sides start
/// over and draw as wgpu does.
#[test]
fn a_failed_metal_allocation_starts_both_sides_over() {
    let _one = loss_reports();
    let _ = crate::gpu_pick::take_lost();
    let (_, mut wgpu, _) = text_pair(true);
    let mut cases: Vec<_> = crate::render_pixel_tests::pixel_cases(wgpu.renderer()).into_iter().filter(|c| c.name.starts_with("cjk_flood")).take(40).collect();
    let size = cases[0].size;
    // Many quads grow a quad buffer. The picture frame has the text of the frame before it, so
    // its one new texture is the picture's.
    let quads = Frame { clear: [0.0, 0.0, 0.0, 1.0], fade: 1.0, quads: (0..4000).map(|k| Quad { rect: rect((k % 100) as f32 * 8.0, (k / 100) as f32 * 8.0, 6.0, 6.0), color: [0.5, 0.2, 0.9, 1.0] }).collect(), ..cases[3].frame.clone() };
    let pictured = Frame { images: vec![picture(9, rect(20.5, 30.0, 200.0, 100.0), (17, 11), [0.0, 0.0, 1.0, 1.0], false)], ..cases[5].frame.clone() };
    cases.insert(4, crate::render_pixel_tests::PixelCase { name: "many quads".into(), frame: quads, size });
    cases.insert(7, crate::render_pixel_tests::PixelCase { name: "a picture".into(), frame: pictured, size });
    assert_eq!(cases[6].frame.rows, cases[7].frame.rows);
    let mut metal = metal_window(size);
    let mut lost = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        // The first frame's first allocation (its staging chunk), the quad frame's first (the
        // grown buffer), and the picture frame's texture.
        match i {
            0 | 4 => super::fail_allocations(1),
            7 => super::fail_textures(1),
            _ => {}
        }
        let want = wgpu.capture(&c.frame, None, size.0, size.1).unwrap();
        match metal.render(&c.frame, None, None) {
            Err(crate::gpu_window::RenderError::Skip) => {
                let why = crate::gpu_pick::take_lost().expect("the failure is reported as a lost device");
                assert!(why.starts_with("Metal: "), "{why}");
                if i == 7 {
                    assert!(why.contains("picture"), "not the picture's texture: {why}");
                }
                lost.push(c.name.clone());
                assert!(metal.stale().is_some());
                // As recovery does: the window rebuilt on the device, then it draws again.
                metal.rebuild(&crate::gpu_shared::GpuShared::Metal(test_device().clone()), None).unwrap();
                assert!(metal.stale().is_none());
                assert_eq!(metal.renderer_mut().atlas().generation(), 0, "the atlas started over");
                metal.render(&c.frame, None, None).unwrap().kept.unwrap();
            }
            other => other.unwrap_or_else(|_| panic!("{}: an error that is not a skipped frame", c.name)).kept.unwrap(),
        }
        super::fail_allocations(0);
        super::fail_textures(0);
        let got = super::readback::read_rgba(&test_device().queue, metal.offscreen_texture(), size.0, size.1).unwrap();
        crate::render_pixel_tests::assert_same(&c.name, &want, &got, size.0);
    }
    assert_eq!(lost, [cases[0].name.as_str(), "many quads", "a picture"]);
    assert!(crate::gpu_pick::take_lost().is_none());
}

/// With allocations failing for good, the app's recovery (`gpu_pick::Recovery`, driven here as
/// `gpu_recovery_tick` drives it) tries to rebuild the window `MAX_TRIES` times and gives up,
/// which is when the restart banner shows; meanwhile no frame redraws. A failure again right
/// after a recovery gives up at once.
#[test]
fn a_lasting_allocation_failure_ends_in_the_restart_banner() {
    use crate::gpu_pick::{Action, Recovery, MAX_TRIES};
    use crate::gpu_window::RenderError;
    let _one = loss_reports();
    let _ = crate::gpu_pick::take_lost();
    let frame = Frame { clear: [0.0, 0.0, 0.0, 1.0], fade: 1.0, quads: vec![Quad { rect: rect(1.0, 1.0, 9.0, 9.0), color: [1.0, 0.0, 0.0, 1.0] }], ..Default::default() };
    // Every Metal allocation of this thread fails from here on.
    let mut win = metal_window((64, 32));
    super::fail_allocations(u32::MAX);
    let mut recovery = Recovery::new(1);
    let mut now = std::time::Instant::now();
    assert!(matches!(win.render(&frame, None, None), Err(RenderError::Skip)));
    recovery.lost(now);
    assert!(crate::gpu_pick::take_lost().is_some());
    let mut attempts = 0;
    loop {
        match recovery.poll(now) {
            Action::Attempt { .. } => {
                attempts += 1;
                crate::gpu_pick::next_generation();
                match win.rebuild(&crate::gpu_shared::GpuShared::Metal(test_device().clone()), None) {
                    Ok(()) => panic!("a rebuild with no allocations worked"),
                    Err(_) => recovery.failed(now),
                }
            }
            Action::WaitUntil(t) => now = t,
            Action::GaveUp => break,
            Action::Idle => panic!("recovery went idle while the window is stale"),
        }
    }
    assert_eq!(attempts, MAX_TRIES);
    // The window stays stale and refuses to draw rather than looping.
    assert!(win.stale().is_some());
    assert!(matches!(win.render(&frame, None, None), Err(RenderError::Refused(_))));
    super::fail_allocations(0);

    // A rebuild that works, then the same failure within a minute: recovery gives up.
    let mut win = metal_window((64, 32));
    let mut recovery = Recovery::new(1);
    let now = std::time::Instant::now();
    super::fail_allocations(1);
    let fresh = Frame { quads: vec![Quad { rect: rect(0.0, 0.0, 4.0, 4.0), color: [0.0, 1.0, 0.0, 1.0] }], ..frame.clone() };
    assert!(matches!(win.render(&fresh, None, None), Err(RenderError::Skip)));
    assert!(crate::gpu_pick::take_lost().is_some());
    recovery.lost(now);
    assert!(matches!(recovery.poll(now), Action::Attempt { .. }));
    win.rebuild(&crate::gpu_shared::GpuShared::Metal(test_device().clone()), None).unwrap();
    recovery.recovered(now);
    win.render(&frame, None, None).unwrap().kept.unwrap();
    // The staging chunk is reused now, so a new picture is what allocates.
    let pictured = Frame { images: vec![picture(11, rect(2.0, 2.0, 20.0, 10.0), (5, 3), [0.0, 0.0, 1.0, 1.0], false)], ..frame.clone() };
    super::fail_textures(1);
    assert!(matches!(win.render(&pictured, None, None), Err(RenderError::Skip)));
    super::fail_textures(0);
    assert!(crate::gpu_pick::take_lost().is_some());
    recovery.lost(now + std::time::Duration::from_secs(1));
    assert_eq!(recovery.poll(now), Action::GaveUp);
}

/// A screenshot whose texture cannot be made fails alone: it changed nothing, so no loss is
/// reported, the window is not stale, and the next frame draws as wgpu does.
#[test]
fn a_failed_capture_texture_is_only_an_error() {
    let _one = loss_reports();
    let _ = crate::gpu_pick::take_lost();
    let (_, mut wgpu, _) = text_pair(true);
    let c = crate::render_pixel_tests::pixel_cases(wgpu.renderer()).into_iter().find(|c| !c.frame.rows.is_empty()).expect("a case with text");
    let mut metal = metal_window(c.size);
    super::fail_textures(1);
    assert!(metal.capture(&c.frame, None, c.size.0, c.size.1).is_err(), "the capture texture was made");
    super::fail_textures(0);
    assert!(crate::gpu_pick::take_lost().is_none(), "a failed screenshot was reported as a lost device");
    assert!(metal.stale().is_none());
    metal.render(&c.frame, None, None).unwrap().kept.unwrap();
    let got = super::readback::read_rgba(&test_device().queue, metal.offscreen_texture(), c.size.0, c.size.1).unwrap();
    crate::render_pixel_tests::assert_same(&c.name, &wgpu.capture(&c.frame, None, c.size.0, c.size.1).unwrap(), &got, c.size.0);
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

/// The wgpu window's pixels as its last `render` left them.
fn wgpu_pixels(on: &crate::gpu_window::GpuHandles, win: &WindowGpu) -> Vec<u8> {
    match win {
        WindowGpu::Wgpu(w) => read_back(on, w.offscreen_texture()),
        WindowGpu::Metal(_) => unreachable!(),
    }
}

/// The Metal window's pixels as its last `render` left them.
fn metal_pixels(m: &MetalWindow) -> Vec<u8> {
    let crate::gpu_shared::GpuShared::Metal(shared) = m.shared() else { unreachable!() };
    let t = m.offscreen_texture();
    super::readback::read_rgba(&shared.queue, t, t.width() as u32, t.height() as u32).unwrap()
}

fn wgpu_chain(win: &WindowGpu) -> &crate::render_fx_gpu::Chain {
    match win {
        WindowGpu::Wgpu(w) => w.chain().expect("an effect chain"),
        WindowGpu::Metal(_) => unreachable!(),
    }
}

/// Both windows at `size`, each with `compiled` as its effect over `terms`.
fn effect_pair(size: (u32, u32), compiled: &[crate::render_fx::Compiled], terms: crate::render_fx::RegionTerms) -> (crate::gpu_window::GpuHandles, WindowGpu, MetalWindow) {
    let (on, mut wgpu) = wgpu_window(size);
    let mut metal = metal_window(size);
    wgpu.set_effect(Some(compiled.to_vec()), terms.clone()).unwrap();
    metal.set_effect(Some(compiled.to_vec()), terms).unwrap();
    (on, wgpu, metal)
}

/// Draws `steps` on both windows as the app does (`render` with the frame's region, the scene
/// kept unless the frame is built, `keep_output` from its own region, a frame the chain can leave
/// out left out) and compares every frame and the chains' state after it. The scene switches to
/// `changed` and back on each built frame. Returns how many frames shaded only their region.
fn match_effect_frames(on: &crate::gpu_window::GpuHandles, wgpu: &mut WindowGpu, metal: &mut MetalWindow, frame: &Frame, changed: Option<&Frame>, steps: &[crate::render::tests::RegionStep], name: &str, near: &mut Vec<String>) -> usize {
    let mut showing = frame;
    let mut regions = 0;
    for st in steps {
        if st.switch {
            showing = if std::ptr::eq(showing, frame) { changed.unwrap_or(frame) } else { frame };
        }
        let fx = || crate::gpu_window::FxFrame { uniforms: &st.u, region: Some(&st.both), same_scene: !st.built, keep_output: st.both.own.is_some() };
        let still = wgpu.skip_if_still(&st.u, !st.built);
        assert_eq!(still, metal.skip_if_still(&st.u, !st.built), "{name} {} ms: the chains disagree on a still frame", st.ms);
        if still {
            continue;
        }
        wgpu.render(showing, Some(fx()), None).unwrap().kept.unwrap();
        metal.render(showing, Some(fx()), None).unwrap().kept.unwrap();
        compare(&format!("{name} {} ms", st.ms), &wgpu_pixels(on, wgpu), &metal_pixels(metal), metal.size().0, near);
        let (a, b) = (wgpu_chain(wgpu), metal.chain().expect("a Metal chain"));
        assert_eq!((a.shaded_region(), a.frame_kept()), (b.shaded_region(), b.frame_kept()), "{name} {} ms: shaded region, frame kept", st.ms);
        regions += usize::from(b.shaded_region());
    }
    let (a, b) = (wgpu_chain(wgpu), metal.chain().unwrap());
    assert_eq!((a.uniform_writes(), a.kept_made(), a.skips()), (b.uniform_writes(), b.kept_made(), b.skips()), "{name}: uniform writes, kept outputs made, skips");
    regions
}

/// Every shipped plugin on Metal and on wgpu's Metal backend, through the region tests' timeline
/// (`region_timeline`): the events timeline on one agent screen, and the same with every third
/// frame drawn anew over the next screen's rows on another, Live Lines on for every other plugin.
/// Every frame is byte-equal, the chains shade the same region frames, and a screenshot through
/// the effect (each window's own capture chain) is equal too and differs from the plain picture.
#[test]
fn metal_effects_match_wgpu_for_every_plugin() {
    use crate::render::tests::{agent_frame, preset_of, region_timeline, with_rows_of, Plan, AGENT_SCREENS};
    let plugins: Vec<_> = crate::render_fx::discover(None).0.into_iter().filter(|p| p.builtin()).collect();
    assert_eq!(plugins.len(), 13);
    let probe = metal_window(SIZE);
    let mut near = Vec::new();
    let mut regions = 0;
    for (i, p) in plugins.iter().enumerate() {
        let live_lines = i % 2 == 1;
        let compiled = p.compile_with(&[("LINE_EFFECT", live_lines)]).unwrap();
        let terms = crate::render_fx::chain_region(&[&p.manifest]);
        let preset = preset_of(p);
        let screens = [AGENT_SCREENS[i % AGENT_SCREENS.len()], AGENT_SCREENS[(i + 7) % AGENT_SCREENS.len()]];
        for (k, screen) in screens.iter().enumerate() {
            let (frame, size, bg) = agent_frame(probe.renderer(), screen, preset);
            let next = agent_frame(probe.renderer(), AGENT_SCREENS[(i + 8) % AGENT_SCREENS.len()], preset).0;
            let changed = with_rows_of(&frame, &next, probe.renderer().cell.1);
            let plan = if k == 0 { Plan { events: true, hold: 1000, every: 50, ..Default::default() } } else { Plan { events: true, hold: 500, every: 50, built: 3, ..Default::default() } };
            let steps = region_timeline(probe.renderer().cell, terms.as_deref().expect("a region"), &frame, size, bg, preset, live_lines, &plan);
            let (on, mut wgpu, mut metal) = effect_pair(size, &compiled, terms.clone());
            let name = format!("{} {screen}{}", p.id, if k == 0 { "" } else { " built" });
            regions += match_effect_frames(&on, &mut wgpu, &mut metal, &frame, Some(&changed), &steps, &name, &mut near);
            // A screenshot shows the effect as the screen does: one taken while an event's look
            // runs full screen.
            let st = steps.iter().find(|st| st.ms > 0 && st.both.with_last.is_none()).unwrap_or(steps.last().unwrap());
            let fx = || Some(crate::gpu_window::FxFrame { uniforms: &st.u, region: None, same_scene: false, keep_output: false });
            let shot = metal.capture(&frame, fx(), size.0, size.1).unwrap();
            compare(&format!("{name} capture"), &wgpu.capture(&frame, fx(), size.0, size.1).unwrap(), &shot, size.0, &mut near);
            assert_ne!(shot, metal.capture(&frame, None, size.0, size.1).unwrap(), "{name}: the effect changed nothing");
        }
    }
    assert!(regions > 0, "no frame shaded only its region");
    report_near(&near);
}

/// Effect frames that only animate make no Metal buffer, texture or staging chunk once the first
/// frames sized them: the chain reuses its textures, uniform buffers and staging.
#[test]
fn steady_effect_frames_make_no_metal_objects() {
    use crate::render::tests::{agent_frame, preset_of, region_timeline, Plan};
    let p = crate::render_fx::discover(None).0.into_iter().find(|p| p.id == "lightning").expect("lightning");
    let probe = metal_window(SIZE);
    let preset = preset_of(&p);
    let (frame, size, bg) = agent_frame(probe.renderer(), "codex", preset);
    let terms = crate::render_fx::chain_region(&[&p.manifest]);
    let mut metal = metal_window(size);
    metal.set_effect(Some(p.compile().unwrap()), terms.clone()).unwrap();
    let steps = region_timeline(probe.renderer().cell, terms.as_deref().unwrap(), &frame, size, bg, preset, false, &Plan { hold: 2000, ..Default::default() });
    let made = |m: &MetalWindow| (m.chain().unwrap().objects_made(), m.staging_made(), m.text_objects_made());
    let mut after = None;
    for (n, st) in steps.iter().enumerate() {
        let fx = crate::gpu_window::FxFrame { uniforms: &st.u, region: Some(&st.both), same_scene: !st.built, keep_output: st.both.own.is_some() };
        metal.render(&frame, Some(fx), None).unwrap().kept.unwrap();
        if n == 2 {
            after = Some(made(&metal));
        }
    }
    assert!(steps.len() > 30);
    assert_eq!(made(&metal), after.unwrap(), "animation frames made Metal objects");
    assert!(metal.chain().unwrap().uniform_writes() as usize >= steps.len(), "every frame wrote its clock");
}

/// User plugin folders, each (id, files as name and text), in a temp folder of their own.
fn user_plugins(tag: &str, plugins: &[(&str, &[(&str, &str)])]) -> crate::test_tmp::TempDir {
    let dir = crate::test_tmp::TempDir::new(tag);
    for (id, files) in plugins {
        std::fs::create_dir_all(dir.file(id)).unwrap();
        for (name, text) in *files {
            std::fs::write(dir.file(id).join(name), text).unwrap();
        }
    }
    dir
}

/// `App::fx_sync` for one window: each wanted plugin compiled, the chain built from the ones
/// that did, and every failure in the banner's words. Returns the errors, whether the effect runs
/// and its region terms.
fn sync_effects(win: &mut WindowGpu, wanted: &[&str], user: &std::path::Path) -> (Vec<String>, bool, crate::render_fx::RegionTerms) {
    let (plugins, _) = crate::render_fx::discover(Some(user));
    let (mut shaders, mut errors, mut manifests) = (Vec::new(), Vec::new(), Vec::new());
    for id in wanted {
        let Some(p) = plugins.iter().find(|p| p.id == *id) else {
            errors.push(format!("{id}: no such effect"));
            continue;
        };
        match p.compile_with(&[]) {
            Ok(list) => {
                shaders.extend(list);
                manifests.push(&p.manifest);
            }
            Err(e) => errors.push(e),
        }
    }
    let any = !shaders.is_empty();
    let terms = if any { crate::render_fx::chain_region(&manifests) } else { None };
    let active = match win.set_effect(Some(shaders), terms.clone()) {
        Ok(()) => any,
        Err(e) => {
            errors.push(format!("the graphics card refused an effect: {e}"));
            false
        }
    };
    (errors, active, terms)
}

fn metal_of(win: &WindowGpu) -> &MetalWindow {
    match win {
        WindowGpu::Metal(m) => m,
        WindowGpu::Wgpu(_) => unreachable!(),
    }
}

const BROKEN_GLSL: &str = "// A user's effect with a mistake on line 7.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 c = texture(iChannel0, uv);
    float glow = 0.5 + 0.5 * sin(iTime);
    c.rgb *= glow;
    c.rgb += no_such_value;
    fragColor = c;
}
";

/// A user GLSL plugin with an error on line 7 reports the same text and line on both backends,
/// and the window keeps drawing: alone it leaves no effect (the plain picture), beside a good
/// plugin the good one still runs, equal on both.
#[test]
fn a_bad_plugin_reports_the_same_error_on_metal() {
    let dir = user_plugins("bad", &[("broken", &[("plugin.toml", "name = \"Broken\"\nshaders = [\"broken.glsl\"]\nanimation = \"always\"\n"), ("broken.glsl", BROKEN_GLSL)])]);
    let probe = metal_window(SIZE);
    let (frame, size, bg) = crate::render::tests::agent_frame(probe.renderer(), "codex", [255, 128, 0]);
    let (on, mut wgpu) = wgpu_window(size);
    let mut metal = WindowGpu::Metal(metal_window(size));
    let mut near = Vec::new();
    for wanted in [&["broken"][..], &["broken", "aurora"][..]] {
        let (a, a_active, terms) = sync_effects(&mut wgpu, wanted, &dir);
        let (b, b_active, _) = sync_effects(&mut metal, wanted, &dir);
        assert_eq!(a, b, "{wanted:?}: the errors differ");
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(a[0].starts_with("Broken (broken.glsl):\nline 7: "), "{}", a[0]);
        assert_eq!((a_active, b_active), (wanted.len() > 1, wanted.len() > 1));
        let u = {
            let mut u = crate::render_fx::Uniforms::default();
            let mut clock = crate::render_fx::Clock::default();
            let t0 = std::time::Instant::now();
            clock.frame(&mut u, t0, size, frame.fx.as_ref().and_then(|f| f.cursor), true);
            clock.frame(&mut u, t0 + std::time::Duration::from_millis(400), size, frame.fx.as_ref().and_then(|f| f.cursor), true);
            u.background = [bg.r as f32 / 255.0, bg.g as f32 / 255.0, bg.b as f32 / 255.0];
            u
        };
        // As the app: the effect frame only while one runs.
        let fx = || a_active.then(|| crate::gpu_window::FxFrame { uniforms: &u, region: None, same_scene: false, keep_output: terms.is_some() });
        wgpu.render(&frame, fx(), None).unwrap().kept.unwrap();
        metal.render(&frame, fx(), None).unwrap().kept.unwrap();
        let got = metal_pixels(metal_of(&metal));
        compare(&format!("{wanted:?}"), &wgpu_pixels(&on, &wgpu), &got, size.0, &mut near);
        let plain = metal.capture(&frame, None, size.0, size.1).unwrap();
        assert_eq!(got == plain, !a_active, "{wanted:?}: an effect ran {}", a_active);
    }
    report_near(&near);
}

const USER_WGSL: &str = "
struct Globals {
    iResolution: vec3<f32>,
    iTime: f32,
}
@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var<uniform> g: Globals;
@group(0) @binding(2) var smp: sampler;

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = p.xy / g.iResolution.xy;
    let c = textureSampleLevel(screen, smp, uv + vec2<f32>(0.003 * sin(g.iTime * 3.0 + uv.y * 40.0), 0.0), 0.0);
    let wave = 0.5 + 0.5 * sin(g.iTime * 2.0 + uv.x * 12.0);
    return vec4<f32>(mix(c.rgb, vec3<f32>(1.0) - c.rgb, 0.35 * wave), 0.25);
}
";

/// A user WGSL plugin (its own uniform struct, the same bindings) runs on Metal as on wgpu, over
/// the region timeline: every frame byte-equal, and the effect shows.
#[test]
fn a_user_wgsl_plugin_runs_on_metal() {
    use crate::render::tests::{agent_frame, region_timeline, Plan};
    let dir = user_plugins("wgsl", &[("ripple", &[("plugin.toml", "name = \"Ripple\"\nshaders = [\"ripple.wgsl\"]\nanimation = \"always\"\nregion = [\"cursor:3\"]\n"), ("ripple.wgsl", USER_WGSL)])]);
    let probe = metal_window(SIZE);
    let (frame, size, bg) = agent_frame(probe.renderer(), "hermes", [200, 80, 255]);
    let (on, mut wgpu) = wgpu_window(size);
    let mut metal = WindowGpu::Metal(metal_window(size));
    let (a, active, terms) = sync_effects(&mut wgpu, &["ripple"], &dir);
    let (b, ..) = sync_effects(&mut metal, &["ripple"], &dir);
    assert!(a.is_empty() && b.is_empty() && active, "{a:?} {b:?}");
    let steps = region_timeline(probe.renderer().cell, terms.as_deref().unwrap(), &frame, size, bg, [200, 80, 255], false, &Plan { events: true, hold: 300, every: 50, ..Default::default() });
    let WindowGpu::Metal(mut m) = metal else { unreachable!() };
    let mut near = Vec::new();
    match_effect_frames(&on, &mut wgpu, &mut m, &frame, None, &steps, "ripple", &mut near);
    assert_ne!(metal_pixels(&m), m.capture(&frame, None, size.0, size.1).unwrap(), "the effect changed nothing");
    report_near(&near);
}

/// An effect texture that cannot be made mid-frame follows the allocation rule: the frame is
/// skipped and reported as a lost device; once rebuilt (and the effect built again, as the app
/// does) the window draws as wgpu does. A screenshot chain's texture failing is only an error.
#[test]
fn a_failed_effect_allocation_is_a_lost_device() {
    use crate::render::tests::{agent_frame, preset_of, region_timeline, Plan};
    let _one = loss_reports();
    let _ = crate::gpu_pick::take_lost();
    let p = crate::render_fx::discover(None).0.into_iter().find(|p| p.id == "neon").expect("neon");
    let probe = metal_window(SIZE);
    let preset = preset_of(&p);
    let (frame, size, bg) = agent_frame(probe.renderer(), "opencode", preset);
    let compiled = p.compile().unwrap();
    let terms = crate::render_fx::chain_region(&[&p.manifest]);
    let (on, mut wgpu, mut metal) = effect_pair(size, &compiled, terms.clone());
    let steps = region_timeline(probe.renderer().cell, terms.as_deref().unwrap(), &frame, size, bg, preset, false, &Plan { hold: 400, ..Default::default() });
    let st = &steps[0];
    let fx = || Some(crate::gpu_window::FxFrame { uniforms: &st.u, region: Some(&st.both), same_scene: false, keep_output: st.both.own.is_some() });
    // The frame's first texture is the chain's frame texture.
    super::fail_textures(1);
    assert_eq!(metal.render(&frame, fx(), None).unwrap_err(), crate::gpu_window::RenderError::Skip);
    super::fail_textures(0);
    let why = crate::gpu_pick::take_lost().expect("reported as a lost device");
    assert!(why.starts_with("Metal: ") && why.contains("effect"), "{why}");
    metal.rebuild(&crate::gpu_shared::GpuShared::Metal(test_device().clone()), None).unwrap();
    assert!(metal.chain().is_none(), "a rebuilt window builds its effect again");
    metal.set_effect(Some(compiled.clone()), terms.clone()).unwrap();
    let mut near = Vec::new();
    match_effect_frames(&on, &mut wgpu, &mut metal, &frame, None, &steps, "neon after a lost frame", &mut near);
    // A screenshot's own chain: only an error, and the live chain draws on.
    let mut fresh = metal_window(size);
    fresh.set_effect(Some(compiled), terms).unwrap();
    // The screenshot's own texture first, then the capture chain's frame texture.
    super::fail_textures_after(1, 1);
    let e = fresh.capture(&frame, fx(), size.0, size.1).expect_err("the capture chain's texture was made");
    assert!(e.contains("effect texture"), "failed for another reason: {e}");
    super::fail_textures(0);
    assert!(crate::gpu_pick::take_lost().is_none(), "a failed screenshot was reported as a lost device");
    assert!(fresh.stale().is_none());
    let shot = fresh.capture(&frame, fx(), size.0, size.1).unwrap();
    compare("capture after a failed one", &wgpu.capture(&frame, fx(), size.0, size.1).unwrap(), &shot, size.0, &mut near);
    report_near(&near);
}
