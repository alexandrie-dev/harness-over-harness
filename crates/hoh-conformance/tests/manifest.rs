//! `manifest.spec.ts` (8 `it` + the 40-row `it.each` of invalid metadata) against `manifest::parse`.
use hoh_conformance::manifest::{WriteMode, parse, writes_current_session_fixtures};
use serde_json::{Value, json};

fn ok(source: &str) -> Value {
    parse(source, "snapshot.yml").unwrap()
}

#[test]
fn parses_an_owning_scenario() {
    assert_eq!(
        ok("version: 1\nprofile: headless\n"),
        json!({"version":1,"profile":"headless"})
    );
}

#[test]
fn accepts_the_standalone_sdk_minimal_profile_and_web() {
    assert_eq!(
        ok("version: 1\nprofile: sdk-minimal\n"),
        json!({"version":1,"profile":"sdk-minimal"})
    );
    // `web` is a profile of the source (`PROFILES`), and `scenario` is optional there.
    assert_eq!(
        ok("version: 1\nprofile: web\n"),
        json!({"version":1,"profile":"web"})
    );
}

#[test]
fn parses_an_explicitly_retained_session_generation() {
    assert_eq!(
        ok(
            "version: 1\nprofile: headless\nsessionFormat:\n  version: 0\n  coverage: [multi-hop, packed-row]\n"
        ),
        json!({"version":1,"profile":"headless","sessionFormat":{"version":0,"coverage":["multi-hop","packed-row"]}})
    );
}

#[test]
fn keeps_retained_generations_read_only_while_current_fixtures_track_writer_output() {
    let current = ok("version: 1\nprofile: headless\n");
    let borrower = ok("version: 1\nprofile: web\nsession:\n  source: ../owner/session.jsonl\n");
    let retained = ok(
        "version: 1\nprofile: headless\nsessionFormat:\n  version: 0\n  coverage: [multi-hop]\n",
    );
    use WriteMode::{Record, Refresh, Replay};
    assert!(!writes_current_session_fixtures(&current, Replay));
    assert!(writes_current_session_fixtures(&current, Record));
    assert!(writes_current_session_fixtures(&current, Refresh));
    for other in [&borrower, &retained] {
        assert!(!writes_current_session_fixtures(other, Record));
        assert!(!writes_current_session_fixtures(other, Refresh));
    }
}

#[test]
fn trims_like_js_so_a_bom_is_not_blank_but_nel_is_not_whitespace() {
    // JS `String.prototype.trim` strips U+FEFF but not U+0085 (Rust `str::trim` is the opposite).
    for source in [
        "version: 1\nprofile: acp\nsession: {source: \"\u{feff}\"}\n",
        "version: 1\nprofile: acp\ninput: {task: \"\u{feff}\"}\n",
    ] {
        assert!(parse(source, "snapshot.yml").is_err(), "{source:?}");
    }
    let nel_task = "version: 1\nprofile: acp\ninput: {task: \"\u{85}\"}\n";
    assert!(parse(nel_task, "snapshot.yml").is_ok());
    let nel_source = "version: 1\nprofile: acp\nsession: {source: \"\u{85}\"}\n";
    assert!(parse(nel_source, "snapshot.yml").is_ok());
}

#[test]
fn parses_a_read_only_session_reference() {
    assert_eq!(
        ok(
            "version: 1\nprofile: web\nsession:\n  source: ../../session/tool-call-turn/session.jsonl\n"
        ),
        json!({"version":1,"profile":"web","session":{"source":"../../session/tool-call-turn/session.jsonl"}})
    );
}

