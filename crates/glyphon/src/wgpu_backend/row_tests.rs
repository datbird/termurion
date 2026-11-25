//! Termurion patches 6 to 9: the row renderer, its core half and these buffers, against the
//! text renderer, pixel for pixel.

use super::*;
use crate::{
    Attrs, Buffer, Color, ContentType, CustomGlyph, Family, FontSystem, GlyphDraws, Metrics,
    RasterizeCustomGlyphRequest, RasterizedCustomGlyph, Resolution, RowArea, Shaping, SwashCache,
    TextArea, TextBounds, TextRenderer,
};
use wgpu::TextureFormat;

/// Termurion patch 9: `RowRenderer`'s first size and its table's chunk, as the core has them.
const FIRST_CAPACITY: u32 = 4096;
const TABLE_CHUNK_ROWS: u32 = (ROW_TABLE_CHUNK / ROW_UNIFORM_SIZE) as u32;

const W: u32 = 320;
const H: u32 = 240;
const FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;
const MONO: &str = "Termurion Wide Bold Test";
const RTL: &str = "Termurion RTL Test";

fn no_custom_glyph(_: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
    None
}

struct Rig {
    device: Device,
    queue: Queue,
    cache: Cache,
    viewport: Viewport,
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    gpu_atlas: GpuAtlas,
    rows: RowRenderer,
    gpu_rows: GpuRows,
    uploads: UploadBelt,
    draws: RowDraws,
}

impl Rig {
    fn new() -> Self {
        let (device, queue) = crate::test_gpu::shared();
        let cache = Cache::new(&device);
        let mut viewport = Viewport::new(&device, &cache);
        viewport.update(&queue, Resolution { width: W, height: H });
        let mut font_system = FontSystem::new_with_fonts([]);
        for data in [
            &include_bytes!("../../../../tests/fixtures/fonts/TermurionWideBoldTest-Regular.ttf")[..],
            &include_bytes!("../../../../tests/fixtures/fonts/TermurionRtlTest-Regular.ttf")[..],
        ] {
            font_system.db_mut().load_font_data(data.to_vec());
        }
        let atlas = TextAtlas::new(device.limits().max_texture_dimension_2d);
        let gpu_atlas = GpuAtlas::new(&device, &cache, &atlas);
        let rows = RowRenderer::new(&atlas);
        let gpu_rows = GpuRows::new(&device, &cache, FORMAT, MultisampleState::default(), &rows);
        Self {
            device,
            queue,
            cache,
            viewport,
            font_system,
            swash: SwashCache::new(),
            atlas,
            gpu_atlas,
            rows,
            gpu_rows,
            uploads: UploadBelt::new(),
            draws: RowDraws::default(),
        }
    }

    fn shape(&mut self, text: &str, family: &str) -> Buffer {
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(16.0, 20.0));
        buffer.set_text(text, &Attrs::new().family(Family::Name(family)), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    fn prepare_rows<'a>(&mut self, rows: impl IntoIterator<Item = RowArea<'a>> + Clone) {
        self.prepare_rows_with(rows, no_custom_glyph);
    }

