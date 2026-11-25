//! What one frame draws, built on the CPU (`Renderer::prepare`) and run by a GPU backend. Nothing
//! here names a GPU type, so every backend runs the same list.

use std::ops::Range;

/// glyphon's CPU half makes these (atlas texture changes, row buffer writes and draws, glyph
/// instances); every backend applies the same ones.
pub use glyphon::{AtlasOp, GlyphDraws, RowDraws};

/// The persistent vertex buffers a frame draws from. Each keeps its bytes between frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuadBuf {
    Under,
    Over,
    Pops,
    Picture,
    Backdrop,
    Cover,
}

impl QuadBuf {
    pub const ALL: [QuadBuf; 6] = [QuadBuf::Under, QuadBuf::Over, QuadBuf::Pops, QuadBuf::Picture, QuadBuf::Backdrop, QuadBuf::Cover];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Bytes per quad vertex (position, color) and per image vertex (position, uv).
pub const QUAD_STRIDE: u64 = 6 * 4;
pub const IMAGE_STRIDE: u64 = 4 * 4;

/// One change to a persistent buffer, applied in order before the frame draws.
#[derive(Clone, Debug, PartialEq)]
pub enum BufferWrite {
    /// The buffer is made again with room for `size` bytes; what it held is gone, so a full
    /// `Quads` write at offset 0 always follows.
    Grow { which: QuadBuf, size: u64 },
    Quads { which: QuadBuf, offset: u64, bytes: Vec<u8> },
}

/// Everything a frame sends to the GPU besides the draw calls.
#[derive(Debug, Default)]
pub struct Uploads {
    pub buffers: Vec<BufferWrite>,
    pub atlas: Vec<AtlasOp>,
    /// Byte buffers of spent writes, kept so the next frame's writes do not allocate.
    /// Capped by `POOL_COUNT` and `POOL_BYTES`, so one huge frame does not pin its memory.
    pool: Vec<Vec<u8>>,
}

/// Most spare buffers kept: a steady frame has a few writes per quad buffer.
const POOL_COUNT: usize = 32;
/// Most spare capacity kept in all (8 MiB).
const POOL_BYTES: usize = 8 << 20;

impl Uploads {
    /// Empties the lists for the next frame, keeping their storage.
    pub fn reset(&mut self) {
        for w in self.buffers.drain(..) {
            if let BufferWrite::Quads { bytes, .. } = w {
                let held: usize = self.pool.iter().map(|b| b.capacity()).sum();
                if self.pool.len() < POOL_COUNT && held + bytes.capacity() <= POOL_BYTES {
                    self.pool.push(bytes);
                }
            }
        }
        self.atlas.clear();
    }

    fn push_quads(&mut self, which: QuadBuf, offset: u64, from: &[u8]) {
        let mut bytes = self.pool.pop().unwrap_or_default();
        bytes.clear();
        bytes.extend_from_slice(from);
        self.buffers.push(BufferWrite::Quads { which, offset, bytes });
    }
}

/// One picture: the texture it samples (kept by the backend by `key` and `repeat`) and its six
/// vertices in the `Picture` or `Backdrop` buffer.
#[derive(Clone, Debug)]
pub struct ImageDraw {
    pub key: (u64, usize),
    pub repeat: bool,
    pub size: (u32, u32),
    pub pixels: std::sync::Arc<Vec<u8>>,
    pub vertices: Range<u32>,
}

/// A clip rectangle in whole pixels: left, top, right, bottom.
pub type Bounds = [i32; 4];

impl Default for DrawList {
    fn default() -> DrawList {
        DrawList {
            clear: [0.0; 4],
            size: (0, 0),
            quad_uniform: [0.0; 4],
            backdrop: Vec::new(),
            under: 0..0,
            picture: Vec::new(),
            pops: 0..0,
            rows: RowDraws::default(),
            glyphs: GlyphDraws::default(),
            over: 0..0,
            fade: None,
        }
    }
}

/// One frame, in draw order: clear, backdrop images, quads under the pictures, pictures, popup
/// boxes, text rows and box glyphs, the strokes and overlay, then the fade pass. `rows` also
/// carries the row buffer writes, applied before anything draws.
#[derive(Clone, Debug)]
pub struct DrawList {
    pub clear: [f32; 4],
    pub size: (u32, u32),
    /// The quad and image shaders' screen size (width, height, 0, 0).
    pub quad_uniform: [f32; 4],
    pub backdrop: Vec<ImageDraw>,
    /// Vertex ranges in the `Under`, `Pops` and `Over` buffers; an empty one draws nothing.
    pub under: Range<u32>,
    pub picture: Vec<ImageDraw>,
    pub pops: Range<u32>,
    pub rows: RowDraws,
    pub glyphs: GlyphDraws,
    pub over: Range<u32>,
    /// The fade (0 to 1) the whole target is scaled by, drawn with the six `Cover` vertices.
    pub fade: Option<f32>,
}

const CHUNK: usize = 4096;

/// What one persistent vertex buffer holds, so a frame uploads only the 4 KB chunks that changed
/// and a run of changed chunks goes up as one write.
#[derive(Debug)]
pub struct Shadow {
    which: QuadBuf,
    cap: u64,
    last: Vec<u8>,
    /// Bytes uploaded so far (tests).
    pub written: u64,
}

impl Shadow {
    pub fn new(which: QuadBuf) -> Shadow {
        Shadow { which, cap: CHUNK as u64, last: Vec::new(), written: 0 }
    }

