//! Termurion patch 6: a text renderer that keeps each row's prepared glyph instances on the GPU,
//! so a row that did not change costs no `prepare_glyph` work in the next frame. Clipping to the
//! row's bounds happens in the vertex shader (`row_shader.wgsl`), with the same integer steps as
//! `prepare_glyph`, so the pixels match `TextRenderer` byte for byte.
//!
//! Termurion patch 9: this is the CPU half. It decides slots, row ids and batches, and hands a
//! backend the buffer writes and draws as [`RowDraws`].

use crate::{
    text_render::{prepare_glyph, text_glyph_image, GlyphBounds, GlyphMetadata, GlyphSystem, GlyphonCacheKey},
    Buffer, Color, FontSystem, GlyphKey, GlyphToRender, PrepareError, RasterizeCustomGlyphRequest,
    RasterizedCustomGlyph, Resolution, SwashCache, TextAtlas, TextBounds,
};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    mem,
    ops::Range,
    slice,
};

/// One row of text for [`RowRenderer::prepare_cpu`].
#[derive(Clone)]
pub struct RowArea<'a> {
    /// Names the buffer's content: give a new serial whenever the buffer is shaped again.
    pub serial: u64,
    /// The shaped text of the row.
    pub buffer: &'a Buffer,
    /// The left edge of the buffer.
    pub left: f32,
    /// The top edge of the buffer.
    pub top: f32,
    /// The visible bounds of the row.
    pub bounds: TextBounds,
    /// The color of glyphs that carry none.
    pub default_color: Color,
}

/// Termurion patch 6: the size of `RowUniform` in `row_shader.wgsl`.
pub const ROW_UNIFORM_SIZE: u64 = 32;
/// Termurion patch 8: the size of `RowTable` in `row_shader.wgsl`, the most a GLES 3.0 uniform
/// block holds. A draw binds the chunk its `RowBatch::chunk` names.
pub const ROW_TABLE_CHUNK: u64 = 16384;

/// The smallest slot, in instances.
const MIN_SLOT: u32 = 64;
/// The instance buffer's first size, in instances.
const FIRST_CAPACITY: u32 = 4096;
/// Rows in one chunk of the row table, `RowTable` in `row_shader.wgsl`.
const TABLE_CHUNK_ROWS: u32 = (ROW_TABLE_CHUNK / ROW_UNIFORM_SIZE) as u32;
const INSTANCE_SIZE: usize = mem::size_of::<GlyphToRender>();
/// Write bytes kept between frames; a bigger burst (a whole screen after growth) is let go.
const KEEP_BYTES: usize = 8 << 20;

/// Termurion patch 9: one change to the row renderer's GPU buffers. A backend applies them in
/// order; byte ranges index [`RowDraws::bytes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowWrite {
    /// The instance buffer and its row id buffer are made again with room for `capacity`
    /// instances. What they held is gone; every live slot is written again after this.
    Grow { capacity: u32 },
    /// The row table is made again with room for `rows` entries (whole chunks).
    GrowTable { rows: u32 },
    /// Glyph instances (`GlyphToRender` bytes) from instance `at`.
    Instances { at: u32, bytes: Range<usize> },
    /// Row ids (one `u32` per instance, native endian) from instance `at`.
    Ids { at: u32, bytes: Range<usize> },
    /// Row table entries (`ROW_UNIFORM_SIZE` bytes each) from row id `first`.
    Table { first: u32, bytes: Range<usize> },
}

/// Termurion patch 8: rows in adjacent slots, drawn with one call: instances `start..end`,
/// with the row table's chunk `chunk` bound (offset `chunk * ROW_TABLE_CHUNK`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowBatch {
    pub chunk: u32,
    pub start: u32,
    pub end: u32,
}

