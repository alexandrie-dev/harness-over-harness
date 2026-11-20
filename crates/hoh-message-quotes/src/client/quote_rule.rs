//! Quote UserText rule and a small browser-independent evaluator.

use super::quote_text::{message_quote_ranges, quote_text_of, slice_utf16};

/// Atomic node produced by the quote rule or an ordinary text span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserTextNode {
    Quote {
        start: usize,
        end: usize,
        text: String,
    },
    Text {
        start: usize,
        end: usize,
        text: String,
        display: Option<String>,
    },
}

impl UserTextNode {
    pub fn start(&self) -> usize {
        match self {
            Self::Quote { start, .. } | Self::Text { start, .. } => *start,
        }
    }

    pub fn end(&self) -> usize {
        match self {
            Self::Quote { end, .. } | Self::Text { end, .. } => *end,
        }
    }
}

/// Match the next quote node or adjacent paragraph separator at/after `from`.
pub fn quote_rule_match(text: &str, from: usize) -> Option<UserTextNode> {
    for range in message_quote_ranges(text) {
        for start in [
            range.start.checked_sub(2),
            Some(range.start),
            Some(range.end),
        ] {
            let Some(start) = start.filter(|start| *start >= from) else {
                continue;
            };
            if start == range.start {
                let source = slice_utf16(text, start, range.end)?;
                return Some(UserTextNode::Quote {
                    start,
                    end: range.end,
                    text: quote_text_of(source),
                });
            }
            if slice_utf16(text, start, start + 2) == Some("\n\n") {
                return Some(UserTextNode::Text {
                    start,
                    end: start + 2,
                    text: "\n\n".into(),
                    display: Some(" ".into()),
                });
            }
        }
    }
    None
}

/// Evaluate the quote rule while preserving every original UTF-16 span.
pub fn evaluate_quote_text(text: &str) -> Vec<UserTextNode> {
    let total = text.encode_utf16().count();
    let mut nodes = Vec::new();
    let mut cursor = 0;
    while let Some(node) = quote_rule_match(text, cursor) {
        if node.start() > cursor {
            let source = slice_utf16(text, cursor, node.start())
                .expect("rule offsets are scalar boundaries");
            nodes.push(UserTextNode::Text {
                start: cursor,
                end: node.start(),
                text: source.into(),
                display: None,
            });
        }
        cursor = node.end();
        nodes.push(node);
    }
    if cursor < total {
        nodes.push(UserTextNode::Text {
            start: cursor,
            end: total,
            text: slice_utf16(text, cursor, total)
                .expect("rule offsets are scalar boundaries")
                .into(),
            display: None,
        });
    }
    nodes
}