    fn prepare_rows_with<'a>(
        &mut self,
        rows: impl IntoIterator<Item = RowArea<'a>> + Clone,
        rasterize: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) {
        self.try_prepare_rows(rows, rasterize).unwrap();
    }

    /// The core's prepare, then its atlas ops and row writes applied and submitted.
    fn try_prepare_rows<'a>(
        &mut self,
        rows: impl IntoIterator<Item = RowArea<'a>> + Clone,
        rasterize: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) -> Result<(), crate::PrepareError> {
        let prepared = self.rows.prepare_cpu(
            &mut self.font_system,
            &mut self.atlas,
            Resolution { width: W, height: H },
            rows,
            &mut self.swash,
            rasterize,
            &mut self.draws,
        );
        self.flush();
        prepared
    }

    fn flush(&mut self) {
        let mut ops = Vec::new();
        self.atlas.take_ops(&mut ops);
        self.gpu_atlas.apply(&self.device, &self.queue, &ops);
        self.gpu_rows.apply(&self.device, &mut self.uploads, &self.draws);
        self.draws.clear_writes();
        self.uploads.submit(&self.queue, []);
    }

    /// Prepares and draws the rows with this rig's `RowRenderer`.
    fn draw_rows<'a>(&mut self, rows: impl IntoIterator<Item = RowArea<'a>> + Clone) -> Vec<u8> {
        self.prepare_rows(rows);
        let (rows, atlas, viewport, draws) = (&self.gpu_rows, &self.gpu_atlas, &self.viewport, &self.draws);
        draw(&self.device, &self.queue, |pass| rows.render(atlas, viewport, pass, draws).unwrap())
    }

    /// Draws the same rows with a fresh `TextRenderer` and atlas, the path rows must match.
    fn draw_text<'a>(&mut self, rows: impl IntoIterator<Item = RowArea<'a>>) -> Vec<u8> {
        let mut atlas = TextAtlas::new(self.device.limits().max_texture_dimension_2d);
        let mut gpu_atlas = GpuAtlas::new(&self.device, &self.cache, &atlas);
        let mut text = GpuText::new(&self.device, &self.cache, FORMAT, MultisampleState::default(), None);
        let areas = rows.into_iter().map(|r| TextArea {
            buffer: r.buffer,
            left: r.left,
            top: r.top,
            scale: 1.0,
            bounds: r.bounds,
            default_color: r.default_color,
            custom_glyphs: &[],
        });
        let mut glyphs = GlyphDraws::default();
        TextRenderer::new()
            .prepare_cpu(
                &mut self.font_system,
                &mut atlas,
                Resolution { width: W, height: H },
                areas,
                &mut self.swash,
                no_custom_glyph,
                &mut glyphs,
            )
            .unwrap();
        let mut ops = Vec::new();
        atlas.take_ops(&mut ops);
        gpu_atlas.apply(&self.device, &self.queue, &ops);
        text.prepare(&self.device, &self.queue, None, &glyphs);
        let viewport = &self.viewport;
        draw(&self.device, &self.queue, |pass| text.render(&gpu_atlas, viewport, pass).unwrap())
    }
}

/// Renders into a cleared offscreen target and reads it back.
fn draw(device: &Device, queue: &Queue, record: impl FnOnce(&mut RenderPass<'_>)) -> Vec<u8> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.05, b: 0.1, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        record(&mut pass);
    }
    let out = device.create_buffer(&BufferDescriptor {
        label: None,
        size: (W * H * 4) as u64,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(W * 4),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));
    out.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = out.slice(..).get_mapped_range().unwrap().to_vec();
    bytes
}

fn row(serial: u64, buffer: &Buffer, left: f32, top: f32, bounds: TextBounds) -> RowArea<'_> {
    RowArea {
        serial,
        buffer,
        left,
        top,
        bounds,
        default_color: Color::rgb(230, 200 - serial as u8 * 10, 120 + serial as u8 * 9),
    }
}

fn whole() -> TextBounds {
    TextBounds { left: 0, top: 0, right: W as i32, bottom: H as i32 }
}

fn b(left: i32, top: i32, right: i32, bottom: i32) -> TextBounds {
    TextBounds { left, top, right, bottom }
}

fn assert_same(rows: &[u8], text: &[u8], what: &str) {
    let blank = draw(&crate::test_gpu::shared().0, &crate::test_gpu::shared().1, |_| {});
    assert_ne!(text, &blank[..], "{what}: the text path drew nothing");
    if let Some(i) = rows.iter().zip(text).position(|(a, b)| a != b) {
        let px = i / 4;
        panic!("{what}: first difference at x {} y {}", px as u32 % W, px as u32 / W);
    }
}

/// Ten rows with mixed bounds: clipped on each side, past the viewport, fractional tops.
fn mixed_rows(rig: &mut Rig) -> (Vec<Buffer>, [(f32, f32, TextBounds); 10]) {
    let texts = [
        ("Clipped on the left side here", MONO),
        ("Clipped on the right side here", MONO),
        ("Top cut through the glyphs", MONO),
        ("Bottom cut through gyphs jq", MONO),
        ("Bounds past every viewport edge", MONO),
        ("Starts left of the viewport edge", MONO),
        ("Hangs below the viewport bottom", MONO),
        ("\u{628}\u{64e}\u{628}\u{650} fractional tops", RTL),
        ("Box cut on all four sides gjy", MONO),
        ("Zero-size bounds draw nothing", MONO),
    ];
    let buffers: Vec<Buffer> = texts.iter().map(|(t, f)| rig.shape(t, f)).collect();
    let placed = [
        (4.0, 2.0, b(37, -5, 400, 400)),
        (4.0, 22.0, b(0, 0, 151, 400)),
        (4.0, 47.0, b(0, 53, 400, 400)),
        (9.25, 64.0, b(0, 0, 400, 77)),
        (2.0, 85.0, b(-100, -100, 1000, 1000)),
        (-31.5, 106.0, whole()),
        (6.0, 228.0, whole()),
        (40.0, 140.5, whole()),
        (12.0, 170.0, b(30, 175, 140, 184)),
        (12.0, 195.0, b(50, 200, 50, 200)),
    ];
    (buffers, placed)
}