/// Termurion patch 9: what [`RowRenderer::prepare_cpu`] hands a backend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowDraws {
    /// Buffer changes, oldest first. `prepare_cpu` adds to them; the caller empties them with
    /// `clear_writes` once a backend applied them.
    pub writes: Vec<RowWrite>,
    pub bytes: Vec<u8>,
    /// The draws of the last `prepare_cpu`, in order (empty after a failed one).
    pub batches: Vec<RowBatch>,
}

impl RowDraws {
    /// Forgets the applied writes, keeping their storage unless a burst made it large.
    pub fn clear_writes(&mut self) {
        self.writes.clear();
        self.bytes.clear();
        if self.bytes.capacity() > KEEP_BYTES {
            self.bytes = Vec::new();
        }
    }

    /// The bytes a write's range names.
    pub fn bytes_of(&self, range: &Range<usize>) -> &[u8] {
        &self.bytes[range.clone()]
    }
}

/// `RowUniform` in `row_shader.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RowUniform {
    offset: [i32; 2],
    _pad: [i32; 2],
    clip: [i32; 4],
}

const _: () = assert!(mem::size_of::<RowUniform>() as u64 == ROW_UNIFORM_SIZE);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RowKey {
    serial: u64,
    left: u32,
    color: u32,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    start: u32,
    /// The capacity is `MIN_SLOT << class`.
    class: u32,
}

struct Entry {
    /// The top the instances were made at.
    top: f32,
    /// The distinct y terms of the glyphs when every one is whole, else `None`.
    y_terms: Option<Vec<f32>>,
    instances: Vec<GlyphToRender>,
    /// Where each layout run's instances end, so a run TextRenderer would cull is left out.
    run_ends: Vec<u32>,
    keys: Vec<GlyphKey>,
    slot: Option<Slot>,
    frame: u64,
    // Termurion patch 8: the entry's place in the row table, stable while it is kept.
    id: u32,
    /// What the table holds for this entry when it drew in `drawn_frame`.
    uniform: RowUniform,
    drawn_frame: u64,
}

impl Entry {
    /// The `dy` that draws this entry at `top`, or `None` when it must be prepared again.
    fn shift_to(&self, top: f32) -> Option<i32> {
        if top.to_bits() == self.top.to_bits() {
            return Some(0);
        }
        let terms = self.y_terms.as_ref()?;
        if !(self.top >= 0.0 && top >= 0.0) {
            return None;
        }
        // physical() truncates gy + top; check each term with the same float steps so an
        // addition that rounds across a whole number never shifts wrongly.
        let dy = top.trunc() as i32 - self.top.trunc() as i32;
        let moves = |gy: f32| gy.mul_add(1.0, top).trunc() as i32 - gy.mul_add(1.0, self.top).trunc() as i32;
        terms.iter().all(|&gy| moves(gy) == dy).then_some(dy)
    }

    /// The slot range holding the instances of `runs`.
    fn drawn(&self, slot: Slot, runs: std::ops::Range<usize>) -> (u32, u32) {
        let from = if runs.start == 0 { 0 } else { self.run_ends[runs.start - 1] };
        let to = self.run_ends[runs.end - 1];
        (slot.start + from, to - from)
    }
}

fn row_key(row: &RowArea) -> RowKey {
    RowKey {
        serial: row.serial,
        left: row.left.to_bits(),
        color: row.default_color.0,
    }
}

struct Draw {
    start: u32,
    len: u32,
    id: u32,
    uniform: RowUniform,
    /// Where a following row's slot must start to join this draw's batch: the slot's end when
    /// only padding follows the drawn instances, else the drawn end (culled runs follow).
    joins_at: u32,
}

