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

/// The font each system draws the cases in. The harness fails if it is missing rather than
/// falling back to another face.
#[cfg(target_os = "linux")]
const FACE: &str = "DejaVu Sans Mono";
#[cfg(target_os = "macos")]
const FACE: &str = "Menlo";
#[cfg(windows)]
const FACE: &str = "Cascadia Mono";
const PX: f32 = 14.0;
const FLOOD_FRAMES: usize = 300;

/// A renderer as the tests make it, on the shared device.
fn fresh(view_formats: bool) -> Renderer {
    let (device, queue) = test_gpu::shared();
    let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb, FACE, PX);
    assert!(r.has_family(FACE), "the pixel harness draws in {FACE}, which is not installed here");
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
    r.draw_offscreen(&c.frame, c.size.0, c.size.1).unwrap_or_else(|e| panic!("{}: the draw failed: {e}", c.name))
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

// The row path (Task 13): rows drawn through glyphon's `RowRenderer` must match the full path,
// which prepares every row through `TextRenderer` each frame.

/// A renderer on the row path (`on`) or the full path.
fn rows(view_formats: bool, on: bool) -> Renderer {
    let mut r = fresh(view_formats);
    r.set_row_instances(on);
    r
}

/// Draws `c` on both renderers and asserts the pictures are the same.
fn same(on: &mut Renderer, off: &mut Renderer, c: &PixelCase, view_formats: bool) {
    let name = format!("{} (view formats {view_formats})", c.name);
    assert_same(&name, &pixels(on, c), &pixels(off, c), c.size.0);
}

fn span(text: &str) -> crate::scene::Span {
    crate::scene::Span { text: text.into(), color: crate::palette::rgb(230, 230, 230), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }
}

fn row(text: &str, x: f32, y: f32, clip: crate::layout::Rect, scale: f32) -> crate::scene::TextRow {
    crate::scene::TextRow { x, y, spans: vec![span(text)].into(), clip, scale, face: crate::render::DEFAULT_FACE }
}

fn text_frame(rows: Vec<crate::scene::TextRow>) -> Frame {
    Frame { clear: [0.0, 0.0, 0.0, 1.0], fade: 1.0, rows, ..Default::default() }
}

/// A pane of `s` filling a window of `size`, drawn with `r`'s cell.
fn screen_case(name: &str, r: &Renderer, s: &crate::screen::Screen, size: (u32, u32), cursor: bool) -> PixelCase {
    let rect = crate::layout::Rect { x: 0.0, y: 0.0, w: size.0 as f32, h: size.1 as f32 };
    let mut frame = Frame { clear: crate::scene::rgba(s.palette.bg, 1.0), fade: 1.0, ..Default::default() };
    let look = crate::cursor_look::CursorLook::shown(cursor);
    crate::scene::pane_with(&mut frame, s, rect, r.cell, true, &look, &Default::default(), None, crate::render::DEFAULT_FACE, None);
    PixelCase { name: name.into(), frame, size }
}

fn grid_size(r: &Renderer, cols: f32, lines: f32) -> (u32, u32) {
    ((cols * r.cell.0).ceil() as u32, (lines * r.cell.1).ceil() as u32)
}

#[test]
fn row_instances_match_the_full_path_on_every_case() {
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let (mut on, mut off) = (rows(vf, true), rows(vf, false));
        for c in pixel_cases(&on) {
            same(&mut on, &mut off, &c, vf);
        }
        assert!(on.row_prepares() > 0, "the row path drew no rows");
        assert_eq!(off.row_prepares(), 0);
    }
}

#[test]
fn scrolled_rows_reuse_their_instances() {
    let mut r = rows(false, true);
    let size = grid_size(&r, 80.0, 20.0);
    let mut s = crate::screen::Screen::new(80, 20);
    for n in 0..20 {
        let end = if n < 19 { "\r\n" } else { "" };
        s.feed(format!("line {n:02} of the scroll test{end}").as_bytes());
    }
    let c = screen_case("twenty rows", &r, &s, size, true);
    pixels(&mut r, &c);
    let before = r.row_prepares();
    assert!(before >= 20, "the first frame prepared {before} rows");
    s.feed(b"\r\nline 20 of the scroll test");
    let c = screen_case("one slot up", &r, &s, size, true);
    pixels(&mut r, &c);
    let grown = r.row_prepares() - before;
    assert!(grown <= 2, "scrolling one line prepared {grown} rows");
}

