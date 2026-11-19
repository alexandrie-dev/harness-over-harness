//! Port of credentials-local/tests/local.spec.ts.
mod support;

use hoh_credentials::{CredentialInfo, ResolvedCredential};
use hoh_credentials_local::{Config, resolve_spec};
use hoh_launch_environment::LaunchEnvironmentSource::{Process, ProjectEnv, UserEnv};
use serde_json::json;
use support::*;

fn dots(count: usize) -> String {
    "•".repeat(count)
}

fn file(value: &str) -> Option<ResolvedCredential> {
    Some(ResolvedCredential {
        value: value.into(),
        source: "file".into(),
    })
}

fn masked_file(masked: String) -> CredentialInfo {
    CredentialInfo {
        configured: true,
        source: Some("file".into()),
        writable: true,
        masked: Some(masked),
    }
}

fn plain_info(source: &str) -> CredentialInfo {
    CredentialInfo {
        configured: true,
        source: Some(source.into()),
        writable: true,
        masked: None,
    }
}

fn cwd_resolve(path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(hoh_util_workspace_path::node_path::resolve(
        &std::env::current_dir().unwrap().to_string_lossy(),
        &[path],
    ))
}

#[test]
fn resolve_spec_defaults_to_credentials_yaml_under_the_harness_home_with_watching_on() {
    let spec = resolve_spec(&Config {
        dsh_home: Some("/custom/home".into()),
        ..Config::default()
    })
    .unwrap();
    assert_eq!(spec.filename, cwd_resolve("/custom/home/.credentials.yaml"));
    assert!(spec.watch);
    assert_eq!(spec.debounce_ms, 100.0);
}

#[test]
fn resolve_spec_lets_an_explicit_path_win_over_the_home() {
    let spec = resolve_spec(&Config {
        path: Some("/etc/dsh/creds.yaml".into()),
        dsh_home: Some("/ignored".into()),
        watch: Some(false),
        debounce_ms: Some(5.0),
    })
    .unwrap();
    assert_eq!(spec.filename, cwd_resolve("/etc/dsh/creds.yaml"));
    assert!(!spec.watch);
    assert_eq!(spec.debounce_ms, 5.0);
}

async fn describes_only_the_masked_preview(value: String, masked: String) {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    credentials.set(&key, &value).await.unwrap();
    let info = credentials.describe(&key).await.unwrap();
    assert_eq!(info, masked_file(masked.clone()));
    assert_eq!(masked.chars().count(), value.chars().count());
    assert!(!serde_json::to_string(&info).unwrap().contains(&value));
    assert_eq!(credentials.resolve(&key).await.unwrap(), file(&value));
    booted.shutdown().await;
}

macro_rules! mask_case {
    ($name:ident, $value:expr, $masked:expr) => {
        #[tokio::test]
        async fn $name() {
            describes_only_the_masked_preview($value, $masked).await;
        }
    };
}

mask_case!(
    describes_only_the_masked_preview_for_plain,
    "plain".into(),
    dots(5)
);
mask_case!(
    describes_only_the_masked_preview_for_sixteen_letters,
    "abcdefghijklmnop".into(),
    dots(16)
);
mask_case!(
    describes_only_the_masked_preview_for_seventeen_letters,
    "abcdefghijklmnopq".into(),
    format!("abcde{}nopq", dots(8))
);
mask_case!(
    describes_only_the_masked_preview_for_sk_prefix_hiding_fewer_than_eight,
    "sk-abcdefghijklmn".into(),
    dots(17)
);
mask_case!(
    describes_only_the_masked_preview_for_sk_prefix_hiding_exactly_the_edge,
    "sk-abcdefghijklmnop".into(),
    dots(19)
);
mask_case!(
    describes_only_the_masked_preview_for_sk_prefix_past_the_edge,
    "sk-abcdefghijklmnopq".into(),
    format!("sk-abcde{}nopq", dots(8))
);
mask_case!(
    describes_only_the_masked_preview_for_an_underscore_prefix,
    "vendor_abcdefghijklmnopq".into(),
    format!("vendor_abcde{}nopq", dots(8))
);
mask_case!(
    describes_only_the_masked_preview_for_a_double_hyphen_prefix,
    "sk-proj-abcdefghijklmnopq".into(),
    format!("sk-proj-{}nopq", dots(13))
);
mask_case!(
    describes_only_the_masked_preview_for_a_mixed_prefix,
    "abc-def_abcdefghijklmnopq".into(),
    format!("abc-def_a{}nopq", dots(12))
);
mask_case!(
    describes_only_the_masked_preview_for_a_trailing_hyphen,
    "abcdefghijklmnop-".into(),
    dots(17)
);
mask_case!(
    describes_only_the_masked_preview_for_sixteen_emoji,
    "😀".repeat(16),
    dots(16)
);
mask_case!(
    describes_only_the_masked_preview_for_seventeen_emoji,
    "😀".repeat(17),
    format!("{}{}{}", "😀".repeat(5), dots(8), "😀".repeat(4))
);
mask_case!(
    describes_only_the_masked_preview_for_an_emoji_after_a_prefix,
    format!("sk-{}", "😀".repeat(17)),
    format!("sk-{}{}{}", "😀".repeat(5), dots(8), "😀".repeat(4))
);
mask_case!(
    describes_only_the_masked_preview_for_an_emoji_identifier,
    "😀_abcdefghijklmno😀😀".into(),
    format!("😀_abcde{}no😀😀", dots(8))
);