/// Draws rows of text, keeping each row's glyph instances on the GPU between frames.
///
/// Termurion patch 9: the CPU half. The buffers it fills are the backend's; their sizes start at
/// [`RowRenderer::capacity`] instances and [`RowRenderer::table_rows`] table entries.
pub struct RowRenderer {
    /// In instances.
    pub(crate) capacity: u32,
    /// The first instance no slot has used yet.
    pub(crate) end: u32,
    /// Free slot starts, one set per class (lowest first, so a new screen lays out in draw order).
    free: Vec<BTreeSet<u32>>,
    /// Row ids given back by dropped entries, and the first id never given out.
    free_ids: BTreeSet<u32>,
    next_id: u32,
    entries: HashMap<RowKey, Vec<Entry>>,
    draws: Vec<Draw>,
    pub(crate) table_rows: u32,
    /// What the table holds now, one entry per row id (`None` where unknown).
    table_cpu: Vec<Option<RowUniform>>,
    generation: u64,
    frame: u64,
    prepared_rows: u64,
    /// Batches the last `prepare_cpu` made.
    issued: u64,
    /// Keys of entries dropped by `clear`, released at the next `prepare_cpu`.
    unreleased: Vec<GlyphKey>,
    /// Scratch kept between frames: the rows wanted, the changed table entries.
    wanted: HashSet<RowKey>,
    changed: Vec<(u32, RowUniform)>,
}

impl RowRenderer {
    /// Creates a new `RowRenderer` for rows drawn from `atlas`.
    pub fn new(atlas: &TextAtlas) -> Self {
        Self {
            capacity: FIRST_CAPACITY,
            end: 0,
            free: Vec::new(),
            free_ids: BTreeSet::new(),
            next_id: 0,
            entries: HashMap::new(),
            draws: Vec::new(),
            table_rows: TABLE_CHUNK_ROWS,
            table_cpu: vec![None; TABLE_CHUNK_ROWS as usize],
            generation: atlas.generation(),
            frame: 0,
            prepared_rows: 0,
            issued: 0,
            unreleased: Vec::new(),
            wanted: HashSet::new(),
            changed: Vec::new(),
        }
    }

    /// The instance buffers' size in instances (a backend makes them this big at first).
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The row table's size in entries (a backend makes it this big at first).
    pub fn table_rows(&self) -> u32 {
        self.table_rows
    }

    /// Rows that ran `prepare_glyph` since this renderer was made (for tests).
    pub fn prepared_rows(&self) -> u64 {
        self.prepared_rows
    }

    /// Draw calls the last `prepare_cpu` made (for tests).
    pub fn draws_issued(&self) -> u64 {
        self.issued
    }

    /// Forgets every prepared row; the next frame prepares all of them again.
    pub fn clear(&mut self) {
        for (_, list) in self.entries.drain() {
            for entry in list {
                self.unreleased.extend(entry.keys);
            }
        }
        self.free.clear();
        self.free_ids.clear();
        self.next_id = 0;
        self.end = 0;
        self.draws.clear();
        self.issued = 0;
    }

    /// Termurion patch 9: `clear`, releasing the glyphs now (the row path turned off).
    pub fn release(&mut self, atlas: &mut TextAtlas) {
        self.clear();
        for key in self.unreleased.drain(..) {
            atlas.release(key);
        }
    }

