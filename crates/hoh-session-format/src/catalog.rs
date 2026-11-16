use std::sync::Arc;

use hoh_util_values::js_string;
use indexmap::IndexMap;
use serde_json::Value;

use crate::chain::{SessionFormatChain, SessionFormatMigrationStream, create_session_format_chain};
use crate::context::SessionFormatEventCollector;
use crate::error::SessionFormatError;
use crate::json::{
    inspect_header_version, inspect_session_format_version, session_format_version_labeled,
    snapshot_session_format_header_labeled,
};
use crate::types::{
    SessionFormatArtifact, SessionFormatArtifactDecoder, SessionFormatArtifactRestorer,
    SessionFormatCatalogOptions, SessionFormatChainOptions, SessionFormatCodec,
    SessionFormatCurrentEncoder, SessionFormatEvent, SessionFormatHeader,
    SessionFormatHeaderReadResult, SessionFormatJsonObject, SessionFormatMigrationContext,
    SessionFormatRestoreOptions, SessionFormatValidation,
};

/// Compile a build-static physical codec and adjacent migration catalog.
pub fn create_session_format_catalog(
    options: SessionFormatCatalogOptions,
) -> Result<SessionFormatCatalog, SessionFormatError> {
    let chain = create_session_format_chain(SessionFormatChainOptions {
        current_version: options.current_version,
        migrations: options.migrations,
        restore_current_header: options.restore_current_header,
    })?;
    let mut codecs: IndexMap<u64, Arc<dyn SessionFormatCodec>> = IndexMap::new();
    for codec in options.codecs {
        let version = session_format_version_labeled(
            &Value::from(codec.version()),
            "Session format codec version",
        )?;
        if codecs.contains_key(&version) {
            return Err(SessionFormatError::new(format!(
                "Session format codec v{version} is duplicated"
            )));
        }
        codecs.insert(version, codec);
    }
    for version in 0..=chain.current_version() {
        if !codecs.contains_key(&version) {
            return Err(SessionFormatError::new(format!(
                "Session format codec v{version} is missing"
            )));
        }
    }
    if codecs.len() as u64 != chain.current_version() + 1 {
        // More codecs than versions 0..=current, so one key is newer than current.
        let invalid = codecs
            .keys()
            .copied()
            .find(|version| *version > chain.current_version())
            .unwrap_or(chain.current_version());
        return Err(SessionFormatError::new(format!(
            "Session format codec v{invalid} is newer than current v{}",
            chain.current_version()
        )));
    }
    Ok(SessionFormatCatalog {
        chain,
        codecs,
        current_encoder: options.current_encoder,
        restore_current: options.restore_current,
        restore_transformed_current: options.restore_transformed_current,
    })
}

/// Build-static physical dispatch and adjacent migration catalog.
pub struct SessionFormatCatalog {
    chain: SessionFormatChain,
    codecs: IndexMap<u64, Arc<dyn SessionFormatCodec>>,
    current_encoder: Arc<dyn SessionFormatCurrentEncoder>,
    restore_current: SessionFormatArtifactRestorer,
    restore_transformed_current: SessionFormatArtifactRestorer,
}

impl SessionFormatCatalog {
    pub fn current_version(&self) -> u64 {
        self.chain.current_version()
    }

    /// Classify and translate one header without reading event rows.
    pub fn read_header(&self, header_value: &Value) -> SessionFormatHeaderReadResult {
        let current_version = self.chain.current_version();
        let stored_version = match inspect_session_format_version(header_value) {
            Ok(version) => version,
            Err(error) => return malformed(current_version, &error, None),
        };
        if stored_version > current_version {
            return SessionFormatHeaderReadResult::Unsupported {
                stored_version,
                target_version: current_version,
                reason: format!(
                    "stored Session uses newer format v{stored_version}; this build writes v{current_version}"
                ),
            };
        }
        let Some(codec) = self.codecs.get(&stored_version) else {
            // Construction proves every supported version has exactly one codec.
            return SessionFormatHeaderReadResult::Unsupported {
                stored_version,
                target_version: current_version,
                reason: format!("this build has no Session format codec for v{stored_version}"),
            };
        };
        let migrated = codec
            .decode_header(header_value)
            .and_then(|decoded| {
                snapshot_session_format_header_labeled(
                    &decoded,
                    &format!("format v{stored_version} header"),
                )
            })
            .and_then(|decoded| self.chain.migrate_header(&decoded));
        match migrated {
            Ok(header) if stored_version == current_version => {
                SessionFormatHeaderReadResult::Current {
                    stored_version,
                    target_version: current_version,
                    header,
                }
            }
            Ok(header) => SessionFormatHeaderReadResult::MigrationRequired {
                stored_version,
                target_version: current_version,
                header,
            },
            Err(error) if error.is_unsupported_migration() => {
                SessionFormatHeaderReadResult::Unsupported {
                    stored_version,
                    target_version: current_version,
                    reason: error.to_string(),
                }
            }
            Err(error) => malformed(current_version, &error, Some(stored_version)),
        }
    }

