use std::any::Any;
use std::sync::Arc;

use crate::error::SessionFormatError;

/// Scalar value admitted at the durable Session JSON boundary: any `serde_json::Value`.
pub type SessionFormatJsonPrimitive = serde_json::Value;

/// Lossless JSON value admitted at the durable Session boundary.
pub type SessionFormatJsonValue = serde_json::Value;

/// Lossless JSON object admitted at the durable Session boundary (key order preserved).
pub type SessionFormatJsonObject = serde_json::Map<String, serde_json::Value>;

/// Logical Session metadata shared by supported historical and current formats. A JSON object
/// whose known keys are `version`, `id`, `createdAt`, `cwd`, `parentSession`, `isSeeded`, `origin`,
/// `delegationDepth` and `agentPreset`; other members are kept.
pub type SessionFormatHeader = SessionFormatJsonObject;

/// One decoded logical Session event: a JSON object with `type`, `seq`, `time` and `data`.
pub type SessionFormatEvent = SessionFormatJsonObject;

/// One detached complete logical Session artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionFormatArtifact {
    pub header: SessionFormatHeader,
    /// Exact inherited prefix length, available only after a body read.
    pub inherited_event_count: u64,
    pub events: Vec<SessionFormatEvent>,
}

/// Whether an adjacent edge consumes physical decode output or a prior migration's validated output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFormatSourceKind {
    Decoded,
    Transformed,
}

/// Headers and inherited cut supplied when one adjacent body stage is created.
#[derive(Debug, Clone, Copy)]
pub struct SessionFormatMigrationStageInput<'a> {
    /// Validated source metadata for this adjacent edge.
    pub source_header: &'a SessionFormatHeader,
    /// Validated target metadata produced by this edge's header migration.
    pub target_header: &'a SessionFormatHeader,
    /// Inherited prefix length in source coordinates, or `None` until body decoding completes.
    pub source_inherited_event_count: Option<u64>,
    pub source_kind: SessionFormatSourceKind,
}

/// One independently maintained adjacent streaming migration.
pub trait SessionFormatMigration: Send + Sync {
    fn name(&self) -> &str;
    // The source field is `fromVersion`; the name is kept.
    #[allow(clippy::wrong_self_convention)]
    fn from_version(&self) -> u64;
    fn to_version(&self) -> u64;
    /// Convert one header without reading event bodies.
    fn migrate_header(
        &self,
        header: SessionFormatHeader,
    ) -> Result<SessionFormatHeader, SessionFormatError>;
    /// Create the stateful body stage for one source artifact.
    fn create_stage(
        &self,
        input: SessionFormatMigrationStageInput<'_>,
    ) -> Result<Box<dyn SessionFormatMigrationStage>, SessionFormatError>;
    /// Refuse any header that the adjacent target writer cannot emit.
    fn validate_target_header(
        &self,
        header: &SessionFormatHeader,
    ) -> Result<(), SessionFormatError>;
}

/// Restore and validate a detached current header without reading event bodies.
pub type SessionFormatHeaderRestorer = Arc<
    dyn Fn(SessionFormatHeader) -> Result<SessionFormatHeader, SessionFormatError> + Send + Sync,
>;

/// Restore and validate a complete (or transformed) current artifact.
pub type SessionFormatArtifactRestorer = Arc<
    dyn Fn(SessionFormatArtifact) -> Result<SessionFormatArtifact, SessionFormatError>
        + Send
        + Sync,
>;

/// Inputs that compile the unique complete migration chain.
#[derive(Clone)]
pub struct SessionFormatChainOptions {
    pub current_version: u64,
    pub migrations: Vec<Arc<dyn SessionFormatMigration>>,
    pub restore_current_header: SessionFormatHeaderRestorer,
}

/// Physical-row failure policy selected once for one restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFormatRecovery {
    Strict,
    Recoverable,
}

/// Pure physical JSON codec for one Session version's supported representations.
pub trait SessionFormatCodec: Send + Sync {
    fn version(&self) -> u64;
    /// Decode one physical header into body-independent logical metadata.
    fn decode_header(
        &self,
        value: &serde_json::Value,
    ) -> Result<SessionFormatHeader, SessionFormatError>;
    /// Create one row-at-a-time decoder with an explicit failure policy.
    fn create_decoder(
        &self,
        header_value: &serde_json::Value,
        recovery: SessionFormatRecovery,
    ) -> Result<Box<dyn SessionFormatArtifactDecoder>, SessionFormatError>;
}

