//! `<think>…</think>` inside content, including a tag split across chunks.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Reasoning(String),
}

pub struct ThinkTagParser {
    in_think: bool,
    /// Partial tag at the end of the previous input.
    pending: String,
    /// Whitespace right after a tag is dropped (`</think>\n\n` before the answer).
    trim_leading: bool,
}

impl ThinkTagParser {
    /// `implicit_open`: the output starts inside the thinking block.
    pub fn new(implicit_open: bool) -> Self {
        Self {
            in_think: implicit_open,
            pending: String::new(),
            trim_leading: implicit_open,
        }
    }

    pub fn push(&mut self, input: &str) -> Vec<Segment> {
        let mut buffer = std::mem::take(&mut self.pending);
        buffer.push_str(input);
        let mut out = Vec::new();
        loop {
            let tag = if self.in_think {
                buffer.find(CLOSE).map(|at| (at, CLOSE))
            } else {
                // A stray close tag outside a block is dropped, never shown as text.
                match (buffer.find(OPEN), buffer.find(CLOSE)) {
                    (Some(o), Some(c)) if c < o => Some((c, CLOSE)),
                    (Some(o), _) => Some((o, OPEN)),
                    (None, Some(c)) => Some((c, CLOSE)),
                    (None, None) => None,
                }
            };
            match tag {
                Some((at, tag)) => {
                    self.emit(&buffer[..at], &mut out);
                    self.in_think = tag == OPEN;
                    self.trim_leading = true;
                    buffer.drain(..at + tag.len());
                }
                None => {
                    let keep = partial_tag_len(&buffer);
                    let cut = buffer.len() - keep;
                    self.emit(&buffer[..cut], &mut out);
                    self.pending = buffer[cut..].to_owned();
                    return out;
                }
            }
        }
    }

    /// End of stream: a held partial tag was ordinary text after all.
    pub fn finish(&mut self) -> Vec<Segment> {
        let rest = std::mem::take(&mut self.pending);
        let mut out = Vec::new();
        self.emit(&rest, &mut out);
        out
    }

    fn emit(&mut self, text: &str, out: &mut Vec<Segment>) {
        let text = if self.trim_leading {
            let trimmed = text.trim_start();
            if trimmed.is_empty() {
                return;
            }
            self.trim_leading = false;
            trimmed
        } else {
            text
        };
        if text.is_empty() {
            return;
        }
        let owned = text.to_owned();
        out.push(if self.in_think {
            Segment::Reasoning(owned)
        } else {
            Segment::Text(owned)
        });
    }
}

/// Length of the longest suffix of `text` that is a proper prefix of either tag.
fn partial_tag_len(text: &str) -> usize {
    [OPEN, CLOSE]
        .iter()
        .flat_map(|tag| (1..tag.len()).rev().map(move |n| &tag[..n]))
        .filter(|prefix| text.ends_with(prefix))
        .map(str::len)
        .max()
        .unwrap_or(0)
}
