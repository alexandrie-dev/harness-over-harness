mod support;
use serde_json::json;
use support::layers;
fn hmr(layers: &[hoh_loader::Layer]) -> hoh_loader::Entry {
    hoh_loader::compose(&[], layers, &mut |w| panic!("{w}"))
        .entries
        .into_iter()
        .find(|r| r.id == "hmr")
        .unwrap()
}
#[test]
fn enables_configuration_watching_in_the_base_and_web_compositions() {
    assert_eq!(
        hmr(&layers(&["base", "web-app"])).config,
        json!({"root":[]})
    );
    assert_ne!(hmr(&layers(&["base"])).disabled, true);
}
#[test]
fn disables_hmr_with_a_bundle_override() {
    for mode in ["headless", "sdk-app", "acp-app"] {
        let mut l = layers(&["base", mode]);
        assert_eq!(hmr(&l).disabled, true);
        l.push(hoh_loader::Layer {
            source: hoh_loader::LayerSource::Profile,
            file: None,
            patches: hoh_loader::parse_patches(
                "- id: hmr\n  disabled: false\n",
                std::path::Path::new("/profile/patch.yml"),
            )
            .unwrap(),
        });
        let row = hmr(&l);
        assert_eq!(row.disabled, false);
        assert_eq!(row.config, json!({"root":[]}));
    }
}
#[test]
fn selects_module_roots_through_a_later_yaml_layer() {
    let mut l = layers(&["base"]);
    l.push(hoh_loader::Layer {
        source: hoh_loader::LayerSource::Profile,
        file: None,
        patches: hoh_loader::parse_patches(
            "- id: hmr\n  config: {root: ['.']}\n",
            std::path::Path::new("/profile/patch.yml"),
        )
        .unwrap(),
    });
    assert_eq!(hmr(&l).config, json!({"root":["."]}));
}
#[test]
fn keeps_the_standalone_sdk_minimal_tree_free_of_hmr() {
    assert!(
        !hoh_loader::compose(&[], &layers(&["sdk-minimal"]), &mut |w| panic!("{w}"))
            .entries
            .iter()
            .any(|r| r.id == "hmr")
    );
}
