use std::collections::VecDeque;

use super::{OutputStream, ProcessChunk, ProcessId};

#[derive(Debug)]
pub(crate) struct OutputBuffer {
    chunks: VecDeque<StoredChunk>,
    first_position: u64,
    next_position: u64,
    retained_bytes: usize,
    capacity_bytes: usize,
}

#[derive(Debug)]
struct StoredChunk {
    stream: OutputStream,
    start: u64,
    bytes: Vec<u8>,
}

impl OutputBuffer {
    pub(crate) fn new(_process_id: ProcessId, capacity_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            first_position: 0,
            next_position: 0,
            retained_bytes: 0,
            capacity_bytes,
        }
    }

    pub(crate) fn push(&mut self, stream: OutputStream, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let start = self.next_position;
        self.next_position = self.next_position.saturating_add(bytes.len() as u64);
        self.retained_bytes = self.retained_bytes.saturating_add(bytes.len());
        self.chunks.push_back(StoredChunk {
            stream,
            start,
            bytes: bytes.to_vec(),
        });
        self.evict();
    }

    pub(crate) fn read(&self, position: u64, max_bytes: usize) -> BufferRead {
        let effective = position.max(self.first_position).min(self.next_position);
        let mut remaining = max_bytes;
        let mut chunks = Vec::new();
        let mut next = effective;

        for chunk in &self.chunks {
            let end = chunk.start + chunk.bytes.len() as u64;
            if end <= effective || remaining == 0 {
                continue;
            }
            let offset = effective.saturating_sub(chunk.start) as usize;
            let available = &chunk.bytes[offset..];
            let requested = available.len().min(remaining);
            let take = complete_utf8_prefix_len(available, requested);
            let bytes = &available[..take];
            let start = chunk.start + offset as u64;
            next = start + take as u64;
            chunks.push(ProcessChunk {
                stream: chunk.stream,
                start,
                end: next,
                text: String::from_utf8_lossy(bytes).into_owned(),
            });
            remaining -= take;
        }

        BufferRead {
            chunks,
            next_position: next,
            evicted: position < self.first_position,
            truncated: next < self.next_position,
        }
    }

    pub(crate) fn next_position(&self) -> u64 {
        self.next_position
    }

    fn evict(&mut self) {
        while self.retained_bytes > self.capacity_bytes {
            let Some(mut chunk) = self.chunks.pop_front() else {
                break;
            };
            let excess = self.retained_bytes - self.capacity_bytes;
            if excess < chunk.bytes.len() {
                let evicted = complete_utf8_suffix_start(&chunk.bytes, excess);
                chunk.bytes.drain(..evicted);
                chunk.start += evicted as u64;
                self.retained_bytes -= evicted;
                self.first_position = chunk.start;
                self.chunks.push_front(chunk);
                break;
            }
            self.retained_bytes -= chunk.bytes.len();
            self.first_position = chunk.start + chunk.bytes.len() as u64;
        }
    }
}

fn complete_utf8_prefix_len(bytes: &[u8], requested: usize) -> usize {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return requested;
    };
    let mut end = requested;
    while end < bytes.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    end
}

fn complete_utf8_suffix_start(bytes: &[u8], requested: usize) -> usize {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return requested;
    };
    let mut start = requested;
    while start < bytes.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

pub(crate) struct BufferRead {
    pub(crate) chunks: Vec<ProcessChunk>,
    pub(crate) next_position: u64,
    pub(crate) evicted: bool,
    pub(crate) truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_read_reports_eviction_without_returning_history() {
        let id = ProcessId::new();
        let mut buffer = OutputBuffer::new(id, 4);
        buffer.push(OutputStream::Stdout, b"abcdef");
        let read = buffer.read(0, 8);
        assert!(read.evicted);
        assert_eq!(read.chunks[0].text, "cdef");
        assert_eq!(read.next_position, 6);
    }

    #[test]
    fn read_and_eviction_preserve_utf8_boundaries() {
        let id = ProcessId::new();
        let mut buffer = OutputBuffer::new(id, 4);
        buffer.push(OutputStream::Stdout, "aébc".as_bytes());
        let retained = buffer.read(0, 8);
        assert!(retained.evicted);
        assert_eq!(retained.chunks[0].text, "ébc");

        let first = buffer.read(retained.chunks[0].start, 2);
        assert_eq!(first.chunks[0].text, "é");
        assert!(!first.chunks[0].text.contains('\u{fffd}'));
    }
}