#[test]
fn rows_draw_like_the_text_renderer() {
    let mut rig = Rig::new();
    let (buffers, placed) = mixed_rows(&mut rig);
    let areas = || {
        buffers
            .iter()
            .zip(placed)
            .enumerate()
            .map(|(i, (buf, (left, top, bounds)))| row(i as u64, buf, left, top, bounds))
    };
    let rows = rig.draw_rows(areas());
    let text = rig.draw_text(areas());
    assert_same(&rows, &text, "first frame");
    // The second frame draws every row from its kept instances.
    let rows = rig.draw_rows(areas());
    assert_eq!(rig.rows.prepared_rows(), 10);
    assert_same(&rows, &text, "kept rows");
}

#[test]
fn a_moved_whole_row_is_not_prepared_again() {
    for top in [10.0, 10.25] {
        let mut rig = Rig::new();
        let buf = rig.shape("Moves down by whole pixels", MONO);
        rig.draw_rows([row(1, &buf, 3.0, top, whole())]);
        assert_eq!(rig.rows.prepared_rows(), 1);
        let moved = rig.draw_rows([row(1, &buf, 3.0, top + 20.0, whole())]);
        assert_eq!(rig.rows.prepared_rows(), 1, "top {top}: the row was prepared again");
        let fresh = rig.draw_text([row(1, &buf, 3.0, top + 20.0, whole())]);
        assert_same(&moved, &fresh, "moved row");
    }
}

#[test]
fn a_row_moved_by_half_a_pixel_is_prepared_again() {
    let mut rig = Rig::new();
    // Arabic marks sit at a fractional y, so the truncation can move them apart.
    let buf = rig.shape("\u{628}\u{64e}\u{628}\u{650}\u{628}\u{64e}", RTL);
    rig.draw_rows([row(1, &buf, 3.0, 10.0, whole())]);
    let moved = rig.draw_rows([row(1, &buf, 3.0, 10.5, whole())]);
    assert_eq!(rig.rows.prepared_rows(), 2);
    let fresh = rig.draw_text([row(1, &buf, 3.0, 10.5, whole())]);
    assert_same(&moved, &fresh, "half-pixel move");
}

#[test]
fn rows_not_drawn_release_their_glyphs() {
    let mut rig = Rig::new();
    let a = rig.shape("Held while drawn", MONO);
    let c = rig.shape("\u{628}\u{64e}\u{628}\u{650}", RTL);
    rig.prepare_rows([row(1, &a, 0.0, 0.0, whole()), row(2, &c, 0.0, 20.0, whole())]);
    assert!(rig.atlas.held() > 0);
    rig.prepare_rows([]);
    assert_eq!(rig.atlas.held(), 0);
    // Keys dropped by clear are released at the next prepare.
    rig.prepare_rows([row(1, &a, 0.0, 0.0, whole())]);
    assert!(rig.atlas.held() > 0);
    rig.rows.clear();
    rig.prepare_rows([]);
    assert_eq!(rig.atlas.held(), 0);
}