#[tokio::test]
async fn treats_an_absent_file_as_an_empty_writable_store() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    assert_eq!(credentials.resolve(&key).await.unwrap(), None);
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        CredentialInfo {
            configured: false,
            source: None,
            writable: true,
            masked: None
        }
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn serves_file_entries_alongside_comments_and_quoted_values() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(
        &path,
        "version: 1\nrefs:\n  # notes\n  DSH_CRED_TEST: plain\n  DSH_CRED_OTHER: \"with space\"\n",
    );
    let booted = boot(config(&path)).await;
    let credentials = booted.credentials();
    assert_eq!(
        credentials
            .resolve(&reference("DSH_CRED_TEST"))
            .await
            .unwrap(),
        file("plain")
    );
    assert_eq!(
        credentials
            .resolve(&reference("DSH_CRED_OTHER"))
            .await
            .unwrap(),
        file("with space")
    );
    assert_eq!(
        credentials
            .describe(&reference("DSH_CRED_TEST"))
            .await
            .unwrap(),
        masked_file(dots(5))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn lets_the_managed_file_win_over_a_non_empty_process_environment() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: from-file\n");
    let booted = boot_layered(
        config(&path),
        &[layer(Process, None, &[("DSH_CRED_TEST", "from-env")])],
    )
    .await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("from-file"));
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        masked_file(dots(9))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn treats_an_empty_environment_value_as_absent_falling_through_to_the_file() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: stored\n");
    let booted = boot_layered(
        config(&path),
        &[layer(Process, None, &[("DSH_CRED_TEST", "")])],
    )
    .await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("stored"));
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        masked_file(dots(6))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn fails_boot_loud_when_the_document_exists_but_cannot_be_read() {
    let dir = temp_dir();
    let path = dir.path().join("occupied");
    std::fs::create_dir(&path).unwrap();
    assert!(try_boot(None, config(&path)).await.is_err());
}

// .credentials.yaml > inherited process env > invoking directory .env > $DSH_HOME/.env.

#[tokio::test]
async fn lets_the_stored_value_beat_the_user_env_so_a_ui_write_takes_effect_immediately() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: stored\n");
    let booted = boot_layered(
        config(&path),
        &[
            layer(Process, None, &[]),
            layer(
                UserEnv,
                Some("/home/.dsh/.env"),
                &[("DSH_CRED_TEST", "older-user-env")],
            ),
        ],
    )
    .await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("stored"));
    // A key sitting in the user's .env does not make the stored one unwritable.
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        masked_file(dots(6))
    );
    credentials.set(&key, "rotated").await.unwrap();
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("rotated"));
    booted.shutdown().await;
}

#[tokio::test]
async fn stores_over_a_launch_environment_value_and_reveals_it_again_after_unset() {
    let dir = temp_dir();
    let booted = boot_layered(
        config(&doc(&dir)),
        &[layer(Process, None, &[("DSH_CRED_TEST", "from-launch")])],
    )
    .await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    let env = Some(ResolvedCredential {
        value: "from-launch".into(),
        source: "env".into(),
    });
    assert_eq!(credentials.resolve(&key).await.unwrap(), env);
    assert_eq!(credentials.describe(&key).await.unwrap(), plain_info("env"));
    credentials.set(&key, "stored").await.unwrap();
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("stored"));
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        masked_file(dots(6))
    );
    credentials.unset(&key).await.unwrap();
    assert_eq!(credentials.resolve(&key).await.unwrap(), env);
    assert_eq!(credentials.describe(&key).await.unwrap(), plain_info("env"));
    booted.shutdown().await;
}

