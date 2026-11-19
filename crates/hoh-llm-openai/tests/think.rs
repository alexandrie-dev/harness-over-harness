use hoh_llm_openai::think::{Segment, ThinkTagParser};

fn run(implicit: bool, parts: &[&str]) -> Vec<Segment> {
    let mut parser = ThinkTagParser::new(implicit);
    let mut out = Vec::new();
    for part in parts {
        out.extend(parser.push(part));
    }
    out.extend(parser.finish());
    merge(out)
}

/// Adjacent segments of one kind are one piece of text; chunking must not matter.
fn merge(segments: Vec<Segment>) -> Vec<Segment> {
    let mut merged: Vec<Segment> = Vec::new();
    for segment in segments {
        match (merged.last_mut(), segment) {
            (Some(Segment::Text(a)), Segment::Text(b)) => a.push_str(&b),
            (Some(Segment::Reasoning(a)), Segment::Reasoning(b)) => a.push_str(&b),
            (_, segment) => merged.push(segment),
        }
    }
    merged
}

fn text(s: &str) -> Segment {
    Segment::Text(s.into())
}
fn reasoning(s: &str) -> Segment {
    Segment::Reasoning(s.into())
}

#[test]
fn reasoning_then_answer() {
    assert_eq!(
        run(false, &["<think>\nhmm</think>\n\nAnswer"]),
        vec![reasoning("hmm"), text("Answer")]
    );
}

#[test]
fn open_tag_split_across_chunks() {
    assert_eq!(
        run(false, &["<thi", "nk>idea</think>ok"]),
        vec![reasoning("idea"), text("ok")]
    );
}

#[test]
fn close_tag_split_across_chunks() {
    assert_eq!(
        run(false, &["<think>idea</th", "ink>ok"]),
        vec![reasoning("idea"), text("ok")]
    );
}

#[test]
fn every_split_point_gives_the_same_result() {
    let input = "<think>a b</think>\n\nanswer <b> and 1 < 2";
    let expected = run(false, &[input]);
    assert_eq!(
        expected,
        vec![reasoning("a b"), text("answer <b> and 1 < 2")]
    );
    for cut in (0..input.len()).filter(|c| input.is_char_boundary(*c)) {
        assert_eq!(
            run(false, &[&input[..cut], &input[cut..]]),
            expected,
            "cut at {cut}"
        );
    }
    let chars: Vec<String> = input.chars().map(String::from).collect();
    let refs: Vec<&str> = chars.iter().map(String::as_str).collect();
    assert_eq!(run(false, &refs), expected);
}

#[test]
fn partial_tag_at_end_of_stream_is_text() {
    assert_eq!(run(false, &["a <thi"]), vec![text("a <thi")]);
}

#[test]
fn unclosed_block_ends_as_reasoning() {
    assert_eq!(
        run(false, &["<think>cut off by max tokens"]),
        vec![reasoning("cut off by max tokens")]
    );
}

#[test]
fn implicit_open_starts_inside_the_block() {
    assert_eq!(
        run(true, &["\nplan</think>\n\nAnswer"]),
        vec![reasoning("plan"), text("Answer")]
    );
}

#[test]
fn explicit_open_does_not_treat_plain_text_as_reasoning() {
    assert_eq!(
        run(false, &["just an answer"]),
        vec![text("just an answer")]
    );
}

#[test]
fn stray_close_tag_is_dropped() {
    assert_eq!(
        run(false, &["before</think>after"]),
        vec![text("beforeafter")]
    );
}

#[test]
fn text_between_two_blocks() {
    assert_eq!(
        run(false, &["<think>a</think>x<think>b</think>y"]),
        vec![reasoning("a"), text("x"), reasoning("b"), text("y")]
    );
}
