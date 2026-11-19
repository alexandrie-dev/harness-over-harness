//! Bounded line comparison with unified hunks and timeout degradation.

use crate::types::WorkspaceDiffHunk;
use std::{
    mem::size_of,
    time::{Duration, Instant},
};

const CONTEXT_LINES: usize = 3;
const MAX_MYERS_WORKING_BYTES: usize = 32 * 1_024 * 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comparison {
    pub hunks: Vec<WorkspaceDiffHunk>,
    pub coarse: bool,
    pub added: u64,
    pub deleted: u64,
}

#[derive(Clone, Copy)]
enum Op<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

struct TraceLayer {
    min_k: isize,
    values: Vec<isize>,
}

impl TraceLayer {
    fn get(&self, k: isize) -> isize {
        self.values[(k - self.min_k) as usize]
    }
}

fn content_line_iter(text: &str) -> impl Iterator<Item = &str> {
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .take(if text.is_empty() { 0 } else { usize::MAX })
}

fn content_lines(text: &str) -> Vec<&str> {
    content_line_iter(text).collect()
}

fn coarse(before: &str, after: &str, old_lines: usize, new_lines: usize) -> Comparison {
    Comparison {
        hunks: vec![WorkspaceDiffHunk {
            old_start: 1,
            old_lines: old_lines as u64,
            new_start: 1,
            new_lines: new_lines as u64,
            lines: content_line_iter(before)
                .map(|line| format!("-{line}"))
                .chain(content_line_iter(after).map(|line| format!("+{line}")))
                .collect(),
        }],
        coarse: true,
        added: new_lines as u64,
        deleted: old_lines as u64,
    }
}

