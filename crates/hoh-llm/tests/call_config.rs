mod common;
use common::*;
use hoh_llm::*;
#[test]
fn compares_every_field_including_the_stop_list_element_wise() {
    let base = config();
    assert!(call_config_equals(&base, &base));
    for c in [
        LlmCallConfig {
            provider: "x".into(),
            ..base.clone()
        },
        LlmCallConfig {
            model: "x".into(),
            ..base.clone()
        },
        LlmCallConfig {
            controls: LlmCallControls {
                reasoning_effort: Some("high".into()),
                ..Default::default()
            },
            ..base.clone()
        },
        LlmCallConfig {
            controls: LlmCallControls {
                temperature: Some(0.5),
                ..Default::default()
            },
            ..base.clone()
        },
        LlmCallConfig {
            controls: LlmCallControls {
                max_tokens: Some(1),
                ..Default::default()
            },
            ..base.clone()
        },
        LlmCallConfig {
            controls: LlmCallControls {
                stop: Some(vec!["a".into()]),
                ..Default::default()
            },
            ..base.clone()
        },
    ] {
        assert!(!call_config_equals(&base, &c));
    }
    let mut a = base.clone();
    a.controls.stop = Some(vec!["a".into(), "b".into()]);
    assert!(call_config_equals(&a, &a.clone()));
    let mut b = a.clone();
    b.controls.stop.as_mut().unwrap().reverse();
    assert!(!call_config_equals(&a, &b));
    b.controls.stop = Some(vec!["a".into()]);
    assert!(!call_config_equals(&a, &b));
}
#[test]
fn never_freezes_an_abort_signal_the_live_cancellation_channel_keeps_working() {
    let controller = hoh_timeout::AbortController::new();
    let request = GenerateOptions {
        signal: Some(controller.signal()),
        ..options()
    };
    let copy = request.clone();
    controller.abort();
    assert!(request.signal.unwrap().aborted());
    assert!(copy.signal.unwrap().aborted());
}
#[test]
fn marks_only_the_exact_request_object_and_preserves_its_identity() {
    let mut request = options();
    let copy = request.clone();
    assert!(!is_agent_loop_request(&request));
    let address = std::ptr::from_ref(&request);
    assert_eq!(
        std::ptr::from_ref(mark_agent_loop_request(&mut request)),
        address
    );
    assert!(is_agent_loop_request(&request));
    assert!(!is_agent_loop_request(&copy));
    assert!(!is_agent_loop_request(&request.clone()));
}