#[tokio::test]
async fn serves_the_user_env_only_when_nothing_is_stored() {
    let dir = temp_dir();
    let booted = boot_layered(
        config(&doc(&dir)),
        &[
            layer(Process, None, &[]),
            layer(
                UserEnv,
                Some("/home/.dsh/.env"),
                &[("DSH_CRED_TEST", "from-user-env")],
            ),
        ],
    )
    .await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    assert_eq!(
        credentials.resolve(&key).await.unwrap(),
        Some(ResolvedCredential {
            value: "from-user-env".into(),
            source: "user-env".into()
        })
    );
    // Writable: storing a key replaces it as the effective one.
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        plain_info("user-env")
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn serves_the_invoking_project_env_over_the_user_one_but_never_over_the_store() {
    let dir = temp_dir();
    let path = doc(&dir);
    // The more specific location wins over the user's home file, and a stored key must never lose.
    let layers = [
        layer(Process, None, &[]),
        layer(
            ProjectEnv,
            Some("/work/.env"),
            &[("DSH_CRED_TEST", "from-project")],
        ),
        layer(
            UserEnv,
            Some("/home/.dsh/.env"),
            &[("DSH_CRED_TEST", "from-user")],
        ),
    ];
    let key = reference("DSH_CRED_TEST");
    let bare = boot_layered(config(&path), &layers).await;
    assert_eq!(
        bare.credentials().resolve(&key).await.unwrap(),
        Some(ResolvedCredential {
            value: "from-project".into(),
            source: "project-env".into()
        })
    );
    assert_eq!(
        bare.credentials().describe(&key).await.unwrap(),
        plain_info("project-env")
    );
    bare.shutdown().await;

    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: stored\n");
    let stored = boot_layered(config(&path), &layers).await;
    assert_eq!(
        stored.credentials().resolve(&key).await.unwrap(),
        file("stored")
    );
    stored.shutdown().await;
}

#[tokio::test]
async fn refuses_a_document_other_os_users_can_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir();
    let path = doc(&dir);
    std::fs::write(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: leaked\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    // Before the contents are read at all: serving secrets out of a world-readable file would make
    // the 0600 the provider writes meaningless.
    let failure = try_boot(None, config(&path)).await.err().unwrap();
    assert!(
        failure.contains("readable beyond its owner (mode 644)"),
        "{failure}"
    );
    assert!(!failure.contains("leaked"));
}

#[tokio::test]
async fn propagates_a_permission_check_that_fails_for_a_reason_other_than_absence() {
    let dir = temp_dir();
    let not_a_directory = dir.path().join("occupied");
    std::fs::write(&not_a_directory, "a regular file\n").unwrap();
    // The parent is a file, so the check fails with ENOTDIR rather than concluding "no credentials
    // yet".
    let failure = try_boot(None, config(&not_a_directory.join(".credentials.yaml")))
        .await
        .err()
        .unwrap();
    assert!(failure.contains("ENOTDIR"), "{failure}");
}

#[tokio::test]
async fn propagates_a_read_that_fails_for_a_reason_other_than_absence() {
    use std::os::unix::fs::DirBuilderExt;
    let dir = temp_dir();
    let path = doc(&dir);
    // Owner-only, so the permission check passes, and unreadable as a file: the store is present
    // but cannot be parsed, which must fail the launch rather than silently serve nothing.
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let failure = try_boot(None, config(&path)).await.err().unwrap();
    assert!(failure.contains("EISDIR"), "{failure}");
}

#[tokio::test]
async fn keeps_the_managed_store_writable_when_the_inherited_environment_also_names_it() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: stored\n");
    let booted = boot_layered(
        config(&path),
        &[
            layer(Process, None, &[("DSH_CRED_TEST", "from-shell")]),
            layer(
                UserEnv,
                Some("/home/.dsh/.env"),
                &[("DSH_CRED_TEST", "from-user-env")],
            ),
        ],
    )
    .await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("stored"));
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        masked_file(dots(6))
    );
    credentials.set(&key, "next").await.unwrap();
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("next"));
    assert_eq!(
        credentials.describe(&key).await.unwrap(),
        masked_file(dots(4))
    );
    booted.shutdown().await;
}

// Every rejection below is a boot failure rather than a skipped entry: inside `refs` and `records`,
// an ignored entry would read as "the secret I stored has no effect". Only a foreign top-level key
// is tolerated.
async fn fails_boot_on(text: &str, fragment: &str) {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, text);
    let failure = try_boot(None, config(&path))
        .await
        .err()
        .unwrap_or_else(|| panic!("boot unexpectedly succeeded for {text:?}"));
    assert!(
        failure.contains(fragment),
        "{failure:?} does not contain {fragment:?}"
    );
}