#[test]
fn growth_prepares_every_row_again() {
    let mut rig = Rig::new();
    let first = rig.shape("The first row stays the same", MONO);
    rig.draw_rows([row(1, &first, 0.0, 0.0, whole())]);
    assert_eq!(rig.rows.prepared_rows(), 1);
    let size = rig.atlas.mask_size();
    // Large glyphs fill the 256 px atlas; every row holds them, so it must grow.
    let mut big = Buffer::new(&mut rig.font_system, Metrics::new(90.0, 100.0));
    big.set_text(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        &Attrs::new().family(Family::Name(MONO)),
        Shaping::Advanced,
        None,
    );
    big.shape_until_scroll(&mut rig.font_system, false);
    let generation = rig.atlas.generation();
    let areas = || [row(1, &first, 0.0, 0.0, whole()), row(2, &big, 0.0, 20.0, whole())];
    let rows = rig.draw_rows(areas());
    assert!(rig.atlas.generation() > generation, "the atlas did not grow");
    assert!(rig.atlas.mask_size() > size);
    // The kept first row is reused in the pass that grew, then both rows are prepared again.
    assert_eq!(rig.rows.prepared_rows(), 1 + 1 + 2);
    let text = rig.draw_text(areas());
    assert_same(&rows, &text, "after growth");
}

#[test]
fn many_rows_grow_the_buffers_and_scroll_from_kept_instances() {
    let mut rig = Rig::new();
    let buffers: Vec<Buffer> = (0..100)
        .map(|n| rig.shape(&format!("{n:03} the row text that fills forty-four cells"), MONO))
        .collect();
    let areas = |up: usize| {
        buffers.iter().enumerate().skip(up).map(move |(n, buf)| {
            row(n as u64 % 10, buf, 1.0, (n - up) as f32 * 2.4 + 0.25, whole())
        })
    };
    // Distinct serials, but the same colors as `row` makes them.
    let areas = |up: usize| {
        areas(up).enumerate().map(move |(i, mut r)| {
            r.serial = (i + up) as u64 + 100;
            r
        })
    };
    let rows = rig.draw_rows(areas(0));
    assert!(rig.rows.capacity > FIRST_CAPACITY, "the instance buffer did not double");
    let text = rig.draw_text(areas(0));
    assert_same(&rows, &text, "100 rows");
    let rows = rig.draw_rows(areas(3));
    assert_eq!(rig.rows.prepared_rows(), 100, "scrolled rows were prepared again");
    let text = rig.draw_text(areas(3));
    assert_same(&rows, &text, "scrolled 3 up");
}

/// A new row every frame, the others moving up: after a warm-up the belt reuses its chunks
/// and the rows still match the text path.
#[test]
fn streaming_rows_reuse_the_upload_chunks() {
    let mut rig = Rig::new();
    let buffers: Vec<Buffer> = (0..80).map(|n| rig.shape(&format!("{n:03} a streamed line"), MONO)).collect();
    let areas = |first: usize| (first..first + 8).map(|n| RowArea { serial: n as u64 + 1000, ..row(1, &buffers[n], 1.0, (n - first) as f32 * 20.0, whole()) }).collect::<Vec<_>>();
    for first in 0..20 {
        rig.draw_rows(areas(first));
    }
    let warm = rig.uploads.buffers_made();
    assert!(warm > 0, "nothing went through the belt");
    for first in 20..70 {
        rig.draw_rows(areas(first));
    }
    assert_eq!(rig.uploads.buffers_made(), warm, "streaming rows made new staging buffers");
    let rows = rig.draw_rows(areas(70));
    let text = rig.draw_text(areas(70));
    assert_same(&rows, &text, "streamed rows");
}

#[test]
fn a_released_slot_is_used_again() {
    let mut rig = Rig::new();
    let a = rig.shape("First row of text", MONO);
    let c = rig.shape("Second row of text", MONO);
    let d = rig.shape("Third row of text", MONO);
    rig.prepare_rows([row(1, &a, 0.0, 0.0, whole()), row(2, &c, 0.0, 20.0, whole())]);
    let end = rig.rows.end;
    rig.prepare_rows([row(2, &c, 0.0, 20.0, whole())]);
    rig.prepare_rows([row(2, &c, 0.0, 20.0, whole()), row(3, &d, 0.0, 40.0, whole())]);
    assert_eq!(rig.rows.end, end, "the new row took a fresh slot instead of the freed one");
    let rows = rig.draw_rows([row(2, &c, 0.0, 20.0, whole()), row(3, &d, 0.0, 40.0, whole())]);
    let text = rig.draw_text([row(2, &c, 0.0, 20.0, whole()), row(3, &d, 0.0, 40.0, whole())]);
    assert_same(&rows, &text, "reused slot");
}