    fn artifact_codec(
        &self,
        header_value: &Value,
    ) -> Result<(u64, &Arc<dyn SessionFormatCodec>), SessionFormatError> {
        let stored_version = inspect_session_format_version(header_value)?;
        if stored_version > self.chain.current_version() {
            return Err(SessionFormatError::unsupported_migration(format!(
                "stored Session uses newer format v{stored_version}; this build writes v{}",
                self.chain.current_version()
            )));
        }
        let codec = self.codecs.get(&stored_version).ok_or_else(|| {
            // Construction proves every supported version has exactly one codec.
            SessionFormatError::unsupported_migration(format!(
                "this build has no Session format codec for v{stored_version}"
            ))
        })?;
        Ok((stored_version, codec))
    }

    /// Encode one current physical header record.
    pub fn encode_current_header(
        &self,
        header: &SessionFormatHeader,
        inherited_event_count: u64,
    ) -> Result<SessionFormatJsonObject, SessionFormatError> {
        if inspect_header_version(header)? != self.chain.current_version() {
            return Err(SessionFormatError::new(format!(
                "encodeCurrent requires Session format v{}",
                self.chain.current_version()
            )));
        }
        let encoded = self
            .current_encoder
            .encode_header(header, inherited_event_count)?;
        if inspect_header_version(&encoded)? != self.chain.current_version() {
            return Err(SessionFormatError::new(
                "current Session codec returned a non-current header",
            ));
        }
        Ok(encoded)
    }

    /// Encode one current physical event record.
    pub fn encode_current_event(
        &self,
        event: &SessionFormatEvent,
    ) -> Result<SessionFormatJsonObject, SessionFormatError> {
        self.current_encoder.encode_event(event)
    }

    /// Create one single-pass physical-row restore into current logical events.
    pub fn create_restore(
        &self,
        header_value: &Value,
        options: SessionFormatRestoreOptions,
    ) -> Result<SessionFormatRestore, SessionFormatError> {
        let (stored_version, codec) = self.artifact_codec(header_value)?;
        let decoder = codec.create_decoder(header_value, options.recovery)?;
        let source_cut = decoder.header_inherited_event_count();
        let current_version = self.chain.current_version();
        if stored_version == current_version {
            return Ok(SessionFormatRestore {
                kind: RestoreKind::Current(CurrentSessionFormatRestore {
                    decoder,
                    source_inherited_event_count: source_cut,
                    restore_artifact: match options.validation {
                        SessionFormatValidation::Current => Some(Arc::clone(&self.restore_current)),
                        SessionFormatValidation::Transformed => None,
                    },
                    current_version,
                    collector: SessionFormatEventCollector::new(),
                }),
            });
        }
        let migration = self.chain.create_stream(
            decoder.header().clone(),
            source_cut,
            SessionFormatEventCollector::new(),
        )?;
        Ok(SessionFormatRestore {
            kind: RestoreKind::Migrating(MigratingSessionFormatRestore {
                header: migration.header().clone(),
                decoder,
                source_inherited_event_count: source_cut,
                migration,
                restore_artifact: match options.validation {
                    SessionFormatValidation::Current => Arc::clone(&self.restore_current),
                    SessionFormatValidation::Transformed => {
                        Arc::clone(&self.restore_transformed_current)
                    }
                },
                validation: options.validation,
                source_version: stored_version,
                current_version,
            }),
        })
    }
}

/// One caller-owned physical-row restore whose final value is a current logical artifact.
pub struct SessionFormatRestore {
    kind: RestoreKind,
}

enum RestoreKind {
    Current(CurrentSessionFormatRestore),
    Migrating(MigratingSessionFormatRestore),
}

impl SessionFormatRestore {
    /// Current logical header available before body decoding.
    pub fn header(&self) -> &SessionFormatHeader {
        match &self.kind {
            RestoreKind::Current(restore) => restore.decoder.header(),
            RestoreKind::Migrating(restore) => &restore.header,
        }
    }

    /// Decode one physical row in file order.
    pub fn decode_row(&mut self, row_value: Value) -> Result<(), SessionFormatError> {
        match &mut self.kind {
            RestoreKind::Current(restore) => restore.decode_row(row_value),
            RestoreKind::Migrating(restore) => restore.decode_row(row_value),
        }
    }