macro_rules! rejects {
    ($name:ident, $text:expr, $fragment:expr) => {
        #[tokio::test]
        async fn $name() {
            fails_boot_on($text, $fragment).await;
        }
    };
}

rejects!(
    fails_boot_on_a_non_mapping_root,
    "just a string\n",
    "must be a mapping"
);
rejects!(
    fails_boot_on_a_sequence_root,
    "- DSH_CRED_TEST\n",
    "must be a mapping"
);
// A flat document the boot migration cannot prove it understands is refused by name rather than
// read as an empty store or rewritten. (The recognized all-string flat layout upgrades in place
// instead — migration.rs owns that path.)
rejects!(
    fails_boot_on_the_flat_layout_with_a_non_string_value,
    "DSH_CRED_TEST: [nope]\n",
    "nest the existing 1 entry under"
);
rejects!(
    fails_boot_on_the_flat_layout_with_several_entries_and_a_non_string_value,
    "DSH_CRED_TEST: a\nDSH_CRED_OTHER: [b]\n",
    "nest the existing 2 entries under"
);
rejects!(
    fails_boot_on_the_flat_layout_with_an_empty_value,
    "DSH_CRED_TEST: \"\"\n",
    "pre-release flat layout"
);
rejects!(
    fails_boot_on_the_flat_layout_with_an_unaddressable_key,
    "not-a-ref: value\n",
    "pre-release flat layout"
);
rejects!(
    fails_boot_on_the_flat_layout_with_a_non_string_key,
    "1: a\n",
    "pre-release flat layout"
);
rejects!(
    fails_boot_on_the_flat_layout_under_document_directives,
    "%YAML 1.2\n---\nDSH_CRED_TEST: a\n",
    "pre-release flat layout"
);
rejects!(
    fails_boot_on_a_future_version,
    "version: 2\nrefs: {}\n",
    "this build reads version 1"
);
rejects!(
    fails_boot_on_a_non_mapping_refs_section,
    "version: 1\nrefs: nope\n",
    "must be a mapping"
);
rejects!(
    fails_boot_on_a_key_that_is_not_a_posix_identifier,
    "version: 1\nrefs:\n  not-a-ref: value\n",
    "credential ref"
);
rejects!(
    fails_boot_on_a_non_string_value,
    "version: 1\nrefs:\n  DSH_CRED_TEST: 123\n",
    "must be a string"
);
rejects!(
    fails_boot_on_an_empty_value,
    "version: 1\nrefs:\n  DSH_CRED_TEST: \"\"\n",
    "is empty"
);
rejects!(
    fails_boot_on_a_record_key_that_is_not_scoped,
    "version: 1\nrecords:\n  codex:\n    kind: grant\n    payload: 1\n",
    "must be \"<scope>/<id>\""
);
rejects!(
    fails_boot_on_a_record_that_is_not_a_mapping,
    "version: 1\nrecords:\n  llm-pi-ai/codex: token\n",
    "must be a mapping"
);
rejects!(
    fails_boot_on_a_record_with_no_kind,
    "version: 1\nrecords:\n  llm-pi-ai/codex:\n    payload: 1\n",
    "has no kind"
);
rejects!(
    fails_boot_on_a_record_with_an_unknown_kind,
    "version: 1\nrecords:\n  llm-pi-ai/codex:\n    kind: token\n",
    "unknown kind"
);
rejects!(
    fails_boot_on_a_record_with_an_unknown_field,
    "version: 1\nrecords:\n  llm-pi-ai/codex:\n    kind: grant\n    payload: 1\n    extra: 2\n",
    "unknown field \"extra\""
);
rejects!(
    fails_boot_on_a_grant_with_no_payload,
    "version: 1\nrecords:\n  llm-pi-ai/codex:\n    kind: grant\n",
    "has no payload"
);
// YAML spells values JSON has none for. The seam promises an owner its payload comes back exactly
// as written, which a lossy round trip breaks.
rejects!(
    fails_boot_on_a_non_finite_payload_number,
    "version: 1\nrecords:\n  llm-pi-ai/codex:\n    kind: grant\n    payload:\n      ratio: .inf\n",
    "non-finite number"
);
rejects!(
    fails_boot_on_a_cyclic_payload,
    "version: 1\nrecords:\n  llm-pi-ai/codex:\n    kind: grant\n    payload: &loop\n      self: *loop\n",
    "is cyclic"
);
rejects!(
    fails_boot_on_an_api_key_record_with_an_empty_key,
    "version: 1\nrecords:\n  llm-pi-ai/acme:\n    kind: api-key\n    key: \"\"\n",
    "non-string or empty key"
);
rejects!(
    fails_boot_on_an_api_key_record_env_that_is_not_a_mapping,
    "version: 1\nrecords:\n  llm-pi-ai/acme:\n    kind: api-key\n    env: nope\n",
    "non-mapping env"
);
rejects!(
    fails_boot_on_an_api_key_record_env_value_that_is_empty,
    "version: 1\nrecords:\n  llm-pi-ai/acme:\n    kind: api-key\n    env:\n      AWS_PROFILE: \"\"\n",
    "must be a non-empty string"
);
rejects!(
    fails_boot_on_duplicate_keys,
    "version: 1\nrefs:\n  DSH_CRED_TEST: one\n  DSH_CRED_TEST: two\n",
    "invalid document"
);
rejects!(
    fails_boot_on_malformed_yaml,
    "DSH_CRED_TEST: \"unterminated\n",
    "invalid document"
);