#[test]
fn fractional_tops_match_the_full_path() {
    use crate::layout::Rect;
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let (mut on, mut off) = (rows(vf, true), rows(vf, false));
        let size = (420, 320);
        let window = Rect { x: 0.0, y: 0.0, w: size.0 as f32, h: size.1 as f32 };
        for k in 0..8 {
            // A fractional step each frame, some frames back to a whole one.
            let dy = [0.0, 0.37, 0.5, 1.0, 1.63, 2.25, 0.0, 20.5][k];
            let mut list = Vec::new();
            for n in 0..12 {
                let y = n as f32 * 20.0 + 0.5 + dy;
                // Half the rows clip to their own line, the others to the window.
                let clip = if n % 2 == 0 { Rect { x: 0.0, y, w: window.w, h: 20.0 } } else { window };
                list.push(row(&format!("row {n} gy{} \u{e9}\u{e8} {k}", n * 3), 4.25, y, clip, 1.0));
            }
            // A porthole heading.
            list.push(row("Heading at scale 1.25", 8.0, 262.5 + dy, window, 1.25));
            let c = PixelCase { name: format!("fractional-{k}"), frame: text_frame(list), size };
            same(&mut on, &mut off, &c, vf);
        }
        assert!(on.row_prepares() > 0);
    }
}

#[test]
fn atlas_growth_and_eviction_mid_run_match() {
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let (mut on, mut off) = (rows(vf, true), rows(vf, false));
        // At 256 px a full screen of CJK does not fit even on the full path (AtlasFull at frame
        // 14), so the cap is 512: the atlas grows from 256 once, then evicts.
        on.atlas().set_max_size(512);
        off.atlas().set_max_size(512);
        let (grown, evicted) = (on.atlas().generation(), on.atlas().evictions());
        for c in pixel_cases(&on).iter().filter(|c| is_flood(c)) {
            same(&mut on, &mut off, c, vf);
        }
        assert!(on.atlas().generation() != grown, "the capped flood never grew the atlas");
        assert!(on.atlas().evictions() > evicted, "the capped flood never evicted");
    }
}

#[test]
fn the_switch_turns_the_row_path_off() {
    let mut r = rows(false, false);
    let probe = pixel_cases(&r).into_iter().next().unwrap();
    pixels(&mut r, &probe);
    assert_eq!(r.row_prepares(), 0);
    // Turned on, rows hold their glyphs; turned off again, every hold is let go.
    r.set_row_instances(true);
    pixels(&mut r, &probe);
    assert!(r.row_prepares() > 0 && r.atlas().held() > 0);
    r.set_row_instances(false);
    assert_eq!(r.atlas().held(), 0, "turning rows off released their glyphs");
    let prepared = r.row_prepares();
    pixels(&mut r, &probe);
    assert_eq!(r.row_prepares(), prepared);
    assert_eq!(r.atlas().held(), 0);
}