    /// Finish every decoder and migration stage and return the current artifact.
    pub fn finish(self) -> Result<SessionFormatArtifact, SessionFormatError> {
        match self.kind {
            RestoreKind::Current(restore) => restore.finish(),
            RestoreKind::Migrating(restore) => restore.finish(),
        }
    }
}

struct CurrentSessionFormatRestore {
    decoder: Box<dyn SessionFormatArtifactDecoder>,
    source_inherited_event_count: Option<u64>,
    /// `None` is the identity restorer of physical-only validation.
    restore_artifact: Option<SessionFormatArtifactRestorer>,
    current_version: u64,
    collector: SessionFormatEventCollector,
}

impl CurrentSessionFormatRestore {
    fn decode_row(&mut self, row_value: Value) -> Result<(), SessionFormatError> {
        self.decoder.decode_row(row_value, &mut self.collector)
    }

    fn finish(mut self) -> Result<SessionFormatArtifact, SessionFormatError> {
        let inherited_event_count = finish_decoder(
            self.decoder.as_mut(),
            &mut self.collector,
            self.source_inherited_event_count,
        )?;
        // The source's artifact holds the decoder's own header object: take its final state.
        let artifact = SessionFormatArtifact {
            header: self.decoder.header().clone(),
            inherited_event_count,
            events: self.collector.values,
        };
        let restored = match &self.restore_artifact {
            Some(restore) => restore(artifact)?,
            None => artifact,
        };
        restore_current_version(restored, self.current_version)
    }
}

struct MigratingSessionFormatRestore {
    header: SessionFormatHeader,
    decoder: Box<dyn SessionFormatArtifactDecoder>,
    source_inherited_event_count: Option<u64>,
    migration: SessionFormatMigrationStream<SessionFormatEventCollector>,
    restore_artifact: SessionFormatArtifactRestorer,
    validation: SessionFormatValidation,
    source_version: u64,
    current_version: u64,
}

impl MigratingSessionFormatRestore {
    fn decode_row(&mut self, row_value: Value) -> Result<(), SessionFormatError> {
        self.decoder.decode_row(row_value, &mut self.migration)
    }

    fn finish(mut self) -> Result<SessionFormatArtifact, SessionFormatError> {
        finish_decoder(
            self.decoder.as_mut(),
            &mut self.migration,
            self.source_inherited_event_count,
        )?;
        let inherited_event_count = self.migration.finish()?;
        let artifact = SessionFormatArtifact {
            header: self.header,
            inherited_event_count,
            events: self.migration.into_output().values,
        };
        let restored = match (self.restore_artifact)(artifact) {
            Ok(restored) => restored,
            Err(error) => {
                if self.validation == SessionFormatValidation::Current
                    || error.is_unsupported_migration()
                {
                    return Err(error);
                }
                let detail = error.to_string();
                return Err(SessionFormatError::UnsupportedMigration {
                    message: format!(
                        "Session migration from v{} to v{} refuses the transformed artifact: {detail}",
                        self.source_version, self.current_version
                    ),
                    cause: Some(Box::new(error)),
                });
            }
        };
        restore_current_version(restored, self.current_version)
    }
}

fn finish_decoder(
    decoder: &mut dyn SessionFormatArtifactDecoder,
    context: &mut dyn SessionFormatMigrationContext,
    source_inherited_event_count: Option<u64>,
) -> Result<u64, SessionFormatError> {
    let inherited_event_count = decoder.finish(context)?;
    if source_inherited_event_count.is_some_and(|declared| declared != inherited_event_count) {
        return Err(SessionFormatError::new(
            "streaming decoder changed its predeclared inherited cut",
        ));
    }
    Ok(inherited_event_count)
}

fn restore_current_version(
    artifact: SessionFormatArtifact,
    current_version: u64,
) -> Result<SessionFormatArtifact, SessionFormatError> {
    let version = artifact.header.get("version");
    let matches_current = version
        .and_then(|value| session_format_version_labeled(value, "version").ok())
        .is_some_and(|value| value == current_version);
    if !matches_current {
        return Err(SessionFormatError::new(format!(
            "current Session restorer returned v{}; expected v{current_version}",
            version.map_or_else(|| "undefined".to_owned(), js_string)
        )));
    }
    Ok(artifact)
}

fn malformed(
    target_version: u64,
    error: &SessionFormatError,
    stored_version: Option<u64>,
) -> SessionFormatHeaderReadResult {
    SessionFormatHeaderReadResult::Malformed {
        stored_version,
        target_version,
        reason: error.to_string(),
    }
}
