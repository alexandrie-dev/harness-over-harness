//! SQLite FTS5-backed session query provider.

mod provider;
pub mod query;
pub mod schema;

use hoh_error::BoxError;
use hoh_kernel::{Context, Inject, Service, Undo, plugin};
use hoh_session::Sessions;
use hoh_session_query::{
    SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY,
    SESSION_QUERY_DEFAULT_PREPARED_SESSION_CACHE_SIZE, SESSION_QUERY_READ_WINDOW_MAX,
    mount_session_query,
};
use provider::SqliteSearchProvider;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use schema::{
    JournalMode, SESSION_QUERY_SQLITE_APPLICATION_ID, SESSION_QUERY_SQLITE_SCHEMA_VERSION,
};

pub const SESSION_QUERY_SQLITE_PATH_KEY: &str = "launcherSessionQueryPath";
pub const SESSION_QUERY_SQLITE_DEFAULT_LIMIT: u64 = 20;
pub const SESSION_QUERY_SQLITE_MAX_LIMIT: u64 = 100;
pub const SESSION_QUERY_SQLITE_SNIPPET_CHARS: u64 = 240;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OpenAt {
    #[default]
    Startup,
    FirstSearch,
    Never,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub path: String,
    #[serde(default)]
    pub open_at: OpenAt,
    #[serde(default)]
    pub journal_mode: JournalMode,
    #[serde(default = "default_limit")]
    pub default_limit: u64,
    #[serde(default = "max_limit")]
    pub max_limit: u64,
    #[serde(default = "snippet_chars")]
    pub snippet_chars: u64,
    #[serde(default = "read_window_max")]
    pub read_window_max: u64,
    #[serde(default = "persisted_read_concurrency")]
    pub persisted_read_concurrency: usize,
    #[serde(default = "prepared_session_cache_size")]
    pub prepared_session_cache_size: usize,
}

const fn default_limit() -> u64 {
    SESSION_QUERY_SQLITE_DEFAULT_LIMIT
}
const fn max_limit() -> u64 {
    SESSION_QUERY_SQLITE_MAX_LIMIT
}
const fn snippet_chars() -> u64 {
    SESSION_QUERY_SQLITE_SNIPPET_CHARS
}
const fn read_window_max() -> u64 {
    SESSION_QUERY_READ_WINDOW_MAX
}
const fn persisted_read_concurrency() -> usize {
    SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY
}
const fn prepared_session_cache_size() -> usize {
    SESSION_QUERY_DEFAULT_PREPARED_SESSION_CACHE_SIZE
}

impl JsonSchema for Config {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Config".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::try_from(serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["path"],
            "properties": {
                "path": { "type": "string" },
                "openAt": { "enum": ["startup", "first-search", "never"], "default": "startup" },
                "journalMode": { "enum": ["wal", "delete", "truncate", "persist"], "default": "wal" },
                "defaultLimit": { "type": "integer", "minimum": 1, "maximum": 9_007_199_254_740_990_u64, "multipleOf": 1, "default": SESSION_QUERY_SQLITE_DEFAULT_LIMIT },
                "maxLimit": { "type": "integer", "minimum": 1, "maximum": 9_007_199_254_740_990_u64, "multipleOf": 1, "default": SESSION_QUERY_SQLITE_MAX_LIMIT },
                "snippetChars": { "type": "integer", "minimum": 1, "multipleOf": 1, "default": SESSION_QUERY_SQLITE_SNIPPET_CHARS },
                "readWindowMax": { "type": "integer", "minimum": 0, "multipleOf": 1, "default": SESSION_QUERY_READ_WINDOW_MAX },
                "persistedReadConcurrency": { "type": "integer", "minimum": 1, "maximum": 9_007_199_254_740_991_u64, "multipleOf": 1, "default": SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY },
                "preparedSessionCacheSize": { "type": "integer", "minimum": 1, "maximum": 9_007_199_254_740_991_u64, "multipleOf": 1, "default": SESSION_QUERY_DEFAULT_PREPARED_SESSION_CACHE_SIZE }
            }
        }))
        .unwrap_or_else(|_| Schema::from(true))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: String::new(),
            open_at: OpenAt::Startup,
            journal_mode: JournalMode::Wal,
            default_limit: default_limit(),
            max_limit: max_limit(),
            snippet_chars: snippet_chars(),
            read_window_max: read_window_max(),
            persisted_read_concurrency: persisted_read_concurrency(),
            prepared_session_cache_size: prepared_session_cache_size(),
        }
    }
}

impl Config {
    fn validate(&self) -> Result<(), BoxError> {
        if hoh_util_values::js_trim(&self.path).is_empty() {
            return Err(invalid_config("path must not be blank"));
        }
        for (name, value) in [
            ("defaultLimit", self.default_limit),
            ("maxLimit", self.max_limit),
        ] {
            if value == 0 || value > query::SQLITE_MAX_PAGE_LIMIT {
                return Err(invalid_config(format!(
                    "{name} must be an integer between 1 and {}",
                    query::SQLITE_MAX_PAGE_LIMIT
                )));
            }
        }
        if self.snippet_chars == 0 {
            return Err(invalid_config("snippetChars must be a positive integer"));
        }
        if self.persisted_read_concurrency == 0
            || self.persisted_read_concurrency > hoh_session::MAX_SAFE_INTEGER as usize
        {
            return Err(invalid_config(
                "persistedReadConcurrency must be a positive safe integer",
            ));
        }
        if self.prepared_session_cache_size == 0
            || self.prepared_session_cache_size > hoh_session::MAX_SAFE_INTEGER as usize
        {
            return Err(invalid_config(
                "preparedSessionCacheSize must be a positive safe integer",
            ));
        }
        if self.default_limit > self.max_limit {
            return Err(invalid_config(
                "defaultLimit must be less than or equal to maxLimit",
            ));
        }
        Ok(())
    }

    pub(crate) fn limits(&self) -> query::QueryLimits {
        query::QueryLimits {
            default_limit: self.default_limit,
            max_limit: self.max_limit,
        }
    }
}

fn invalid_config(detail: impl std::fmt::Display) -> BoxError {
    Box::new(hoh_session_query::SessionQueryError::new(
        format!("session-search SQLite config: {detail}"),
        hoh_session_query::SessionQueryErrorCode::InvalidConfig,
    ))
}

#[derive(Inject)]
pub struct Deps {
    sessions: Service<Sessions>,
}

#[plugin(SessionQuerySqlite, name = "session-query-sqlite")]
pub async fn apply(
    ctx: Context,
    deps: Deps,
    config: hoh_kernel::Cfg<Config>,
) -> Result<(), BoxError> {
    let config = config.get();
    config.validate()?;
    let provider = Arc::new(SqliteSearchProvider::new(
        ctx.clone(),
        (*deps.sessions).clone(),
        (*config).clone(),
    ));
    if config.open_at == OpenAt::Startup {
        provider.open_startup().await?;
    }
    mount_session_query(
        &ctx,
        deps.sessions,
        provider.clone(),
        hoh_session_query::Config {
            read_window_max: config.read_window_max,
            persisted_read_concurrency: config.persisted_read_concurrency,
            prepared_session_cache_size: config.prepared_session_cache_size,
        },
    )
    .await?;
    ctx.effect("sessionQuerySqlite.close", || {
        Undo::future(move || async move {
            provider.close().await;
            Ok(())
        })
    })?;
    Ok(())
}