#[tokio::test]
async fn never_puts_a_credential_value_in_a_diagnostic() {
    let dir = temp_dir();
    let path = doc(&dir);
    let secret = "sk-live-DO-NOT-LOG-abcdef123456";
    // The yaml parser's own message quotes the offending source line, which in this document is the
    // secret itself.
    write_credentials(&path, &format!("DSH_CRED_TEST: \"{secret}\n"));
    let failure = try_boot(None, config(&path)).await.err().unwrap();
    assert!(failure.contains("invalid document"), "{failure}");
    // The position survives; the line's contents do not.
    assert!(failure.contains("line 2, column 1"), "{failure}");
    assert!(!failure.contains(secret), "{failure}");
}

// Deviation from the source, which prints the offending `version` / `kind` value: a mistyped
// field can hold a pasted secret, so the message names the field and never the value.
#[tokio::test]
async fn never_puts_an_unknown_version_or_kind_value_in_a_diagnostic() {
    let secret = "sk-founder-secret";
    let documents = [
        format!("version: {secret}\nrefs:\n  A: x\n"),
        format!("version: \"{secret}\"\n"),
        format!("version: 1\nrecords:\n  llm-pi-ai/codex:\n    kind: {secret}\n    payload: 1\n"),
    ];
    for text in &documents {
        let dir = temp_dir();
        let path = doc(&dir);
        write_credentials(&path, text);
        let failure = try_boot(None, config(&path)).await.err().unwrap();
        assert!(
            failure.contains("unsupported version") || failure.contains("unknown kind"),
            "{failure}"
        );
        assert!(!failure.contains(secret), "{failure}");
    }
}

#[test]
fn the_document_debug_rendering_never_prints_a_value() {
    let secret = "sk-founder-secret";
    let text = format!(
        "version: 1\nrefs:\n  A: {secret}\nrecords:\n  llm-pi-ai/codex:\n    kind: grant\n    payload:\n      token: {secret}\n  llm-pi-ai/azure:\n    kind: api-key\n    key: {secret}\nOTHER: {secret}\n"
    );
    let document = hoh_credentials_local::parse_credentials_document(&text, "memory").unwrap();
    let rendered = format!("{document:?} {document:#?}");
    assert!(!rendered.contains(secret), "{rendered}");
    assert!(rendered.contains("llm-pi-ai/codex"), "{rendered}");
}

#[tokio::test]
async fn writes_into_a_document_with_explicit_start_and_end_markers() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "---\nversion: 1\nrefs:\n  A: old\n...\n");
    let booted = boot(config(&path)).await;
    let credentials = booted.credentials();
    credentials.set(&reference("B"), "new").await.unwrap();
    assert_eq!(
        read(&path),
        "---\nversion: 1\nrefs:\n  A: old\n  B: new\n...\n"
    );
    credentials.unset(&reference("A")).await.unwrap();
    assert_eq!(read(&path), "---\nversion: 1\nrefs:\n  B: new\n...\n");
    credentials.unset(&reference("B")).await.unwrap();
    assert_eq!(read(&path), "---\nversion: 1\nrefs: {}\n...\n");
    credentials.set(&reference("C"), "again").await.unwrap();
    assert_eq!(read(&path), "---\nversion: 1\nrefs:\n  C: again\n...\n");
    let reread = boot(config(&path)).await;
    assert_eq!(
        resolved(&*reread.credentials(), "C").await,
        Some(("again".into(), "file".into()))
    );
    reread.shutdown().await;
    booted.shutdown().await;
}

