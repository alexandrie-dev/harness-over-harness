//! Port of `tests/catalog.spec.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hoh_session_format::{
    SessionFormatArtifact, SessionFormatArtifactDecoder, SessionFormatCatalog,
    SessionFormatCatalogOptions, SessionFormatCodec, SessionFormatCurrentEncoder,
    SessionFormatError, SessionFormatEvent, SessionFormatEventRun, SessionFormatHeader,
    SessionFormatHeaderReadResult, SessionFormatHeaderRestorer, SessionFormatJsonObject,
    SessionFormatMigration, SessionFormatMigrationContext, SessionFormatMigrationStage,
    SessionFormatMigrationStageInput, SessionFormatRecovery, SessionFormatRestoreOptions,
    SessionFormatValidation, create_session_format_catalog,
};
use serde_json::{Value, json};

type Res<T> = Result<T, SessionFormatError>;
type Ctx<'a> = &'a mut dyn SessionFormatMigrationContext;

fn object(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().expect("object literal")
}

fn old_header() -> SessionFormatHeader {
    object(json!({
        "version": 0, "id": "old", "createdAt": 1, "isSeeded": false, "delegationDepth": 0,
    }))
}

fn header_at(version: i64) -> SessionFormatHeader {
    let mut header = old_header();
    header.insert("version".to_owned(), json!(version));
    header
}

fn event() -> SessionFormatEvent {
    object(json!({ "type": "turn/start", "seq": 0, "time": 2, "data": { "turn": 1 } }))
}

fn strict_current() -> SessionFormatRestoreOptions {
    SessionFormatRestoreOptions {
        recovery: SessionFormatRecovery::Strict,
        validation: SessionFormatValidation::Current,
    }
}

