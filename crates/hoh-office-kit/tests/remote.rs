//! The source Remote transport is intentionally absent; exercise the ordinary public service methods retained by §8.

use hoh_office_kit::*;

#[test]
fn remote_methods_remain_object_safe_service_operations() {
    fn accepts(_: &dyn OfficeKitApi) {}
    let _ = accepts;
    assert_eq!(
        OfficeKitErrorCode::UnsupportedFormat.as_str(),
        "unsupported-format"
    );
    assert_eq!(OfficeKitErrorCode::SourceChanged.as_str(), "source-changed");
}
