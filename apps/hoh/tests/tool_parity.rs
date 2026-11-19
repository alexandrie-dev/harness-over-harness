#[path = "support/tool_parity.rs"]
mod tool_parity;
use tool_parity::*;
#[test]
fn accepts_an_exact_directional_exception_and_rejects_it_after_the_profiles_converge() {
    let exceptions = [Exception {
        tool: "desktop_only",
        side: Side::Desktop,
        reason: "Requires a graphical client.",
    }];
    assert!(
        assert_tool_parity(
            ToolMode::Standard,
            &["read"],
            &["read", "desktop_only"],
            Some(&exceptions)
        )
        .is_ok()
    );
    assert_eq!(
        assert_tool_parity(ToolMode::Standard, &["read"], &["read"], Some(&exceptions))
            .unwrap_err(),
        "SDK/Desktop tool mismatch: standard\nMissing from SDK: (none)\nOnly in SDK: (none)\nStale exceptions: desktop:desktop_only"
    );
    assert_eq!(
        assert_tool_parity(
            ToolMode::Standard,
            &["read", "desktop_only"],
            &["read"],
            Some(&exceptions)
        )
        .unwrap_err(),
        "SDK/Desktop tool mismatch: standard\nMissing from SDK: (none)\nOnly in SDK: desktop_only\nStale exceptions: desktop:desktop_only"
    );
}
#[test]
fn reports_unexpected_differences_in_both_directions_without_hiding_shared_additions() {
    assert!(
        assert_tool_parity(
            ToolMode::Standard,
            &["read", "new_shared"],
            &["new_shared", "read"],
            Some(&[])
        )
        .is_ok()
    );
    assert_eq!(
        assert_tool_parity(
            ToolMode::Ptc,
            &["read", "sdk_extra"],
            &["read", "present"],
            Some(&[])
        )
        .unwrap_err(),
        "SDK/Desktop tool mismatch: ptc\nMissing from SDK: present\nOnly in SDK: sdk_extra\nStale exceptions: (none)"
    );
}
#[test]
fn rejects_empty_observations_undocumented_exceptions_and_duplicate_exceptions() {
    assert_eq!(
        assert_tool_parity(ToolMode::Standard, &[], &[], Some(&[])).unwrap_err(),
        "Empty tool inventory: standard"
    );
    assert_eq!(
        assert_tool_parity(
            ToolMode::Standard,
            &["read"],
            &["read"],
            Some(&[Exception {
                tool: "extra",
                side: Side::Sdk,
                reason: " "
            }])
        )
        .unwrap_err(),
        "Tool exception requires a reason: extra"
    );
    let exceptions = [
        Exception {
            tool: "extra",
            side: Side::Sdk,
            reason: "Intentional.",
        },
        Exception {
            tool: "extra",
            side: Side::Sdk,
            reason: "Intentional.",
        },
    ];
    assert_eq!(
        assert_tool_parity(
            ToolMode::Standard,
            &["read", "extra"],
            &["read"],
            Some(&exceptions)
        )
        .unwrap_err(),
        "SDK/Desktop tool mismatch: standard\nMissing from SDK: (none)\nOnly in SDK: (none)\nStale exceptions: sdk:extra"
    );
}
