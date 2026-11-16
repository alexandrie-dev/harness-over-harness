use hoh_deque::Deque;
use std::rc::Rc;

#[test]
fn removes_tail_appended_entries_in_fifo_order() {
    let mut q = Deque::new();
    assert_eq!(q.size(), 0);
    assert_eq!(q.pop_front(), None);
    q.push_back(1);
    q.push_back(2);
    assert_eq!(q.size(), 2);
    assert_eq!(q.pop_front(), Some(1));
    assert_eq!(q.pop_front(), Some(2));
    assert_eq!(q.size(), 0);
}
#[test]
fn prepends_entries_before_existing_head() {
    let mut q = Deque::new();
    q.push_back(3);
    q.push_front(2);
    q.push_front(1);
    assert_eq!(
        [q.pop_front(), q.pop_front(), q.pop_front()],
        [Some(1), Some(2), Some(3)]
    );
}
#[test]
fn appends_through_array_boundary_without_growing() {
    let mut q = Deque::new();
    for n in 0..8 {
        q.push_back(n);
    }
    for n in 0..6 {
        assert_eq!(q.pop_front(), Some(n));
    }
    for n in 8..=16 {
        q.push_back(n);
    }
    assert_eq!(q.capacity(), 16);
    for n in 6..=16 {
        assert_eq!(q.pop_front(), Some(n));
    }
}
#[test]
fn preserves_order_across_wrapping_growth_and_sparse_compaction() {
    let mut q = Deque::new();
    for n in 0..32 {
        q.push_back(n);
    }
    for n in 0..24 {
        assert_eq!(q.pop_front(), Some(n));
    }
    assert_eq!(q.capacity(), 16);
    for n in 32..128 {
        q.push_back(n);
    }
    for n in 24..128 {
        assert_eq!(q.pop_front(), Some(n));
    }
    assert_eq!(q.size(), 0);
    assert_eq!(q.capacity(), 16);
}
#[test]
fn releases_removed_reference_before_sparse_compaction() {
    let mut q = Deque::new();
    let removed = Rc::new(());
    q.push_back(removed.clone());
    q.push_back(Rc::new(()));
    let value = q.pop_front().unwrap();
    assert!(Rc::ptr_eq(&value, &removed));
    assert_eq!(Rc::strong_count(&removed), 2);
    assert_eq!(q.capacity(), 16);
}
#[test]
fn drops_retained_storage_and_remains_reusable_after_clear() {
    let mut q = Deque::new();
    let retained = Rc::new(());
    q.push_back(retained.clone());
    for _ in 1..64 {
        q.push_back(Rc::new(()));
    }
    assert_eq!(q.capacity(), 64);
    q.clear();
    assert_eq!(q.size(), 0);
    assert_eq!(q.pop_front(), None);
    assert_eq!(Rc::strong_count(&retained), 1);
    assert_eq!(q.capacity(), 16);
    let value = Rc::new(());
    q.push_back(value.clone());
    assert!(Rc::ptr_eq(&q.pop_front().unwrap(), &value));
}
#[test]
fn uses_size_to_distinguish_undefined_entry_from_empty_deque() {
    let mut q = Deque::new();
    q.push_back(None::<()>);
    assert_eq!(q.size(), 1);
    assert_eq!(q.pop_front(), Some(None));
    assert_eq!(q.size(), 0);
}