#[test]
fn parses_composition_recording_header_and_replay_metadata() {
    let source = "version: 1\nscenario: sdk-case\nprofile: sdk\ncomposition: continuable-subagent\nrecording: authored\nheader:\n  class: continuable-subagent\n  pin: true\n  systemPromptSource: session/text-turn\n  toolSchemasSource: session/text-turn\n  childSystemPrompts: [1]\n  childToolSchemas: [1, 2]\n  changes: 1\n  promptChanges: 2\nreplay:\n  override: true\nplatform: posix\npermission: workspace-write\nenvironment:\n  DSH_SNAPSHOT_FAILURE: enabled\nworkspace:\n  setup: fixed-mtimes\n  final: true\n  parent: outside-temp\ninput:\n  task: Rejected before persistence.\n  attachments:\n    - id: sha256:abc\n      mediaType: image/png\n      data: aGVsbG8=\n";
    let parsed = ok(source);
    assert_eq!(parsed["header"]["childToolSchemas"], json!([1, 2]));
    assert_eq!(
        parsed["workspace"],
        json!({"setup":"fixed-mtimes","final":true,"parent":"outside-temp"})
    );
    assert_eq!(
        parsed["input"]["attachments"],
        json!([{"id":"sha256:abc","mediaType":"image/png","data":"aGVsbG8="}])
    );
}

#[test]
fn parses_independently_optional_header_and_input_fields() {
    assert_eq!(
        ok("version: 1\nprofile: headless\nheader:\n  class: default\ninput:\n  task: Run once.\n"),
        json!({"version":1,"profile":"headless","header":{"class":"default"},"input":{"task":"Run once."}})
    );
    assert_eq!(
        ok(
            "version: 1\nprofile: sdk\ninput:\n  attachments:\n    - id: sha256:one\n      mediaType: image/png\n      data: AQ==\n"
        )["input"],
        json!({"attachments":[{"id":"sha256:one","mediaType":"image/png","data":"AQ=="}]})
    );
}

