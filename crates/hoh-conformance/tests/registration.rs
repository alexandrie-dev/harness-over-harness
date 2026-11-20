//! `suite.spec.ts` 269–490: the registration contract and the shared snapshot content checks.
use hoh_conformance::registration::*;
use std::collections::HashMap;

fn scenario(name: &str) -> Declaration {
    Declaration {
        name: name.into(),
        ..Default::default()
    }
}
fn pin(name: &str) -> Declaration {
    Declaration {
        pins_header: true,
        ..scenario(name)
    }
}
fn class(mut d: Declaration, class: &str) -> Declaration {
    d.header_class = Some(class.into());
    d
}
fn rejected(scenarios: &[Declaration], needle: &str) {
    let error = validate_registration(scenarios).unwrap_err();
    assert!(error.contains(needle), "{needle}: {error}");
}

/// :270
#[test]
fn throws_when_a_scenarios_header_class_has_no_pinning_scenario() {
    rejected(
        &[scenario("pinless")],
        "no scenario pins the request-header content of class \"default\"",
    );
    // A pinned class does not cover a different class's members.
    rejected(
        &[pin("pinned"), class(scenario("classless-orphan"), "other")],
        "class \"other\" (needed by classless-orphan)",
    );
}

/// :293
#[test]
fn throws_when_two_scenarios_pin_the_same_header_class() {
    rejected(
        &[pin("first-pin"), pin("second-pin")],
        "header class \"default\" pinned by both first-pin and second-pin",
    );
}

/// :307
#[test]
fn throws_when_scenario_names_are_duplicated() {
    rejected(
        &[pin("duplicate"), scenario("duplicate")],
        "duplicate scenario name \"duplicate\"",
    );
}

/// :321 — four rows: a source or count declared away from a header pin.
#[test]
fn rejects_sources_and_counts_away_from_a_header_pin() {
    type Setter = fn(&mut Declaration);
    let rows: [(&str, Setter); 4] = [
        ("systemPromptSource", |d| {
            d.system_prompt_source = Some("owner".into())
        }),
        ("toolSchemasSource", |d| {
            d.tool_schemas_source = Some("owner".into())
        }),
        ("expectedHeaderChanges", |d| {
            d.expected_header_changes = Some(1)
        }),
        ("expectedPromptChanges", |d| {
            d.expected_prompt_changes = Some(1)
        }),
    ];
    for (field, set) in rows {
        let mut plain = scenario("plain");
        set(&mut plain);
        rejected(
            &[plain],
            &format!("plain.{field} is only valid on a header-pinning scenario"),
        );
    }
}

/// :345
#[test]
fn rejects_an_unknown_or_non_pinning_sidecar_source() {
    let mut owner = pin("pin");
    owner.system_prompt_source = Some("missing".into());
    rejected(
        &[owner],
        "pin names unknown system-prompt snapshot source \"missing\"",
    );
    let mut pinning = pin("pin");
    pinning.tool_schemas_source = Some("plain".into());
    rejected(
        &[pinning, scenario("plain")],
        "pin names non-pinning tool-schema snapshot source \"plain\"",
    );
}

/// :380
#[test]
fn rejects_a_sidecar_source_that_redirects_the_same_artifact() {
    let mut redirect = class(pin("redirect"), "redirect");
    redirect.system_prompt_source = Some("owner".into());
    let mut consumer = class(pin("consumer"), "consumer");
    consumer.system_prompt_source = Some("redirect".into());
    rejected(
        &[pin("owner"), redirect, consumer],
        "consumer names system-prompt snapshot source \"redirect\", which does not own its sidecar",
    );
}

/// :409
#[test]
fn rejects_shared_sidecars_with_different_header_change_counts() {
    let mut owner = pin("owner");
    owner.expected_header_changes = Some(1);
    let mut consumer = class(pin("consumer"), "consumer");
    consumer.tool_schemas_source = Some("owner".into());
    rejected(
        &[owner, consumer],
        "consumer and owner declare different expectedHeaderChanges counts for shared tool-schema snapshot",
    );
}

/// :436
#[test]
fn rejects_shared_prompt_sidecars_with_different_prompt_change_counts() {
    let mut owner = pin("owner");
    owner.expected_prompt_changes = Some(1);
    let mut consumer = class(pin("consumer"), "consumer");
    consumer.system_prompt_source = Some("owner".into());
    rejected(
        &[owner, consumer],
        "consumer and owner declare different expectedPromptChanges counts for shared system-prompt snapshot",
    );
}

/// A valid table registers (the positive case of the contract).
#[test]
fn a_consistent_table_registers() {
    let mut shared = class(pin("shared-pin"), "shared");
    shared.system_prompt_source = Some("pin-turn".into());
    shared.tool_schemas_source = Some("pin-turn".into());
    shared.expected_header_changes = Some(1);
    shared.expected_prompt_changes = Some(1);
    let mut main = pin("pin-turn");
    main.header_class = Some("main".into());
    main.expected_header_changes = Some(1);
    main.expected_prompt_changes = Some(1);
    validate_registration(&[main, shared, class(scenario("plain-turn"), "main")]).unwrap();
}

/// :465
#[test]
fn accepts_identical_claims_and_rejects_order_dependent_shared_output() {
    let mut claims = HashMap::new();
    let path = "shared/system-prompt.expected.md";
    claim_shared_snapshot(&mut claims, path, "first", "prompt\n").unwrap();
    claim_shared_snapshot(&mut claims, path, "second", "prompt\n").unwrap();
    assert_eq!(
        claims[path],
        SharedSnapshotClaim {
            scenario: "first".into(),
            content: "prompt\n".into()
        }
    );
    let error = claim_shared_snapshot(&mut claims, path, "third", "different\n").unwrap_err();
    assert!(
        error.contains("diverged between first and third"),
        "{error}"
    );
}

/// :478
#[test]
fn rejects_identical_committed_content_under_different_paths() {
    assert_unique_snapshot_contents(
        "prompt",
        &[
            ("one/system-prompt.expected.md", "one\n"),
            ("two/system-prompt.expected.md", "two\n"),
        ],
    )
    .unwrap();
    let error = assert_unique_snapshot_contents(
        "prompt",
        &[
            ("one/system-prompt.expected.md", "same\n"),
            ("two/system-prompt.expected.md", "same\n"),
        ],
    )
    .unwrap_err();
    assert!(
        error.contains("identical prompt snapshots appear in one/system-prompt.expected.md and two/system-prompt.expected.md"),
        "{error}"
    );
}
