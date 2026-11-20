use hoh_error::HarnessError;
use hoh_session::SessionId;
use hoh_session_query::{
    SessionAvailability, SessionEventMetadataFilter as EventFilter, SessionEventSearchRequest,
    SessionEventSurface, SessionQueryErrorCode as Code, SessionResultFilter as Filter,
    SessionResultRange as Range, SessionSearchCursor, SessionSearchRequest,
};
use hoh_session_query_sqlite::query::*;

fn limits() -> QueryLimits {
    QueryLimits {
        default_limit: 2,
        max_limit: 3,
    }
}

fn code(error: &hoh_error::BoxError) -> &str {
    HarnessError::of(&**error).expect("HarnessError").code()
}

#[test]
fn normalizes_both_scopes_defaults_and_cursors() {
    let session = normalize_session_request(
        SessionSearchRequest {
            query: "  alpha\n beta  ".into(),
            session_filters: vec![],
            event_filters: vec![],
            limit: None,
            cursor: None,
        },
        limits(),
    )
    .unwrap();
    assert_eq!(session.query, "alpha beta");
    assert_eq!(session.limit, 2);
    let event = normalize_event_request(
        SessionEventSearchRequest {
            session_id: SessionId::from("s"),
            query: "needle".into(),
            filters: vec![EventFilter::Seq(Range {
                from: Some(1.0),
                to: None,
            })],
            limit: None,
            cursor: Some(SessionSearchCursor("next".into())),
        },
        limits(),
    )
    .unwrap();
    assert_eq!(event.limit, 2);
    assert_eq!(event.cursor.unwrap().0, "next");
}

#[test]
fn rejects_blank_nul_and_bad_limits() {
    for query in [" \n ", "bad\0query"] {
        let error = normalize_session_request(
            SessionSearchRequest {
                query: query.into(),
                session_filters: vec![],
                event_filters: vec![],
                limit: None,
                cursor: None,
            },
            limits(),
        )
        .unwrap_err();
        assert_eq!(code(&error), Code::InvalidQuery.as_str());
    }
    for limit in [0, 4] {
        let error = normalize_event_request(
            SessionEventSearchRequest {
                session_id: SessionId::from("s"),
                query: "x".into(),
                filters: vec![],
                limit: Some(limit),
                cursor: None,
            },
            limits(),
        )
        .unwrap_err();
        assert_eq!(code(&error), Code::InvalidLimit.as_str());
    }
}

#[test]
fn reports_invalid_filters_before_an_invalid_query_in_both_scopes() {
    let invalid_range = Range {
        from: Some(2.0),
        to: Some(1.0),
    };
    let session_error = normalize_session_request(
        SessionSearchRequest {
            query: String::new(),
            session_filters: vec![Filter::CreatedAt(invalid_range)],
            event_filters: vec![],
            limit: None,
            cursor: None,
        },
        limits(),
    )
    .unwrap_err();
    assert_eq!(code(&session_error), "SESSION_QUERY_INVALID_FILTER");

    let event_error = normalize_event_request(
        SessionEventSearchRequest {
            session_id: "session".into(),
            query: String::new(),
            filters: vec![EventFilter::Seq(invalid_range)],
            limit: None,
            cursor: None,
        },
        limits(),
    )
    .unwrap_err();
    assert_eq!(code(&event_error), "SESSION_QUERY_INVALID_FILTER");
}

#[test]
fn normalization_owns_filter_values_before_any_queued_work() {
    let mut cwd = String::from("/before");
    let normalized = normalize_session_request(
        SessionSearchRequest {
            query: "needle".into(),
            session_filters: vec![Filter::Cwd(vec![Some(cwd.clone())])],
            event_filters: vec![EventFilter::Type(vec!["user/message".into()])],
            limit: None,
            cursor: None,
        },
        limits(),
    )
    .unwrap();
    cwd.replace_range(.., "/after");
    assert_eq!(
        normalized.session_filters,
        vec![Filter::Cwd(vec![Some("/before".into())])]
    );
    assert_eq!(
        normalized.event_filters,
        vec![EventFilter::Type(vec!["user/message".into()])]
    );
}

#[test]
fn compiles_all_session_predicates() {
    let result = build_session_where(&[
        Filter::Id(vec![SessionId::from("a"), SessionId::from("b")]),
        Filter::Cwd(vec![Some("/a".into()), None]),
        Filter::CreatedAt(Range {
            from: Some(1.0),
            to: Some(2.0),
        }),
        Filter::Parent(vec![Some(SessionId::from("p")), None]),
        Filter::Availability(vec![SessionAvailability::Live]),
    ])
    .unwrap();
    assert_eq!(
        result.sql,
        "session_id IN (?, ?) AND (cwd IN (?) OR cwd IS NULL) AND CAST(created_at AS INTEGER) >= ? AND CAST(created_at AS INTEGER) <= ? AND (parent_session IN (?) OR parent_session IS NULL) AND live = 1"
    );
    assert_eq!(result.predicate_count, 6);
    assert_eq!(result.params.len(), 6);
    assert_eq!(build_session_where(&[Filter::Id(vec![])]).unwrap().sql, "0");
}