#[test]
fn rejects_invalid_metadata() {
    let cases: &[(&str, &str)] = &[
        ("", "manifest must be a mapping"),
        (
            "version: 2\nprofile: acp\n",
            "manifest.version must equal 1",
        ),
        (
            "version: 1\nprofile: private\n",
            "manifest.profile must be headless, sdk, sdk-minimal, acp, or web",
        ),
        (
            "version: 1\nprofile: acp\nextra: true\n",
            "manifest has unknown field(s): extra",
        ),
        (
            "version: 1\nprofile: acp\ncomposition: Not_Safe\n",
            "manifest.composition must be a lower-kebab-case name",
        ),
        (
            "version: 1\nprofile: acp\nrecording: maybe\n",
            "manifest.recording must be live or authored",
        ),
        (
            "version: 1\nprofile: acp\nheader: {}\n",
            "manifest.header.class must be a lower-kebab-case name",
        ),
        (
            "version: 1\nprofile: acp\nheader:\n  class: base\n  pin: false\n",
            "manifest.header.pin must equal true when present",
        ),
        (
            "version: 1\nprofile: acp\nheader:\n  class: base\n  childToolSchemas: [1, 1]\n",
            "manifest.header.childToolSchemas must be an array of unique positive integers",
        ),
        (
            "version: 1\nprofile: acp\nheader:\n  class: base\n  changes: -1\n",
            "manifest.header.changes must be a non-negative integer",
        ),
        (
            "version: 1\nprofile: acp\nheader:\n  class: base\n  promptChanges: 1.5\n",
            "manifest.header.promptChanges must be a non-negative integer",
        ),
        (
            "version: 1\nprofile: acp\nheader:\n  class: base\n  systemPromptSource: ../bad\n",
            "manifest.header.systemPromptSource must be a lower-kebab-case name or corpus-relative path",
        ),
        (
            "version: 1\nprofile: acp\nreplay:\n  override: false\n",
            "manifest.replay.override must equal true",
        ),
        (
            "version: 1\nprofile: acp\nplatform: windows\n",
            "manifest.platform must be posix or pwsh",
        ),
        (
            "version: 1\nprofile: acp\npermission: root\n",
            "manifest.permission must be read-only, workspace-write, or danger-full-access",
        ),
        (
            "version: 1\nprofile: acp\npermission: admin\n",
            "manifest.permission must be read-only, workspace-write, or danger-full-access",
        ),
        (
            "version: 1\nprofile: acp\nenvironment:\n  lower: value\n",
            "manifest.environment must map uppercase environment names to strings",
        ),
        (
            "version: 1\nprofile: acp\nworkspace: {}\n",
            "manifest.workspace must not be empty",
        ),
        (
            "version: 1\nprofile: acp\nworkspace:\n  final: false\n",
            "manifest.workspace.final must equal true when present",
        ),
        (
            "version: 1\nprofile: acp\nworkspace:\n  parent: temp\n",
            "manifest.workspace.parent must equal outside-temp",
        ),
        (
            "version: 1\nprofile: acp\ninput:\n  task: \"\"\n",
            "manifest.input.task must be a non-empty string when present",
        ),
        (
            "version: 1\nprofile: acp\ninput: {}\n",
            "manifest.input must declare task or attachments",
        ),
        (
            "version: 1\nprofile: acp\ninput:\n  attachments: []\n",
            "manifest.input.attachments must be a non-empty array",
        ),
        (
            "version: 1\nprofile: acp\ninput:\n  attachments:\n    - id: raw\n      mediaType: image/png\n      data: AQ==\n",
            "manifest.input.attachments[0].id must start with sha256:",
        ),
        (
            "version: 1\nprofile: acp\ninput:\n  attachments:\n    - id: sha256:one\n      mediaType: image\n      data: AQ==\n",
            "manifest.input.attachments[0].mediaType must be a MIME type",
        ),
        (
            "version: 1\nprofile: acp\ninput:\n  attachments:\n    - id: sha256:one\n      mediaType: image/png\n      data: \"\"\n",
            "manifest.input.attachments[0].data must be non-empty base64",
        ),
        (
            "version: 1\nprofile: acp\ninput:\n  attachments:\n    - id: sha256:one\n      mediaType: image/png\n      data: AQ==\n    - id: sha256:one\n      mediaType: image/png\n      data: Ag==\n",
            "manifest.input.attachments must have unique ids",
        ),
        (
            "version: 1\nprofile: acp\nsessionFormat:\n  version: -1\n  coverage: [multi-hop]\n",
            "manifest.sessionFormat.version must be a non-negative safe integer",
        ),
        (
            "version: 1\nprofile: acp\nsessionFormat:\n  version: 0\n  coverage: []\n",
            "manifest.sessionFormat.coverage must be a non-empty array of unique supported coverage names",
        ),
        (
            "version: 1\nprofile: acp\nsessionFormat:\n  version: 0\n  coverage: [multi-hop, multi-hop]\n",
            "manifest.sessionFormat.coverage must be a non-empty array of unique supported coverage names",
        ),
        (
            "version: 1\nprofile: acp\nsessionFormat:\n  version: 0\n  coverage: [unknown]\n",
            "manifest.sessionFormat.coverage must be a non-empty array of unique supported coverage names",
        ),
        (
            "version: 1\nprofile: acp\nsession: {}\n",
            "manifest.session.source must be a non-empty string",
        ),
        (
            "version: 1\nprofile: acp\nsession:\n  source: /tmp/session.jsonl\n",
            "manifest.session.source must be a relative POSIX path",
        ),
        (
            "version: 1\nprofile: acp\nsession:\n  source: ..\\session.jsonl\n",
            "manifest.session.source must be a relative POSIX path",
        ),
        (
            "version: 1\nprofile: acp\nsession:\n  source: ../session.jsonl\nsessionFormat:\n  version: 0\n  coverage: [multi-hop]\n",
            "manifest.sessionFormat is only valid when the scenario owns its Session fixtures",
        ),
        ("version: 1\nprofile: !!js acp\n", "invalid YAML"),
    ];
    for (source, message) in cases {
        let error = parse(source, "case/snapshot.yml").unwrap_err();
        assert!(
            error.starts_with("session-snapshot: case/snapshot.yml: "),
            "{error}"
        );
        assert!(
            error.contains(message),
            "{source:?}: expected {message:?}, got {error:?}"
        );
    }
}

/// Tail: js-yaml reads `-0` as the number -0, which `Object.is` rejects as a generation.
#[test]
fn a_negative_zero_session_format_version_is_rejected() {
    let error = parse(
        "version: 1\nprofile: acp\nsessionFormat:\n  version: -0\n  coverage: [multi-hop]\n",
        "snapshot.yml",
    )
    .unwrap_err();
    assert!(
        error.ends_with("manifest.sessionFormat.version must be a non-negative safe integer"),
        "{error}"
    );
}