/// TextRenderer skips a row whose line box misses its bounds, even where a glyph overhangs into
/// them; the row path must skip it too.
#[test]
fn rows_whose_line_misses_their_bounds_draw_nothing() {
    use crate::layout::Rect;
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let (mut on, mut off) = (rows(vf, true), rows(vf, false));
        let size = (240, 140);
        let ch = on.cell.1;
        let pane = Rect { x: 0.0, y: 50.0, w: 240.0, h: 40.0 };
        let window = Rect { x: 0.0, y: 0.0, w: 240.0, h: 140.0 };
        // Marks stacked below a row ending just above the pane, and above a row starting just
        // below it.
        let below = format!("g{}", "\u{0323}\u{0329}".repeat(6));
        let above = format!("A{}", "\u{0301}\u{0308}".repeat(6));
        let above_y = pane.y - ch - 1.5;
        let below_y = pane.y + pane.h + 1.5;
        let at = |clip: Rect| text_frame(vec![row(&below, 10.0, above_y, clip, 1.0), row(&above, 10.0, below_y, clip, 1.0)]);
        // The case only means something if the overhang reaches into the pane.
        let unclipped = pixels(&mut off, &PixelCase { name: "unclipped".into(), frame: at(window), size });
        let ink = (pane.y as usize..(pane.y + pane.h) as usize).any(|y| (0..size.0 as usize).any(|x| unclipped[(y * size.0 as usize + x) * 4] > 0));
        assert!(ink, "no glyph overhangs into the pane");
        same(&mut on, &mut off, &PixelCase { name: "outside rows".into(), frame: at(pane), size }, vf);
    }
}

/// Frame to frame changes a pane goes through, each on the row path against the full path.
#[test]
fn row_instances_follow_every_kind_of_change() {
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let (mut on, mut off) = (rows(vf, true), rows(vf, false));
        let mut s = crate::screen::Screen::new(60, 16);
        for n in 0..50 {
            s.feed(format!("{n:02} the quick brown fox \x1b[1mjumps\x1b[0m over \u{4e2d}\u{6587}\r\n").as_bytes());
        }
        let mut size = grid_size(&on, 60.0, 16.0);
        let step = |name: &str, on: &mut Renderer, off: &mut Renderer, s: &crate::screen::Screen, size: (u32, u32), cursor: bool| {
            let c = screen_case(name, on, s, size, cursor);
            same(on, off, &c, vf);
        };
        step("start", &mut on, &mut off, &s, size, true);
        s.feed(b"\x1b[4;5Hchanged in place");
        step("one row changed", &mut on, &mut off, &s, size, true);
        s.term_mut().scroll_display(alacritty_terminal::grid::Scroll::Delta(3));
        step("scrolled back 3", &mut on, &mut off, &s, size, true);
        s.term_mut().scroll_display(alacritty_terminal::grid::Scroll::Delta(-1));
        step("scrolled back 2", &mut on, &mut off, &s, size, true);
        s.term_mut().scroll_display(alacritty_terminal::grid::Scroll::Bottom);
        step("back at the bottom", &mut on, &mut off, &s, size, true);
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(2), Column(3)), Side::Left);
        sel.update(Point::new(Line(5), Column(20)), Side::Right);
        s.term_mut().selection = Some(sel);
        step("a selection", &mut on, &mut off, &s, size, true);
        s.term_mut().selection = None;
        step("no selection", &mut on, &mut off, &s, size, true);
        s.feed(b"\x1b[10;30H");
        step("the cursor moved", &mut on, &mut off, &s, size, true);
        step("the cursor blinked off", &mut on, &mut off, &s, size, false);
        size = (size.0 + 37, size.1 + 23);
        step("a bigger window", &mut on, &mut off, &s, size, true);
        for r in [&mut on, &mut off] {
            r.set_font_px(PX + 3.0);
        }
        size = grid_size(&on, 60.0, 16.0);
        step("a bigger font", &mut on, &mut off, &s, size, true);
    }
}

#[test]
fn draw_prefs_turns_the_row_path_off() {
    let probe = fresh(false);
    let case = pixel_cases(&probe).into_iter().next().unwrap();
    let mut globals = crate::globals::Globals::empty(std::path::Path::new("none.toml"));
    globals.set("RedrawOnlyChangedRows", toml::Value::Boolean(false));
    let mut r = fresh(false);
    crate::app::apply_row_switch(&mut r, &globals);
    pixels(&mut r, &case);
    assert_eq!(r.row_prepares(), 0, "the setting off still drew through the row path");
    globals.set("RedrawOnlyChangedRows", toml::Value::Boolean(true));
    crate::app::apply_row_switch(&mut r, &globals);
    pixels(&mut r, &case);
    assert!(r.row_prepares() > 0);
}

