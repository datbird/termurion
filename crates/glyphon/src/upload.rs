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
    /// Bytes of every chunk not let go, and what recent frames had in use (patch 10).
    owned: BufferAddress,
    keep: crate::HighWater,
}

impl Default for UploadBelt {
    fn default() -> Self {
        Self::new()
    }
}

impl UploadBelt {
    pub fn new() -> Self {
        Self { encoder: None, active: Vec::new(), closed: Vec::new(), free: Arc::new(Mutex::new(Vec::new())), made: 0, owned: 0, keep: crate::HighWater::default() }
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
            let size = size.max(CHUNK).next_multiple_of(wgpu::MAP_ALIGNMENT);
            self.owned += size;
            Chunk {
                buffer: device.create_buffer(&BufferDescriptor {
                    label: Some("upload chunk"),
                    size,
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
        // The smallest that fits, so a small write leaves a big chunk for a big one.
        let i = free.iter().enumerate().filter(|(_, c)| c.buffer.size() >= size).min_by_key(|(_, c)| c.buffer.size())?.0;
        Some(free.swap_remove(i))
    }

    /// The sizes of the chunks free now (tests).
    #[cfg(test)]
    fn free_sizes(&self) -> Vec<BufferAddress> {
        self.free.lock().unwrap().iter().map(|c| c.buffer.size()).collect()
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
        // What is in use now (this frame and those the GPU still reads) sets the mark; free
        // chunks past it go, largest first, outside the lock.
        let mut spares = std::mem::take(&mut *self.free.lock().unwrap());
        let spare: BufferAddress = spares.iter().map(|c| c.buffer.size()).sum();
        self.keep.note((self.owned - spare) as usize);
        let mut owned = self.owned as usize;
        self.keep.let_go(&mut owned, &mut spares, |c| Some(c.buffer.size() as usize), CHUNK as usize);
        self.owned = owned as BufferAddress;
        let mut free = self.free.lock().unwrap();
        spares.append(&mut free);
        *free = spares;
        drop(free);
        for chunk in self.closed.drain(..) {
            let free = self.free.clone();
            let buffer = chunk.buffer.clone();
            buffer.slice(..).map_async(MapMode::Write, move |mapped| {
                if mapped.is_ok() {
                    free.lock().unwrap().push(Chunk { buffer: chunk.buffer, used: 0 });
                }
            });
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(device: &Device, queue: &Queue, target: &Buffer, belt: &mut UploadBelt, sizes: &[u64]) {
        for (i, &n) in sizes.iter().enumerate() {
            belt.write(device, target, (i as u64 * 64) % CHUNK, &vec![7u8; n as usize]);
        }
        belt.submit(queue, []);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        // Another test's poll may run our map callbacks: wait until every chunk is back.
        let start = std::time::Instant::now();
        while belt.free_sizes().iter().sum::<u64>() < belt.owned {
            assert!(start.elapsed().as_secs() < 10, "chunks never came back");
            std::thread::yield_now();
            let _ = device.poll(wgpu::PollType::Poll);
        }
    }

    fn rig() -> (Device, Queue, Buffer) {
        let (device, queue) = crate::test_gpu::shared();
        let target = device.create_buffer(&BufferDescriptor { label: None, size: 4 * CHUNK, usage: BufferUsages::COPY_DST, mapped_at_creation: false });
        (device, queue, target)
    }

    /// A steady frame bigger than the old caps (eight free chunks, one big one): 60 writes that
    /// each need a chunk of their own plus three big writes. After the first frame it makes no
    /// chunk.
    #[test]
    fn a_steady_big_frame_makes_no_chunks() {
        let (device, queue, target) = rig();
        let mut belt = UploadBelt::new();
        let mut sizes = vec![CHUNK * 3 / 4; 60];
        sizes.extend([CHUNK + 4096, 2 * CHUNK, 3 * CHUNK]);
        frame(&device, &queue, &target, &mut belt, &sizes);
        let made = belt.buffers_made();
        for _ in 0..20 {
            frame(&device, &queue, &target, &mut belt, &sizes);
        }
        assert_eq!(belt.buffers_made(), made, "steady frames made new chunks");
    }

    /// After a burst, small frames give the chunks back within the decay window, and then make
    /// no new ones.
    #[test]
    fn a_burst_is_let_go_after_the_decay_window() {
        let (device, queue, target) = rig();
        let mut belt = UploadBelt::new();
        for _ in 0..4 {
            frame(&device, &queue, &target, &mut belt, &[CHUNK + 4096, 2 * CHUNK, 3 * CHUNK, 4096, 4096]);
        }
        for _ in 0..crate::DECAY_FRAMES + 1 {
            frame(&device, &queue, &target, &mut belt, &[4096, 512, 64]);
        }
        let held: u64 = belt.free_sizes().iter().sum();
        assert!(held <= CHUNK, "free after the small frames: {:?}", belt.free_sizes());
        let made = belt.buffers_made();
        for _ in 0..20 {
            frame(&device, &queue, &target, &mut belt, &[4096, 512, 64]);
        }
        assert_eq!(belt.buffers_made(), made, "small frames made new chunks");
    }
}
