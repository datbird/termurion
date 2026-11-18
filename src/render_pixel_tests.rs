//! A pixel harness: the same frame must draw to the same pixels however the renderer got there.
//! Later row-renderer work is checked against these cases (and the GL backend through
//! `TERMURION_TEST_BACKEND=gl`).

use crate::render::tests::agent_frame;
use crate::render::{test_gpu, Renderer};
use crate::scene::Frame;

pub(crate) struct PixelCase {
    pub name: String,
    pub frame: Frame,
    pub size: (u32, u32),
}

const FACE: &str = "Cascadia Mono";
const PX: f32 = 14.0;
const FLOOD_FRAMES: usize = 300;

/// A renderer as the tests make it, on the shared device.
fn fresh(view_formats: bool) -> Renderer {
    let (device, queue) = test_gpu::shared();
    let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb, FACE, PX);
    r.set_view_formats(view_formats);
    r
}

/// One distinct row of CJK, emoji and a little ASCII per call (a fixed generator, no clock).
fn flood_row(n: usize) -> String {
    let mut seed = (n as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15);
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut row = format!("{n:03} ");
    for _ in 0..22 {
        let v = next();
        let c = match v % 8 {
            0 => char::from_u32(0x1f600 + (v >> 8) as u32 % 0x50).unwrap(),
            1 => char::from_u32(b'a' as u32 + (v >> 8) as u32 % 26).unwrap(),
            _ => char::from_u32(0x4e00 + (v >> 8) as u32 % 0x51a5).unwrap(),
        };
        row.push(c);
    }
    row
}

/// Every baseline frame, every recorded agent screen, and the 300 frames of `cjk_flood`.
pub(crate) fn pixel_cases(r: &Renderer) -> Vec<PixelCase> {
    let mut out: Vec<PixelCase> = crate::scene::agent_box_tests::baseline_cases()
        .into_iter()
        .map(|(name, frame, _)| PixelCase { name, frame, size: (808, 486) })
        .collect();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/agents/");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let f = e.ok()?.file_name().into_string().ok()?;
            f.strip_suffix(".ansi").or_else(|| f.strip_suffix(".raw")).map(String::from)
        })
        .collect();
    names.sort();
    for name in names {
        let (frame, size, _) = agent_frame(r, &name, [255, 0, 0]);
        out.push(PixelCase { name: format!("agent-{name}"), frame, size });
    }
    // One new row per frame, the screen scrolling once it is full.
    let cell = r.cell;
    let size = ((100.0 * cell.0).ceil() as u32, (30.0 * cell.1).ceil() as u32);
    let rect = crate::layout::Rect { x: 0.0, y: 0.0, w: size.0 as f32, h: size.1 as f32 };
    let mut s = crate::screen::Screen::new(100, 30);
    for n in 0..FLOOD_FRAMES {
        s.feed(format!("{}\r\n", flood_row(n)).as_bytes());
        let mut frame = Frame { clear: crate::scene::rgba(s.palette.bg, 1.0), fade: 1.0, ..Default::default() };
        let look = crate::cursor_look::CursorLook::shown(true);
        crate::scene::pane_with(&mut frame, &s, rect, cell, true, &look, &Default::default(), None, crate::render::DEFAULT_FACE, None);
        out.push(PixelCase { name: format!("cjk_flood-{n:03}"), frame, size });
    }
    out
}

pub(crate) fn pixels(r: &mut Renderer, c: &PixelCase) -> Vec<u8> {
    r.draw_offscreen(&c.frame, c.size.0, c.size.1).unwrap()
}

/// Panics on the first difference, naming its pixel and how many differ.
pub(crate) fn assert_same(name: &str, a: &[u8], b: &[u8], w: u32) {
    assert_eq!(a.len(), b.len(), "{name}: the pictures differ in size");
    let px = |s: &[u8], i: usize| [s[i * 4], s[i * 4 + 1], s[i * 4 + 2], s[i * 4 + 3]];
    let n = a.len() / 4;
    let mut first = None;
    let mut count = 0;
    for i in 0..n {
        if px(a, i) != px(b, i) {
            count += 1;
            first.get_or_insert(i);
        }
    }
    if let Some(i) = first {
        let w = w as usize;
        panic!("{name}: {count} pixels differ, first at x {} y {}: {:?} vs {:?}", i % w, i / w, px(a, i), px(b, i));
    }
}

fn is_flood(c: &PixelCase) -> bool {
    c.name.starts_with("cjk_flood")
}

/// Whether this device can draw with `view_formats` on (GL cannot: no VIEW_FORMATS downlevel flag).
fn can_run(view_formats: bool) -> bool {
    let (_, adapter, _, _) = test_gpu::shared_all();
    let ok = !view_formats || adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::VIEW_FORMATS);
    if !ok {
        eprintln!("skipped: this backend has no view formats");
    }
    ok
}

fn drawn_after_others(view_formats: bool) {
    let probe = fresh(view_formats);
    // Every other flood frame is the same kind of draw as the next: a sample keeps this quick.
    let cases: Vec<PixelCase> = pixel_cases(&probe).into_iter().enumerate().filter(|(i, c)| !is_flood(c) || i % 60 == 0).map(|(_, c)| c).collect();
    let fresh_pixels: Vec<Vec<u8>> = cases.iter().map(|c| pixels(&mut fresh(view_formats), c)).collect();
    let mut shared = fresh(view_formats);
    for pass in 0..2 {
        for (c, want) in cases.iter().zip(&fresh_pixels) {
            assert_same(&format!("{} (pass {pass}, view formats {view_formats})", c.name), &pixels(&mut shared, c), want, c.size.0);
        }
    }
}

fn scroll_matches_fresh(view_formats: bool) {
    let mut scrolling = fresh(view_formats);
    let cases: Vec<PixelCase> = pixel_cases(&scrolling).into_iter().filter(is_flood).collect();
    assert_eq!(cases.len(), FLOOD_FRAMES);
    for (n, c) in cases.iter().enumerate() {
        let got = pixels(&mut scrolling, c);
        if n % 10 == 0 {
            assert_same(&format!("{} (view formats {view_formats})", c.name), &got, &pixels(&mut fresh(view_formats), c), c.size.0);
        }
    }
}

#[test]
fn every_case_draws_the_same_after_other_frames() {
    for on in [true, false] {
        if can_run(on) {
            drawn_after_others(on);
        }
    }
}

#[test]
fn the_scroll_matches_fresh_draws() {
    for on in [true, false] {
        if can_run(on) {
            scroll_matches_fresh(on);
        }
    }
}

#[test]
fn assert_same_names_the_first_difference() {
    let a = vec![0u8; 4 * 6];
    let mut b = a.clone();
    b[4 * 4 + 1] = 9;
    b[4 * 5] = 1;
    let err = std::panic::catch_unwind(|| assert_same("x", &a, &b, 3)).unwrap_err();
    let msg = err.downcast_ref::<String>().unwrap();
    assert!(msg.contains("2 pixels differ") && msg.contains("x 1 y 1") && msg.contains("[0, 9, 0, 0]"), "{msg}");
}
