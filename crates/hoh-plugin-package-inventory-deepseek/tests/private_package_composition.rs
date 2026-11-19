mod support;

use hoh_loader::PluginRegistry;
use serde_json::json;
use support::{FIELD, Harness, Noop, file_url};

#[tokio::test]
async fn reports_an_unversioned_package_by_name_alongside_a_versioned_sibling() {
    for private in [Some(true), Some(false), None] {
        let harness = Harness::start(Some(true), |root, registry: &mut PluginRegistry| {
            let parent = root.join("inspector");
            let skill = parent.join("skill");
            let sibling = parent.join("sibling");
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::create_dir_all(&sibling).unwrap();
            std::fs::write(
                parent.join("package.json"),
                r#"{"name":"inspector","version":"1.0.0","type":"module"}"#,
            )
            .unwrap();
            std::fs::write(
                skill.join("package.json"),
                serde_json::to_vec(&match private {
                    Some(private) => {
                        json!({"name":"inspector-skill","private":private,"type":"module"})
                    }
                    None => json!({"name":"inspector-skill","type":"module"}),
                })
                .unwrap(),
            )
            .unwrap();
            std::fs::write(
                sibling.join("package.json"),
                r#"{"name":"inspector-sibling","version":"2.0.0","type":"module"}"#,
            )
            .unwrap();
            std::fs::write(skill.join("plugin.js"), "").unwrap();
            std::fs::write(sibling.join("plugin.js"), "").unwrap();
            let skill = file_url(&skill.join("plugin.js"));
            let sibling = file_url(&sibling.join("plugin.js"));
            registry.add::<Noop>(&skill).add::<Noop>(&sibling);
            format!("- name: {skill}\n- name: {sibling}\n")
        })
        .await;
        assert_eq!(
            harness.fields(None).await[FIELD],
            json!({"version":1,"packages":[
                {"name":"inspector-sibling","version":"2.0.0"},
                {"name":"inspector-skill"}
            ]})
        );
        harness.close().await;
    }
}
