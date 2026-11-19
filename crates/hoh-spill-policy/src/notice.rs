//! Browser-safe formatting and recognition of persisted spill-policy notices.

use hoh_output_retention::{Omitted, RetentionUnit, describe_omitted};
use hoh_spill::SpillRef;

const OPEN: &str = "(";
const CLOSE: &str = ")";
const LOCATION: &str = " Full formatted result stored at: ";
const GUIDANCE_SEPARATOR: &str = ". ";
const SEPARATOR: &str = "\n\n";
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Format one persisted recovery notice without a leading preview separator.
pub fn format_spill_notice(omitted: &Omitted, reference: &SpillRef, images: u64) -> String {
    let image_notice = if images > 0 {
        format!(" Omitted {images} images.")
    } else {
        String::new()
    };
    format!(
        "{OPEN}{}{image_notice}{LOCATION}{}{GUIDANCE_SEPARATOR}{}{CLOSE}",
        describe_omitted(omitted, RetentionUnit::Bytes),
        reference.locator,
        reference.retrieval_hint
    )
}

fn canonical_count(text: &str) -> Option<u64> {
    if text.is_empty()
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    let count = text.parse::<u64>().ok()?;
    (count <= MAX_SAFE_INTEGER).then_some(count)
}

fn is_omission(mut text: &str) -> bool {
    if let Some(prefix) = text.strip_suffix(" images.")
        && let Some((base, count)) = prefix.rsplit_once(" Omitted ")
    {
        let Some(count) = canonical_count(count) else {
            return false;
        };
        if count == 0 {
            return false;
        }
        text = base;
    }
    if text.is_empty() || text == "More bytes were omitted." {
        return true;
    }
    let Some(count) = text
        .strip_prefix("Omitted ")
        .and_then(|text| text.strip_suffix(" bytes."))
    else {
        return false;
    };
    canonical_count(count).is_some()
}

/// Recognize a complete final spill-policy notice in persisted text.
pub fn has_spill_notice(text: &str) -> bool {
    if !text.ends_with(CLOSE) {
        return false;
    }
    let mut start = 0;
    loop {
        let next = text[start..].find("\n\n(").map(|offset| start + offset);
        let end = next.unwrap_or(text.len() - CLOSE.len());
        let candidate = &text[start..end];
        if candidate.starts_with(OPEN)
            && let Some(location) = candidate.find(LOCATION)
            && is_omission(&candidate[OPEN.len()..location])
            && text[start + location + LOCATION.len()..].contains(GUIDANCE_SEPARATOR)
        {
            return true;
        }
        let Some(next) = next else {
            return false;
        };
        start = next + SEPARATOR.len();
    }
}
