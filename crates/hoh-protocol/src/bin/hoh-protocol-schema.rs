use std::{fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/schema"));
    fs::create_dir_all(&destination)?;
    for (name, schema) in hoh_protocol::schemas() {
        fs::write(
            destination.join(format!("{name}.json")),
            format!("{}\n", serde_json::to_string_pretty(&schema)?),
        )?;
    }
    Ok(())
}
