mod support;
use hoh_loader::{LayerSource, compose};
use hoh_official_bundles::PROFILE_TEMPLATES;
#[test]
fn carries_one_shared_resource_consumer_without_a_server() {
    for (name, template) in PROFILE_TEMPLATES.iter() {
        let home = tempfile::tempdir().unwrap();
        let profile = hoh_loader::load_profile(
            "dsh",
            name,
            &hoh::profile_boot::ShippedBundles,
            home.path(),
            &[],
            &support::env(home.path()),
        )
        .unwrap();
        let rows = compose(&[], &profile.layers, &mut |w| panic!("{w}")).entries;
        let resource = "@deepseek-ai/dsh-mcp-resources";
        let consumers = rows
            .iter()
            .filter(|r| r.name == resource)
            .collect::<Vec<_>>();
        assert_eq!(consumers.len(), 1);
        assert_eq!(consumers[0].id, "mcp-resources");
        assert!(consumers[0].config.is_null());
        assert!(!rows.iter().any(|r| r.name == "@deepseek-ai/dsh-mcp-client"));
        let owners = profile
            .layers
            .iter()
            .filter_map(|l| match &l.source {
                LayerSource::Bundle(name)
                    if support::manifest(name)["dependencies"]
                        .get(resource)
                        .is_some() =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            owners,
            [if *name == "sdk-minimal" {
                "@deepseek-ai/dsh-sdk-minimal"
            } else {
                "@deepseek-ai/dsh-base"
            }]
        );
        assert_eq!(
            profile
                .layers
                .iter()
                .filter(|l| matches!(l.source, LayerSource::Bundle(_)))
                .count(),
            template.bundles.len()
        );
    }
}
#[test]
fn mounts_the_server_manager_only_with_the_web_application() {
    for name in PROFILE_TEMPLATES.keys() {
        let home = tempfile::tempdir().unwrap();
        let p = hoh_loader::load_profile(
            "dsh",
            name,
            &hoh::profile_boot::ShippedBundles,
            home.path(),
            &[],
            &support::env(home.path()),
        )
        .unwrap();
        let rows = compose(&[], &p.layers, &mut |w| panic!("{w}")).entries;
        let managers = rows
            .iter()
            .filter(|r| r.name == "@deepseek-ai/dsh-mcp-manager")
            .collect::<Vec<_>>();
        if *name == "web" {
            assert_eq!(managers.len(), 1);
            assert_eq!(managers[0].id, "mcp-manager");
            assert_eq!(managers[0].inject, Some(vec!["credentials".into()]));
        } else {
            assert!(managers.is_empty());
        }
    }
}
