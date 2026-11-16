use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

use crate::{Change, PatchOptions};

#[derive(Debug)]
struct Component {
    added: bool,
    removed: bool,
    count: usize,
    previous: Option<Rc<Component>>,
}

impl Drop for Component {
    fn drop(&mut self) {
        let mut previous = self.previous.take();
        while let Some(component) = previous {
            match Rc::try_unwrap(component) {
                Ok(mut component) => previous = component.previous.take(),
                // Another path still owns this prefix; its last owner will release it.
                Err(_) => break,
            }
        }
    }
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
    let started_at = Instant::now();

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

    while edit_length <= max_edit_length
        && options
            .timeout_ms
            .is_none_or(|timeout| started_at.elapsed().as_millis() <= u128::from(timeout))
    {
        let first_diagonal =
            min_diagonal_to_consider.map_or(-edit_length, |min: isize| min.max(-edit_length));
        for diagonal_path in (first_diagonal..).step_by(2) {
            if diagonal_path
                > max_diagonal_to_consider.map_or(edit_length, |max: isize| max.min(edit_length))
            {
                break;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Follow the known winning path for these inputs without the quadratic diagonal search.
    fn alternating_insertion_path(n: usize) -> (Path, Vec<String>, Vec<String>) {
        let old_tokens: Vec<_> = "a\n"
            .repeat(n)
            .split_inclusive('\n')
            .map(str::to_owned)
            .collect();
        let new_tokens: Vec<_> = "a\nb\n"
            .repeat(n)
            .split_inclusive('\n')
            .map(str::to_owned)
            .collect();
        let mut path = Path {
            old_pos: -1,
            last_component: None,
        };
        extract_common(&mut path, &new_tokens, &old_tokens, 0);
        for edit_length in 1..=n {
            path = add_to_path(&path, true, false, 0);
            extract_common(&mut path, &new_tokens, &old_tokens, -(edit_length as isize));
        }
        (path, old_tokens, new_tokens)
    }

    #[test]
    fn winning_chain_of_four_hundred_thousand_components_drops_on_small_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let n = 200_000;
                let (path, old_tokens, new_tokens) = alternating_insertion_path(n);
                let first = {
                    let mut component = path.last_component.as_ref().unwrap();
                    while let Some(previous) = &component.previous {
                        component = previous;
                    }
                    Rc::downgrade(component)
                };
                let changes = build_values(path.last_component, &new_tokens, &old_tokens);
                assert_eq!(changes.len(), 2 * n);
                for pair in changes.chunks_exact(2) {
                    assert_eq!(
                        pair[0],
                        Change {
                            value: "a\n".to_owned(),
                            added: false,
                            removed: false,
                            count: 1,
                        }
                    );
                    assert_eq!(
                        pair[1],
                        Change {
                            value: "b\n".to_owned(),
                            added: true,
                            removed: false,
                            count: 1,
                        }
                    );
                }
                assert!(first.upgrade().is_none());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn abandoned_shared_chains_drop_on_small_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let (path, _, _) = alternating_insertion_path(200_000);
                let last = Rc::downgrade(path.last_component.as_ref().unwrap());
                let mut best_path =
                    BTreeMap::from([(0, path.clone()), (1, add_to_path(&path, false, true, 1))]);
                drop(path);
                best_path.remove(&0);
                assert!(last.upgrade().is_some());
                drop(best_path);
                assert!(last.upgrade().is_none());
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