/// The last frame's rows must not pin a full atlas: TextRenderer draws each of these frames,
/// so the row renderer must too.
#[test]
fn rows_from_last_frame_never_pin_a_full_atlas() {
    let mut rig = Rig::new();
    rig.atlas.set_max_size(256);
    let big = |rig: &mut Rig, text: &str| {
        let mut buf = Buffer::new(&mut rig.font_system, Metrics::new(100.0, 110.0));
        buf.set_text(text, &Attrs::new().family(Family::Name(MONO)), Shaping::Advanced, None);
        buf.shape_until_scroll(&mut rig.font_system, false);
        buf
    };
    let first = big(&mut rig, "abcdefghi");
    let second = big(&mut rig, "nopqrsuvx");
    rig.draw_rows([row(1, &first, 0.0, 0.0, whole())]);
    rig.atlas.trim();
    let held = rig.atlas.held();
    for frame in 1..4 {
        let rows = rig.draw_rows([row(2, &second, 0.0, 0.0, whole())]);
        rig.atlas.trim();
        assert_eq!(rig.atlas.held(), held, "frame {frame}: holds piled up");
        let text = rig.draw_text([row(2, &second, 0.0, 0.0, whole())]);
        assert_same(&rows, &text, "after the swap");
    }
    assert_eq!(rig.atlas.generation(), 0, "the atlas grew past its cap");
}

/// A row whose line box misses its bounds is skipped, like TextRenderer skips the run, even
/// where its glyphs reach into the bounds.
#[test]
fn a_line_outside_its_bounds_is_culled() {
    let mut rig = Rig::new();
    // Glyphs twice the line height overhang the line box above and below.
    let mut tall = Buffer::new(&mut rig.font_system, Metrics::new(32.0, 10.0));
    tall.set_text("Tall gjy glyphs", &Attrs::new().family(Family::Name(MONO)), Shaping::Advanced, None);
    tall.shape_until_scroll(&mut rig.font_system, false);
    let shown = rig.shape("A row inside its bounds", MONO);
    let pane = b(0, 60, W as i32, 120);
    let areas = |bounds| [row(1, &shown, 4.0, 80.0, whole()), row(2, &tall, 4.0, 48.5, bounds), row(3, &tall, 4.0, 121.5, bounds)];
    // The overhang reaches into the pane when nothing clips it.
    let unclipped = rig.draw_text(areas(whole()));
    let control = rig.draw_text([row(1, &shown, 4.0, 80.0, whole())]);
    let px = |y: i32| (y * W as i32 * 4) as usize..((y + 1) * W as i32 * 4) as usize;
    assert!((60..120).any(|y| unclipped[px(y)] != control[px(y)]), "no overhang reaches the pane");
    let rows = rig.draw_rows(areas(pane));
    let text = rig.draw_text(areas(pane));
    assert_same(&rows, &text, "culled lines");
    assert_eq!(rig.rows.prepared_rows(), 1, "a culled row was prepared");
}

fn solid(req: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
    Some(RasterizedCustomGlyph {
        data: vec![255; req.width as usize * req.height as usize],
        content_type: ContentType::Mask,
    })
}

/// Growth from the rows re-uploads every cached glyph, custom ones too, so it needs the
/// caller's rasterizer (a box glyph cached by TextRenderer used to panic here).
#[test]
fn growth_from_the_rows_keeps_custom_glyphs() {
    let mut rig = Rig::new();
    let blank = rig.shape("", MONO);
    let glyphs = [CustomGlyph {
        id: 7,
        width: 12.0,
        height: 12.0,
        snap_to_physical_pixel: true,
        ..Default::default()
    }];
    let area = TextArea {
        buffer: &blank,
        left: 0.0,
        top: 0.0,
        scale: 1.0,
        bounds: whole(),
        default_color: Color::rgb(255, 255, 255),
        custom_glyphs: &glyphs,
    };
    TextRenderer::new()
        .prepare_cpu(&mut rig.font_system, &mut rig.atlas, Resolution { width: W, height: H }, [area], &mut rig.swash, solid, &mut GlyphDraws::default())
        .unwrap();
    rig.flush();
    let mut big = Buffer::new(&mut rig.font_system, Metrics::new(90.0, 100.0));
    big.set_text(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        &Attrs::new().family(Family::Name(MONO)),
        Shaping::Advanced,
        None,
    );
    big.shape_until_scroll(&mut rig.font_system, false);
    let generation = rig.atlas.generation();
    rig.prepare_rows_with([row(1, &big, 0.0, 0.0, whole())], solid);
    assert!(rig.atlas.generation() > generation, "the atlas did not grow");
}