/// A font change must reshape the badge and toolbelt rows, not keep the old text.
#[test]
fn a_badge_or_toolbelt_font_change_reshapes_the_rows() {
    for toolbelt in [false, true] {
        let mut s = span("Badge Text 123");
        s.badge = !toolbelt;
        s.toolbelt = toolbelt;
        let clip = crate::layout::Rect { x: 0.0, y: 0.0, w: 300.0, h: 40.0 };
        let mut r = crate::scene::TextRow { x: 4.0, y: 4.0, spans: vec![s].into(), clip, scale: 1.0, face: crate::render::DEFAULT_FACE };
        r.scale = 1.0;
        let frame = text_frame(vec![r]);
        let set = |r: &mut Renderer| {
            if toolbelt {
                r.set_toolbelt_family(FACE);
                r.set_toolbelt_size(20.0);
            } else {
                r.set_badge_family(FACE);
            }
        };
        let mut changed = fresh(false);
        changed.draw_offscreen(&frame, 300, 40).unwrap();
        set(&mut changed);
        let got = changed.draw_offscreen(&frame, 300, 40).unwrap();
        let mut start = fresh(false);
        set(&mut start);
        assert_same(if toolbelt { "toolbelt font change" } else { "badge font change" }, &got, &start.draw_offscreen(&frame, 300, 40).unwrap(), 300);
    }
}

/// Box glyphs cached on the atlas, then rows that grow it: growth re-uploads the box glyphs, so
/// the row path needs the box rasterizer (it panicked without it).
#[test]
fn growth_from_the_rows_keeps_box_glyphs() {
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let (mut on, mut off) = (rows(vf, true), rows(vf, false));
        let size = grid_size(&on, 100.0, 30.0);
        let mut s = crate::screen::Screen::new(100, 30);
        s.feed("\u{250c}\u{2500}\u{2500}\u{2500}\u{2510}\r\n\u{2502} a \u{2502}\r\n\u{2514}\u{2500}\u{2500}\u{2500}\u{2518} \u{2588}\u{2591}".as_bytes());
        let boxes = screen_case("boxes", &on, &s, size, true);
        assert!(!boxes.frame.glyphs.is_empty(), "the frame has no box glyphs");
        same(&mut on, &mut off, &boxes, vf);
        let generation = on.atlas().generation();
        for c in pixel_cases(&on).iter().filter(|c| is_flood(c)).take(40) {
            same(&mut on, &mut off, c, vf);
        }
        assert!(on.atlas().generation() != generation, "the flood never grew the atlas");
        same(&mut on, &mut off, &boxes, vf);
    }
}

fn quad(x: f32, color: [f32; 4]) -> crate::scene::Quad {
    crate::scene::Quad { rect: crate::layout::Rect { x, y: 2.0, w: 3.0, h: 5.0 }, color }
}

fn quad_frame(n: usize, first: [f32; 4]) -> Frame {
    let quads = (0..n).map(|i| quad(i as f32 * 4.0, if i == 0 { first } else { [0.0, 0.5, 0.0, 1.0] })).collect();
    Frame { clear: [0.0, 0.0, 0.0, 1.0], fade: 1.0, quads, ..Default::default() }
}

#[test]
fn persistent_buffers_write_only_what_changed() {
    let mut r = fresh(false);
    // 200 quads is 28800 bytes: seven chunks.
    r.draw_offscreen(&quad_frame(200, [1.0, 0.0, 0.0, 1.0]), 808, 40).unwrap();
    let first = r.bytes_written();
    assert!(first >= 200 * 144, "the first frame uploads everything");
    r.draw_offscreen(&quad_frame(200, [1.0, 0.0, 0.0, 1.0]), 808, 40).unwrap();
    assert_eq!(r.bytes_written(), first, "the same frame uploads nothing");
    // One quad's color, inside the first chunk.
    r.draw_offscreen(&quad_frame(200, [0.0, 0.0, 1.0, 1.0]), 808, 40).unwrap();
    assert!(r.bytes_written() - first <= 4096, "one changed quad wrote {} bytes", r.bytes_written() - first);
    assert!(r.bytes_written() > first);
}