/// Stateful physical-row decoder used by streaming persistence restores.
pub trait SessionFormatArtifactDecoder: Send {
    fn header(&self) -> &SessionFormatHeader;
    /// Inherited cut known before body decoding; current formats may derive it at EOF.
    fn header_inherited_event_count(&self) -> Option<u64> {
        None
    }
    /// Decode one physical row and synchronously emit its events or compact run.
    fn decode_row(
        &mut self,
        row_value: serde_json::Value,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<(), SessionFormatError>;
    /// Finish row validation and return the exact inherited cut.
    fn finish(
        &mut self,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<u64, SessionFormatError>;
}

/// Stateless physical record encoder for the installed current format.
pub trait SessionFormatCurrentEncoder: Send + Sync {
    /// Encode the physical header record for one current artifact.
    fn encode_header(
        &self,
        header: &SessionFormatHeader,
        inherited_event_count: u64,
    ) -> Result<SessionFormatJsonObject, SessionFormatError>;
    /// Encode one current logical event as one physical record.
    fn encode_event(
        &self,
        event: &SessionFormatEvent,
    ) -> Result<SessionFormatJsonObject, SessionFormatError>;
}

/// A codec-owned compact run that adjacent migrations may consume without expanding.
pub trait SessionFormatEventRun: Any + Send {
    fn run_type(&self) -> &str;
    fn first_seq(&self) -> u64;
    fn event_count(&self) -> u64;
    /// Expand the run for a migration that has no direct handler.
    fn expand(self: Box<Self>) -> Box<dyn Iterator<Item = SessionFormatEvent>>;
}

/// Synchronous output channel owned by a compiled migration stream.
pub trait SessionFormatMigrationContext {
    /// Deliver one settled event to the next stage before returning.
    fn emit_event(&mut self, event: SessionFormatEvent) -> Result<(), SessionFormatError>;
    /// Deliver one compact event run to the next stage before returning.
    fn emit_run(&mut self, run: Box<dyn SessionFormatEventRun>) -> Result<(), SessionFormatError>;
}

/// Stateful adjacent migration stage used by streaming persistence restores.
pub trait SessionFormatMigrationStage: Send {
    /// Target inherited cut when it is unchanged and known before EOF.
    fn header_inherited_event_count(&self) -> Option<u64> {
        None
    }
    /// Transform one source event and synchronously emit every settled target item.
    fn transform_event(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<(), SessionFormatError>;
    /// Transform one compact source run without requiring an intermediate expansion array.
    fn transform_run(
        &mut self,
        run: Box<dyn SessionFormatEventRun>,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<(), SessionFormatError>;
    /// Emit trailing target items and return the exact target inherited cut.
    fn finish(
        &mut self,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<u64, SessionFormatError>;
}

/// Header-only classification that never inspects event rows.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionFormatHeaderReadResult {
    /// The stored version is current.
    Current {
        stored_version: u64,
        target_version: u64,
        /// Latest logical header. The exact inherited cut requires a body read.
        header: SessionFormatHeader,
    },
    /// The stored version is older and must be migrated.
    MigrationRequired {
        stored_version: u64,
        target_version: u64,
        header: SessionFormatHeader,
    },
    Unsupported {
        stored_version: u64,
        target_version: u64,
        reason: String,
    },
    Malformed {
        stored_version: Option<u64>,
        target_version: u64,
        reason: String,
    },
}

impl SessionFormatHeaderReadResult {
    /// The `status` string of the source: `current`, `migration-required`, `unsupported`, `malformed`.
    pub fn status(&self) -> &'static str {
        match self {
            Self::Current { .. } => "current",
            Self::MigrationRequired { .. } => "migration-required",
            Self::Unsupported { .. } => "unsupported",
            Self::Malformed { .. } => "malformed",
        }
    }
}

/// Inputs for a build-static physical codec and migration catalog.
#[derive(Clone)]
pub struct SessionFormatCatalogOptions {
    pub current_version: u64,
    pub migrations: Vec<Arc<dyn SessionFormatMigration>>,
    /// Restore and validate a detached current header without reading event bodies.
    pub restore_current_header: SessionFormatHeaderRestorer,
    pub codecs: Vec<Arc<dyn SessionFormatCodec>>,
    /// Restore and validate a complete current artifact.
    pub restore_current: SessionFormatArtifactRestorer,
    /// Encode current records without materializing an artifact-sized row array.
    pub current_encoder: Arc<dyn SessionFormatCurrentEncoder>,
    /// Validate an exclusively owned transformed artifact.
    pub restore_transformed_current: SessionFormatArtifactRestorer,
}

/// Which validation a restore applies to a migrated or current artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFormatValidation {
    /// Apply released current-format validation only after migration; current input receives only
    /// codec validation.
    Transformed,
    /// Apply all installed current-format validation.
    Current,
}

/// Policies applied by one physical-row restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFormatRestoreOptions {
    pub recovery: SessionFormatRecovery,
    pub validation: SessionFormatValidation,
}