/// A kept row that is wanted but culled still holds its glyphs; a full atlas must let them go
/// and try again rather than fail.
#[test]
fn a_full_atlas_is_retried_after_dropping_unreached_rows() {
    let mut rig = Rig::new();
    rig.atlas.set_max_size(256);
    let big = |rig: &mut Rig, text: &str| {
        let mut buf = Buffer::new(&mut rig.font_system, Metrics::new(100.0, 110.0));
        buf.set_text(text, &Attrs::new().family(Family::Name(MONO)), Shaping::Advanced, None);
        buf.shape_until_scroll(&mut rig.font_system, false);
        buf
    };
    let first = big(&mut rig, "abcdefghi");
    let second = big(&mut rig, "nopqrsuvx");
    rig.draw_rows([row(1, &first, 0.0, 0.0, whole())]);
    rig.atlas.trim();
    let off = b(0, 500, W as i32, 600);
    let areas = || [row(1, &first, 0.0, 0.0, off), row(2, &second, 0.0, 0.0, whole())];
    let rows = rig.draw_rows(areas());
    let text = rig.draw_text(areas());
    assert_same(&rows, &text, "after the retry");
    assert_eq!(rig.atlas.generation(), 0, "the atlas grew past its cap");
}

/// A 56-row screen of mixed lengths (a few blank), rows overlapping so the draw order shows.
fn screen_rows(rig: &mut Rig) -> Vec<Buffer> {
    (0..57)
        .map(|n| {
            let text = match n % 7 {
                3 => String::new(),
                5 => format!("{n:02} a longer row that runs well past the edge of the window, past sixty-four glyphs"),
                _ => format!("{n:02} row {}", "x".repeat(n % 11)),
            };
            rig.shape(&text, MONO)
        })
        .collect()
}

/// Rows `first..first + 56` of `buffers`, one serial per buffer, 4 px apart.
fn screen_areas(buffers: &[Buffer], first: usize) -> Vec<RowArea<'_>> {
    (first..first + 56)
        .map(|n| RowArea {
            serial: n as u64 + 2000,
            ..row(1, &buffers[n], 2.0, (n - first) as f32 * 4.0, whole())
        })
        .collect()
}

#[test]
fn batched_rows_draw_like_one_draw_per_row() {
    let mut rig = Rig::new();
    let (buffers, placed) = mixed_rows(&mut rig);
    let areas = || {
        buffers
            .iter()
            .zip(placed)
            .enumerate()
            .map(|(i, (buf, (left, top, bounds)))| row(i as u64, buf, left, top, bounds))
            .collect::<Vec<_>>()
    };
    let rows = rig.draw_rows(areas());
    assert!(rig.rows.draws_issued() < 10, "the mixed rows drew one by one");
    let text = rig.draw_text(areas());
    assert_same(&rows, &text, "mixed bounds");

    let mut rig = Rig::new();
    let buffers = screen_rows(&mut rig);
    let rows = rig.draw_rows(screen_areas(&buffers, 0));
    assert_eq!(rig.rows.draws_issued(), 1, "a fresh screen is not one batch");
    let text = rig.draw_text(screen_areas(&buffers, 0));
    assert_same(&rows, &text, "56 rows");
}

#[test]
fn a_scrolled_screen_draws_in_few_batches() {
    let mut rig = Rig::new();
    let buffers = screen_rows(&mut rig);
    rig.draw_rows(screen_areas(&buffers, 0));
    let prepared = rig.rows.prepared_rows();
    let rows = rig.draw_rows(screen_areas(&buffers, 1));
    assert_eq!(rig.rows.prepared_rows(), prepared + 1, "kept rows were prepared again");
    assert!(rig.rows.draws_issued() <= 3, "the scroll drew {} batches", rig.rows.draws_issued());
    let text = rig.draw_text(screen_areas(&buffers, 1));
    assert_same(&rows, &text, "scrolled one up");
}

