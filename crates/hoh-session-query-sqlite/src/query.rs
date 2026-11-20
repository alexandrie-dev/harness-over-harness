use hoh_error::BoxError;
use hoh_session_query::{
    SessionAvailability, SessionEventMetadataFilter, SessionEventResultFilter,
    SessionEventSearchRequest, SessionEventSurface, SessionQueryError,
    SessionQueryErrorCode as Code, SessionResultFilter, SessionSearchCursor, SessionSearchRequest,
    materialize_session_event_result_filters, materialize_session_result_filters,
};
use hoh_util_values::{is_js_white_space, js_json_stringify};
use rusqlite::types::Value;
use serde_json::{Map, json};

pub const FTS_HIGHLIGHT_START: char = '\u{fdd0}';
pub const FTS_HIGHLIGHT_END: char = '\u{fdd1}';
pub const SQLITE_MAX_PAGE_LIMIT: u64 = 9_007_199_254_740_990;
pub const SQLITE_PORTABLE_VARIABLE_LIMIT: usize = 32_766;
pub const SQLITE_FTS5_OUTER_PREDICATE_LIMIT: usize = 14;

type Result<T> = std::result::Result<T, BoxError>;

fn error(message: impl Into<String>, code: Code) -> BoxError {
    Box::new(SessionQueryError::new(message, code))
}

pub fn assert_portable_binding_count(count: usize) -> Result<()> {
    if count > SQLITE_PORTABLE_VARIABLE_LIMIT {
        return Err(error(
            format!(
                "session-search request exceeds SQLite's portable {SQLITE_PORTABLE_VARIABLE_LIMIT}-variable limit; reduce filter values"
            ),
            Code::InvalidFilter,
        ));
    }
    Ok(())
}