    /// The capacity a new buffer starts with.
    pub const START: u64 = CHUNK as u64;

    /// Forgets what the buffer holds, so the next write sends every byte.
    pub fn forget(&mut self) {
        self.last.clear();
    }

    /// Brings the buffer to `bytes`. An empty list writes nothing and keeps what the buffer holds.
    pub fn write(&mut self, bytes: &[u8], out: &mut Uploads) {
        if bytes.is_empty() {
            return;
        }
        let which = self.which;
        let len = bytes.len() as u64;
        if len > self.cap {
            while self.cap < len {
                self.cap *= 2;
            }
            out.buffers.push(BufferWrite::Grow { which, size: self.cap });
            out.push_quads(which, 0, bytes);
            self.written += len;
        } else {
            let mut run: Option<usize> = None;
            let mut flush = |from: usize, to: usize, written: &mut u64| {
                out.push_quads(which, from as u64, &bytes[from..to]);
                *written += (to - from) as u64;
            };
            for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
                let start = i * CHUNK;
                // A chunk past the old end, or a different length, counts as changed.
                if self.last.get(start..start + chunk.len()) != Some(chunk) {
                    run.get_or_insert(start);
                } else if let Some(from) = run.take() {
                    flush(from, start, &mut self.written);
                }
            }
            if let Some(from) = run {
                flush(from, bytes.len(), &mut self.written);
            }
        }
        self.last.clear();
        self.last.extend_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_list_has_no_gpu_types() {
        let src = include_str!("draw_list.rs");
        for name in [concat!("wg", "pu"), concat!("obj", "c2")] {
            assert!(!src.contains(name), "draw_list.rs names {name}");
        }
    }

    fn offsets(out: &Uploads) -> Vec<(u64, usize)> {
        out.buffers
            .iter()
            .filter_map(|w| match w {
                BufferWrite::Quads { offset, bytes, .. } => Some((*offset, bytes.len())),
                BufferWrite::Grow { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_shadow_writes_only_changed_chunks() {
        let mut s = Shadow::new(QuadBuf::Under);
        let mut out = Uploads::default();
        let mut bytes = vec![1u8; 3 * CHUNK + 10];
        s.write(&bytes, &mut out);
        assert_eq!(out.buffers.first(), Some(&BufferWrite::Grow { which: QuadBuf::Under, size: 4 * CHUNK as u64 }));
        assert_eq!(offsets(&out), [(0, bytes.len())]);
        out.reset();
        s.write(&bytes, &mut out);
        assert!(out.buffers.is_empty(), "the same bytes wrote {out:?}");
        // Chunks 0 and 2 change: two writes; chunk 3 (the short tail) is the same.
        bytes[5] = 2;
        bytes[2 * CHUNK + 1] = 2;
        s.write(&bytes, &mut out);
        assert_eq!(offsets(&out), [(0, CHUNK), (2 * CHUNK as u64, CHUNK)]);
        assert_eq!(s.written, (bytes.len() + 2 * CHUNK) as u64);
    }

    #[test]
    fn a_shadow_keeps_its_bytes_over_an_empty_frame() {
        let mut s = Shadow::new(QuadBuf::Over);
        let mut out = Uploads::default();
        s.write(&[7; 100], &mut out);
        out.reset();
        s.write(&[], &mut out);
        s.write(&[7; 100], &mut out);
        assert!(out.buffers.is_empty());
        s.forget();
        s.write(&[7; 100], &mut out);
        assert_eq!(offsets(&out), [(0, 100)]);
    }

    #[test]
    fn a_longer_list_writes_its_new_tail() {
        let mut s = Shadow::new(QuadBuf::Pops);
        let mut out = Uploads::default();
        s.write(&[1; 100], &mut out);
        out.reset();
        let mut more = vec![1u8; 200];
        more[150] = 3;
        s.write(&more, &mut out);
        // The first chunk is one chunk of a different length: written whole.
        assert_eq!(offsets(&out), [(0, 200)]);
    }

    #[test]
    fn uploads_reuse_their_byte_buffers() {
        let mut s = Shadow::new(QuadBuf::Under);
        let mut out = Uploads::default();
        s.write(&[1; 100], &mut out);
        let BufferWrite::Quads { bytes, .. } = &out.buffers[0] else { panic!("{out:?}") };
        let first = bytes.as_ptr();
        out.reset();
        s.write(&[2; 100], &mut out);
        let BufferWrite::Quads { bytes, .. } = &out.buffers[0] else { panic!("{out:?}") };
        assert_eq!((bytes.as_ptr(), bytes.as_slice()), (first, &[2u8; 100][..]), "the second frame's write took a new buffer");
    }

    #[test]
    fn the_spare_pool_is_capped() {
        let mut s = Shadow::new(QuadBuf::Under);
        let mut out = Uploads::default();
        // One very large frame, then several small ones.
        s.write(&vec![1u8; 3 * POOL_BYTES], &mut out);
        out.reset();
        for i in 0..5u8 {
            s.write(&[i; 100], &mut out);
            out.reset();
        }
        assert!(out.pool.len() <= POOL_COUNT);
        assert!(out.pool.iter().map(|b| b.capacity()).sum::<usize>() <= POOL_BYTES, "the pool pinned the big frame");
        // Many small writes at once stop at the count.
        for i in 0..POOL_COUNT * 2 {
            out.push_quads(QuadBuf::Under, i as u64, &[0; 8]);
        }
        out.reset();
        assert_eq!(out.pool.len(), POOL_COUNT);
    }
}