#[test]
fn compiles_all_event_predicates() {
    let result = build_event_where(&[
        EventFilter::Seq(Range {
            from: Some(1.0),
            to: None,
        }),
        EventFilter::Time(Range {
            from: None,
            to: Some(9.0),
        }),
        EventFilter::Type(vec!["user/message".into()]),
        EventFilter::Surface(vec![
            SessionEventSurface::Current,
            SessionEventSurface::LogOnly,
        ]),
    ])
    .unwrap();
    assert_eq!(
        result.sql,
        "CAST(seq AS INTEGER) >= ? AND CAST(time AS INTEGER) <= ? AND type IN (?) AND surface IN (?, ?)"
    );
    assert_eq!(result.params.len(), 5);
}

#[test]
fn enforces_fts_predicate_budget() {
    let safe = (0..SQLITE_FTS5_OUTER_PREDICATE_LIMIT)
        .map(|_| Filter::Id(vec![SessionId::from("safe")]))
        .collect::<Vec<_>>();
    assert_eq!(
        build_session_where(&safe).unwrap().predicate_count,
        SQLITE_FTS5_OUTER_PREDICATE_LIMIT
    );
    let mut over = safe;
    over.push(Filter::Id(vec![SessionId::from("over")]));
    assert_eq!(
        code(&build_session_where(&over).unwrap_err()),
        Code::InvalidFilter.as_str()
    );
}

#[test]
fn enforces_the_portable_sqlite_variable_boundary() {
    assert!(assert_portable_binding_count(SQLITE_PORTABLE_VARIABLE_LIMIT).is_ok());
    let error = assert_portable_binding_count(SQLITE_PORTABLE_VARIABLE_LIMIT + 1).unwrap_err();
    assert_eq!(code(&error), Code::InvalidFilter.as_str());
    assert_eq!(
        error.to_string(),
        "session-search request exceeds SQLite's portable 32766-variable limit; reduce filter values"
    );
}

#[test]
fn quotes_match_syntax_as_data() {
    assert_eq!(
        quote_fts_data("say \"needle\" OR *"),
        "\"say \"\"needle\"\" OR *\""
    );
}

#[test]
fn canonicalizes_filter_order() {
    let first = NormalizedSessionRequest {
        query: "needle".into(),
        limit: 2,
        cursor: None,
        session_filters: vec![
            Filter::Cwd(vec![Some("/b".into()), Some("/a".into())]),
            Filter::CreatedAt(Range {
                from: Some(1.0),
                to: None,
            }),
        ],
        event_filters: vec![],
    };
    let second = NormalizedSessionRequest {
        query: "needle".into(),
        limit: 2,
        cursor: None,
        session_filters: vec![
            Filter::CreatedAt(Range {
                from: Some(1.0),
                to: None,
            }),
            Filter::Cwd(vec![Some("/a".into()), Some("/b".into())]),
        ],
        event_filters: vec![],
    };
    assert_eq!(
        request_fingerprint_session(&first),
        request_fingerprint_session(&second)
    );
}

#[test]
fn normalizes_bounds_and_positions_unicode_snippets() {
    assert_eq!(make_snippet("  short\ntext  ", 20), "short text");
    assert_eq!(
        make_snippet(
            &format!("abcde{FTS_HIGHLIGHT_START}f{FTS_HIGHLIGHT_END}"),
            1
        ),
        "…"
    );
    assert_eq!(make_snippet("abcdefghij", 5), "abcd…");
    assert_eq!(
        make_snippet(
            &format!("ab{FTS_HIGHLIGHT_START}c{FTS_HIGHLIGHT_END}defghij"),
            5
        ),
        "…bcd…"
    );
    assert_eq!(
        make_snippet(
            &format!("ab{FTS_HIGHLIGHT_START}c{FTS_HIGHLIGHT_END}defghij"),
            3
        ),
        "…c…"
    );
    assert_eq!(
        make_snippet(
            &format!("abcde{FTS_HIGHLIGHT_START}f{FTS_HIGHLIGHT_END}"),
            2
        ),
        "…f"
    );
    assert_eq!(
        make_snippet(
            &format!("abcde{FTS_HIGHLIGHT_START}f{FTS_HIGHLIGHT_END}"),
            5
        ),
        "…cdef"
    );
}

#[test]
fn sanitizes_reserved_fts_markers() {
    assert_eq!(
        sanitize_fts_text(&format!("a\0{FTS_HIGHLIGHT_START}{FTS_HIGHLIGHT_END}b")),
        "a���b"
    );
}
