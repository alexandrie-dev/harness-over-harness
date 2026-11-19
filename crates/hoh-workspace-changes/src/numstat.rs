//! Parsing of `git diff-tree --numstat -z` output.

use hoh_error::BoxError;

/// One numstat record relative to the repository root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumstatEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub added: u64,
    pub deleted: u64,
    pub binary: bool,
}

/// Parse complete NUL-terminated numstat output, including rename records.
pub fn parse_numstat(output: &str) -> Result<Vec<NumstatEntry>, BoxError> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    if !output.ends_with('\0') {
        return Err("numstat output is not NUL-terminated".into());
    }
    let mut queue = output[..output.len() - 1].split('\0');
    let mut entries = Vec::new();
    while let Some(record) = queue.next() {
        let first = record
            .find('\t')
            .ok_or_else(|| format!("malformed numstat record: {record}"))?;
        let second = record[first + 1..]
            .find('\t')
            .map(|index| first + 1 + index)
            .ok_or_else(|| format!("malformed numstat record: {record}"))?;
        let added = &record[..first];
        let deleted = &record[first + 1..second];
        let mut path = record[second + 1..].to_owned();
        let old_path = if path.is_empty() {
            let old = queue
                .next()
                .ok_or("malformed numstat rename record")?
                .to_owned();
            path = queue
                .next()
                .ok_or("malformed numstat rename record")?
                .to_owned();
            Some(old)
        } else {
            None
        };
        let binary = added == "-";
        entries.push(NumstatEntry {
            path,
            old_path,
            added: if binary { 0 } else { added.parse()? },
            deleted: if binary { 0 } else { deleted.parse()? },
            binary,
        });
    }
    Ok(entries)
}
