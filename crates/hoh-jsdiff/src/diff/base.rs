use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Change, PatchOptions};

#[derive(Debug)]
struct Component {
    added: bool,
    removed: bool,
    count: usize,
    previous: Option<Rc<Component>>,
}

#[derive(Clone, Debug)]
struct Path {
    old_pos: isize,
    last_component: Option<Rc<Component>>,
}

pub(crate) fn diff(
    old_tokens: Vec<String>,
    new_tokens: Vec<String>,
    options: &PatchOptions,
) -> Option<Vec<Change>> {
    let new_len = new_tokens.len() as isize;
    let old_len = old_tokens.len() as isize;
    let mut edit_length = 1;
    let mut max_edit_length = new_len + old_len;
    if let Some(limit) = options.max_edit_length {
        let limit = isize::try_from(limit).unwrap_or(isize::MAX);
        max_edit_length = max_edit_length.min(limit);
    }
    let started_at = now_millis();
    let abort_after_timestamp = options.timeout_ms.map_or(u128::MAX, |timeout| {
        started_at.saturating_add(u128::from(timeout))
    });

    let mut initial_path = Path {
        old_pos: -1,
        last_component: None,
    };
    let mut new_pos = extract_common(&mut initial_path, &new_tokens, &old_tokens, 0);
    if initial_path.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return Some(build_values(
            initial_path.last_component,
            &new_tokens,
            &old_tokens,
        ));
    }

    let mut best_path = BTreeMap::from([(0, initial_path)]);
    let mut min_diagonal_to_consider = None;
    let mut max_diagonal_to_consider = None;

    while edit_length <= max_edit_length && now_millis() <= abort_after_timestamp {
        let first_diagonal =
            min_diagonal_to_consider.map_or(-edit_length, |min: isize| min.max(-edit_length));
        let last_diagonal =
            max_diagonal_to_consider.map_or(edit_length, |max: isize| max.min(edit_length));

        for diagonal_path in (first_diagonal..=last_diagonal).step_by(2) {
            let remove_path = best_path.remove(&(diagonal_path - 1));
            let add_path = best_path.get(&(diagonal_path + 1)).cloned();

            let can_add = add_path.as_ref().is_some_and(|path| {
                let add_path_new_pos = path.old_pos - diagonal_path;
                0 <= add_path_new_pos && add_path_new_pos < new_len
            });
            let can_remove = remove_path
                .as_ref()
                .is_some_and(|path| path.old_pos + 1 < old_len);

            if !can_add && !can_remove {
                best_path.remove(&diagonal_path);
                continue;
            }

            let prefer_add = match (&remove_path, &add_path) {
                (Some(remove), Some(add)) => can_add && remove.old_pos < add.old_pos,
                _ => false,
            };
            let mut base_path = if !can_remove || prefer_add {
                match add_path.as_ref() {
                    Some(path) => add_to_path(path, true, false, 0),
                    None => continue,
                }
            } else {
                match remove_path.as_ref() {
                    Some(path) => add_to_path(path, false, true, 1),
                    None => continue,
                }
            };

            new_pos = extract_common(&mut base_path, &new_tokens, &old_tokens, diagonal_path);

            if base_path.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return Some(build_values(
                    base_path.last_component,
                    &new_tokens,
                    &old_tokens,
                ));
            }

            best_path.insert(diagonal_path, base_path.clone());
            if base_path.old_pos + 1 >= old_len {
                max_diagonal_to_consider = Some(
                    max_diagonal_to_consider
                        .map_or(diagonal_path - 1, |max| max.min(diagonal_path - 1)),
                );
            }
            if new_pos + 1 >= new_len {
                min_diagonal_to_consider = Some(
                    min_diagonal_to_consider
                        .map_or(diagonal_path + 1, |min| min.max(diagonal_path + 1)),
                );
            }
        }

        edit_length += 1;
    }

    None
}

fn add_to_path(path: &Path, added: bool, removed: bool, old_pos_inc: isize) -> Path {
    let last = path.last_component.as_ref();
    let last_component = match last {
        Some(last) if last.added == added && last.removed == removed => Component {
            count: last.count + 1,
            added,
            removed,
            previous: last.previous.clone(),
        },
        _ => Component {
            count: 1,
            added,
            removed,
            previous: last.cloned(),
        },
    };

    Path {
        old_pos: path.old_pos + old_pos_inc,
        last_component: Some(Rc::new(last_component)),
    }
}

fn extract_common(
    base_path: &mut Path,
    new_tokens: &[String],
    old_tokens: &[String],
    diagonal_path: isize,
) -> isize {
    let new_len = new_tokens.len() as isize;
    let old_len = old_tokens.len() as isize;
    let mut old_pos = base_path.old_pos;
    let mut new_pos = old_pos - diagonal_path;
    let mut common_count = 0;

    while new_pos + 1 < new_len
        && old_pos + 1 < old_len
        && old_tokens[(old_pos + 1) as usize] == new_tokens[(new_pos + 1) as usize]
    {
        new_pos += 1;
        old_pos += 1;
        common_count += 1;
    }

    if common_count > 0 {
        base_path.last_component = Some(Rc::new(Component {
            count: common_count,
            previous: base_path.last_component.clone(),
            added: false,
            removed: false,
        }));
    }

    base_path.old_pos = old_pos;
    new_pos
}

fn build_values(
    last_component: Option<Rc<Component>>,
    new_tokens: &[String],
    old_tokens: &[String],
) -> Vec<Change> {
    let mut components = Vec::new();
    let mut next_component = last_component;
    while let Some(component) = next_component {
        next_component = component.previous.clone();
        components.push(component);
    }
    components.reverse();

    let mut new_pos = 0;
    let mut old_pos = 0;
    let mut changes = Vec::with_capacity(components.len());
    for component in components {
        let value = if !component.removed {
            let value = new_tokens[new_pos..new_pos + component.count].concat();
            new_pos += component.count;
            if !component.added {
                old_pos += component.count;
            }
            value
        } else {
            let value = old_tokens[old_pos..old_pos + component.count].concat();
            old_pos += component.count;
            value
        };
        changes.push(Change {
            value,
            added: component.added,
            removed: component.removed,
            count: component.count,
        });
    }
    changes
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}