    /// Prepares the rows to draw, in the order they draw. Rows not given are dropped. The buffer
    /// writes are added to `out.writes` (they must reach the GPU even when this fails) and
    /// `out.batches` is replaced. `rows` is walked more than once.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_cpu<'a, I>(
        &mut self,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        resolution: Resolution,
        rows: I,
        cache: &mut SwashCache,
        mut rasterize_custom_glyph: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
        out: &mut RowDraws,
    ) -> Result<(), PrepareError>
    where
        I: IntoIterator<Item = RowArea<'a>> + Clone,
    {
        out.batches.clear();
        self.issued = 0;
        for key in self.unreleased.drain(..) {
            atlas.release(key);
        }

        // Rows not in this frame let go of their glyphs first, so they never take atlas space the
        // new rows need (the atlas never has to hold two screens at once).
        let mut wanted = mem::take(&mut self.wanted);
        wanted.clear();
        wanted.extend(rows.clone().into_iter().map(|r| row_key(&r)));
        self.drop_entries(atlas, |key, _| !wanted.contains(key));
        self.wanted = wanted;

        let mut retried = false;
        loop {
            // A grown atlas may have moved what the kept instances point at.
            if atlas.generation() != self.generation {
                self.drop_entries(atlas, |_, _| true);
                self.generation = atlas.generation();
            }
            self.frame += 1;
            self.draws.clear();
            let prepared = self.prepare_rows(
                font_system,
                atlas,
                resolution,
                rows.clone(),
                cache,
                &mut rasterize_custom_glyph,
                out,
            );
            if let Err(e) = prepared {
                self.draws.clear();
                // A full atlas: let go of every kept row this pass did not reach and try once more.
                if !retried {
                    retried = true;
                    let frame = self.frame;
                    self.drop_entries(atlas, |_, e| e.frame != frame);
                    continue;
                }
                return Err(e);
            }
            if atlas.generation() == self.generation {
                break;
            }
        }

        let frame = self.frame;
        self.drop_entries(atlas, |_, e| e.frame != frame);
        self.write_table(out);
        self.batch(out);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_rows<'a>(
        &mut self,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        resolution: Resolution,
        rows: impl IntoIterator<Item = RowArea<'a>>,
        cache: &mut SwashCache,
        rasterize_custom_glyph: &mut impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
        out: &mut RowDraws,
    ) -> Result<(), PrepareError> {
        for row in rows {
            let row = &row;
            // The layout runs TextRenderer would draw: it skips runs whose line box misses the
            // unclamped bounds, then draws while they meet them.
            let visible = |run: &cosmic_text::LayoutRun| {
                let start = (row.top + run.line_top) as i32;
                let end = start + run.line_height as i32;
                start <= row.bounds.bottom && row.bounds.top <= end
            };
            let first = row.buffer.layout_runs().take_while(|r| !visible(r)).count();
            let shown = row.buffer.layout_runs().skip(first).take_while(visible).count();
            if shown == 0 {
                continue;
            }
            let runs = first..first + shown;

            let bounds = GlyphBounds::clamped(row.bounds, resolution);
            let clip = [bounds.x.min, bounds.y.min, bounds.x.max, bounds.y.max];
            let key = row_key(row);

            let frame = self.frame;
            let uniform = |dy: i32| RowUniform {
                offset: [0, dy],
                _pad: [0, 0],
                clip,
            };
            // Termurion patch 8: an entry holds one table entry per frame, so an entry already
            // drawn this frame with another uniform cannot draw again; a fresh one is made.
            let usable = |e: &Entry| {
                let dy = e.shift_to(row.top)?;
                (e.drawn_frame != frame || e.uniform == uniform(dy)).then_some(dy)
            };
            let found = self.entries.get_mut(&key).and_then(|list| {
                let exact = list
                    .iter()
                    .position(|e| e.top.to_bits() == row.top.to_bits() && usable(e).is_some());
                let i = exact.or_else(|| list.iter().position(|e| usable(e).is_some()))?;
                let entry = &mut list[i];
                entry.frame = frame;
                let dy = usable(entry)?;
                Some((i, dy, entry.slot.map(|s| entry.drawn(s, runs.clone()))))
            });

            let (i, dy, drawn) = match found {
                Some(hit) => hit,
                None => {
                    let mut system = GlyphSystem {
                        atlas: &mut *atlas,
                        cache: &mut *cache,
                        font_system: &mut *font_system,
                    };
                    let mut entry = prepare_row(&mut system, row, &mut *rasterize_custom_glyph)?;
                    entry.frame = frame;
                    entry.id = self.free_ids.pop_first().unwrap_or_else(|| {
                        self.next_id += 1;
                        self.next_id - 1
                    });
                    self.prepared_rows += 1;
                    for &k in &entry.keys {
                        atlas.hold(k);
                    }
                    if !entry.instances.is_empty() {
                        let slot = self.alloc(entry.instances.len() as u32, out);
                        entry.slot = Some(slot);
                        write_slot(out, &entry);
                    }
                    let drawn = entry.slot.map(|s| entry.drawn(s, runs.clone()));
                    let list = self.entries.entry(key).or_default();
                    list.push(entry);
                    (list.len() - 1, 0, drawn)
                }
            };

            if let Some((start, len)) = drawn.filter(|&(_, len)| len > 0) {
                let entry = &mut self.entries.get_mut(&key).expect("the entry just found")[i];
                entry.drawn_frame = frame;
                entry.uniform = uniform(dy);
                let slot = entry.slot.expect("a drawn entry has a slot");
                let joins_at = if start + len == slot.start + entry.instances.len() as u32 {
                    slot.start + (MIN_SLOT << slot.class)
                } else {
                    start + len
                };
                self.draws.push(Draw { start, len, id: entry.id, uniform: entry.uniform, joins_at });
            }
        }
        Ok(())
    }

    /// Takes a slot for `len` instances, doubling the buffer when none is free.
    fn alloc(&mut self, len: u32, out: &mut RowDraws) -> Slot {
        let class = (len.max(MIN_SLOT).next_power_of_two() / MIN_SLOT).trailing_zeros();
        if let Some(start) = self.free.get_mut(class as usize).and_then(BTreeSet::pop_first) {
            return Slot { start, class };
        }
        let size = MIN_SLOT << class;
        if self.end + size > self.capacity {
            let mut capacity = self.capacity * 2;
            while self.end + size > capacity {
                capacity *= 2;
            }
            out.writes.push(RowWrite::Grow { capacity });
            self.capacity = capacity;
            // The new buffers start empty: write every live row from its CPU copy.
            for entry in self.entries.values().flatten().filter(|e| e.slot.is_some()) {
                write_slot(out, entry);
            }
        }
        let start = self.end;
        self.end += size;
        Slot { start, class }
    }

    /// Drops the entries `drop` picks, releasing their glyphs and freeing their slots.
    fn drop_entries(&mut self, atlas: &mut TextAtlas, drop: impl Fn(&RowKey, &Entry) -> bool) {
        let (free, free_ids) = (&mut self.free, &mut self.free_ids);
        self.entries.retain(|key, list| {
            list.retain_mut(|entry| {
                if !drop(key, entry) {
                    return true;
                }
                for &k in &entry.keys {
                    atlas.release(k);
                }
                if let Some(slot) = entry.slot {
                    let class = slot.class as usize;
                    if free.len() <= class {
                        free.resize_with(class + 1, BTreeSet::new);
                    }
                    free[class].insert(slot.start);
                }
                free_ids.insert(entry.id);
                false
            });
            !list.is_empty()
        });
    }

    /// Writes the table entries of the drawn rows that changed, growing the table when an id
    /// is past its end.
    fn write_table(&mut self, out: &mut RowDraws) {
        if self.next_id > self.table_rows {
            let rows = self.next_id.div_ceil(TABLE_CHUNK_ROWS) * TABLE_CHUNK_ROWS;
            out.writes.push(RowWrite::GrowTable { rows });
            self.table_rows = rows;
            self.table_cpu = vec![None; rows as usize];
        }

        let changed = &mut self.changed;
        changed.clear();
        for draw in &self.draws {
            if self.table_cpu[draw.id as usize] != Some(draw.uniform) {
                self.table_cpu[draw.id as usize] = Some(draw.uniform);
                changed.push((draw.id, draw.uniform));
            }
        }
        changed.sort_unstable_by_key(|&(id, _)| id);

        // Write each run of changed ids with one call.
        let mut i = 0;
        while i < changed.len() {
            let first = i;
            while i + 1 < changed.len() && changed[i + 1].0 == changed[i].0 + 1 {
                i += 1;
            }
            i += 1;
            let from = out.bytes.len();
            for (_, u) in &changed[first..i] {
                out.bytes.extend_from_slice(unsafe {
                    slice::from_raw_parts(u as *const RowUniform as *const u8, ROW_UNIFORM_SIZE as usize)
                });
            }
            out.writes.push(RowWrite::Table { first: changed[first].0, bytes: from..out.bytes.len() });
        }
    }

    /// Termurion patch 8: joins the draws into batches. A draw joins the batch before it only
    /// when its slot starts where that batch's last slot ends and its id is in the same table
    /// chunk, so the rows draw in exactly the order they were given.
    fn batch(&mut self, out: &mut RowDraws) {
        out.batches.clear();
        // Where a draw must start to join the last batch.
        let mut joins_at = 0;
        for draw in &self.draws {
            let chunk = draw.id / TABLE_CHUNK_ROWS;
            match out.batches.last_mut() {
                Some(b) if joins_at == draw.start && b.chunk == chunk => b.end = draw.start + draw.len,
                _ => out.batches.push(RowBatch { chunk, start: draw.start, end: draw.start + draw.len }),
            }
            joins_at = draw.joins_at;
        }
        self.issued = out.batches.len() as u64;
    }
}

