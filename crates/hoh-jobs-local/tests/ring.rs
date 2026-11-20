//! Port of `tests/ring.spec.ts`.

use hoh_jobs::{JobAppendOptions, JobChannel, JobChunk, JobOutputRead};
use hoh_jobs_local::ring::OutputRing;

fn chunk(at: u64, text: &str, channel: Option<JobChannel>, gap_before: bool) -> JobChunk {
    JobChunk {
        at,
        text: text.to_owned(),
        channel,
        gap_before,
    }
}

fn plain() -> JobAppendOptions {
    JobAppendOptions::default()
}

#[test]
fn assigns_absolute_offsets_by_utf8_byte_length_and_drops_empty_chunks_silently() {
    let mut ring = OutputRing::new();
    assert!(!ring.append("", plain(), 1024));
    assert!(ring.append(
        "héllo",
        JobAppendOptions {
            channel: Some(JobChannel::Stdout),
            gap_before: false
        },
        1024
    ));
    assert!(ring.append(
        "wörld",
        JobAppendOptions {
            channel: Some(JobChannel::Stderr),
            gap_before: true
        },
        1024
    ));
    assert_eq!(ring.total, 12);
    assert_eq!(ring.earliest, 0);
    assert_eq!(ring.retained_bytes, 12);
    assert_eq!(
        ring.read_from(0),
        JobOutputRead {
            chunks: vec![
                chunk(0, "héllo", Some(JobChannel::Stdout), false),
                chunk(6, "wörld", Some(JobChannel::Stderr), true),
            ],
            next: 12,
            lossy: false,
        }
    );
}

#[test]
fn reads_from_a_foreign_offset_by_returning_the_whole_overlapping_chunk() {
    let mut ring = OutputRing::new();
    ring.append("abc", plain(), 1024);
    ring.append("def", plain(), 1024);
    assert_eq!(ring.read_from(4).chunks, vec![chunk(3, "def", None, false)]);
    assert_eq!(
        ring.read_from(6),
        JobOutputRead {
            chunks: vec![],
            next: 6,
            lossy: false
        }
    );
}

#[test]
fn trims_whole_head_chunks_past_the_cap_and_reports_the_evicted_span_as_lossy() {
    let mut ring = OutputRing::new();
    ring.append("aaaa", plain(), 8);
    ring.append("bbbb", plain(), 8);
    ring.append("cc", plain(), 8);
    // 10 bytes retained > cap 8: the oldest whole chunk goes.
    assert_eq!(ring.earliest, 4);
    assert_eq!(ring.retained_bytes, 6);
    assert_eq!(
        ring.read_from(0),
        JobOutputRead {
            chunks: vec![chunk(4, "bbbb", None, false), chunk(8, "cc", None, false)],
            next: 10,
            lossy: true,
        }
    );
    assert!(!ring.read_from(4).lossy);
}

#[test]
fn keeps_a_lone_oversized_chunk_as_a_utf8_safe_tail_marked_gap_before() {
    let mut ring = OutputRing::new();
    // 3 two-byte code points = 6 bytes; a 3-byte cap must not start inside a code point.
    ring.append("ééé", plain(), 3);
    assert_eq!(ring.retained_bytes, 2);
    assert_eq!(ring.earliest, 4);
    assert_eq!(
        ring.read_from(0),
        JobOutputRead {
            chunks: vec![chunk(4, "é", None, true)],
            next: 6,
            lossy: true,
        }
    );
}

#[test]
fn re_trims_to_a_smaller_cap_on_demand_and_leaves_total_untouched() {
    let mut ring = OutputRing::new();
    ring.append("12345678", plain(), 64);
    ring.append("abcdefgh", plain(), 64);
    ring.trim(8);
    assert_eq!(ring.total, 16);
    assert_eq!(ring.earliest, 8);
    assert_eq!(
        ring.read_from(8),
        JobOutputRead {
            chunks: vec![chunk(8, "abcdefgh", None, false)],
            next: 16,
            lossy: false,
        }
    );
    // Trimming an empty ring is a no-op that keeps earliest at total.
    let mut empty = OutputRing::new();
    empty.trim(1);
    assert_eq!(empty.earliest, 0);
}