pub fn assert_fts5_outer_predicate_count(count: usize) -> Result<()> {
    if count > SQLITE_FTS5_OUTER_PREDICATE_LIMIT {
        return Err(error(
            format!(
                "session-search request exceeds the supported SQLite FTS5 outer-predicate budget of {SQLITE_FTS5_OUTER_PREDICATE_LIMIT}; reduce filters"
            ),
            Code::InvalidFilter,
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub struct QueryLimits {
    pub default_limit: u64,
    pub max_limit: u64,
}

#[derive(Clone, Debug)]
pub struct NormalizedSessionRequest {
    pub query: String,
    pub session_filters: Vec<SessionResultFilter>,
    pub event_filters: Vec<SessionEventMetadataFilter>,
    pub limit: u64,
    pub cursor: Option<SessionSearchCursor>,
}

#[derive(Clone, Debug)]
pub struct NormalizedEventRequest {
    pub session_id: hoh_session::SessionId,
    pub query: String,
    pub filters: Vec<SessionEventMetadataFilter>,
    pub limit: u64,
    pub cursor: Option<SessionSearchCursor>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SqlWhere {
    pub sql: String,
    pub params: Vec<Value>,
    pub predicate_count: usize,
}

pub fn normalize_session_request(
    request: SessionSearchRequest,
    limits: QueryLimits,
) -> Result<NormalizedSessionRequest> {
    let session_filters = materialize_session_result_filters(&request.session_filters)?;
    let event_filters = materialize_metadata_filters(&request.event_filters)?;
    let query = normalize_query(&request.query)?;
    let limit = normalize_limit(request.limit, limits)?;
    Ok(NormalizedSessionRequest {
        query,
        session_filters,
        event_filters,
        limit,
        cursor: request.cursor,
    })
}

pub fn normalize_event_request(
    request: SessionEventSearchRequest,
    limits: QueryLimits,
) -> Result<NormalizedEventRequest> {
    let filters = materialize_metadata_filters(&request.filters)?;
    let query = normalize_query(&request.query)?;
    let limit = normalize_limit(request.limit, limits)?;
    Ok(NormalizedEventRequest {
        session_id: request.session_id,
        query,
        filters,
        limit,
        cursor: request.cursor,
    })
}

fn materialize_metadata_filters(
    filters: &[SessionEventMetadataFilter],
) -> Result<Vec<SessionEventMetadataFilter>> {
    let generic = filters
        .iter()
        .map(|filter| match filter {
            SessionEventMetadataFilter::Seq(v) => SessionEventResultFilter::Seq(*v),
            SessionEventMetadataFilter::Time(v) => SessionEventResultFilter::Time(*v),
            SessionEventMetadataFilter::Type(v) => SessionEventResultFilter::Type(v.clone()),
            SessionEventMetadataFilter::Surface(v) => SessionEventResultFilter::Surface(v.clone()),
        })
        .collect::<Vec<_>>();
    materialize_session_event_result_filters(&generic)?
        .into_iter()
        .map(|filter| match filter {
            SessionEventResultFilter::Seq(v) => Ok(SessionEventMetadataFilter::Seq(v)),
            SessionEventResultFilter::Time(v) => Ok(SessionEventMetadataFilter::Time(v)),
            SessionEventResultFilter::Type(v) => Ok(SessionEventMetadataFilter::Type(v)),
            SessionEventResultFilter::Surface(v) => Ok(SessionEventMetadataFilter::Surface(v)),
            SessionEventResultFilter::Text(_) => Err(error(
                "session-search metadata filters do not accept text clauses",
                Code::InvalidFilter,
            )),
        })
        .collect()
}

fn normalize_query(value: &str) -> Result<String> {
    let trimmed = hoh_util_values::js_trim(value);
    let mut query = String::new();
    let mut in_space = false;
    for ch in trimmed.chars() {
        if is_js_white_space(ch) {
            in_space = true;
        } else {
            if in_space && !query.is_empty() {
                query.push(' ');
            }
            in_space = false;
            query.push(ch);
        }
    }
    if query.is_empty() {
        return Err(error(
            "session-search query must contain non-whitespace text",
            Code::InvalidQuery,
        ));
    }
    if query.contains('\0') {
        return Err(error(
            "session-search query must not contain NUL",
            Code::InvalidQuery,
        ));
    }
    Ok(sanitize_fts_text(&query))
}

fn normalize_limit(value: Option<u64>, limits: QueryLimits) -> Result<u64> {
    let limit = value.unwrap_or(limits.default_limit);
    let max = limits.max_limit.min(SQLITE_MAX_PAGE_LIMIT);
    if limit == 0 || limit > max {
        return Err(error(
            format!("session-search limit must be an integer between 1 and {max}"),
            Code::InvalidLimit,
        ));
    }
    Ok(limit)
}

pub fn build_session_where(filters: &[SessionResultFilter]) -> Result<SqlWhere> {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    for filter in filters {
        match filter {
            SessionResultFilter::Id(values) => add_list(
                &mut clauses,
                &mut params,
                "session_id",
                values.iter().map(ToString::to_string).collect(),
            )?,
            SessionResultFilter::Cwd(values) => {
                add_nullable_list(&mut clauses, &mut params, "cwd", values)?
            }
            SessionResultFilter::CreatedAt(range) => {
                add_range(&mut clauses, &mut params, "created_at", range)?
            }
            SessionResultFilter::Parent(values) => {
                let values = values
                    .iter()
                    .map(|v| v.as_ref().map(ToString::to_string))
                    .collect::<Vec<_>>();
                add_nullable_list(&mut clauses, &mut params, "parent_session", &values)?;
            }
            SessionResultFilter::Availability(values) => {
                let mut live = false;
                let mut persisted = false;
                for value in values {
                    match value {
                        SessionAvailability::Live => live = true,
                        SessionAvailability::Persisted => persisted = true,
                    }
                }
                if !live && !persisted {
                    clauses.push("0".to_owned());
                } else if live != persisted {
                    clauses.push(if live { "live = 1" } else { "persisted = 1" }.to_owned());
                }
            }
        }
    }
    assert_fts5_outer_predicate_count(clauses.len())?;
    Ok(SqlWhere {
        predicate_count: clauses.len(),
        sql: clauses.join(" AND "),
        params,
    })
}

pub fn build_event_where(filters: &[SessionEventMetadataFilter]) -> Result<SqlWhere> {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    for filter in filters {
        match filter {
            SessionEventMetadataFilter::Seq(range) => {
                add_range(&mut clauses, &mut params, "seq", range)?
            }
            SessionEventMetadataFilter::Time(range) => {
                add_range(&mut clauses, &mut params, "time", range)?
            }
            SessionEventMetadataFilter::Type(values) => {
                add_list(&mut clauses, &mut params, "type", values.clone())?
            }
            SessionEventMetadataFilter::Surface(values) => add_list(
                &mut clauses,
                &mut params,
                "surface",
                values.iter().map(surface_name).map(str::to_owned).collect(),
            )?,
        }
    }
    assert_fts5_outer_predicate_count(clauses.len())?;
    Ok(SqlWhere {
        predicate_count: clauses.len(),
        sql: clauses.join(" AND "),
        params,
    })
}

fn add_list(
    clauses: &mut Vec<String>,
    params: &mut Vec<Value>,
    column: &str,
    values: Vec<String>,
) -> Result<()> {
    if values.is_empty() {
        clauses.push("0".to_owned());
        return Ok(());
    }
    assert_portable_binding_count(params.len() + values.len())?;
    clauses.push(format!(
        "{column} IN ({})",
        vec!["?"; values.len()].join(", ")
    ));
    params.extend(values.into_iter().map(Value::Text));
    Ok(())
}

fn add_nullable_list(
    clauses: &mut Vec<String>,
    params: &mut Vec<Value>,
    column: &str,
    values: &[Option<String>],
) -> Result<()> {
    if values.is_empty() {
        clauses.push("0".to_owned());
        return Ok(());
    }
    let concrete = values.iter().filter_map(Clone::clone).collect::<Vec<_>>();
    let mut parts = Vec::new();
    if !concrete.is_empty() {
        assert_portable_binding_count(params.len() + concrete.len())?;
        parts.push(format!(
            "{column} IN ({})",
            vec!["?"; concrete.len()].join(", ")
        ));
        params.extend(concrete.into_iter().map(Value::Text));
    }
    if values.iter().any(Option::is_none) {
        parts.push(format!("{column} IS NULL"));
    }
    clauses.push(format!("({})", parts.join(" OR ")));
    Ok(())
}

fn add_range(
    clauses: &mut Vec<String>,
    params: &mut Vec<Value>,
    column: &str,
    range: &hoh_session_query::SessionResultRange,
) -> Result<()> {
    if let Some(from) = range.from {
        assert_portable_binding_count(params.len() + 1)?;
        clauses.push(format!("CAST({column} AS INTEGER) >= ?"));
        params.push(Value::Real(from));
    }
    if let Some(to) = range.to {
        assert_portable_binding_count(params.len() + 1)?;
        clauses.push(format!("CAST({column} AS INTEGER) <= ?"));
        params.push(Value::Real(to));
    }
    Ok(())
}

pub fn quote_fts_data(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

pub fn sanitize_fts_text(text: &str) -> String {
    text.replace(['\0', FTS_HIGHLIGHT_START, FTS_HIGHLIGHT_END], "�")
}

pub fn make_snippet(marked_text: &str, max_chars: usize) -> String {
    let (clean, match_start) = normalize_marked_text(marked_text);
    let characters = clean.chars().collect::<Vec<_>>();
    if characters.len() <= max_chars {
        return clean;
    }
    if max_chars == 1 {
        return "…".to_owned();
    }
    let matched = match_start.min(characters.len() - 1);
    let mut start = matched.saturating_sub(max_chars / 3);
    let mut prefix = usize::from(start > 0);
    let mut suffix = 1usize;
    let mut content = max_chars.saturating_sub(prefix + suffix);
    if content < 1 {
        start = matched;
        suffix = 0;
        content = max_chars - prefix;
    } else if matched >= start + content {
        start = matched - content + 1;
    }
    let mut end = characters.len().min(start + content);
    if end == characters.len() {
        suffix = 0;
        content = max_chars - prefix;
        start = end.saturating_sub(content);
        prefix = usize::from(start > 0);
    }
    end = characters.len().min(start + content);
    let mut result = String::new();
    if prefix != 0 {
        result.push('…');
    }
    result.extend(&characters[start..end]);
    if suffix != 0 {
        result.push('…');
    }
    result
}

fn normalize_marked_text(marked: &str) -> (String, usize) {
    let mut chars = Vec::new();
    let mut match_start = None;
    for ch in marked.chars() {
        if ch == FTS_HIGHLIGHT_START {
            match_start.get_or_insert(chars.len());
        } else if ch == FTS_HIGHLIGHT_END {
        } else if is_js_white_space(ch) {
            if !chars.is_empty() && chars.last() != Some(&' ') {
                chars.push(' ');
            }
        } else {
            chars.push(ch);
        }
    }
    if chars.last() == Some(&' ') {
        chars.pop();
    }
    (chars.iter().collect(), match_start.unwrap_or(0))
}

pub fn request_fingerprint_session(request: &NormalizedSessionRequest) -> String {
    let mut object = Map::new();
    object.insert("scope".into(), json!("sessions"));
    object.insert("query".into(), json!(request.query));
    object.insert(
        "sessionFilters".into(),
        serde_json::Value::Array(canonical_session_filters(&request.session_filters)),
    );
    object.insert(
        "eventFilters".into(),
        serde_json::Value::Array(canonical_event_filters(&request.event_filters)),
    );
    object.insert("limit".into(), json!(request.limit));
    js_json_stringify(&serde_json::Value::Object(object))
}

pub fn request_fingerprint_event(request: &NormalizedEventRequest) -> String {
    let mut object = Map::new();
    object.insert("scope".into(), json!("events"));
    object.insert("sessionId".into(), json!(request.session_id));
    object.insert("query".into(), json!(request.query));
    object.insert(
        "filters".into(),
        serde_json::Value::Array(canonical_event_filters(&request.filters)),
    );
    object.insert("limit".into(), json!(request.limit));
    js_json_stringify(&serde_json::Value::Object(object))
}

fn canonical_session_filters(filters: &[SessionResultFilter]) -> Vec<serde_json::Value> {
    let mut result = filters
        .iter()
        .map(|filter| match filter {
            SessionResultFilter::Id(v) => list_filter("id", v.iter().map(ToString::to_string)),
            SessionResultFilter::Cwd(v) => nullable_filter("cwd", v),
            SessionResultFilter::CreatedAt(v) => range_filter("created-at", v),
            SessionResultFilter::Parent(v) => {
                let values = v
                    .iter()
                    .map(|x| x.as_ref().map(ToString::to_string))
                    .collect::<Vec<_>>();
                nullable_filter("parent", &values)
            }
            SessionResultFilter::Availability(v) => list_filter(
                "availability",
                v.iter().map(|x| match x {
                    SessionAvailability::Live => "live".to_owned(),
                    SessionAvailability::Persisted => "persisted".to_owned(),
                }),
            ),
        })
        .collect::<Vec<_>>();
    sort_json(&mut result);
    result
}

fn canonical_event_filters(filters: &[SessionEventMetadataFilter]) -> Vec<serde_json::Value> {
    let mut result = filters
        .iter()
        .map(|filter| match filter {
            SessionEventMetadataFilter::Seq(v) => range_filter("seq", v),
            SessionEventMetadataFilter::Time(v) => range_filter("time", v),
            SessionEventMetadataFilter::Type(v) => list_filter("type", v.iter().cloned()),
            SessionEventMetadataFilter::Surface(v) => {
                list_filter("surface", v.iter().map(surface_name).map(str::to_owned))
            }
        })
        .collect::<Vec<_>>();
    sort_json(&mut result);
    result
}

fn list_filter(kind: &str, values: impl Iterator<Item = String>) -> serde_json::Value {
    let mut values = values.collect::<Vec<_>>();
    values.sort();
    json!({"kind": kind, "values": values})
}

fn nullable_filter(kind: &str, values: &[Option<String>]) -> serde_json::Value {
    let mut values = values.to_vec();
    values.sort();
    json!({"kind": kind, "values": values})
}

fn range_filter(kind: &str, range: &hoh_session_query::SessionResultRange) -> serde_json::Value {
    json!({"kind": kind, "from": range.from, "to": range.to})
}

fn sort_json(values: &mut [serde_json::Value]) {
    values.sort_by_key(js_json_stringify);
}

pub(crate) fn surface_name(surface: &SessionEventSurface) -> &'static str {
    match surface {
        SessionEventSurface::Current => "current",
        SessionEventSurface::Shadowed => "shadowed",
        SessionEventSurface::LogOnly => "log-only",
    }
}
