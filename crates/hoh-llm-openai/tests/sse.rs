use hoh_llm_openai::sse::{MAX_LINE_BYTES, SseEvent, SseOverflow, SseParser};

fn events(parts: &[&[u8]]) -> Vec<SseEvent> {
    let mut parser = SseParser::new();
    let mut all = Vec::new();
    for part in parts {
        all.extend(parser.push(part).unwrap());
    }
    all.extend(parser.finish());
    all
}

fn data(text: &str) -> SseEvent {
    SseEvent {
        event: None,
        data: text.into(),
    }
}

#[test]
fn frames_by_blank_line() {
    assert_eq!(
        events(&[b"data: a\n\ndata: b\n\n"]),
        vec![data("a"), data("b")]
    );
}

#[test]
fn data_split_across_chunks_at_every_byte() {
    let input = "data: {\"a\":\"привет\"}\n\ndata: [DONE]\n\n".as_bytes();
    let expected = events(&[input]);
    assert_eq!(expected.len(), 2);
    for cut in 0..input.len() {
        assert_eq!(
            events(&[&input[..cut], &input[cut..]]),
            expected,
            "cut at {cut}"
        );
    }
    let bytes: Vec<&[u8]> = input.chunks(1).collect();
    assert_eq!(events(&bytes), expected);
}

#[test]
fn crlf_cr_and_split_crlf() {
    assert_eq!(
        events(&[b"data: a\r\n\r\ndata: b\r\r"]),
        vec![data("a"), data("b")]
    );
    assert_eq!(
        events(&[b"data: a\r", b"\n\r", b"\ndata: b\n\n"]),
        vec![data("a"), data("b")]
    );
}

#[test]
fn comments_and_unknown_fields_are_ignored() {
    assert_eq!(
        events(&[b": keep-alive\n\n: ping\ndata: a\nid: 7\nretry: 1\n\n"]),
        vec![data("a")]
    );
}

#[test]
fn multiple_data_lines_join_with_newline_and_event_name_is_kept() {
    assert_eq!(
        events(&[b"event: error\ndata: x\ndata: y\n\n"]),
        vec![SseEvent {
            event: Some("error".into()),
            data: "x\ny".into()
        }]
    );
}

#[test]
fn space_after_colon_is_optional_and_bom_is_dropped() {
    assert_eq!(
        events(&["\u{feff}data:a\n\ndata:  b\n\n".as_bytes()]),
        vec![data("a"), data(" b")]
    );
}

#[test]
fn unterminated_tail_is_not_an_event() {
    assert_eq!(
        events(&[b"data: a\n\ndata: {\"partial\":"]),
        vec![data("a")]
    );
    assert_eq!(events(&[b"data: a\n\ndata: b\n"]), vec![data("a")]);
}

#[test]
fn bare_done_at_end_of_input_is_tolerated() {
    assert_eq!(
        events(&[b"data: a\n\ndata: [DONE]"]),
        vec![data("a"), data("[DONE]")]
    );
    assert_eq!(events(&[b"data: [DONE]\n"]), vec![data("[DONE]")]);
}

#[test]
fn overlong_line_is_rejected() {
    let mut parser = SseParser::new();
    let line = vec![b'x'; MAX_LINE_BYTES + 1];
    assert_eq!(parser.push(&line), Err(SseOverflow));
}
