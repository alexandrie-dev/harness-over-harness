use hoh_office_kit::locate::{KitModuleResolution, resolve_kit_module};

#[test]
fn reports_the_selected_entry_or_the_lookup_failure() {
    let entry = std::path::PathBuf::from("/kit/lib/index.js");
    match resolve_kit_module(|| Ok(entry.clone())) {
        KitModuleResolution::Available { entry: actual, .. } => assert_eq!(actual, entry),
        KitModuleResolution::Missing { .. } => panic!("expected available Kit"),
    }
    match resolve_kit_module(|| {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing").into())
    }) {
        KitModuleResolution::Missing { error } => assert_eq!(error.to_string(), "missing"),
        KitModuleResolution::Available { .. } => panic!("expected missing Kit"),
    }
}