#[test]
fn a_bigger_list_grows_the_buffer() {
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let mut grown = fresh(vf);
        let mut alone = fresh(vf);
        let (small, big) = (quad_frame(3, [1.0, 0.0, 0.0, 1.0]), quad_frame(30, [1.0, 0.0, 0.0, 1.0]));
        grown.draw_offscreen(&small, 200, 40).unwrap();
        let after_big = grown.draw_offscreen(&big, 200, 40).unwrap();
        assert_same("a frame with 10x the quads", &after_big, &alone.draw_offscreen(&big, 200, 40).unwrap(), 200);
        // And back down: only the new length draws, no stale tail.
        let after_small = grown.draw_offscreen(&small, 200, 40).unwrap();
        assert_same("a list that shrank", &after_small, &fresh(vf).draw_offscreen(&small, 200, 40).unwrap(), 200);
    }
}

#[test]
fn persistent_buffers_match_the_full_path() {
    for vf in [true, false] {
        if !can_run(vf) {
            continue;
        }
        let (mut kept, mut full) = (fresh(vf), fresh(vf));
        for c in pixel_cases(&kept) {
            full.forget_buffers();
            let want = pixels(&mut full, &c);
            assert_same(&format!("{} (view formats {vf})", c.name), &pixels(&mut kept, &c), &want, c.size.0);
        }
    }
}

/// Claude Code's bottom area redrawn each frame, a new line above it every other frame: after a
/// warm-up the upload path makes no GPU buffer (each `Queue::write_buffer` made one).
#[test]
fn streaming_frames_make_no_upload_buffers() {
    let mut r = fresh(false);
    let size = grid_size(&r, 120.0, 30.0);
    let mut s = crate::screen::Screen::new(120, 30);
    let bar = "\u{2500}".repeat(116);
    s.feed(b"\r\n\r\n\r\n\r\n");
    let frame = |s: &mut crate::screen::Screen, r: &mut Renderer, tick: usize| {
        let mut out = String::from("\r\x1b[2A\x1b[J");
        if tick % 2 == 0 {
            out.push_str(&format!("the renderer keeps each row it built, line {tick}\r\n"));
        }
        out.push_str(&format!("{} Thinking ({tick}s)\r\n", ["\u{00b7}", "\u{2722}", "\u{2733}"][tick % 3]));
        out.push_str(&format!("\u{256d}{bar}\u{256e}\r\n\u{2502} > {}\u{2502}\r\n\u{2570}{bar}\u{256f}\x1b[1A\r\x1b[4C", " ".repeat(113)));
        s.feed(out.as_bytes());
        let c = screen_case("claude", r, s, size, true);
        assert!(!c.frame.glyphs.is_empty(), "the frame has no box glyphs");
        pixels(r, &c);
    };
    for tick in 0..30 {
        frame(&mut s, &mut r, tick);
    }
    let warm = r.upload_buffers_made();
    for tick in 30..90 {
        frame(&mut s, &mut r, tick);
    }
    assert_eq!(r.upload_buffers_made(), warm, "streaming frames made new upload buffers");
}

/// The CPU half needs no device: a renderer made without one prepares every baseline frame.
#[test]
fn prepare_is_pure_cpu() {
    let mut r = Renderer::cpu_only(FACE, PX);
    for (name, frame, _) in crate::scene::agent_box_tests::baseline_cases() {
        let (list, up) = r.prepare(&frame, 808, 486);
        assert_eq!(list.size, (808, 486), "{name}");
        assert_eq!(list.under.len(), frame.quads.len() * 6, "{name}");
        assert_eq!(list.picture.len(), frame.images.len(), "{name}");
        assert!(list.rows.rows.len() >= frame.rows.len(), "{name}");
        assert!(up.atlas.is_empty(), "{name}");
    }
    assert!(r.draw_offscreen(&Frame::default(), 8, 8).is_err(), "a renderer with no device drew");
}
