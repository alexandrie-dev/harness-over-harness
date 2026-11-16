use crate::diff::base;
use crate::{Change, PatchOptions};

/// Diffs two blocks of text, treating each line as a token.
pub fn diff_lines(old_str: &str, new_str: &str, options: &PatchOptions) -> Option<Vec<Change>> {
    base::diff(tokenize(old_str), tokenize(new_str), options)
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split_inclusive('\n')
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}
