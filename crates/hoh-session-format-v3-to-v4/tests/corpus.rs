mod common;
#[path = "common/corpus_contract.rs"]
mod contract;
#[test]
fn refuses_recorded_parent_child_clock_conflicts_and_migrates_consistent_copies_including_failed_startup()
 {
    contract::native_contract();
}
