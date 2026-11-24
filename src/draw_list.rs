//! What one frame draws, built on the CPU (`Renderer::prepare`) and run by a GPU backend. Nothing
//! here names a GPU type, so every backend runs the same list.

use std::ops::Range;

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

/// A glyph atlas change (filled in by the glyph work; empty while glyph prepare runs in the backend).
#[derive(Clone, Debug, PartialEq)]
pub enum AtlasOp {}

/// Everything a frame sends to the GPU besides the draw calls.
#[derive(Debug, Default)]
pub struct Uploads {
    pub buffers: Vec<BufferWrite>,
    pub atlas: Vec<AtlasOp>,
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

/// One shaped text row: its slot in the renderer's row buffers, the serial it was shaped under
/// (the row renderer keeps a row's glyphs by it) and where it draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowDraw {
    pub slot: usize,
    pub serial: u64,
    pub left: f32,
    pub top: f32,
    pub bounds: Bounds,
}

/// The frame's text rows, in draw order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowDraws {
    /// Rows keep their prepared glyphs between frames (`RedrawOnlyChangedRows`); off, every row
    /// is prepared again each frame.
    pub kept: bool,
    pub rows: Vec<RowDraw>,
}

/// The box and block glyphs, one group per clip rectangle.
#[derive(Clone, Debug, Default)]
pub struct GlyphDraws {
    pub groups: Vec<(Bounds, Vec<glyphon::CustomGlyph>)>,
}

/// One frame, in draw order: clear, backdrop images, quads under the pictures, pictures, popup
/// boxes, text rows and box glyphs, the strokes and overlay, then the fade pass.
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
    pub fn write(&mut self, bytes: &[u8], out: &mut Vec<BufferWrite>) {
        if bytes.is_empty() {
            return;
        }
        let which = self.which;
        let len = bytes.len() as u64;
        if len > self.cap {
            while self.cap < len {
                self.cap *= 2;
            }
            out.push(BufferWrite::Grow { which, size: self.cap });
            out.push(BufferWrite::Quads { which, offset: 0, bytes: bytes.to_vec() });
            self.written += len;
        } else {
            let mut run: Option<usize> = None;
            let mut flush = |from: usize, to: usize, written: &mut u64| {
                out.push(BufferWrite::Quads { which, offset: from as u64, bytes: bytes[from..to].to_vec() });
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

    fn offsets(out: &[BufferWrite]) -> Vec<(u64, usize)> {
        out.iter()
            .filter_map(|w| match w {
                BufferWrite::Quads { offset, bytes, .. } => Some((*offset, bytes.len())),
                BufferWrite::Grow { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_shadow_writes_only_changed_chunks() {
        let mut s = Shadow::new(QuadBuf::Under);
        let mut out = Vec::new();
        let mut bytes = vec![1u8; 3 * CHUNK + 10];
        s.write(&bytes, &mut out);
        assert_eq!(out.first(), Some(&BufferWrite::Grow { which: QuadBuf::Under, size: 4 * CHUNK as u64 }));
        assert_eq!(offsets(&out), [(0, bytes.len())]);
        out.clear();
        s.write(&bytes, &mut out);
        assert!(out.is_empty(), "the same bytes wrote {out:?}");
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
        let mut out = Vec::new();
        s.write(&[7; 100], &mut out);
        out.clear();
        s.write(&[], &mut out);
        s.write(&[7; 100], &mut out);
        assert!(out.is_empty());
        s.forget();
        s.write(&[7; 100], &mut out);
        assert_eq!(offsets(&out), [(0, 100)]);
    }

    #[test]
    fn a_longer_list_writes_its_new_tail() {
        let mut s = Shadow::new(QuadBuf::Pops);
        let mut out = Vec::new();
        s.write(&[1; 100], &mut out);
        out.clear();
        let mut more = vec![1u8; 200];
        more[150] = 3;
        s.write(&more, &mut out);
        // The first chunk is one chunk of a different length: written whole.
        assert_eq!(offsets(&out), [(0, 200)]);
    }
}
