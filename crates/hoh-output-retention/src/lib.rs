//! Bounded model-facing output: ordered items, byte windows and omission notices.
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, Visitor},
};
use std::{collections::VecDeque, fmt};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Omitted {
    None,
    Exact { count: u64 },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushDecision {
    pub kept: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedItems<T> {
    pub items: Vec<T>,
    pub truncated: bool,
    pub seen: u64,
    pub kept: u64,
    pub omitted: Omitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedText {
    pub text: String,
    pub truncated: bool,
    pub omitted_bytes: Omitted,
}

// Rust's typed API cannot accept a negative/fractional size. Validate untyped
// configuration at deserialization, preserving the source's diagnostic text.
struct BudgetVisitor(&'static str);
impl BudgetVisitor {
    fn error<E: de::Error>(&self) -> E {
        E::custom(format!("{} must be a non-negative integer", self.0))
    }
}
impl<'de> Visitor<'de> for BudgetVisitor {
    type Value = u64;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} must be a non-negative integer", self.0)
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<u64, E> {
        Ok(value)
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<u64, E> {
        u64::try_from(value).map_err(|_| self.error())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<u64, E> {
        if value.is_finite()
            && value >= 0.0
            && value.fract() == 0.0
            && value < 18_446_744_073_709_551_616.0
        {
            Ok(value as u64)
        } else {
            Err(self.error())
        }
    }
}
fn max_items<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    d.deserialize_any(BudgetVisitor("maxItems"))
}
fn max_bytes<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    d.deserialize_any(BudgetVisitor("maxBytes"))
}
fn head_bytes<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    d.deserialize_any(BudgetVisitor("headBytes"))
}
fn tail_bytes<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    d.deserialize_any(BudgetVisitor("tailBytes"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ItemRetentionStrategy {
    Head {
        #[serde(deserialize_with = "max_items")]
        max_items: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TextRetentionStrategy {
    Head {
        #[serde(deserialize_with = "max_bytes")]
        max_bytes: u64,
    },
    Tail {
        #[serde(deserialize_with = "max_bytes")]
        max_bytes: u64,
    },
    HeadTail {
        #[serde(deserialize_with = "head_bytes")]
        head_bytes: u64,
        #[serde(deserialize_with = "tail_bytes")]
        tail_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetentionStrategy {
    Head,
    Tail,
    HeadTail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetentionUnit {
    Items,
    Bytes,
    Chars,
    Lines,
}
impl fmt::Display for RetentionUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Items => "items",
            Self::Bytes => "bytes",
            Self::Chars => "chars",
            Self::Lines => "lines",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RetentionLimit {
    Count(u64),
    HeadTail { head: u64, tail: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionNotice {
    pub scope: String,
    pub strategy: RetentionStrategy,
    pub unit: RetentionUnit,
    pub limit: RetentionLimit,
    pub kept: u64,
    pub omitted: Omitted,
}

pub struct ItemRetainer<T> {
    max_items: u64,
    items: Vec<T>,
    seen: u64,
    omitted_count: u64,
}
impl<T> ItemRetainer<T> {
    pub fn new(strategy: ItemRetentionStrategy) -> Self {
        let ItemRetentionStrategy::Head { max_items } = strategy;
        Self {
            max_items,
            items: vec![],
            seen: 0,
            omitted_count: 0,
        }
    }
    pub fn push(&mut self, item: T) -> PushDecision {
        self.seen += 1;
        if (self.items.len() as u64) < self.max_items {
            self.items.push(item);
            PushDecision {
                kept: true,
                truncated: false,
            }
        } else {
            self.omitted_count += 1;
            PushDecision {
                kept: false,
                truncated: true,
            }
        }
    }
    pub fn finish(&self) -> RetainedItems<T>
    where
        T: Clone,
    {
        RetainedItems {
            items: self.items.clone(),
            truncated: self.omitted_count > 0,
            seen: self.seen,
            kept: self.items.len() as u64,
            omitted: omitted(self.omitted_count),
        }
    }
}

fn omitted(count: u64) -> Omitted {
    if count > 0 {
        Omitted::Exact { count }
    } else {
        Omitted::None
    }
}

fn trim_trailing_partial_utf8(bytes: &[u8]) -> &[u8] {
    let mut offset = bytes.len();
    while offset > 0 && bytes[offset - 1] & 0xc0 == 0x80 && bytes.len() - (offset - 1) <= 3 {
        offset -= 1;
    }
    if offset == 0 {
        return bytes;
    }
    let index = offset - 1;
    let lead = bytes[index];
    let expected = if lead < 0x80 {
        1
    } else if lead < 0xe0 {
        2
    } else if lead < 0xf0 {
        3
    } else if lead < 0xf8 {
        4
    } else {
        0
    };
    if expected > 0 && bytes.len() - index < expected {
        &bytes[..index]
    } else {
        bytes
    }
}
fn trim_leading_continuation_utf8(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| byte & 0xc0 != 0x80)
        .unwrap_or(bytes.len());
    &bytes[start..]
}
fn decode(bytes: &[u8]) -> String {
    // TextDecoder's default ignoreBOM=false consumes one leading UTF-8 BOM.
    String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)).into_owned()
}

pub struct TextRetainer {
    prefix_cap: u64,
    suffix_cap: u64,
    prefix: Vec<u8>,
    suffix: VecDeque<u8>,
    total: u64,
}
impl TextRetainer {
    pub fn new(strategy: TextRetentionStrategy) -> Self {
        let (prefix_cap, suffix_cap) = match strategy {
            TextRetentionStrategy::Head { max_bytes } => (max_bytes, 0),
            TextRetentionStrategy::Tail { max_bytes } => (0, max_bytes),
            TextRetentionStrategy::HeadTail {
                head_bytes,
                tail_bytes,
            } => (head_bytes, tail_bytes),
        };
        Self {
            prefix_cap,
            suffix_cap,
            prefix: vec![],
            suffix: VecDeque::new(),
            total: 0,
        }
    }
    pub fn push(&mut self, chunk: impl AsRef<[u8]>) -> PushDecision {
        let bytes = chunk.as_ref();
        let before = self.total;
        self.total += bytes.len() as u64;
        let take = (self.prefix_cap - self.prefix.len() as u64).min(bytes.len() as u64) as usize;
        self.prefix.extend_from_slice(&bytes[..take]);
        if self.suffix_cap > 0 {
            let keep = self.suffix_cap.min(bytes.len() as u64) as usize;
            let old_room = self.suffix_cap - keep as u64;
            let discard = (self.suffix.len() as u64).saturating_sub(old_room) as usize;
            self.suffix.drain(..discard);
            self.suffix.extend(&bytes[bytes.len() - keep..]);
        }
        let after_omitted = self.omitted_at(self.total);
        PushDecision {
            kept: after_omitted <= self.omitted_at(before),
            truncated: after_omitted > 0,
        }
    }
    fn omitted_at(&self, total: u64) -> u64 {
        let prefix_len = total.min(self.prefix_cap);
        total - prefix_len - (total - prefix_len).min(self.suffix_cap)
    }
    pub fn finish(&self) -> RetainedText {
        let prefix_len = self.total.min(self.prefix_cap);
        let suffix_len = (self.total - prefix_len).min(self.suffix_cap) as usize;
        let suffix: Vec<_> = self
            .suffix
            .iter()
            .skip(self.suffix.len() - suffix_len)
            .copied()
            .collect();
        let (text, kept) = if self.omitted_at(self.total) > 0 {
            let prefix = trim_trailing_partial_utf8(&self.prefix);
            let suffix = trim_leading_continuation_utf8(&suffix);
            (
                decode(prefix) + &decode(suffix),
                prefix.len() + suffix.len(),
            )
        } else {
            let mut all = self.prefix.clone();
            all.extend_from_slice(&suffix);
            (decode(&all), all.len())
        };
        let count = self.total - kept as u64;
        RetainedText {
            text,
            truncated: count > 0,
            omitted_bytes: omitted(count),
        }
    }
}

pub fn describe_omitted(omitted: &Omitted, unit: RetentionUnit) -> String {
    match omitted {
        Omitted::None => String::new(),
        Omitted::Exact { count } => format!("Omitted {count} {unit}."),
        Omitted::Unknown => format!("More {unit} were omitted."),
    }
}
pub fn format_retention_notice(
    notice: &RetentionNotice,
    recovery: impl FnOnce(&RetentionNotice) -> String,
) -> String {
    [
        describe_omitted(&notice.omitted, notice.unit),
        recovery(notice),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

/// Keep a prefix bounded in UTF-16 code units, dropping a split surrogate pair.
pub fn truncate_without_splitting_surrogate_pair(text: &str, max_chars: u64) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16() as u64;
        if units > max_chars {
            return &text[..index];
        }
    }
    text
}
