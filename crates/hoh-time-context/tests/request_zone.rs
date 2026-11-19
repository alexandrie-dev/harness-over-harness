use hoh_error::HarnessError;
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_time_context::TimeContextError;
use hoh_time_context::request_zone::{
    BrowserTimeZoneContext, derive_browser_time_zone_context, render_browser_time_zone_context,
};
use serde_json::json;

fn browser_message(time_zone: &str) -> UserMessage {
    let mut source = MessageSource::new("user");
    source
        .data
        .insert("rpcId".into(), json!(format!("rpc-{time_zone}")));
    source
        .data
        .insert("clientTimeZone".into(), json!(time_zone));
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(time_zone)],
        source,
    })
}

#[test]
fn derives_missing_unique_and_sorted_mixed_zones_from_user_rpc_messages_only() {
    let plugin = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("plugin")],
        source: MessageSource::new("test"),
    });
    assert_eq!(
        derive_browser_time_zone_context(&[plugin], &[]).unwrap(),
        BrowserTimeZoneContext::Missing
    );
    assert_eq!(
        derive_browser_time_zone_context(
            &[
                browser_message("Asia/Shanghai"),
                browser_message("Asia/Shanghai"),
            ],
            &[],
        )
        .unwrap(),
        BrowserTimeZoneContext::Resolved {
            time_zone: "Asia/Shanghai".into()
        }
    );
    assert_eq!(
        derive_browser_time_zone_context(
            &[
                browser_message("Asia/Shanghai"),
                browser_message("America/New_York"),
            ],
            &[],
        )
        .unwrap(),
        BrowserTimeZoneContext::Mixed {
            time_zones: vec!["America/New_York".into(), "Asia/Shanghai".into()]
        }
    );
}

#[test]
fn validates_every_browser_zone_before_classifying_a_mixed_turn() {
    assert!(
        derive_browser_time_zone_context(&[browser_message("+08:00")], &[])
            .unwrap_err()
            .to_string()
            .contains("canonical UTC or IANA Area/Location")
    );
    assert!(
        derive_browser_time_zone_context(
            &[
                browser_message("Asia/Shanghai"),
                browser_message("Not/A_Real_Zone"),
            ],
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("browser time zone is unsupported")
    );
    assert!(
        derive_browser_time_zone_context(&[browser_message("Etc/UTC")], &[])
            .unwrap_err()
            .to_string()
            .contains("browser time zone must be canonical")
    );
}

#[test]
fn combines_committed_reported_zones_with_proposed_messages_before_validation() {
    assert_eq!(
        derive_browser_time_zone_context(
            &[browser_message("America/New_York")],
            &["Asia/Shanghai".into()],
        )
        .unwrap(),
        BrowserTimeZoneContext::Mixed {
            time_zones: vec!["America/New_York".into(), "Asia/Shanghai".into()]
        }
    );
    assert_eq!(
        derive_browser_time_zone_context(
            &[browser_message("Not/A_Real_Zone")],
            &["+08:00".into()],
        )
        .unwrap_err()
        .to_string(),
        "browser time zone must be canonical UTC or IANA Area/Location: \"+08:00\""
    );
}

#[test]
fn keeps_exact_refusals_and_the_platform_cause_for_every_unusable_zone() {
    let refusal =
        |zone: &str| derive_browser_time_zone_context(&[browser_message(zone)], &[]).unwrap_err();
    assert_eq!(
        refusal("").to_string(),
        "browser time zone must be canonical UTC or IANA Area/Location: \"\""
    );
    assert_eq!(
        refusal("+08:00").to_string(),
        "browser time zone must be canonical UTC or IANA Area/Location: \"+08:00\""
    );
    assert_eq!(
        refusal(" UTC").to_string(),
        "browser time zone must be canonical UTC or IANA Area/Location: \" UTC\""
    );
    assert_eq!(
        refusal("Etc/UTC").to_string(),
        "browser time zone must be canonical: \"Etc/UTC\""
    );
    let unsupported = refusal("Not/A_Real_Zone");
    assert_eq!(
        unsupported.to_string(),
        "browser time zone is unsupported: \"Not/A_Real_Zone\""
    );
    assert!(unsupported.downcast_ref::<HarnessError>().is_none());
    assert_eq!(
        unsupported
            .downcast_ref::<TimeContextError>()
            .unwrap()
            .name(),
        "TypeError"
    );
    let cause = std::error::Error::source(unsupported.as_ref()).unwrap();
    assert_eq!(
        cause.downcast_ref::<TimeContextError>().unwrap().name(),
        "RangeError"
    );
}

#[test]
fn renders_one_explicit_model_policy_for_every_context() {
    assert!(
        render_browser_time_zone_context(&BrowserTimeZoneContext::Resolved {
            time_zone: "Asia/Shanghai".into()
        })
        .contains("Interpret otherwise-unqualified dates and times in this zone.")
    );
    assert!(
        render_browser_time_zone_context(&BrowserTimeZoneContext::Mixed {
            time_zones: vec!["America/New_York".into(), "Asia/Shanghai".into()]
        })
        .contains("mixed [\"America/New_York\",\"Asia/Shanghai\"]")
    );
    assert!(
        render_browser_time_zone_context(&BrowserTimeZoneContext::Missing).contains("unavailable")
    );
}

#[test]
fn exposes_the_accepted_modern_iana_alias_difference() {
    assert_eq!(
        derive_browser_time_zone_context(&[browser_message("Asia/Kolkata")], &[]).unwrap(),
        BrowserTimeZoneContext::Resolved {
            time_zone: "Asia/Kolkata".into()
        }
    );
    assert_eq!(
        derive_browser_time_zone_context(&[browser_message("Asia/Calcutta")], &[])
            .unwrap_err()
            .to_string(),
        "browser time zone must be canonical: \"Asia/Calcutta\""
    );
}
