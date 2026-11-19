//! The `single` matcher and score from fuzzysort 4.0.2 (MIT).

mod remap;

use hoh_util_values::{is_js_white_space, js_trim};

use remap::remap;

#[derive(Debug)]
pub(crate) struct Prepared {
    lower: Vec<u16>,
    beginnings: Vec<usize>,
}

impl Prepared {
    pub(crate) fn new(target: &str) -> Self {
        let remapped = remap(target);
        Self {
            lower: remapped.to_lowercase().encode_utf16().collect(),
            beginnings: beginnings(&remapped),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Match {
    pub(crate) score: f64,
    pub(crate) first_index: usize,
}

pub(crate) fn single(search: &str, target: &Prepared) -> Option<Match> {
    let query = remap(js_trim(search));
    let lower = query.to_lowercase();
    let search: Vec<u16> = lower.encode_utf16().collect();
    if search.is_empty() {
        return None;
    }
    let (raw_score, first_index) = if search.contains(&u16::from(b' ')) {
        let mut unique = Vec::<String>::new();
        for part in lower
            .split(is_js_white_space)
            .filter(|part| !part.is_empty())
        {
            if !unique.iter().any(|seen| seen == part) {
                unique.push(part.to_owned());
            }
        }
        let parts: Vec<Vec<u16>> = unique
            .iter()
            .map(|part| part.encode_utf16().collect())
            .collect();
        algorithm_spaces(&search, &parts, target)?
    } else if search.len() == 2 {
        algorithm_two(&search, target)?
    } else {
        algorithm(&search, target)?
    };
    Some(Match {
        score: normalize_score(raw_score),
        first_index,
    })
}

fn algorithm_two(search: &[u16], target: &Prepared) -> Option<(f64, usize)> {
    let target_codes = &target.lower;
    let target_len = target_codes.len();
    let mut first = target_codes.iter().position(|code| *code == search[0])?;
    let mut second = target_codes[first + 1..]
        .iter()
        .position(|code| *code == search[1])?
        + first
        + 1;
    let beginnings = &target.beginnings;
    let mut substring_index = find_slice(target_codes, search, first);
    let substring = substring_index.is_some();
    let mut low = lower_bound(beginnings, substring_index.unwrap_or(usize::MAX));
    let mut substring_start =
        substring_index.is_some_and(|index| beginnings.get(low) == Some(&index));
    let mut strict = substring_start && substring_index == Some(first);

    if !strict {
        let mut first_beginning = lower_bound(beginnings, first);
        let mut backtracks = 0;
        while first_beginning < beginnings.len() {
            let strict_first = beginnings[first_beginning];
            if target_codes.get(strict_first) != Some(&search[0]) {
                first_beginning += 1;
                continue;
            }
            let consecutive = strict_first + 1;
            if target_codes.get(consecutive) == Some(&search[1]) {
                first = strict_first;
                second = consecutive;
                strict = true;
                break;
            }
            let mut second_beginning = first_beginning + 1;
            while second_beginning < beginnings.len() && beginnings[second_beginning] <= consecutive
            {
                second_beginning += 1;
            }
            while second_beginning < beginnings.len()
                && target_codes.get(beginnings[second_beginning]) != Some(&search[1])
            {
                second_beginning += 1;
            }
            if second_beginning < beginnings.len() {
                first = strict_first;
                second = beginnings[second_beginning];
                strict = true;
                break;
            }
            backtracks += 1;
            if backtracks > 200 {
                break;
            }
            first_beginning += 1;
        }
    }

    if substring && !substring_start {
        let original = substring_index.expect("substring checked");
        while low < beginnings.len() && beginnings[low] <= original {
            low += 1;
        }
        for &index in &beginnings[low..] {
            if target_codes.get(index) == Some(&search[0])
                && target_codes.get(index + 1) == Some(&search[1])
            {
                substring_index = Some(index);
                substring_start = true;
                break;
            }
        }
    }
    if substring && (!strict || substring_start) {
        first = substring_index.expect("substring checked");
        second = first + 1;
    }
    let mut score = if second - first == 1 {
        0.0
    } else {
        first as f64 - second as f64 * 2.0 - 11.0
    };
    if first != 0 {
        score -= (first * first) as f64 * 0.2;
    }
    if !strict {
        score *= 1000.0;
    } else if beginnings.len() > 24 {
        score *= ((beginnings.len() - 24) * 10) as f64;
    }
    let length_penalty = (target_len - 2) as f64 / 2.0;
    score -= length_penalty;
    if substring {
        score /= 5.0;
    }
    if substring_start {
        score /= 5.0;
    }
    score -= length_penalty;
    let substring_end = substring
        && second == first + 1
        && (second + 1 == target_len || beginnings.binary_search(&(second + 1)).is_ok());
    if substring_end {
        score += 4.0_f64.min(-score / 2.0);
    }
    Some((score, first))
}

#[derive(Debug)]
struct ScoredMatch {
    score: f64,
    indexes: Vec<usize>,
}

fn algorithm(search: &[u16], target: &Prepared) -> Option<(f64, usize)> {
    let mut next = next_beginnings(target.lower.len(), &target.beginnings);
    let result = algorithm_with_next(search, target, &mut next, false, true)?;
    Some((result.score, *result.indexes.iter().min()?))
}

fn algorithm_spaces(search: &[u16], parts: &[Vec<u16>], target: &Prepared) -> Option<(f64, usize)> {
    if parts.is_empty() {
        return None;
    }
    let mut next = next_beginnings(target.lower.len(), &target.beginnings);
    let mut changed = false;
    let mut score = 0.0;
    let mut previous_first = 0;
    let mut seen = Vec::new();
    for (part_index, part) in parts.iter().enumerate() {
        let result = algorithm_with_next(part, target, &mut next, changed, false)?;
        if part_index + 1 < parts.len()
            && result.indexes.windows(2).all(|pair| pair[1] - pair[0] == 1)
        {
            let beginning = result.indexes[result.indexes.len() - 1] + 1;
            let previous = next[beginning - 1];
            let mut index = beginning - 1;
            loop {
                if next[index] != previous {
                    break;
                }
                next[index] = beginning;
                changed = true;
                if index == 0 {
                    break;
                }
                index -= 1;
            }
        }
        score += result.score / parts.len() as f64;
        if result.indexes[0] < previous_first {
            score -= (previous_first - result.indexes[0]) as f64 * 2.0;
        }
        previous_first = result.indexes[0];
        for index in result.indexes {
            if !seen.contains(&index) {
                seen.push(index);
            }
        }
    }

    if target.lower.contains(&u16::from(b' ')) {
        let mut unmodified = next_beginnings(target.lower.len(), &target.beginnings);
        if let Some(spaced) = algorithm_with_next(search, target, &mut unmodified, false, true)
            && spaced.score > score
        {
            return Some((spaced.score, *spaced.indexes.iter().min()?));
        }
    }
    Some((score, *seen.iter().min()?))
}

fn algorithm_with_next(
    search: &[u16],
    target: &Prepared,
    next: &mut [usize],
    changed: bool,
    single_match: bool,
) -> Option<ScoredMatch> {
    let target_codes = &target.lower;
    let target_len = target_codes.len();
    let search_len = search.len();
    let mut simple = Vec::with_capacity(search_len);
    let mut target_index = 0;
    for &code in search {
        while target_codes.get(target_index) != Some(&code) {
            target_index += 1;
            if target_index >= target_len {
                return None;
            }
        }
        simple.push(target_index);
        target_index += 1;
    }

    let current_beginnings = beginnings_from_next(next);
    let beginning_len = current_beginnings.len().max(1);
    if search_len == 1 && !changed && single_match {
        let first_simple = simple[0];
        let strict_match = target
            .beginnings
            .iter()
            .copied()
            .find(|index| target_codes.get(*index) == Some(&search[0]));
        let strict = strict_match.is_some();
        let matched = strict_match.unwrap_or(first_simple);
        let mut score = if matched == 0 {
            0.0
        } else {
            -((matched * matched) as f64) * 0.2
        };
        if !strict {
            score *= 1000.0;
        } else if beginning_len > 24 {
            score *= ((beginning_len - 24) * 10) as f64;
        }
        return Some(ScoredMatch {
            score: score - (target_len - 1) as f64,
            indexes: vec![matched],
        });
    }

    let mut substring_index = if search_len > 1 {
        find_slice(target_codes, search, simple[0])
    } else {
        None
    };
    let is_substring = substring_index.is_some();
    let mut substring_start =
        substring_index.is_some_and(|index| index == 0 || next.get(index - 1) == Some(&index));
    let mut strict = substring_start && substring_index == Some(simple[0]);
    let mut strict_matches = Vec::with_capacity(search_len);
    if !strict {
        let mut search_index = 0;
        target_index = if simple[0] == 0 {
            0
        } else {
            next[simple[0] - 1]
        };
        let mut backtracks = 0;
        while target_index < target_len {
            if search[search_index] == target_codes[target_index] {
                strict_matches.push(target_index);
                search_index += 1;
                if search_index == search_len {
                    strict = true;
                    break;
                }
                target_index += 1;
            } else {
                target_index = next[target_index];
            }
            while target_index >= target_len && search_index > 0 {
                backtracks += 1;
                if backtracks > 200 {
                    break;
                }
                search_index -= 1;
                let previous = strict_matches.pop().expect("strict match exists");
                target_index = next[previous];
            }
            if backtracks > 200 {
                break;
            }
        }
    }

    if is_substring && !substring_start {
        let original = substring_index.expect("substring checked");
        for &index in &current_beginnings {
            if index > original && target_codes.get(index..index + search_len) == Some(search) {
                substring_index = Some(index);
                substring_start = true;
                break;
            }
        }
    }
    let matches = if !strict || substring_start {
        if is_substring {
            let start = substring_index.expect("substring checked");
            (start..start + search_len).collect::<Vec<_>>()
        } else {
            simple
        }
    } else {
        strict_matches
    };
    let match_end = matches[search_len - 1] + 1;
    let substring_end = is_substring
        && match_end - matches[0] == search_len
        && (match_end == target_len || next[match_end - 1] == match_end);
    let score = calculate_score(
        &matches,
        target_len,
        strict,
        beginning_len,
        is_substring,
        substring_start,
        substring_end,
    );
    Some(ScoredMatch {
        score,
        indexes: matches,
    })
}

fn calculate_score(
    matches: &[usize],
    target_len: usize,
    strict: bool,
    beginning_len: usize,
    substring: bool,
    substring_start: bool,
    substring_end: bool,
) -> f64 {
    let mut score = 0.0;
    let mut groups = 0;
    for pair in matches.windows(2) {
        if pair[1] - pair[0] != 1 {
            score -= pair[1] as f64;
            groups += 1;
        }
    }
    let unmatched = matches[matches.len() - 1] - matches[0] - (matches.len() - 1);
    score -= (12 + unmatched) as f64 * groups as f64;
    if matches[0] != 0 {
        score -= (matches[0] * matches[0]) as f64 * 0.2;
    }
    if !strict {
        score *= 1000.0;
    } else if beginning_len > 24 {
        score *= ((beginning_len - 24) * 10) as f64;
    }
    let length_penalty = (target_len - matches.len()) as f64 / 2.0;
    score -= length_penalty;
    let bonus = (1 + matches.len() * matches.len()) as f64;
    if substring {
        score /= bonus;
    }
    if substring_start {
        score /= bonus;
    }
    score -= length_penalty;
    if substring_end {
        score + 4.0_f64.min(-score / 2.0)
    } else {
        score
    }
}

fn beginnings(value: &str) -> Vec<usize> {
    let codes: Vec<u16> = value.encode_utf16().collect();
    let mut result = Vec::new();
    let mut was_upper = false;
    let mut was_alnum = false;
    for (index, &code) in codes.iter().enumerate() {
        let upper = (u16::from(b'A')..=u16::from(b'Z')).contains(&code);
        let alnum = upper
            || (u16::from(b'a')..=u16::from(b'z')).contains(&code)
            || (u16::from(b'0')..=u16::from(b'9')).contains(&code);
        if (upper && !was_upper) || !was_alnum || !alnum {
            result.push(index);
        }
        was_upper = upper;
        was_alnum = alnum;
    }
    result
}

fn next_beginnings(length: usize, beginnings: &[usize]) -> Vec<usize> {
    let mut next = vec![length; length];
    let mut fill = 0;
    for &beginning in beginnings {
        while fill < beginning {
            next[fill] = beginning;
            fill += 1;
        }
    }
    next
}

fn beginnings_from_next(next: &[usize]) -> Vec<usize> {
    if next.is_empty() {
        return Vec::new();
    }
    let mut result = vec![0];
    let mut index = next[0];
    while index < next.len() {
        result.push(index);
        let following = next[index];
        if following <= index {
            break;
        }
        index = following;
    }
    result
}

fn find_slice(target: &[u16], search: &[u16], from: usize) -> Option<usize> {
    if search.len() > target.len() || from > target.len() - search.len() {
        return None;
    }
    (from..=target.len() - search.len())
        .find(|&index| &target[index..index + search.len()] == search)
}

fn lower_bound(values: &[usize], value: usize) -> usize {
    values.partition_point(|candidate| *candidate < value)
}

fn normalize_score(value: f64) -> f64 {
    if value == f64::NEG_INFINITY {
        0.0
    } else if value > 1.0 {
        value
    } else {
        std::f64::consts::E.powf(((-value + 1.0).powf(0.043_07) - 1.0) * -2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Prepared, single};

    #[test]
    fn uses_fuzzysort_ascii_remappings() {
        assert!(single("a\\b", &Prepared::new("a/b")).is_some());
        assert!(single("a`b", &Prepared::new("a'b")).is_some());
        assert!(single("a\"b", &Prepared::new("a'b")).is_some());
        assert!(single("a‘b", &Prepared::new("a'b")).is_some());
        assert!(single(" ", &Prepared::new("anything")).is_none());
    }
}