/// A short row in a slot a long row left: the stale instances past its end draw nothing,
/// even inside one batch with its neighbors.
#[test]
fn padding_draws_nothing() {
    let mut rig = Rig::new();
    let above = rig.shape("The row above", MONO);
    let long = rig.shape(&"Wgjy".repeat(15), MONO);
    let short = rig.shape("abc", MONO);
    let below = rig.shape("The row below", MONO);
    rig.draw_rows([row(1, &above, 0.0, 20.0, whole()), row(2, &long, 0.0, 40.0, whole()), row(3, &below, 0.0, 60.0, whole())]);
    let end = rig.rows.end;
    let areas = || [row(1, &above, 0.0, 20.0, whole()), row(4, &short, 0.0, 40.0, whole()), row(3, &below, 0.0, 60.0, whole())];
    let rows = rig.draw_rows(areas());
    assert_eq!(rig.rows.end, end, "the short row did not take the long row's slot");
    assert_eq!(rig.rows.draws_issued(), 1, "the three rows did not draw as one batch");
    let text = rig.draw_text(areas());
    assert_same(&rows, &text, "short row in a long row's slot");
}

#[test]
fn rows_past_one_table_chunk_draw_right() {
    let mut rig = Rig::new();
    let letters: Vec<Buffer> = (0..26).map(|n| rig.shape(&char::from(b'a' + n).to_string(), MONO)).collect();
    let areas = || {
        (0..600)
            .map(|n| RowArea {
                serial: n as u64 + 5000,
                ..row(1, &letters[n % 26], (n % 30) as f32 * 10.0, (n / 30) as f32 * 12.0, whole())
            })
            .collect::<Vec<_>>()
    };
    let rows = rig.draw_rows(areas());
    let draws = rig.rows.draws_issued();
    assert!((2..=3).contains(&draws), "600 rows drew in {draws} batches");
    let text = rig.draw_text(areas());
    assert_same(&rows, &text, "600 rows");
    // Kept rows past the first chunk draw right from the table too.
    let rows = rig.draw_rows(areas());
    assert_same(&rows, &text, "600 kept rows");
}

#[test]
fn the_table_grows_under_kept_rows() {
    let mut rig = Rig::new();
    let letters: Vec<Buffer> = (0..26).map(|n| rig.shape(&char::from(b'a' + n).to_string(), MONO)).collect();
    let areas = |count: usize| {
        (0..count)
            .map(|n| RowArea {
                serial: n as u64 + 7000,
                ..row(1, &letters[n % 26], (n % 30) as f32 * 10.0, (n / 30) as f32 * 12.0, whole())
            })
            .collect::<Vec<_>>()
    };
    rig.draw_rows(areas(100));
    assert_eq!(rig.rows.table_rows, TABLE_CHUNK_ROWS);
    // The 100 kept rows keep their ids and must be written into the grown table.
    let rows = rig.draw_rows(areas(600));
    assert_eq!(rig.rows.prepared_rows(), 600, "kept rows were prepared again");
    assert!(rig.rows.table_rows > TABLE_CHUNK_ROWS, "the row table did not grow");
    let text = rig.draw_text(areas(600));
    assert_same(&rows, &text, "100 kept rows in a grown table");
}

#[test]
fn a_failed_prepare_draws_nothing() {
    let mut rig = Rig::new();
    rig.atlas.set_max_size(256);
    let small = rig.shape("A row that drew", MONO);
    rig.draw_rows([row(1, &small, 0.0, 0.0, whole())]);
    assert_eq!(rig.rows.draws_issued(), 1);
    // A glyph bigger than the capped atlas never fits.
    let mut huge = Buffer::new(&mut rig.font_system, Metrics::new(400.0, 420.0));
    huge.set_text("W", &Attrs::new().family(Family::Name(MONO)), Shaping::Advanced, None);
    huge.shape_until_scroll(&mut rig.font_system, false);
    let failed = rig.try_prepare_rows([row(1, &small, 0.0, 0.0, whole()), row(2, &huge, 0.0, 20.0, whole())], no_custom_glyph);
    assert!(failed.is_err(), "the huge glyph fit");
    assert_eq!(rig.rows.draws_issued(), 0, "render would replay the last frame");
    assert!(rig.draws.batches.is_empty(), "render would replay the last frame");
}
