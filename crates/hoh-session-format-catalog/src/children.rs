use hoh_session_format::*;
use hoh_session_format_v3_to_v4::create_session_format_v3_to_v4;
pub fn create_session_format_catalog_with_children(
    children: Vec<SessionFormatJsonValue>,
) -> Result<SessionFormatCatalog, SessionFormatError> {
    let mut options = crate::generated::session_format_catalog_options();
    // The fixed inventory's only 3→4 slot is the source declaration being replaced.
    options.migrations[3] = create_session_format_v3_to_v4(children);
    create_session_format_catalog(options)
}
