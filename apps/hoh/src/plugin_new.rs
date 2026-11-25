//! Create an in-repository Rust plugin without modifying existing author files.
use hoh_error::BoxError;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn create(root: &Path, id: &str) -> Result<PathBuf, BoxError> {
    if id.is_empty()
        || id.len() > 64
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("plugin id must start with a lowercase letter, end with a letter or digit, and contain only lowercase letters, digits and hyphens (maximum 64 characters)".into());
    }
    if !root.join("crates/hoh-kernel/Cargo.toml").is_file() {
        return Err("run hoh plugin new from the HoH repository root".into());
    }
    let parent = root.join("plugins");
    fs::create_dir_all(&parent)?;
    if fs::symlink_metadata(&parent)?.file_type().is_symlink() {
        return Err("plugins directory must not be a symlink".into());
    }
    let path = parent.join(id);
    fs::create_dir(&path)?;
    let result = (|| -> Result<(), BoxError> {
        fs::create_dir(path.join("src"))?;
        fs::create_dir(path.join("tests"))?;
        fs::write(
            path.join("Cargo.toml"),
            format!(
                "[package]\nname = \"hoh-plugin-{id}\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nhoh-kernel = {{ path = \"../../crates/hoh-kernel\" }}\n"
            ),
        )?;
        fs::write(
            path.join("src/lib.rs"),
            format!(
                "use hoh_kernel::{{Cfg, Context, Error, NoConfig, Plugin}};\n\npub struct AuthorPlugin;\n\nimpl Plugin for AuthorPlugin {{\n    const NAME: &'static str = \"hoh:plugin-{id}\";\n    type Config = NoConfig;\n    type Deps = ();\n\n    async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {{\n        Ok(())\n    }}\n}}\n"
            ),
        )?;
        fs::write(
            path.join("hoh-plugin.json"),
            format!(
                "{{\n  \"id\": \"{id}\",\n  \"version\": \"0.1.0\",\n  \"entry\": \"AuthorPlugin\",\n  \"permissions\": {{}}\n}}\n"
            ),
        )?;
        let crate_name = format!("hoh_plugin_{}", id.replace('-', "_"));
        fs::write(
            path.join("tests/smoke.rs"),
            format!(
                "use hoh_kernel::Plugin;\nuse {crate_name}::AuthorPlugin;\n\n#[test]\nfn plugin_has_the_expected_name() {{\n    assert_eq!(AuthorPlugin::NAME, \"hoh:plugin-{id}\");\n}}\n"
            ),
        )?;
        Ok(())
    })();
    if result.is_err() {
        // This directory was created exclusively above and cannot contain pre-existing author data.
        let _ = fs::remove_dir_all(&path);
    }
    result?;
    Ok(path)
}
