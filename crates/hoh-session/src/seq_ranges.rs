use crate::{Result, SessionError, SessionSeq, types::seq_number};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EncodedSeq {
    Seq(SessionSeq),
    Range([SessionSeq; 2]),
}
pub fn encode_seq_ranges(values: &[SessionSeq]) -> Vec<EncodedSeq> {
    if !values.windows(2).all(|v| v[0] < v[1]) {
        return values.iter().copied().map(EncodedSeq::Seq).collect();
    }
    let mut result = vec![];
    let mut start = 0;
    while start < values.len() {
        let mut end = start;
        while end + 1 < values.len() && u64::from(values[end + 1]) == u64::from(values[end]) + 1 {
            end += 1;
        }
        if end - start >= 2 {
            result.push(EncodedSeq::Range([values[start], values[end]]));
        } else {
            result.extend(values[start..=end].iter().copied().map(EncodedSeq::Seq));
        }
        start = end + 1;
    }
    result
}
pub fn decode_seq_ranges(value: &Value, max_entries: Option<u64>) -> Result<Vec<SessionSeq>> {
    let entries = value
        .as_array()
        .ok_or_else(|| SessionError::type_error("sourceEventSeqs must be an array"))?;
    let limit = max_entries.unwrap_or(crate::types::MAX_SAFE_INTEGER);
    let mut decoded = vec![];
    let mut has_range = false;
    let seq = |v: &Value| {
        let zero = v.as_f64().is_some_and(|n| n == 0.0);
        seq_number(v).or_else(|| zero.then_some(0)).ok_or_else(|| {
            SessionError::type_error("sourceEventSeqs must contain non-negative safe integers")
        })
    };
    for entry in entries {
        if entry.is_number() {
            let n = seq(entry)?;
            if decoded.len() as u64 >= limit {
                return Err(SessionError::type_error(
                    "sourceEventSeqs exceeds its event sequence",
                ));
            }
            if n == 0 && entry.as_f64().is_some_and(f64::is_sign_negative) {
                return SessionSeq::from_number(-0.0).map(|seq| vec![seq]);
            }
            decoded.push(SessionSeq::new(n)?);
        } else {
            let pair = entry.as_array().filter(|p| p.len() == 2).ok_or_else(|| {
                SessionError::type_error("sourceEventSeqs range entries must be [start, end] pairs")
            })?;
            let start = seq(&pair[0])?;
            let end = seq(&pair[1])?;
            if end < start {
                return Err(SessionError::type_error(
                    "sourceEventSeqs ranges require start <= end",
                ));
            }
            if end - start + 1 > limit.saturating_sub(decoded.len() as u64) {
                return Err(SessionError::type_error(
                    "sourceEventSeqs range exceeds its event sequence",
                ));
            }
            if start == 0 && pair[0].as_f64().is_some_and(f64::is_sign_negative) {
                return SessionSeq::from_number(-0.0).map(|seq| vec![seq]);
            }
            for n in start..=end {
                decoded.push(SessionSeq::new(n)?);
            }
            has_range = true;
        }
    }
    if has_range && !decoded.windows(2).all(|v| v[0] < v[1]) {
        return Err(SessionError::type_error(
            "sourceEventSeqs ranges must be strictly increasing",
        ));
    }
    Ok(decoded)
}
