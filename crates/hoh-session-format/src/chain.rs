use std::sync::Arc;

use indexmap::{IndexMap, IndexSet};
use serde_json::Value;

use crate::error::SessionFormatError;
use crate::json::{
    inspect_header_version, session_format_count, session_format_count_u64,
    session_format_version_labeled, snapshot_session_format_header_labeled,
};
use crate::types::{
    SessionFormatChainOptions, SessionFormatEvent, SessionFormatEventRun, SessionFormatHeader,
    SessionFormatHeaderRestorer, SessionFormatMigration, SessionFormatMigrationContext,
    SessionFormatMigrationStage, SessionFormatMigrationStageInput, SessionFormatSourceKind,
};

/// Validate one adjacent migration declaration and return it.
pub fn define_session_format_migration(
    migration: Arc<dyn SessionFormatMigration>,
) -> Result<Arc<dyn SessionFormatMigration>, SessionFormatError> {
    if migration.name().is_empty() {
        return Err(SessionFormatError::new(
            "Session migration name must be a non-empty string",
        ));
    }
    let name = migration.name();
    let from = session_format_version_labeled(
        &Value::from(migration.from_version()),
        &format!("{name} fromVersion"),
    )?;
    let to = session_format_version_labeled(
        &Value::from(migration.to_version()),
        &format!("{name} toVersion"),
    )?;
    if to != from + 1 {
        return Err(SessionFormatError::new(format!(
            "{name} must declare adjacent v{from}->v{}",
            from + 1
        )));
    }
    Ok(migration)
}

/// Compile a unique, complete adjacent migration chain.
pub fn create_session_format_chain(
    options: SessionFormatChainOptions,
) -> Result<SessionFormatChain, SessionFormatError> {
    SessionFormatChain::new(options)
}

/// Pure adjacent planner and streaming migration compiler.
pub struct SessionFormatChain {
    current_version: u64,
    migrations: Vec<Arc<dyn SessionFormatMigration>>,
    restore_current_header: SessionFormatHeaderRestorer,
}

impl SessionFormatChain {
    fn new(options: SessionFormatChainOptions) -> Result<Self, SessionFormatError> {
        let current_version = session_format_version_labeled(
            &Value::from(options.current_version),
            "current Session format version",
        )?;
        let mut by_from: IndexMap<u64, Arc<dyn SessionFormatMigration>> = IndexMap::new();
        let mut names: IndexSet<String> = IndexSet::new();
        for candidate in options.migrations {
            let migration = define_session_format_migration(candidate)?;
            if by_from.contains_key(&migration.from_version()) {
                return Err(SessionFormatError::new(format!(
                    "Session migration v{}->v{} is duplicated",
                    migration.from_version(),
                    migration.to_version()
                )));
            }
            if names.contains(migration.name()) {
                return Err(SessionFormatError::new(format!(
                    "Session migration name {} is duplicated",
                    json_string(migration.name())
                )));
            }
            names.insert(migration.name().to_owned());
            by_from.insert(migration.from_version(), migration);
        }
        let mut ordered = Vec::new();
        for version in 0..current_version {
            let Some(migration) = by_from.get(&version) else {
                return Err(SessionFormatError::unsupported_migration(format!(
                    "Session migration v{version}->v{} is missing",
                    version + 1
                )));
            };
            ordered.push(Arc::clone(migration));
        }
        if by_from.len() != ordered.len() {
            // `by_from` holds more entries than versions 0..current, so one key is >= current.
            let invalid = by_from
                .keys()
                .copied()
                .find(|version| *version >= current_version)
                .unwrap_or(current_version);
            return Err(SessionFormatError::new(format!(
                "Session migration from v{invalid} does not lead to current v{current_version}"
            )));
        }
        Ok(Self {
            current_version,
            migrations: ordered,
            restore_current_header: options.restore_current_header,
        })
    }

    pub fn current_version(&self) -> u64 {
        self.current_version
    }

