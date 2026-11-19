//! Termurion patch 7: one upload path for every buffer write of a frame. Each `Queue::write_buffer`
//! makes a staging buffer that the next submit frees; the belt keeps its staging chunks and maps
//! them again once the GPU is done with them, so a steady stream of frames makes no buffer.

use std::sync::{Arc, Mutex};
use wgpu::{
    Buffer, BufferAddress, BufferDescriptor, BufferUsages, CommandBuffer, CommandEncoder,
    CommandEncoderDescriptor, Device, MapMode, Queue, SubmissionIndex,
};

/// The first size of a staging chunk; a bigger write gets a chunk of its own size.
const CHUNK: BufferAddress = 256 * 1024;
/// Chunks kept free after a burst (a flood); the rest are let go.
const KEEP_FREE: usize = 8;

struct Chunk {
    buffer: Buffer,
    used: BufferAddress,
}

/// Records buffer writes as copies from reused staging chunks, all into one encoder that
/// [`UploadBelt::submit`] sends ahead of the frame's own commands.
pub struct UploadBelt {
    encoder: Option<CommandEncoder>,
    /// Mapped, taking this frame's writes.
    active: Vec<Chunk>,
    /// Unmapped, read by a submission.
    closed: Vec<Chunk>,
    /// Mapped again after the GPU finished with them.
    free: Arc<Mutex<Vec<Chunk>>>,
    made: u64,
}

impl Default for UploadBelt {
    fn default() -> Self {
        Self::new()
    }
}

impl UploadBelt {
    pub fn new() -> Self {
        Self { encoder: None, active: Vec::new(), closed: Vec::new(), free: Arc::new(Mutex::new(Vec::new())), made: 0 }
    }

    /// Staging buffers made so far (tests: flat while frames stream).
    pub fn buffers_made(&self) -> u64 {
        self.made
    }

    /// Writes `bytes` into `target` at `offset`, as `Queue::write_buffer` would by the next submit.
    /// The length must be a multiple of 4, as for `write_buffer`.
    pub fn write(&mut self, device: &Device, target: &Buffer, offset: BufferAddress, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let size = bytes.len() as BufferAddress;
        assert!(size % wgpu::COPY_BUFFER_ALIGNMENT == 0 && offset % wgpu::COPY_BUFFER_ALIGNMENT == 0, "an upload of {size} bytes at {offset} is not 4-byte aligned");
        let i = self.chunk_for(device, size);
        let chunk = &mut self.active[i];
        let at = chunk.used;
        chunk.used = (at + size).next_multiple_of(wgpu::MAP_ALIGNMENT);
        chunk.buffer.slice(at..at + size).get_mapped_range_mut().expect("an upload chunk stays mapped while it is active").copy_from_slice(bytes);
        let encoder = self.encoder.get_or_insert_with(|| device.create_command_encoder(&CommandEncoderDescriptor { label: Some("uploads") }));
        encoder.copy_buffer_to_buffer(&chunk.buffer, at, target, offset, size);
    }

    /// An active chunk with room for `size`: one already open, else a free one, else a new one.
    fn chunk_for(&mut self, device: &Device, size: BufferAddress) -> usize {
        let fits = |c: &Chunk| c.used + size <= c.buffer.size();
        if let Some(i) = self.active.iter().position(fits) {
            return i;
        }
        let mut taken = self.take_free(size);
        if taken.is_none() {
            // Map callbacks run on a poll; one that is due hands its chunk back now.
            let _ = device.poll(wgpu::PollType::Poll);
            taken = self.take_free(size);
        }
        let chunk = taken.unwrap_or_else(|| {
            self.made += 1;
            Chunk {
                buffer: device.create_buffer(&BufferDescriptor {
                    label: Some("upload chunk"),
                    size: size.max(CHUNK).next_multiple_of(wgpu::MAP_ALIGNMENT),
                    usage: BufferUsages::MAP_WRITE | BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                }),
                used: 0,
            }
        });
        self.active.push(chunk);
        self.active.len() - 1
    }

    fn take_free(&mut self, size: BufferAddress) -> Option<Chunk> {
        let mut free = self.free.lock().unwrap();
        let i = free.iter().position(|c| c.buffer.size() >= size)?;
        Some(free.swap_remove(i))
    }

    /// Submits the recorded copies and then `commands`, in one submission, and maps the chunks
    /// again for when the GPU is done.
    pub fn submit(&mut self, queue: &Queue, commands: impl IntoIterator<Item = CommandBuffer>) -> SubmissionIndex {
        for chunk in self.active.drain(..) {
            chunk.buffer.unmap();
            self.closed.push(chunk);
        }
        let uploads = self.encoder.take().map(CommandEncoder::finish);
        let index = queue.submit(uploads.into_iter().chain(commands));
        for chunk in self.closed.drain(..) {
            let free = self.free.clone();
            let buffer = chunk.buffer.clone();
            buffer.slice(..).map_async(MapMode::Write, move |mapped| {
                if mapped.is_ok() {
                    let mut free = free.lock().unwrap();
                    if free.len() < KEEP_FREE {
                        free.push(Chunk { buffer: chunk.buffer, used: 0 });
                    }
                }
            });
        }
        index
    }
}
