mod support;
use serde_json::json;
use support::*;
#[tokio::test(start_paused = true)]
async fn merge_context_intercepts_from_outer_to_inner() {
    let (k, r, _) = harness();
    let outer = r.intercept("foo", json!({"a":1,"b":1}));
    let inner = outer.intercept("foo", json!({"b":2,"c":3}));
    let got: serde_json::Value = inner.intercepted("foo");
    assert_eq!(got, json!({"a":1,"b":2,"c":3}));
    assert_eq!(
        outer.intercepted::<serde_json::Value>("foo"),
        json!({"a":1,"b":1})
    );
    k.shutdown().await;
}