fn myers<'a>(old: &'a [&'a str], new: &'a [&'a str], deadline: Instant) -> Option<Vec<Op<'a>>> {
    let n = old.len() as isize;
    let m = new.len() as isize;
    let max = (n + m) as usize;
    let offset = max as isize + 1;
    let vector_len = max.checked_mul(2)?.checked_add(3)?;
    let vector_bytes = vector_len.checked_mul(size_of::<isize>())?;
    let operation_bytes = max.checked_mul(size_of::<Op<'_>>())?;
    if vector_bytes.checked_add(operation_bytes)? > MAX_MYERS_WORKING_BYTES {
        return None;
    }
    let mut v = vec![0_isize; vector_len];
    let index = |k: isize| (offset + k) as usize;
    v[index(1)] = 0;
    let mut trace: Vec<TraceLayer> = Vec::new();
    let mut trace_values = 0_usize;
    for d in 0..=max {
        if Instant::now() >= deadline {
            return None;
        }
        let trace_count = trace.len().checked_add(1)?;
        let window_len = d.checked_mul(2)?.checked_add(1)?;
        trace_values = trace_values.checked_add(window_len)?;
        let trace_bytes = trace_values.checked_mul(size_of::<isize>())?;
        let trace_headers = size_of::<Vec<isize>>().checked_mul(trace_count)?;
        let working_bytes = vector_bytes
            .checked_add(operation_bytes)?
            .checked_add(trace_bytes)?
            .checked_add(trace_headers)?;
        if working_bytes > MAX_MYERS_WORKING_BYTES {
            return None;
        }
        let d_signed = d as isize;
        trace.push(TraceLayer {
            min_k: -d_signed,
            values: v[index(-d_signed)..=index(d_signed)].to_vec(),
        });
        let mut k = -d_signed;
        while k <= d_signed {
            let mut x = if k == -d_signed || (k != d_signed && v[index(k - 1)] < v[index(k + 1)]) {
                v[index(k + 1)]
            } else {
                v[index(k - 1)] + 1
            };
            let mut y = x - k;
            let mut scanned = 0_usize;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
                scanned += 1;
                if scanned.is_multiple_of(2_048) && Instant::now() >= deadline {
                    return None;
                }
            }
            v[index(k)] = x;
            if x >= n && y >= m {
                let mut operations = Vec::with_capacity(max);
                let (mut x, mut y) = (n, m);
                for distance in (1..=d).rev() {
                    let distance = distance as isize;
                    let previous = &trace[distance as usize];
                    let k = x - y;
                    let previous_k = if k == -distance
                        || (k != distance && previous.get(k - 1) < previous.get(k + 1))
                    {
                        k + 1
                    } else {
                        k - 1
                    };
                    let previous_x = previous.get(previous_k);
                    let previous_y = previous_x - previous_k;
                    while x > previous_x && y > previous_y {
                        operations.push(Op::Equal(old[(x - 1) as usize]));
                        x -= 1;
                        y -= 1;
                    }
                    if x == previous_x {
                        operations.push(Op::Insert(new[(y - 1) as usize]));
                        y -= 1;
                    } else {
                        operations.push(Op::Delete(old[(x - 1) as usize]));
                        x -= 1;
                    }
                }
                while x > 0 && y > 0 {
                    operations.push(Op::Equal(old[(x - 1) as usize]));
                    x -= 1;
                    y -= 1;
                }
                while x > 0 {
                    operations.push(Op::Delete(old[(x - 1) as usize]));
                    x -= 1;
                }
                while y > 0 {
                    operations.push(Op::Insert(new[(y - 1) as usize]));
                    y -= 1;
                }
                operations.reverse();
                return Some(operations);
            }
            k += 2;
        }
    }
    unreachable!("edit distance cannot exceed n + m")
}

fn hunks(operations: &[Op<'_>]) -> Vec<WorkspaceDiffHunk> {
    let changes: Vec<usize> = operations
        .iter()
        .enumerate()
        .filter_map(|(index, operation)| (!matches!(operation, Op::Equal(_))).then_some(index))
        .collect();
    if changes.is_empty() {
        return Vec::new();
    }
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for change in changes {
        let mut start = change;
        let mut context = 0;
        while start > 0 && context < CONTEXT_LINES {
            start -= 1;
            if matches!(operations[start], Op::Equal(_)) {
                context += 1;
            }
        }
        let mut end = change + 1;
        context = 0;
        while end < operations.len() && context < CONTEXT_LINES {
            if matches!(operations[end], Op::Equal(_)) {
                context += 1;
            }
            end += 1;
        }
        if let Some(last) = windows.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            windows.push((start, end));
        }
    }
    let mut positions = Vec::with_capacity(operations.len() + 1);
    let (mut old, mut new) = (0_u64, 0_u64);
    for operation in operations {
        positions.push((old, new));
        match operation {
            Op::Equal(_) => {
                old += 1;
                new += 1;
            }
            Op::Delete(_) => old += 1,
            Op::Insert(_) => new += 1,
        }
    }
    windows
        .into_iter()
        .map(|(start, end)| {
            let (old_before, new_before) = positions[start];
            let slice = &operations[start..end];
            let old_lines = slice
                .iter()
                .filter(|operation| !matches!(operation, Op::Insert(_)))
                .count() as u64;
            let new_lines = slice
                .iter()
                .filter(|operation| !matches!(operation, Op::Delete(_)))
                .count() as u64;
            let lines = slice
                .iter()
                .map(|operation| match operation {
                    Op::Equal(line) => format!(" {line}"),
                    Op::Delete(line) => format!("-{line}"),
                    Op::Insert(line) => format!("+{line}"),
                })
                .collect();
            WorkspaceDiffHunk {
                old_start: old_before + 1,
                old_lines,
                new_start: new_before + 1,
                new_lines,
                lines,
            }
        })
        .collect()
}

/// Compare two optional texts. A missing side is zero lines; a missing final newline is ignored.
pub fn compare_text(before: Option<&str>, after: Option<&str>, timeout_ms: u64) -> Comparison {
    let before = before.unwrap_or_default();
    let after = after.unwrap_or_default();
    if before == after {
        return Comparison {
            hunks: Vec::new(),
            coarse: false,
            added: 0,
            deleted: 0,
        };
    }
    let old_line_count = content_line_iter(before).count();
    let new_line_count = content_line_iter(after).count();
    let line_reference_bytes = old_line_count
        .checked_add(new_line_count)
        .and_then(|count| count.checked_mul(size_of::<&str>()))
        .unwrap_or(usize::MAX);
    if line_reference_bytes > MAX_MYERS_WORKING_BYTES {
        return coarse(before, after, old_line_count, new_line_count);
    }
    let old = content_lines(before);
    let new = content_lines(after);
    let Some(deadline) = Instant::now().checked_add(Duration::from_millis(timeout_ms)) else {
        return coarse(before, after, old.len(), new.len());
    };
    let Some(operations) = myers(&old, &new, deadline) else {
        return coarse(before, after, old.len(), new.len());
    };
    let added = operations
        .iter()
        .filter(|operation| matches!(operation, Op::Insert(_)))
        .count() as u64;
    let deleted = operations
        .iter()
        .filter(|operation| matches!(operation, Op::Delete(_)))
        .count() as u64;
    Comparison {
        hunks: hunks(&operations),
        coarse: false,
        added,
        deleted,
    }
}