#[tokio::test]
async fn refuses_to_rewrite_a_flow_document_that_would_lose_comments_tags_anchors_or_a_number() {
    for text in [
        "{version: 1, ACCOUNT_ID: 18446744073709551615, refs: {A: x}} # keep\n",
        "{version: 1, ACCOUNT_ID: 18446744073709551615, refs: {A: x}}\n",
        "{version: 1, refs: {A: x}} # keep\n",
        "{version: 1, OWNER: !custom x, refs: {A: x}}\n",
        "%YAML 1.2\n---\n{version: 1, refs: {A: x}}\n",
        "{version: 1, base: &b 1, copy: *b, refs: {A: x}}\n",
    ] {
        let dir = temp_dir();
        let path = doc(&dir);
        write_credentials(&path, text);
        let booted = boot(config(&path)).await;
        let credentials = booted.credentials();
        let failure = credentials.set(&reference("B"), "y").await.unwrap_err();
        assert!(
            failure.to_string().contains("nothing was written"),
            "{failure}"
        );
        assert_eq!(read(&path), text);
        assert_eq!(
            resolved(&*credentials, "A").await,
            Some(("x".into(), "file".into()))
        );
        booted.shutdown().await;
    }
}

#[tokio::test]
async fn keeps_exact_integers_of_fields_it_does_not_manage_in_a_block_document() {
    let dir = temp_dir();
    let path = doc(&dir);
    let text = "version: 1\nACCOUNT_ID: 18446744073709551615 # keep\nrefs:\n  A: x\n";
    write_credentials(&path, text);
    let booted = boot(config(&path)).await;
    booted
        .credentials()
        .set(&reference("B"), "y")
        .await
        .unwrap();
    assert_eq!(
        read(&path),
        "version: 1\nACCOUNT_ID: 18446744073709551615 # keep\nrefs:\n  A: x\n  B: y\n"
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn refuses_to_store_under_an_existing_directory_other_users_can_enter() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = home.join(".credentials.yaml");
    let booted = boot(config(&path)).await;
    let credentials = booted.credentials();
    let failure = credentials
        .set(&reference("A"), "secret-value")
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("chmod 700"), "{failure}");
    assert!(failure.to_string().contains("mode 755"), "{failure}");
    assert!(!path.exists());
    // The directory is left as the user made it; once they tighten it, the write goes through.
    assert_eq!(mode(&home), 0o755);
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    credentials
        .set(&reference("A"), "secret-value")
        .await
        .unwrap();
    assert_eq!(mode(&path), 0o600);
    booted.shutdown().await;
}

#[tokio::test]
async fn follows_a_document_whose_directory_is_created_after_boot() {
    let dir = temp_dir();
    let home = dir.path().join("missing").join("deeper");
    let path = home.join(".credentials.yaml");
    let booted = boot(watching(&path, 10)).await;
    let credentials = booted.credentials();
    let seen = updates(&booted.ctx);
    assert_eq!(resolved(&*credentials, "DSH_CRED_TEST").await, None);

    std::fs::create_dir_all(&home).unwrap();
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: created\n");
    wait_for(|| async {
        resolved(&*credentials, "DSH_CRED_TEST").await == Some(("created".into(), "file".into()))
    })
    .await;

    // The directory goes away and comes back: the watcher keeps following.
    std::fs::remove_dir_all(dir.path().join("missing")).unwrap();
    wait_for(|| async { resolved(&*credentials, "DSH_CRED_TEST").await.is_none() }).await;
    std::fs::create_dir_all(&home).unwrap();
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: recreated\n");
    wait_for(|| async {
        resolved(&*credentials, "DSH_CRED_TEST").await == Some(("recreated".into(), "file".into()))
    })
    .await;
    assert!(snapshot(&seen).len() >= 3, "{:?}", snapshot(&seen));
    booted.shutdown().await;
}

