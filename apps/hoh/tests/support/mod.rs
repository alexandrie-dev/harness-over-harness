#![allow(dead_code)]
use hoh_loader::{BundleSource, EvalEnv, Layer, LayerSource, Profile};
use std::{path::Path, sync::Arc};
pub fn env(root: &Path) -> EvalEnv {
    EvalEnv {
        home: root.join("home"),
        cwd: root.into(),
        exe: root.join("hoh"),
        platform: "linux",
        var: Arc::new(|_| None),
    }
}
pub fn layers(names: &[&str]) -> Vec<Layer> {
    names
        .iter()
        .map(|name| {
            let name = format!("@deepseek-ai/dsh-{name}");
            let bundle = hoh::profile_boot::ShippedBundles.bundle(&name).unwrap();
            let mut patches = vec![];
            for (file, text) in bundle.files {
                patches.extend(
                    hoh_loader::parse_patches(&text, &Path::new("/bundles").join(&name).join(file))
                        .unwrap(),
                );
            }
            Layer {
                source: LayerSource::Bundle(name),
                file: None,
                patches,
            }
        })
        .collect()
}
pub fn profile(root: &Path, name: &str, bundles: &[&str]) -> Profile {
    let dir = root.join("profiles").join(name);
    hoh_loader::init_profile(
        &dir,
        &bundles
            .iter()
            .map(|n| format!("@deepseek-ai/dsh-{n}"))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    Profile::load(
        &dir,
        &hoh::profile_boot::ShippedBundles,
        &root.join("home"),
        &[],
        &env(root),
    )
    .unwrap()
}
pub fn manifest(name: &str) -> serde_json::Value {
    let text = match name {
        "@deepseek-ai/dsh-base" => include_str!("../../assets/bundle/base/package.json"),
        "@deepseek-ai/dsh-headless" => include_str!("../../assets/bundle/headless/package.json"),
        "@deepseek-ai/dsh-acp-app" => include_str!("../../assets/bundle/acp-app/package.json"),
        "@deepseek-ai/dsh-web-app" => include_str!("../../assets/bundle/web-app/package.json"),
        "@deepseek-ai/dsh-sdk-app" => include_str!("../../assets/bundle/sdk-app/package.json"),
        "@deepseek-ai/dsh-sdk-minimal" => {
            include_str!("../../assets/bundle/sdk-minimal/package.json")
        }
        _ => panic!("unknown fixture bundle"),
    };
    hoh_util_values::js_json_parse(text).unwrap()
}