    fn plan(
        &self,
        from_version: &Value,
    ) -> Result<&[Arc<dyn SessionFormatMigration>], SessionFormatError> {
        let from = session_format_version_labeled(from_version, "stored Session format version")?;
        if from > self.current_version {
            return Err(SessionFormatError::unsupported_migration(format!(
                "stored Session uses newer format v{from}; this build writes v{}",
                self.current_version
            )));
        }
        // `from <= current_version == migrations.len()`.
        Ok(&self.migrations[from as usize..])
    }

    /// Compile the complete migration stage chain for one decoded source artifact. The stream owns
    /// `output` and hands it back through `into_output`.
    pub fn create_stream<O: SessionFormatMigrationContext>(
        &self,
        source_header: SessionFormatHeader,
        source_cut: Option<u64>,
        output: O,
    ) -> Result<SessionFormatMigrationStream<O>, SessionFormatError> {
        let mut header = source_header;
        let validated_source_cut = source_cut
            .map(|cut| session_format_count_u64(cut, "Session inherited event count"))
            .transpose()?;
        let mut inherited_event_count = validated_source_cut;
        let mut stages: Vec<CompiledMigrationStage> = Vec::new();
        let plan = self.plan(header.get("version").unwrap_or(&Value::Null))?;
        for (index, migration) in plan.iter().enumerate() {
            let target_header = advance_header(migration.as_ref(), &header)?;
            let stage = migration
                .create_stage(SessionFormatMigrationStageInput {
                    source_header: &header,
                    target_header: &target_header,
                    source_inherited_event_count: inherited_event_count,
                    source_kind: if index == 0 {
                        SessionFormatSourceKind::Decoded
                    } else {
                        SessionFormatSourceKind::Transformed
                    },
                })
                .map_err(|error| unsupported_refusal(migration.as_ref(), error, "Session"))?;
            header = target_header;
            inherited_event_count = stage.header_inherited_event_count();
            stages.push(CompiledMigrationStage {
                migration: Arc::clone(migration),
                stage,
            });
        }
        Ok(SessionFormatMigrationStream {
            header,
            source_inherited_event_count: validated_source_cut,
            stages,
            output,
        })
    }

    /// Convert only a supported header to the current logical representation.
    pub fn migrate_header(
        &self,
        source: &SessionFormatHeader,
    ) -> Result<SessionFormatHeader, SessionFormatError> {
        let mut current = snapshot_session_format_header_labeled(source, "stored Session header")?;
        for migration in self.plan(current.get("version").unwrap_or(&Value::Null))? {
            current = advance_header(migration.as_ref(), &current)?;
        }
        let restored = (self.restore_current_header)(current)?;
        let current = snapshot_session_format_header_labeled(
            &restored,
            "current Session header restoration",
        )?;
        let version = inspect_header_version(&current)?;
        if version != self.current_version {
            return Err(SessionFormatError::new(format!(
                "current Session header restorer returned v{version}; expected v{}",
                self.current_version
            )));
        }
        Ok(current)
    }
}

fn advance_header(
    migration: &dyn SessionFormatMigration,
    source: &SessionFormatHeader,
) -> Result<SessionFormatHeader, SessionFormatError> {
    let name = migration.name();
    let target = snapshot_session_format_header_labeled(source, &format!("{name} header input"))
        .and_then(|input| migration.migrate_header(input))
        .map_err(|error| unsupported_refusal(migration, error, "Session header"))?;
    let current =
        snapshot_session_format_header_labeled(&target, &format!("{name} header output"))?;
    let version = inspect_header_version(&current)?;
    if version != migration.to_version() {
        return Err(SessionFormatError::new(format!(
            "{name} header returned v{version}; expected v{}",
            migration.to_version()
        )));
    }
    migration
        .validate_target_header(&current)
        .map_err(|error| unsupported_refusal(migration, error, "Session header"))?;
    Ok(current)
}

struct CompiledMigrationStage {
    migration: Arc<dyn SessionFormatMigration>,
    stage: Box<dyn SessionFormatMigrationStage>,
}

/// The rest of the chain after some stage: the remaining stages and the final output.
struct Downstream<'a, O: SessionFormatMigrationContext> {
    stages: &'a mut [CompiledMigrationStage],
    output: &'a mut O,
}

