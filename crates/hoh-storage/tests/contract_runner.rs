#[path = "contract.rs"]
mod contract;
#[path = "support/contract_backend.rs"]
mod fixture;

async fn create_harness() -> contract::KvBackendContractHarness {
    fixture::harness(fixture::RejectionFault::None)
}

contract::run_kv_backend_contract!(contract, create_harness);

#[tokio::test]
async fn commit_preamble_detects_restamping_by_a_rejected_write() {
    for operation in ["putRecord", "deleteRecord", "deleteRecords", "setGlobal"] {
        let harness = fixture::harness(fixture::RejectionFault::Restamp);
        let result = tokio::spawn(contract::commits_the_current_stamp_with_its_data(
            harness, operation,
        ))
        .await;
        assert!(
            result.unwrap_err().is_panic(),
            "the {operation} scenario must detect restamping"
        );
    }
}

#[tokio::test]
async fn snapshot_retry_scenario_detects_a_rejected_write_changing_readable_state() {
    let harness = fixture::harness(fixture::RejectionFault::ChangeSnapshot);
    let result = tokio::spawn(contract::restores_the_open_snapshot_after_serialization_failure_and_retries_on_the_same_unit(harness)).await;
    assert!(result.unwrap_err().is_panic());
}