#[tokio::test]
async fn boots_past_an_unknown_top_level_key_warns_by_name_and_keeps_it_through_a_write() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(
        &path,
        "version: 1\nGITLAB_TOKEN: glpat-secret\nACCOUNT_ID: 9007199254740993\nrefs:\n  DSH_CRED_TEST: sk-file\n",
    );
    let booted = boot_layered(config(&path), &[layer(Process, None, &[])]).await;
    let credentials = booted.credentials();
    assert_eq!(
        credentials
            .resolve(&reference("DSH_CRED_TEST"))
            .await
            .unwrap(),
        file("sk-file")
    );
    assert_eq!(
        credentials
            .resolve(&reference("GITLAB_TOKEN"))
            .await
            .unwrap(),
        None
    );
    let warned = warnings(&booted.ctx);
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(
        warned[0].contains("ignoring unknown top-level"),
        "{warned:?}"
    );
    assert!(
        warned[0].contains("\"GITLAB_TOKEN\", \"ACCOUNT_ID\""),
        "{warned:?}"
    );
    assert!(warned[0].contains(&path.to_string_lossy().into_owned()));
    assert!(!warned[0].contains("glpat-secret"));
    credentials
        .set(&reference("DSH_CRED_OTHER"), "sk-other")
        .await
        .unwrap();
    assert_eq!(
        read(&path),
        "version: 1\nGITLAB_TOKEN: glpat-secret\nACCOUNT_ID: 9007199254740993\nrefs:\n  DSH_CRED_TEST: sk-file\n  DSH_CRED_OTHER: sk-other\n"
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn reads_an_empty_document_as_an_empty_store() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "# nothing stored yet\n");
    let booted = boot(config(&path)).await;
    assert_eq!(
        booted
            .credentials()
            .resolve(&reference("DSH_CRED_TEST"))
            .await
            .unwrap(),
        None
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn adds_a_missing_key_to_a_fresh_0600_document_and_emits_the_commit() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    let seen = updates(&booted.ctx);
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    credentials.set(&key, "sk-fresh").await.unwrap();
    assert_eq!(
        read(&path),
        "version: 1\nrefs:\n  DSH_CRED_TEST: sk-fresh\n"
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("sk-fresh"));
    assert_eq!(snapshot(&seen), vec!["DSH_CRED_TEST"]);
    booted.shutdown().await;
}

#[tokio::test]
async fn patches_one_entry_preserving_comments_and_every_untouched_entry() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(
        &path,
        "version: 1\nrefs:\n  # deployment notes\n  DSH_CRED_OTHER: keep\n\n  # the one under edit\n  DSH_CRED_TEST: old\n",
    );
    let booted = boot(config(&path)).await;
    booted
        .credentials()
        .set(&reference("DSH_CRED_TEST"), "new value!")
        .await
        .unwrap();
    assert_eq!(
        read(&path),
        "version: 1\nrefs:\n  # deployment notes\n  DSH_CRED_OTHER: keep\n\n  # the one under edit\n  DSH_CRED_TEST: new value!\n"
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn round_trips_values_no_dotenv_line_could_represent() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    let multi_line = "line one\nline two";
    let mixed_quotes = "both ' and \"";
    let credentials = booted.credentials();
    credentials
        .set(&reference("DSH_CRED_TEST"), multi_line)
        .await
        .unwrap();
    credentials
        .set(&reference("DSH_CRED_OTHER"), mixed_quotes)
        .await
        .unwrap();
    let reread = boot(config(&path)).await;
    let credentials = reread.credentials();
    assert_eq!(
        credentials
            .resolve(&reference("DSH_CRED_TEST"))
            .await
            .unwrap(),
        file(multi_line)
    );
    assert_eq!(
        credentials
            .resolve(&reference("DSH_CRED_OTHER"))
            .await
            .unwrap(),
        file(mixed_quotes)
    );
    assert_eq!(
        credentials
            .describe(&reference("DSH_CRED_TEST"))
            .await
            .unwrap(),
        masked_file(format!("line {} two", dots(8)))
    );
    reread.shutdown().await;
    booted.shutdown().await;
}

#[tokio::test]
async fn unsets_only_the_owning_entry_with_its_own_annotation_and_keeps_an_absent_unset_silent() {
    let dir = temp_dir();
    let path = doc(&dir);
    // Comments above an entry are that entry's annotation and go with it when it is removed.
    write_credentials(
        &path,
        "version: 1\nrefs:\n  # about the doomed one\n  DSH_CRED_TEST: gone\n  # about the survivor\n  DSH_CRED_OTHER: stays\n",
    );
    let booted = boot(config(&path)).await;
    let seen = updates(&booted.ctx);
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");
    credentials.unset(&key).await.unwrap();
    assert_eq!(
        read(&path),
        "version: 1\nrefs:\n  # about the survivor\n  DSH_CRED_OTHER: stays\n"
    );
    credentials.unset(&key).await.unwrap();
    assert_eq!(snapshot(&seen), vec!["DSH_CRED_TEST"]);
    booted.shutdown().await;
}