fn strict_transformed() -> SessionFormatRestoreOptions {
    SessionFormatRestoreOptions {
        recovery: SessionFormatRecovery::Strict,
        validation: SessionFormatValidation::Transformed,
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

type RowFn = Arc<dyn Fn(Value, Ctx<'_>) -> Res<()> + Send + Sync>;

struct TestDecoder {
    header: SessionFormatHeader,
    declared: Option<u64>,
    on_row: RowFn,
    finish_cut: u64,
}

impl SessionFormatArtifactDecoder for TestDecoder {
    fn header(&self) -> &SessionFormatHeader {
        &self.header
    }
    fn header_inherited_event_count(&self) -> Option<u64> {
        self.declared
    }
    fn decode_row(&mut self, row_value: Value, context: Ctx<'_>) -> Res<()> {
        (self.on_row)(row_value, context)
    }
    fn finish(&mut self, _context: Ctx<'_>) -> Res<u64> {
        Ok(self.finish_cut)
    }
}

type DecodeHeaderFn = Arc<dyn Fn(&Value) -> Res<SessionFormatHeader> + Send + Sync>;
type MakeDecoderFn =
    Arc<dyn Fn(&Value) -> Res<Box<dyn SessionFormatArtifactDecoder>> + Send + Sync>;

#[derive(Clone)]
struct TestCodec {
    version: u64,
    decode_header: DecodeHeaderFn,
    make_decoder: MakeDecoderFn,
}

impl SessionFormatCodec for TestCodec {
    fn version(&self) -> u64 {
        self.version
    }
    fn decode_header(&self, value: &Value) -> Res<SessionFormatHeader> {
        (self.decode_header)(value)
    }
    fn create_decoder(
        &self,
        header_value: &Value,
        _recovery: SessionFormatRecovery,
    ) -> Res<Box<dyn SessionFormatArtifactDecoder>> {
        (self.make_decoder)(header_value)
    }
}

fn row_emits_event() -> RowFn {
    Arc::new(|row, context| context.emit_event(row.as_object().cloned().expect("row object")))
}

fn decoder_with(declared: Option<u64>, finish_cut: u64, on_row: RowFn) -> MakeDecoderFn {
    Arc::new(move |header_value| {
        Ok(Box::new(TestDecoder {
            header: header_value.as_object().cloned().expect("header object"),
            declared,
            on_row: Arc::clone(&on_row),
            finish_cut,
        }))
    })
}

fn codec(version: u64, inherited: u64) -> TestCodec {
    TestCodec {
        version,
        decode_header: Arc::new(|value| Ok(value.as_object().cloned().expect("header object"))),
        make_decoder: decoder_with(Some(inherited), inherited, row_emits_event()),
    }
}

struct IdentityEncoder {
    bad_header: bool,
}

impl SessionFormatCurrentEncoder for IdentityEncoder {
    fn encode_header(
        &self,
        header: &SessionFormatHeader,
        _inherited_event_count: u64,
    ) -> Res<SessionFormatJsonObject> {
        let mut encoded = header.clone();
        if self.bad_header {
            encoded.insert("version".to_owned(), json!(0));
        }
        Ok(encoded)
    }
    fn encode_event(&self, event: &SessionFormatEvent) -> Res<SessionFormatJsonObject> {
        Ok(event.clone())
    }
}

type ValidateFn = Arc<dyn Fn(&SessionFormatHeader) -> Res<()> + Send + Sync>;

type StageMaker = Arc<
    dyn Fn(SessionFormatMigrationStageInput<'_>) -> Res<Box<dyn SessionFormatMigrationStage>>
        + Send
        + Sync,
>;

type StageEventFn = Box<dyn FnMut(SessionFormatEvent, Ctx<'_>) -> Res<()> + Send>;
type StageRunFn = Box<dyn FnMut(Box<dyn SessionFormatEventRun>, Ctx<'_>) -> Res<()> + Send>;
type StageFinishFn = Box<dyn FnMut(Ctx<'_>) -> Res<u64> + Send>;

struct TestStage {
    declared: Option<u64>,
    on_event: StageEventFn,
    on_run: StageRunFn,
    on_finish: StageFinishFn,
}

impl SessionFormatMigrationStage for TestStage {
    fn header_inherited_event_count(&self) -> Option<u64> {
        self.declared
    }
    fn transform_event(&mut self, event: SessionFormatEvent, context: Ctx<'_>) -> Res<()> {
        (self.on_event)(event, context)
    }
    fn transform_run(&mut self, run: Box<dyn SessionFormatEventRun>, context: Ctx<'_>) -> Res<()> {
        (self.on_run)(run, context)
    }
    fn finish(&mut self, context: Ctx<'_>) -> Res<u64> {
        (self.on_finish)(context)
    }
}

fn identity_stage(cut: Option<u64>) -> TestStage {
    TestStage {
        declared: cut,
        on_event: Box::new(|event, context| context.emit_event(event)),
        on_run: Box::new(|run, context| context.emit_run(run)),
        on_finish: Box::new(move |_| Ok(cut.unwrap_or(0))),
    }
}

#[derive(Clone)]
struct Edge {
    create_stage: StageMaker,
    validate: ValidateFn,
}

impl SessionFormatMigration for Edge {
    fn name(&self) -> &str {
        "@test/v0-to-v1"
    }
    fn from_version(&self) -> u64 {
        0
    }
    fn to_version(&self) -> u64 {
        1
    }
    fn migrate_header(&self, mut header: SessionFormatHeader) -> Res<SessionFormatHeader> {
        header.insert("version".to_owned(), json!(1));
        Ok(header)
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

fn edge() -> Edge {
    Edge {
        create_stage: Arc::new(|input| {
            Ok(Box::new(identity_stage(input.source_inherited_event_count)))
        }),
        validate: Arc::new(|_| Ok(())),
    }
}

fn edge_with_stage(make: impl Fn() -> TestStage + Send + Sync + 'static) -> Edge {
    Edge {
        create_stage: Arc::new(move |_| Ok(Box::new(make()))),
        ..edge()
    }
}

fn restore_to(version: i64) -> hoh_session_format::SessionFormatArtifactRestorer {
    Arc::new(move |mut artifact: SessionFormatArtifact| {
        artifact.header.insert("version".to_owned(), json!(version));
        Ok(artifact)
    })
}

fn identity_header() -> SessionFormatHeaderRestorer {
    Arc::new(Ok)
}

fn options(
    migration: Edge,
    restore_version: i64,
    source_codec: TestCodec,
    bad_encoder: bool,
    restore_current_header: SessionFormatHeaderRestorer,
) -> SessionFormatCatalogOptions {
    SessionFormatCatalogOptions {
        current_version: 1,
        migrations: vec![Arc::new(migration) as Arc<dyn SessionFormatMigration>],
        restore_current_header,
        codecs: vec![Arc::new(source_codec), Arc::new(codec(1, 0))],
        restore_current: restore_to(restore_version),
        current_encoder: Arc::new(IdentityEncoder {
            bad_header: bad_encoder,
        }),
        restore_transformed_current: restore_to(restore_version),
    }
}

fn catalog_with(
    migration: Edge,
    restore_version: i64,
    source_codec: TestCodec,
    bad_encoder: bool,
    restore_current_header: SessionFormatHeaderRestorer,
) -> SessionFormatCatalog {
    create_session_format_catalog(options(
        migration,
        restore_version,
        source_codec,
        bad_encoder,
        restore_current_header,
    ))
    .expect("catalog")
}

fn catalog() -> SessionFormatCatalog {
    catalog_with(edge(), 1, codec(0, 0), false, identity_header())
}

fn error_of<T>(result: Res<T>) -> SessionFormatError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected failure"),
    }
}

fn cause_message(error: &SessionFormatError) -> String {
    std::error::Error::source(error).expect("cause").to_string()
}

#[test]
fn classifies_headers_and_restores_physical_rows_through_the_compiled_chain() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let current = catalog_with(
        Edge {
            create_stage: Arc::new(move |input| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(identity_stage(input.source_inherited_event_count)))
            }),
            ..edge()
        },
        1,
        codec(0, 0),
        false,
        identity_header(),
    );

    assert_eq!(
        current.read_header(&Value::Object(old_header())),
        SessionFormatHeaderReadResult::MigrationRequired {
            stored_version: 0,
            target_version: 1,
            header: header_at(1),
        }
    );
    assert!(matches!(
        current.read_header(&json!({ "version": 2 })),
        SessionFormatHeaderReadResult::Unsupported {
            stored_version: 2,
            target_version: 1,
            ..
        }
    ));
    assert!(matches!(
        current.read_header(&Value::Object(header_at(1))),
        SessionFormatHeaderReadResult::Current {
            stored_version: 1,
            target_version: 1,
            ..
        }
    ));
    assert!(matches!(
        current.read_header(&json!({ "version": "broken" })),
        SessionFormatHeaderReadResult::Malformed {
            target_version: 1,
            ..
        }
    ));

    let mut restore = current
        .create_restore(&Value::Object(old_header()), strict_current())
        .expect("restore");
    restore.decode_row(Value::Object(event())).expect("row");
    let restored = restore.finish().expect("finish");
    assert_eq!(restored.header["version"], json!(1));
    assert_eq!(restored.events, vec![event()]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn restores_current_rows_without_a_migration_and_encodes_one_record_at_a_time() {
    let current = catalog();
    let header = header_at(1);
    let mut restore = current
        .create_restore(&Value::Object(header.clone()), strict_current())
        .expect("restore");
    restore.decode_row(Value::Object(event())).expect("row");

    assert_eq!(
        restore.finish().expect("finish"),
        SessionFormatArtifact {
            header: header.clone(),
            inherited_event_count: 0,
            events: vec![event()],
        }
    );
    assert_eq!(
        current.encode_current_header(&header, 0).expect("header"),
        header
    );
    assert_eq!(
        current.encode_current_event(&event()).expect("event"),
        event()
    );
    assert!(
        error_of(current.encode_current_header(&old_header(), 0))
            .to_string()
            .contains("requires Session format v1")
    );

    let mut physical_only = current
        .create_restore(&Value::Object(header.clone()), strict_transformed())
        .expect("restore");
    physical_only
        .decode_row(Value::Object(event()))
        .expect("row");
    assert_eq!(
        physical_only.finish().expect("finish"),
        SessionFormatArtifact {
            header,
            inherited_event_count: 0,
            events: vec![event()],
        }
    );

    let mut transformed = current
        .create_restore(&Value::Object(old_header()), strict_transformed())
        .expect("restore");
    transformed.decode_row(Value::Object(event())).expect("row");
    let restored = transformed.finish().expect("finish");
    assert_eq!(restored.header["version"], json!(1));
    assert_eq!(restored.events, vec![event()]);
}

#[test]
fn classifies_migrated_target_validation_failures_as_unsupported() {
    // `it.each([new Error(...), 'non-Error ...'])`: both thrown shapes arrive as `Other` here.
    for failure in [
        "target artifact is incompatible",
        "non-Error target artifact refusal",
    ] {
        let mut opts = options(edge(), 1, codec(0, 0), false, identity_header());
        opts.restore_transformed_current =
            Arc::new(move |_| Err(SessionFormatError::other(failure)));
        let current = create_session_format_catalog(opts).expect("catalog");
        let mut restore = current
            .create_restore(&Value::Object(old_header()), strict_transformed())
            .expect("restore");
        restore.decode_row(Value::Object(event())).expect("row");

        let error = error_of(restore.finish());
        assert!(error.is_unsupported_migration());
        assert_eq!(
            error.to_string(),
            format!("Session migration from v0 to v1 refuses the transformed artifact: {failure}")
        );
        assert_eq!(cause_message(&error), failure);
    }
}

#[test]
fn preserves_current_validation_failures_and_explicit_migration_refusals() {
    let mut opts = options(edge(), 1, codec(0, 0), false, identity_header());
    opts.restore_current = Arc::new(|_| {
        Err(SessionFormatError::other(
            "installed current validation failed",
        ))
    });
    let restore = create_session_format_catalog(opts)
        .expect("catalog")
        .create_restore(&Value::Object(old_header()), strict_current())
        .expect("restore");
    let error = error_of(restore.finish());
    assert!(!error.is_unsupported_migration());
    assert_eq!(error.to_string(), "installed current validation failed");

    let mut opts = options(edge(), 1, codec(0, 0), false, identity_header());
    opts.restore_transformed_current = Arc::new(|_| {
        Err(SessionFormatError::unsupported_migration(
            "explicit target refusal",
        ))
    });
    let restore = create_session_format_catalog(opts)
        .expect("catalog")
        .create_restore(&Value::Object(old_header()), strict_transformed())
        .expect("restore");
    let error = error_of(restore.finish());
    assert!(error.is_unsupported_migration());
    assert_eq!(error.to_string(), "explicit target refusal");
}

#[test]
fn rejects_a_current_encoder_that_returns_a_non_current_header() {
    let current = catalog_with(edge(), 1, codec(0, 0), true, identity_header());

    assert!(
        error_of(current.encode_current_header(&header_at(1), 0))
            .to_string()
            .contains("non-current header")
    );
}

#[test]
fn rejects_duplicate_missing_and_future_codec_declarations() {
    let build = |codecs: Vec<TestCodec>| {
        let mut opts = options(edge(), 1, codec(0, 0), false, identity_header());
        opts.codecs = codecs
            .into_iter()
            .map(|item| Arc::new(item) as Arc<dyn SessionFormatCodec>)
            .collect();
        error_of(create_session_format_catalog(opts)).to_string()
    };
    assert!(build(vec![codec(0, 0), codec(0, 0), codec(1, 0)]).contains("codec v0 is duplicated"));
    assert!(build(vec![codec(0, 0)]).contains("codec v1 is missing"));
    assert!(build(vec![codec(0, 0), codec(1, 0), codec(2, 0)]).contains("codec v2 is newer"));
}

#[test]
fn preserves_unsupported_header_and_migration_failures() {
    let refusing = TestCodec {
        decode_header: Arc::new(|_| Err(SessionFormatError::other("bad header"))),
        make_decoder: Arc::new(|_| Err(SessionFormatError::other("bad body"))),
        ..codec(0, 0)
    };
    let current = catalog_with(edge(), 1, refusing, false, identity_header());
    assert_eq!(
        current.read_header(&Value::Object(old_header())),
        SessionFormatHeaderReadResult::Malformed {
            stored_version: Some(0),
            target_version: 1,
            reason: "bad header".to_owned(),
        }
    );
    assert_eq!(
        error_of(current.create_restore(&Value::Object(old_header()), strict_current()))
            .to_string(),
        "bad body"
    );
    assert!(
        error_of(current.create_restore(&json!({ "version": 2 }), strict_current()))
            .to_string()
            .contains("newer")
    );

    let non_error = catalog_with(
        edge(),
        1,
        TestCodec {
            decode_header: Arc::new(|_| Err(SessionFormatError::other("non-Error header failure"))),
            ..codec(0, 0)
        },
        false,
        identity_header(),
    );
    assert!(matches!(
        non_error.read_header(&Value::Object(old_header())),
        SessionFormatHeaderReadResult::Malformed { reason, .. } if reason == "non-Error header failure"
    ));
}

#[test]
fn classifies_migrated_header_refusals_as_unsupported_and_current_corruption_as_malformed() {
    let unsupported = catalog_with(
        Edge {
            validate: Arc::new(|_| Err(SessionFormatError::other("target header lacks marker"))),
            ..edge()
        },
        1,
        codec(0, 0),
        false,
        identity_header(),
    );
    match unsupported.read_header(&Value::Object(old_header())) {
        SessionFormatHeaderReadResult::Unsupported { reason, .. } => {
            assert!(reason.contains("target header lacks marker"));
        }
        other => panic!("expected unsupported, got {other:?}"),
    }

    let malformed = catalog_with(
        edge(),
        1,
        codec(0, 0),
        false,
        Arc::new(|_| Err(SessionFormatError::other("current header is corrupt"))),
    );
    assert_eq!(
        malformed.read_header(&Value::Object(header_at(1))),
        SessionFormatHeaderReadResult::Malformed {
            stored_version: Some(1),
            target_version: 1,
            reason: "current header is corrupt".to_owned(),
        }
    );
}

#[test]
fn enforces_decoder_stage_and_restorer_lifecycle_results() {
    let changed_cut = edge_with_stage(|| {
        let mut stage = identity_stage(Some(1));
        stage.on_finish = Box::new(|_| Ok(0));
        stage
    });
    let mut seeded = old_header();
    seeded.insert("isSeeded".to_owned(), json!(true));
    let changed = catalog_with(changed_cut, 1, codec(0, 1), false, identity_header())
        .create_restore(&Value::Object(seeded), strict_current())
        .expect("restore");
    assert!(
        error_of(changed.finish())
            .to_string()
            .contains("changed its predeclared inherited cut")
    );

    let trailing = catalog_with(
        edge_with_stage(|| TestStage {
            declared: Some(0),
            on_event: Box::new(|_, _| Ok(())),
            on_run: Box::new(|_, _| Ok(())),
            on_finish: Box::new(|context| {
                context.emit_event(event())?;
                Ok(0)
            }),
        }),
        1,
        codec(0, 0),
        false,
        identity_header(),
    )
    .create_restore(&Value::Object(old_header()), strict_current())
    .expect("restore");
    assert_eq!(trailing.finish().expect("finish").events, vec![event()]);

    let wrong_version = catalog_with(edge(), 0, codec(0, 0), false, identity_header())
        .create_restore(&Value::Object(old_header()), strict_current())
        .expect("restore");
    assert!(
        error_of(wrong_version.finish())
            .to_string()
            .contains("returned v0")
    );

    let changed_decoder_cut = TestCodec {
        make_decoder: decoder_with(Some(0), 1, Arc::new(|_, _| Ok(()))),
        ..codec(0, 0)
    };
    let changed_decoder = catalog_with(edge(), 1, changed_decoder_cut, false, identity_header())
        .create_restore(&Value::Object(old_header()), strict_current())
        .expect("restore");
    assert!(
        error_of(changed_decoder.finish())
            .to_string()
            .contains("decoder changed its predeclared inherited cut")
    );

    let deferred_cut = TestCodec {
        make_decoder: decoder_with(None, 0, Arc::new(|_, _| Ok(()))),
        ..codec(0, 0)
    };
    let deferred = catalog_with(edge(), 1, deferred_cut, false, identity_header())
        .create_restore(&Value::Object(old_header()), strict_current())
        .expect("restore");
    assert_eq!(deferred.finish().expect("finish").inherited_event_count, 0);
}

#[test]
fn expands_an_unhandled_compact_run_without_an_intermediate_array() {
    let source = TestCodec {
        make_decoder: decoder_with(
            Some(0),
            0,
            Arc::new(|_, context| context.emit_run(Box::new(TestRun(vec![event()])))),
        ),
        ..codec(0, 0)
    };
    let mut restore = catalog_with(edge(), 1, source, false, identity_header())
        .create_restore(&Value::Object(old_header()), strict_current())
        .expect("restore");
    restore.decode_row(json!({})).expect("row");

    assert_eq!(restore.finish().expect("finish").events, vec![event()]);
}

#[test]
fn restore_header_is_available_before_body_decoding() {
    let current = catalog();
    let migrating = current
        .create_restore(&Value::Object(old_header()), strict_current())
        .expect("restore");
    assert_eq!(migrating.header()["version"], json!(1));
    let direct = current
        .create_restore(&Value::Object(header_at(1)), strict_current())
        .expect("restore");
    assert_eq!(direct.header(), &header_at(1));
}