/// Makes a row's instances as `TextRenderer` does, at scale 1, with no clipping and no run culling.
fn prepare_row(
    system: &mut GlyphSystem,
    row: &RowArea,
    rasterize_custom_glyph: &mut impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
) -> Result<Entry, PrepareError> {
    let mut instances = Vec::new();
    let mut run_ends = Vec::new();
    let mut keys = Vec::new();
    let mut y_terms: Option<Vec<f32>> = Some(Vec::new());

    for run in row.buffer.layout_runs() {
        for glyph in run.glyphs.iter() {
            // The y term physical() truncates after adding top.
            let gy = glyph.y - glyph.font_size * glyph.y_offset;
            if let Some(terms) = &mut y_terms {
                if gy.fract() != 0.0 {
                    y_terms = None;
                } else if !terms.iter().any(|t| t.to_bits() == gy.to_bits()) {
                    terms.push(gy);
                }
            }

            let physical = glyph.physical((row.left, row.top), 1.0);
            let color = glyph.color_opt.unwrap_or(row.default_color);
            let cache_key = GlyphonCacheKey::Text(physical.cache_key);

            if let Some(instance) = prepare_glyph(
                system,
                GlyphMetadata {
                    x: physical.x,
                    y: physical.y,
                    line_y: run.line_y,
                    color,
                    cache_key,
                    scale_factor: 1.0,
                },
                GlyphBounds::UNBOUNDED,
                |system, _| text_glyph_image(system, physical.cache_key),
                &mut *rasterize_custom_glyph,
            )? {
                instances.push(instance);
                keys.push(GlyphKey(cache_key));
            }
        }
        run_ends.push(instances.len() as u32);
    }

    Ok(Entry {
        top: row.top,
        y_terms,
        instances,
        run_ends,
        keys,
        slot: None,
        frame: 0,
        id: 0,
        uniform: RowUniform::default(),
        drawn_frame: 0,
    })
}

/// Termurion patch 8: writes the entry's whole slot, its instances then zero ones (`dim` 0,
/// which `vs_row` culls) up to the slot's capacity, and its id for every one.
fn write_slot(out: &mut RowDraws, entry: &Entry) {
    let slot = entry.slot.expect("a written entry has a slot");
    let capacity = (MIN_SLOT << slot.class) as usize;
    let from = out.bytes.len();
    out.bytes.extend_from_slice(crate::glyph_bytes(&entry.instances));
    out.bytes.resize(from + capacity * INSTANCE_SIZE, 0);
    out.writes.push(RowWrite::Instances { at: slot.start, bytes: from..out.bytes.len() });
    let from = out.bytes.len();
    for _ in 0..capacity {
        out.bytes.extend_from_slice(&entry.id.to_ne_bytes());
    }
    out.writes.push(RowWrite::Ids { at: slot.start, bytes: from..out.bytes.len() });
}
