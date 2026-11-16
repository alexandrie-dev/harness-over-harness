//! Port of `tests/chain.spec.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hoh_session_format::{
    SessionFormatChain, SessionFormatChainOptions, SessionFormatError, SessionFormatEvent,
    SessionFormatEventCollector, SessionFormatEventRun, SessionFormatHeader,
    SessionFormatMigration, SessionFormatMigrationContext, SessionFormatMigrationStage,
    SessionFormatMigrationStageInput, SessionFormatSourceKind, create_session_format_chain,
    define_session_format_migration,
};
use serde_json::{Value, json};

type Res<T> = Result<T, SessionFormatError>;
type StageFactory = Arc<
    dyn Fn(SessionFormatMigrationStageInput<'_>) -> Res<Box<dyn SessionFormatMigrationStage>>
        + Send
        + Sync,
>;
type HeaderFn = Arc<dyn Fn(SessionFormatHeader) -> Res<SessionFormatHeader> + Send + Sync>;
type ValidateFn = Arc<dyn Fn(&SessionFormatHeader) -> Res<()> + Send + Sync>;

fn object(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().expect("object literal")
}

fn header_v(version: i64) -> SessionFormatHeader {
    object(json!({
        "version": version, "id": "session-1", "createdAt": 1,
        "isSeeded": false, "delegationDepth": 0,
    }))
}

fn event() -> SessionFormatEvent {
    object(json!({ "type": "turn/start", "seq": 0, "time": 2, "data": { "turn": 1 } }))
}

struct Discard;

impl SessionFormatMigrationContext for Discard {
    fn emit_event(&mut self, _event: SessionFormatEvent) -> Res<()> {
        Ok(())
    }
    fn emit_run(&mut self, _run: Box<dyn SessionFormatEventRun>) -> Res<()> {
        Ok(())
    }
}

struct TestRun(Vec<SessionFormatEvent>);

impl SessionFormatEventRun for TestRun {
    fn run_type(&self) -> &str {
        "test-run"
    }
    fn first_seq(&self) -> u64 {
        0
    }
    fn event_count(&self) -> u64 {
        1
    }
    fn expand(self: Box<Self>) -> Box<dyn Iterator<Item = SessionFormatEvent>> {
        Box::new(self.0.into_iter())
    }
}

fn test_run() -> Box<dyn SessionFormatEventRun> {
    Box::new(TestRun(vec![event()]))
}

type EventFn =
    Box<dyn FnMut(SessionFormatEvent, &mut dyn SessionFormatMigrationContext) -> Res<()> + Send>;
type RunFn = Box<
    dyn FnMut(Box<dyn SessionFormatEventRun>, &mut dyn SessionFormatMigrationContext) -> Res<()>
        + Send,
>;
type FinishFn = Box<dyn FnMut(&mut dyn SessionFormatMigrationContext) -> Res<u64> + Send>;

struct TestStage {
    declared: Option<u64>,
    on_event: EventFn,
    on_run: RunFn,
    on_finish: FinishFn,
}

impl SessionFormatMigrationStage for TestStage {
    fn header_inherited_event_count(&self) -> Option<u64> {
        self.declared
    }
    fn transform_event(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Res<()> {
        (self.on_event)(event, context)
    }
    fn transform_run(
        &mut self,
        run: Box<dyn SessionFormatEventRun>,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Res<()> {
        (self.on_run)(run, context)
    }
    fn finish(&mut self, context: &mut dyn SessionFormatMigrationContext) -> Res<u64> {
        (self.on_finish)(context)
    }
}

fn identity_stage(cut: Option<u64>) -> TestStage {
    TestStage {
        declared: cut,
        on_event: Box::new(|value, context| context.emit_event(value)),
        on_run: Box::new(|run, context| context.emit_run(run)),
        on_finish: Box::new(move |_| Ok(cut.unwrap_or(0))),
    }
}

#[derive(Clone)]
struct TestMigration {
    name: String,
    from: u64,
    to: u64,
    migrate: HeaderFn,
    create_stage: StageFactory,
    validate: ValidateFn,
}

impl SessionFormatMigration for TestMigration {
    fn name(&self) -> &str {
        &self.name
    }
    fn from_version(&self) -> u64 {
        self.from
    }
    fn to_version(&self) -> u64 {
        self.to
    }
    fn migrate_header(&self, header: SessionFormatHeader) -> Res<SessionFormatHeader> {
        (self.migrate)(header)
    }
    fn create_stage(
        &self,
        input: SessionFormatMigrationStageInput<'_>,
    ) -> Res<Box<dyn SessionFormatMigrationStage>> {
        (self.create_stage)(input)
    }
    fn validate_target_header(&self, header: &SessionFormatHeader) -> Res<()> {
        (self.validate)(header)
    }
}

fn set_version(version: i64) -> HeaderFn {
    Arc::new(move |mut header| {
        header.insert("version".to_owned(), json!(version));
        Ok(header)
    })
}

fn stage_factory(make: impl Fn(Option<u64>) -> TestStage + Send + Sync + 'static) -> StageFactory {
    Arc::new(move |input| Ok(Box::new(make(input.source_inherited_event_count))))
}

fn migration() -> TestMigration {
    TestMigration {
        name: "@test/v0-to-v1".to_owned(),
        from: 0,
        to: 1,
        migrate: set_version(1),
        create_stage: stage_factory(identity_stage),
        validate: Arc::new(|_| Ok(())),
    }
}

fn chain_with(edge: TestMigration) -> SessionFormatChain {
    create_session_format_chain(SessionFormatChainOptions {
        current_version: 1,
        migrations: vec![Arc::new(edge)],
        restore_current_header: Arc::new(Ok),
    })
    .expect("chain")
}

fn options(
    current_version: u64,
    migrations: Vec<TestMigration>,
    restore: HeaderFn,
) -> SessionFormatChainOptions {
    SessionFormatChainOptions {
        current_version,
        migrations: migrations
            .into_iter()
            .map(|edge| Arc::new(edge) as Arc<dyn SessionFormatMigration>)
            .collect(),
        restore_current_header: restore,
    }
}

fn error_of<T>(result: Res<T>) -> SessionFormatError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected callback to fail"),
    }
}

fn cause_message(error: &SessionFormatError) -> String {
    std::error::Error::source(error).expect("cause").to_string()
}

fn failing_stage(
    event_failure: Option<&'static str>,
    run_failure: Option<&'static str>,
    finish_failure: Option<&'static str>,
) -> TestStage {
    let mut stage = identity_stage(Some(0));
    if let Some(text) = event_failure {
        stage.on_event = Box::new(move |_, _| Err(SessionFormatError::other(text)));
    }
    if let Some(text) = run_failure {
        stage.on_run = Box::new(move |_, _| Err(SessionFormatError::other(text)));
    }
    if let Some(text) = finish_failure {
        stage.on_finish = Box::new(move |_| Err(SessionFormatError::other(text)));
    }
    stage
}

#[test]
fn bypasses_adjacent_stages_for_current_input() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let current = chain_with(TestMigration {
        create_stage: Arc::new(move |input| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(identity_stage(input.source_inherited_event_count)))
        }),
        ..migration()
    });
    let mut stream = current
        .create_stream(header_v(1), Some(0), SessionFormatEventCollector::new())
        .expect("stream");

    stream.emit_event(event()).expect("emit");

    assert_eq!(stream.header(), &header_v(1));
    assert_eq!(stream.finish().expect("finish"), 0);
    assert_eq!(stream.into_output().values, vec![event()]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn composes_one_adjacent_stage_and_its_header_conversion() {
    let current = chain_with(migration());
    let source = header_v(0);
    let mut stream = current
        .create_stream(source.clone(), Some(0), SessionFormatEventCollector::new())
        .expect("stream");

    stream.emit_event(event()).expect("emit");

    assert_eq!(stream.header()["version"], json!(1));
    assert_eq!(stream.finish().expect("finish"), 0);
    assert_eq!(stream.into_output().values, vec![event()]);
    assert_eq!(
        current.migrate_header(&source).expect("header")["version"],
        json!(1)
    );
}

#[test]
fn rejects_invalid_declarations_and_incomplete_chain_construction() {
    let base = migration();
    let message = |result: Res<Arc<dyn SessionFormatMigration>>| error_of(result).to_string();
    assert!(
        message(define_session_format_migration(Arc::new(TestMigration {
            name: String::new(),
            ..base.clone()
        })))
        .contains("name")
    );
    assert!(
        message(define_session_format_migration(Arc::new(TestMigration {
            to: 2,
            ..base.clone()
        })))
        .contains("adjacent")
    );
    let build = |opts: SessionFormatChainOptions| error_of(create_session_format_chain(opts));
    let missing = build(options(1, vec![], Arc::new(Ok)));
    assert!(missing.to_string().contains("missing"));
    assert!(missing.is_unsupported_migration());
    assert!(
        build(options(1, vec![base.clone(), base.clone()], Arc::new(Ok)))
            .to_string()
            .contains("duplicated")
    );
    let duplicate_name = build(options(
        2,
        vec![
            base.clone(),
            TestMigration {
                from: 1,
                to: 2,
                ..base.clone()
            },
        ],
        Arc::new(Ok),
    ))
    .to_string();
    assert_eq!(
        duplicate_name,
        "Session migration name \"@test/v0-to-v1\" is duplicated"
    );
    assert!(
        build(options(
            1,
            vec![
                base.clone(),
                TestMigration {
                    name: "@test/v1-to-v2".to_owned(),
                    from: 1,
                    to: 2,
                    ..base.clone()
                },
            ],
            Arc::new(Ok),
        ))
        .to_string()
        .contains("does not lead")
    );
}

#[test]
fn composes_stages_whose_inherited_cut_is_derived_from_the_body() {
    let first = TestMigration {
        create_stage: Arc::new(|_| {
            let mut stage = identity_stage(None);
            stage.on_finish = Box::new(|_| Ok(0));
            Ok(Box::new(stage))
        }),
        ..migration()
    };
    let second = TestMigration {
        name: "@test/v1-to-v2".to_owned(),
        from: 1,
        to: 2,
        migrate: set_version(2),
        ..migration()
    };
    let current =
        create_session_format_chain(options(2, vec![first, second.clone()], Arc::new(Ok)))
            .expect("chain");

    let mut stream = current
        .create_stream(header_v(0), Some(0), Discard)
        .expect("stream");
    assert_eq!(stream.finish().expect("finish"), 0);

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let complete = create_session_format_chain(options(
        2,
        vec![
            migration(),
            TestMigration {
                create_stage: Arc::new(move |input| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(input.source_kind, SessionFormatSourceKind::Transformed);
                    Ok(Box::new(identity_stage(Some(0))))
                }),
                ..second
            },
        ],
        Arc::new(Ok),
    ))
    .expect("chain");
    let mut stream = complete
        .create_stream(header_v(0), Some(0), Discard)
        .expect("stream");
    assert_eq!(stream.finish().expect("finish"), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn rejects_newer_inputs_and_wrong_header_versions() {
    let current = chain_with(TestMigration {
        migrate: Arc::new(Ok),
        ..migration()
    });
    let source = header_v(0);

    assert!(
        error_of(current.create_stream(header_v(2), Some(0), Discard))
            .to_string()
            .contains("newer")
    );
    assert!(
        error_of(current.create_stream(header_v(-1), Some(0), Discard))
            .to_string()
            .contains("non-negative")
    );
    assert!(
        error_of(current.create_stream(source.clone(), Some(0), Discard))
            .to_string()
            .contains("header returned v0")
    );
    assert!(
        error_of(current.migrate_header(&source))
            .to_string()
            .contains("header returned v0")
    );
}

#[test]
fn classifies_stage_failures_as_unsupported_and_preserves_explicit_refusals() {
    let source = header_v(0);
    let failed = chain_with(TestMigration {
        create_stage: Arc::new(|_| {
            Err(SessionFormatError::other("target relationship is invalid"))
        }),
        ..migration()
    });
    let refusal = error_of(failed.create_stream(source.clone(), Some(0), Discard));
    assert!(refusal.is_unsupported_migration());
    assert!(
        refusal
            .to_string()
            .contains("target relationship is invalid")
    );
    assert_eq!(
        refusal.to_string(),
        "@test/v0-to-v1 refuses this format v0 Session: target relationship is invalid"
    );
    assert_eq!(cause_message(&refusal), "target relationship is invalid");

    let preserved = chain_with(TestMigration {
        create_stage: Arc::new(|_| {
            Err(SessionFormatError::unsupported_migration(
                "explicit edge refusal",
            ))
        }),
        ..migration()
    });
    let kept = error_of(preserved.create_stream(source.clone(), Some(0), Discard));
    assert!(kept.is_unsupported_migration());
    assert_eq!(kept.to_string(), "explicit edge refusal");

    let finishing = chain_with(TestMigration {
        create_stage: Arc::new(|_| {
            Ok(Box::new(failing_stage(
                None,
                None,
                Some("finish relationship is invalid"),
            )))
        }),
        ..migration()
    });
    let mut stream = finishing
        .create_stream(source.clone(), Some(0), Discard)
        .expect("stream");
    let finish_refusal = error_of(stream.finish());
    assert!(finish_refusal.is_unsupported_migration());
    assert_eq!(
        cause_message(&finish_refusal),
        "finish relationship is invalid"
    );

    let event_failing = chain_with(TestMigration {
        create_stage: Arc::new(|_| {
            Ok(Box::new(failing_stage(
                Some("event relationship is invalid"),
                None,
                None,
            )))
        }),
        ..migration()
    });
    let mut stream = event_failing
        .create_stream(source.clone(), Some(0), Discard)
        .expect("stream");
    assert!(
        error_of(stream.emit_event(event()))
            .to_string()
            .contains("event relationship is invalid")
    );

    let run_failing = chain_with(TestMigration {
        create_stage: Arc::new(|_| {
            Ok(Box::new(failing_stage(
                None,
                Some("run relationship is invalid"),
                None,
            )))
        }),
        ..migration()
    });
    let mut stream = run_failing
        .create_stream(source.clone(), Some(0), Discard)
        .expect("stream");
    let run_refusal = error_of(stream.emit_run(test_run()));
    assert!(run_refusal.is_unsupported_migration());
    assert_eq!(cause_message(&run_refusal), "run relationship is invalid");

    let non_error = chain_with(TestMigration {
        create_stage: Arc::new(|_| Err(SessionFormatError::other("non-Error stage refusal"))),
        ..migration()
    });
    assert!(
        error_of(non_error.create_stream(source, Some(0), Discard))
            .to_string()
            .contains("non-Error stage refusal")
    );
}

#[test]
fn collects_compact_runs_as_expanded_events() {
    let mut collector = SessionFormatEventCollector::new();
    collector.emit_run(test_run()).expect("run");
    assert_eq!(collector.values, vec![event()]);
}

#[test]
fn validates_every_adjacent_target_header_and_the_final_current_header() {
    let validated = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&validated);
    let validate: ValidateFn = Arc::new(move |header| {
        counter.fetch_add(1, Ordering::SeqCst);
        if header.get("targetMarker") != Some(&json!(true)) {
            return Err(SessionFormatError::other("target header lacks marker"));
        }
        Ok(())
    });
    let restore: HeaderFn = Arc::new(|header| {
        if !header.get("id").is_some_and(Value::is_string) {
            return Err(SessionFormatError::other("current header lacks id"));
        }
        Ok(header)
    });
    let current = create_session_format_chain(options(
        1,
        vec![TestMigration {
            validate,
            ..migration()
        }],
        Arc::clone(&restore),
    ))
    .expect("chain");
    let source = header_v(0);

    let refusal = error_of(current.migrate_header(&source));
    assert!(refusal.is_unsupported_migration());
    assert!(refusal.to_string().contains("target header lacks marker"));
    assert_eq!(validated.load(Ordering::SeqCst), 1);

    let rejecting = create_session_format_chain(options(
        1,
        vec![TestMigration {
            migrate: Arc::new(|_| Err(SessionFormatError::other("historical header policy"))),
            ..migration()
        }],
        restore,
    ))
    .expect("chain");
    assert!(
        error_of(rejecting.migrate_header(&source))
            .to_string()
            .contains("historical header policy")
    );

    let bad_current = create_session_format_chain(options(
        1,
        vec![migration()],
        Arc::new(|_| Ok(object(json!({ "version": 1 })))),
    ))
    .expect("chain");
    assert!(
        error_of(bad_current.migrate_header(&header_v(1)))
            .to_string()
            .contains("current Session header restoration id")
    );

    let wrong_current_version = create_session_format_chain(options(
        1,
        vec![migration()],
        Arc::new(|mut header| {
            header.insert("version".to_owned(), json!(0));
            Ok(header)
        }),
    ))
    .expect("chain");
    assert!(
        error_of(wrong_current_version.migrate_header(&header_v(1)))
            .to_string()
            .contains("header restorer returned v0")
    );
}
