//! Ordered head/tail retention of text and indivisible images.

use hoh_attachment::ImageAttachmentRef;
use hoh_llm::{ContentBlock, ImageBlock};
use serde::{Deserialize, Serialize};

/// Content whose text can be split and whose images must remain whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RetainableBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        attachment: ImageAttachmentRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offloaded: Option<bool>,
    },
}

impl RetainableBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn text_value(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Image { .. } => None,
        }
    }

    pub(crate) fn text_ref(&self) -> Option<&str> {
        self.text_value()
    }

    pub fn image(&self) -> Option<ImageBlock> {
        match self {
            Self::Text { .. } => None,
            Self::Image {
                attachment,
                offloaded,
            } => Some(ImageBlock {
                attachment: attachment.clone(),
                offloaded: *offloaded,
            }),
        }
    }

    pub(crate) fn from_content(block: &ContentBlock) -> Option<Self> {
        match block.r#type.as_str() {
            "text" => Some(Self::text(block.text_value()?)),
            "image" => {
                let image = block.image()?;
                Some(Self::Image {
                    attachment: image.attachment,
                    offloaded: image.offloaded,
                })
            }
            _ => None,
        }
    }

    pub(crate) fn into_content(self) -> ContentBlock {
        match self {
            Self::Text { text } => ContentBlock::text(text),
            Self::Image {
                attachment,
                offloaded,
            } => ImageBlock {
                attachment,
                offloaded,
            }
            .into(),
        }
    }
}

/// Retained ends and exact quantities excluded from their original sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct RetainedContent {
    pub head: Vec<RetainableBlock>,
    pub tail: Vec<RetainableBlock>,
    pub omitted_bytes: u64,
    pub omitted_images: u64,
}

fn text_slice(text: &str, length: u64, tail: bool) -> String {
    let units = text.encode_utf16().collect::<Vec<_>>();
    let length = usize::try_from(length)
        .unwrap_or(usize::MAX)
        .min(units.len());
    let mut cut = if tail { units.len() - length } else { length };
    if cut > 0
        && cut < units.len()
        && (0xd800..=0xdbff).contains(&units[cut - 1])
        && (0xdc00..=0xdfff).contains(&units[cut])
    {
        if tail {
            cut += 1;
        } else {
            cut -= 1;
        }
    }
    let slice = if tail { &units[cut..] } else { &units[..cut] };
    String::from_utf16(slice).expect("the adjusted UTF-16 slice contains complete scalars")
}

fn drop_utf16_prefix(text: &str, length: u64) -> String {
    let units = text.encode_utf16().collect::<Vec<_>>();
    let cut = usize::try_from(length)
        .unwrap_or(usize::MAX)
        .min(units.len());
    String::from_utf16(&units[cut..]).expect("retained prefix ended at a scalar boundary")
}

fn fit_text(
    text: &str,
    budget: u64,
    tail: bool,
    price: &impl Fn(&RetainableBlock) -> u64,
) -> String {
    let mut low = 0_u64;
    let mut high = text.encode_utf16().count() as u64;
    while low < high {
        let middle = (low + high).div_ceil(2);
        let candidate = text_slice(text, middle, tail);
        if candidate.is_empty() || price(&RetainableBlock::text(&candidate)) <= budget {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    text_slice(text, low, tail)
}

fn text_bytes(blocks: &[RetainableBlock]) -> u64 {
    blocks.iter().fold(0_u64, |total, block| {
        total.saturating_add(block.text_ref().map_or(0, |text| text.len() as u64))
    })
}

fn image_count(blocks: &[RetainableBlock]) -> u64 {
    blocks
        .iter()
        .filter(|block| block.image().is_some())
        .count() as u64
}

/// Retain contiguous ends without moving or partially retaining an image.
pub fn retain_content(
    content: &[RetainableBlock],
    budget: u64,
    price: impl Fn(&RetainableBlock) -> u64,
) -> RetainedContent {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut first = 0_usize;
    let mut last = content.len().checked_sub(1);
    let mut head_characters = 0_u64;
    let mut remaining = budget.div_ceil(2);
    while last.is_some_and(|last| first <= last) {
        let block = &content[first];
        let cost = price(block);
        if cost <= remaining {
            head.push(block.clone());
            remaining -= cost;
            first += 1;
            continue;
        }
        if let Some(value) = block.text_ref() {
            let text = fit_text(value, remaining, false, &price);
            head_characters = text.encode_utf16().count() as u64;
            if !text.is_empty() {
                head.push(RetainableBlock::text(text));
            }
        }
        break;
    }
    remaining = budget / 2;
    while let Some(index) = last.filter(|last| *last >= first) {
        let original = &content[index];
        let split;
        let block = if index == first {
            if let Some(text) = original.text_ref() {
                split = RetainableBlock::text(drop_utf16_prefix(text, head_characters));
                &split
            } else {
                original
            }
        } else {
            original
        };
        let cost = price(block);
        if cost <= remaining {
            tail.push(block.clone());
            remaining -= cost;
            last = index.checked_sub(1);
            continue;
        }
        if let Some(value) = block.text_ref() {
            let text = fit_text(value, remaining, true, &price);
            if !text.is_empty() {
                tail.push(RetainableBlock::text(text));
            }
        }
        break;
    }
    tail.reverse();
    let original_bytes = text_bytes(content);
    let retained_bytes = text_bytes(&head).saturating_add(text_bytes(&tail));
    let original_images = image_count(content);
    let retained_images = image_count(&head).saturating_add(image_count(&tail));
    RetainedContent {
        head,
        tail,
        omitted_bytes: original_bytes.saturating_sub(retained_bytes),
        omitted_images: original_images.saturating_sub(retained_images),
    }
}
