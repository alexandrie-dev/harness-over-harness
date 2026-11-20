//! Live-preferred exact reads, traces, filters and provider-backed search.

mod corpus;
mod engine;
mod error;
mod helpers;
mod observation;
mod types;

pub use corpus::{ColdSessionLog, SessionCorpus, read_cold_session_log};
pub use engine::{SessionQueryEngine, SessionSearchProvider, mount_session_query};
pub use error::{SessionQueryError, SessionQueryErrorCode};
pub use helpers::{
    assert_session_headers_compatible, build_session_event_records,
    build_session_event_search_documents, compile_session_text_filter, extract_session_event_text,
    filter_session_event_documents, filter_session_results,
    materialize_session_event_result_filters, materialize_session_result_filters, trace_event,
    trace_session,
};
pub use observation::{
    ProjectionMode, SessionObservation, SessionObservationOptions, SessionObservationReader,
    SessionObservationSource,
};
pub use types::*;

pub const SESSION_QUERY_READ_WINDOW_MAX: u64 = 50;
pub const SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY: usize = 4;
pub const SESSION_QUERY_DEFAULT_PREPARED_SESSION_CACHE_SIZE: usize = 5;

hoh_kernel::service!(pub SessionQuery: SessionQueryEngine = "sessionQuery");