impl<O: SessionFormatMigrationContext> SessionFormatMigrationContext for Downstream<'_, O> {
    fn emit_event(&mut self, event: SessionFormatEvent) -> Result<(), SessionFormatError> {
        let Some((entry, rest)) = self.stages.split_first_mut() else {
            return self.output.emit_event(event);
        };
        let mut next = Downstream {
            stages: rest,
            output: &mut *self.output,
        };
        entry
            .stage
            .transform_event(event, &mut next)
            .map_err(|error| unsupported_refusal(entry.migration.as_ref(), error, "Session"))
    }

    fn emit_run(&mut self, run: Box<dyn SessionFormatEventRun>) -> Result<(), SessionFormatError> {
        let Some((entry, rest)) = self.stages.split_first_mut() else {
            return self.output.emit_run(run);
        };
        let mut next = Downstream {
            stages: rest,
            output: &mut *self.output,
        };
        entry
            .stage
            .transform_run(run, &mut next)
            .map_err(|error| unsupported_refusal(entry.migration.as_ref(), error, "Session"))
    }
}

/// One composed migration chain that emits settled current events to its owner.
pub struct SessionFormatMigrationStream<O: SessionFormatMigrationContext> {
    header: SessionFormatHeader,
    source_inherited_event_count: Option<u64>,
    stages: Vec<CompiledMigrationStage>,
    output: O,
}

impl<O: SessionFormatMigrationContext> SessionFormatMigrationStream<O> {
    /// Current logical header after every adjacent header migration.
    pub fn header(&self) -> &SessionFormatHeader {
        &self.header
    }

    /// The output that settled events were delivered to.
    pub fn output(&self) -> &O {
        &self.output
    }

    /// Give back the output after `finish`.
    pub fn into_output(self) -> O {
        self.output
    }

    /// Settle all migration stages and return the exact current inherited cut.
    pub fn finish(&mut self) -> Result<u64, SessionFormatError> {
        let mut inherited_event_count = self.source_inherited_event_count;
        for index in 0..self.stages.len() {
            let (head, tail) = self.stages.split_at_mut(index + 1);
            let entry = &mut head[index];
            let mut next = Downstream {
                stages: tail,
                output: &mut self.output,
            };
            let target_cut = entry
                .stage
                .finish(&mut next)
                .map_err(|error| unsupported_refusal(entry.migration.as_ref(), error, "Session"))?;
            if let Some(declared) = entry.stage.header_inherited_event_count()
                && declared != target_cut
            {
                return Err(SessionFormatError::new(format!(
                    "{} changed its predeclared inherited cut",
                    entry.migration.name()
                )));
            }
            inherited_event_count = Some(target_cut);
        }
        match inherited_event_count {
            Some(cut) => session_format_count_u64(cut, "finished Session inherited event count"),
            None => session_format_count(&Value::Null, "finished Session inherited event count"),
        }
    }
}

impl<O: SessionFormatMigrationContext> SessionFormatMigrationContext
    for SessionFormatMigrationStream<O>
{
    fn emit_event(&mut self, event: SessionFormatEvent) -> Result<(), SessionFormatError> {
        Downstream {
            stages: &mut self.stages,
            output: &mut self.output,
        }
        .emit_event(event)
    }

    fn emit_run(&mut self, run: Box<dyn SessionFormatEventRun>) -> Result<(), SessionFormatError> {
        Downstream {
            stages: &mut self.stages,
            output: &mut self.output,
        }
        .emit_run(run)
    }
}

fn unsupported_refusal(
    migration: &dyn SessionFormatMigration,
    error: SessionFormatError,
    subject: &str,
) -> SessionFormatError {
    if error.is_unsupported_migration() {
        return error;
    }
    let detail = error.to_string();
    SessionFormatError::UnsupportedMigration {
        message: format!(
            "{} refuses this format v{} {subject}: {detail}",
            migration.name(),
            migration.from_version()
        ),
        cause: Some(Box::new(error)),
    }
}

/// `JSON.stringify` of a string.
fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
}
