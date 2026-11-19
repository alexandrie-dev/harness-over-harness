use hoh_llm_mock_server::MockLlmBehavior as B;
use hoh_llm_mock_server::cli::{
    MOCK_LLM_CLI_USAGE, MockLlmCliParseResult, parse_mock_llm_cli_args,
};

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn error(values: &[&str]) -> String {
    match parse_mock_llm_cli_args(&args(values)) {
        Ok(_) => panic!("expected parser error"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn help_does_not_require_sequence() {
    assert!(matches!(
        parse_mock_llm_cli_args(&args(&["--help"])),
        Ok(MockLlmCliParseResult::Help)
    ));
    assert!(MOCK_LLM_CLI_USAGE.contains("--sequence"));
}

#[test]
fn parses_every_listener_and_response_option() {
    let parsed = parse_mock_llm_cli_args(&args(&[
        "--sequence",
        "connection_refused,partial_disconnect,success",
        "--host",
        "localhost",
        "--port",
        "9010",
        "--api-key",
        "mock-key",
        "--listen-delay-ms",
        "100",
        "--repeat-last",
        "--success-text",
        "done",
        "--partial-text",
        "half",
        "--reasoning-text",
        "think",
        "--chunk-size",
        "2",
        "--chunk-delay-ms",
        "3",
        "--disconnect-delay-ms",
        "4",
        "--retry-after-ms",
        "5000",
        "--request-id",
        "request-1",
        "--tool-name",
        "lookup",
        "--tool-arguments",
        r#"{"id":1}"#,
    ]))
    .expect("valid arguments");
    let MockLlmCliParseResult::Run(config) = parsed else {
        panic!("expected run")
    };
    assert!(config.starts_unavailable);
    assert_eq!(config.listen_delay_ms, 100);
    assert_eq!(
        config.server.sequence,
        vec![B::PartialDisconnect, B::Success]
    );
    assert_eq!(config.server.host.as_deref(), Some("localhost"));
    assert_eq!(config.server.port, Some(9010.0));
    assert_eq!(config.server.tool_arguments.as_deref(), Some(r#"{"id":1}"#));
}

#[test]
fn ordinary_sequence_uses_standalone_defaults() {
    let parsed = parse_mock_llm_cli_args(&args(&["--sequence", "success"])).expect("valid");
    let MockLlmCliParseResult::Run(config) = parsed else {
        panic!("run")
    };
    assert!(!config.starts_unavailable);
    assert_eq!(config.listen_delay_ms, 0);
    assert_eq!(config.server.port, Some(8_000.0));
    assert_eq!(config.server.repeat_last, Some(false));
}

#[test]
fn unavailable_uses_default_delay() {
    let parsed = parse_mock_llm_cli_args(&args(&[
        "--sequence",
        "connection_refused,success",
        "--port",
        "8001",
    ]))
    .expect("valid");
    let MockLlmCliParseResult::Run(config) = parsed else {
        panic!("run")
    };
    assert_eq!(config.listen_delay_ms, 750);
}

#[test]
fn parses_reproducible_weighted_random() {
    let parsed = parse_mock_llm_cli_args(&args(&[
        "--sequence",
        "random",
        "--repeat-last",
        "--seed",
        "42",
        "--random-weights",
        "success=8,partial_disconnect=2",
    ]))
    .expect("valid");
    let MockLlmCliParseResult::Run(config) = parsed else {
        panic!("run")
    };
    assert_eq!(config.server.random_seed, Some(42.0));
    assert_eq!(
        config.server.random_weights.as_ref().expect("weights")["success"],
        8.0
    );
}

#[test]
fn numbers_accept_javascript_radix_prefixes() {
    let parsed = parse_mock_llm_cli_args(&args(&[
        "--sequence",
        "random",
        "--port",
        "0x1f40",
        "--seed",
        "0b10",
        "--random-weights",
        "success=0x10,empty=0o10",
    ]))
    .expect("JavaScript Number forms are valid");
    let MockLlmCliParseResult::Run(config) = parsed else {
        panic!("run")
    };
    assert_eq!(config.server.port, Some(8_000.0));
    assert_eq!(config.server.random_seed, Some(2.0));
    let weights = config.server.random_weights.expect("weights");
    assert_eq!(weights["success"], 16.0);
    assert_eq!(weights["empty"], 8.0);
}

#[test]
fn rejects_tokenizer_failures() {
    for (argv, marker) in [
        (vec!["--wat"], "Unknown option '--wat'"),
        (vec!["--port"], "Option '--port <value>' argument missing"),
        (
            vec!["--sequence", "success", "stray"],
            "Unexpected argument 'stray'",
        ),
    ] {
        assert!(error(&argv).contains(marker));
    }
}

#[test]
fn rejects_invalid_sequences_and_cross_option_combinations() {
    let cases: &[(&[&str], &str)] = &[
        (&[], "--sequence is required"),
        (&["--sequence", "success,"], "non-empty"),
        (
            &["--sequence", "success,connection_refused"],
            "only as the first",
        ),
        (&["--sequence", "connection_refused"], "must be followed"),
        (&["--sequence", "unknown"], "unknown behavior"),
        (
            &["--sequence", "connection_refused,success", "--port", "0"],
            "nonzero",
        ),
        (
            &["--sequence", "success", "--listen-delay-ms", "5"],
            "requires connection_refused",
        ),
        (&["--sequence", "success", "--seed", "1"], "require random"),
    ];
    for (argv, marker) in cases {
        assert!(error(argv).contains(marker), "{argv:?}");
    }
}

#[test]
fn rejects_bad_numbers_and_random_weights() {
    let cases: &[(&[&str], &str)] = &[
        (&["--port", "NaN", "--sequence", "success"], "finite number"),
        (
            &[
                "--sequence",
                "connection_refused,success",
                "--listen-delay-ms=-1",
            ],
            "integer between",
        ),
        (
            &["--sequence", "random", "--random-weights", "success"],
            "expects behavior=weight",
        ),
        (
            &["--sequence", "random", "--random-weights", "random=1"],
            "concrete behavior",
        ),
        (
            &[
                "--sequence",
                "random",
                "--random-weights",
                "success=1,success=2",
            ],
            "duplicate",
        ),
        (
            &["--sequence", "random", "--random-weights", "success=nope"],
            "finite number",
        ),
    ];
    for (argv, marker) in cases {
        assert!(error(argv).contains(marker), "{argv:?}");
    }
}
