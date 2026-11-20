//! The bounded output ring behind one job: chunks at absolute byte offsets, head eviction that
//! never moves an assigned offset, and non-consuming reads from any offset.
//!
//! Port of `src/ring.ts`.

use hoh_jobs::{JobAppendOptions, JobChannel, JobChunk, JobOutputRead};
use std::collections::VecDeque;

/// One retained ring entry; `bytes` caches the chunk's UTF-8 length.
struct RingChunk {
    at: u64,
    text: String,
    bytes: u64,
    channel: Option<JobChannel>,
    gap_before: bool,
}

/// The UTF-8-safe tail of `text` no longer than `max_bytes`: the byte cut advances past
/// continuation bytes so the surviving text never starts inside a code point.
fn utf8_tail(text: &str, max_bytes: u64) -> (String, u64) {
    let raw = text.as_bytes();
    let mut start = raw
        .len()
        .saturating_sub(usize::try_from(max_bytes).unwrap_or(usize::MAX));
    while start < raw.len() && (raw[start] & 0xC0) == 0x80 {
        start += 1;
    }
    let tail = &text[start..];
    (tail.to_owned(), tail.len() as u64)
}

/// Offsets stay absolute across eviction: `earliest` only ever advances.
#[derive(Default)]
pub struct OutputRing {
    chunks: VecDeque<RingChunk>,
    /// Sum of the retained chunks' byte lengths.
    pub retained_bytes: u64,
    /// Total UTF-8 bytes ever appended: the offset the next chunk starts at.
    pub total: u64,
    /// Offset of the oldest retained byte (equals `total` when nothing is retained).
    pub earliest: u64,
}

impl OutputRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one chunk and trim the head to `cap`; false when the chunk was empty.
    pub fn append(&mut self, text: &str, options: JobAppendOptions, cap: u64) -> bool {
        if text.is_empty() {
            return false;
        }
        let bytes = text.len() as u64;
        self.chunks.push_back(RingChunk {
            at: self.total,
            text: text.to_owned(),
            bytes,
            channel: options.channel,
            gap_before: options.gap_before,
        });
        self.total += bytes;
        self.retained_bytes += bytes;
        self.trim(cap);
        true
    }

    /// Drop retained head chunks until the ring fits `cap`; a single oversized chunk keeps only
    /// its UTF-8-safe tail with a `gapBefore` marker.
    pub fn trim(&mut self, cap: u64) {
        while self.retained_bytes > cap && self.chunks.len() > 1 {
            let Some(dropped) = self.chunks.pop_front() else {
                break;
            };
            self.retained_bytes -= dropped.bytes;
        }
        if self.chunks.len() == 1
            && let Some(single) = self.chunks.front_mut()
            && single.bytes > cap
        {
            let (text, bytes) = utf8_tail(&single.text, cap);
            single.at += single.bytes - bytes;
            single.text = text;
            single.bytes = bytes;
            single.gap_before = true;
            self.retained_bytes = bytes;
        }
        self.earliest = self.chunks.front().map_or(self.total, |chunk| chunk.at);
    }

    /// Retained chunks overlapping `[from, total)` as fresh wire chunks.
    pub fn read_from(&self, from: u64) -> JobOutputRead {
        let chunks = self
            .chunks
            .iter()
            .filter(|chunk| chunk.at + chunk.bytes > from)
            .map(|chunk| JobChunk {
                at: chunk.at,
                text: chunk.text.clone(),
                channel: chunk.channel,
                gap_before: chunk.gap_before,
            })
            .collect();
        JobOutputRead {
            chunks,
            next: self.total,
            lossy: from < self.earliest,
        }
    }
}