#[tokio::test]
async fn rejects_empty_values_but_lets_the_managed_value_replace_the_environment() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: stored\n");
    let booted = boot_layered(
        config(&path),
        &[layer(Process, None, &[("DSH_CRED_TEST", "shadowing")])],
    )
    .await;
    let credentials = booted.credentials();
    let key = reference("DSH_CRED_TEST");

    let failure = credentials.set(&key, "").await.unwrap_err();
    assert!(failure.to_string().contains("empty value"), "{failure}");

    credentials.set(&key, "next").await.unwrap();
    assert_eq!(credentials.resolve(&key).await.unwrap(), file("next"));
    credentials.unset(&key).await.unwrap();
    assert_eq!(
        credentials.resolve(&key).await.unwrap(),
        Some(ResolvedCredential {
            value: "shadowing".into(),
            source: "env".into()
        })
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn leaves_an_empty_mapping_after_unsetting_the_only_entry() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: only\n");
    let booted = boot(config(&path)).await;
    booted
        .credentials()
        .unset(&reference("DSH_CRED_TEST"))
        .await
        .unwrap();
    assert_eq!(read(&path), "version: 1\nrefs: {}\n");
    // The emptied document still reloads as an empty store, not a parse error.
    let reread = boot(config(&path)).await;
    assert_eq!(
        reread
            .credentials()
            .resolve(&reference("DSH_CRED_TEST"))
            .await
            .unwrap(),
        None
    );
    reread.shutdown().await;
    booted.shutdown().await;
}

#[tokio::test]
async fn fails_a_write_loud_when_the_on_disk_document_became_invalid() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    // An external editor left the document unparsable: the read-modify-write must refuse rather
    // than overwrite content it cannot understand.
    write_credentials(&path, "DSH_CRED_TEST: \"unterminated\n");
    let failure = booted
        .credentials()
        .set(&reference("DSH_CRED_OTHER"), "lands")
        .await
        .unwrap_err();
    assert!(
        failure.to_string().contains("invalid document"),
        "{failure}"
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn chains_past_a_rejected_write_so_one_bad_value_cannot_poison_the_queue() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    let credentials = booted.credentials();
    let (empty_key, other_key) = (reference("DSH_CRED_TEST"), reference("DSH_CRED_OTHER"));
    let (bad, good) = tokio::join!(
        credentials.set(&empty_key, ""),
        credentials.set(&other_key, "lands")
    );
    assert!(bad.unwrap_err().to_string().contains("empty value"));
    good.unwrap();
    assert_eq!(read(&path), "version: 1\nrefs:\n  DSH_CRED_OTHER: lands\n");
    booted.shutdown().await;
}

#[tokio::test]
async fn serializes_concurrent_writes_so_both_land_in_the_one_document() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    let credentials = booted.credentials();
    let (first, second) = (reference("DSH_CRED_TEST"), reference("DSH_CRED_OTHER"));
    let (one, two) = tokio::join!(
        credentials.set(&first, "one"),
        credentials.set(&second, "two")
    );
    one.unwrap();
    two.unwrap();
    assert_eq!(
        read(&path),
        "version: 1\nrefs:\n  DSH_CRED_TEST: one\n  DSH_CRED_OTHER: two\n"
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn refuses_writes_after_disposal() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    // Capture the handle first: disposal also removes the service from the context.
    let service = booted.credentials();
    booted.fiber.dispose().await;
    let failure = service
        .set(&reference("DSH_CRED_TEST"), "late")
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("disposed"), "{failure}");
    booted.kernel.shutdown().await;
}

#[tokio::test]
async fn publishes_external_edits_replaces_the_snapshot_wholesale_and_suppresses_self_writes() {
    let dir = temp_dir();
    let path = doc(&dir);
    // Watching starts on an existing document: creation racing watcher setup is a backend
    // readiness gap, not the reload contract under test.
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: boot\n");
    let booted = boot(json!({ "path": path.to_string_lossy(), "debounceMs": 10 })).await;
    let seen = updates(&booted.ctx);
    let credentials = booted.credentials();
    let (key, other) = (reference("DSH_CRED_TEST"), reference("DSH_CRED_OTHER"));

    write_credentials(
        &path,
        "version: 1\nrefs:\n  DSH_CRED_TEST: live\n  DSH_CRED_OTHER: extra\n",
    );
    wait_for(|| async { credentials.resolve(&key).await.unwrap() == file("live") }).await;

    // Wholesale replacement: an entry deleted on disk never lingers in memory.
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_TEST: live\n");
    wait_for(|| async { credentials.resolve(&other).await.unwrap().is_none() }).await;

    let before = snapshot(&seen).len();
    credentials.set(&key, "self-written").await.unwrap();
    pause(400).await;
    // Exactly the committed write's own event: the watcher echo of our own content is recognized
    // by the text cache and publishes nothing extra.
    assert_eq!(snapshot(&seen).len(), before + 1);
    assert_eq!(
        credentials.resolve(&key).await.unwrap(),
        file("self-written")
    );
    booted.shutdown().await;
}
