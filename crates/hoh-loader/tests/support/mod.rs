#![allow(dead_code)]
pub mod live;
use hoh_loader::*;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
pub fn entries(text: &str) -> Vec<Entry> {
    parse_entries(text, Path::new("/fixtures/base.yml")).unwrap()
}
pub fn patches(text: &str) -> Vec<Patch> {
    parse_patches(text, Path::new("/fixtures/patch.yml")).unwrap()
}
pub fn layer(name: &'static str, text: &str) -> Layer {
    Layer {
        source: LayerSource::Launcher(name),
        file: None,
        patches: patches(text),
    }
}
pub fn env(root: &Path) -> EvalEnv {
    EvalEnv {
        home: root.join("home"),
        cwd: root.into(),
        exe: root.join("hoh"),
        platform: "linux",
        var: Arc::new(|_| None),
    }
}
pub struct Source(pub Vec<Bundle>);
impl BundleSource for Source {
    fn bundle(&self, name: &str) -> Result<Bundle, LoadError> {
        self.0
            .iter()
            .find(|b| b.name == name)
            .cloned()
            .ok_or_else(|| LoadError::Shape {
                file: PathBuf::from("bundles"),
                line: 1,
                column: 1,
                message: format!("bundle {name} not found"),
            })
    }
}
pub fn bundle(name: &str, text: &str) -> Bundle {
    Bundle {
        name: name.into(),
        dir: Some(PathBuf::from(format!("/bundles/{name}"))),
        platforms: None,
        files: vec![("cordis.patch.yml".into(), text.into())],
    }
}
pub fn write_profile(root: &Path, names: &[&str]) -> PathBuf {
    let dir = root.join("profiles/custom");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        serde_json::json!({"dsh":{"profile":{"bundles":names}}}).to_string(),
    )
    .unwrap();
    dir
}
pub fn find<'a>(rows: &'a [Entry], id: &str) -> &'a Entry {
    rows.iter().find(|r| r.id == id).unwrap()
}
pub fn json_rows(rows: &[Entry]) -> Value {
    serde_json::to_value(rows).unwrap()
}
