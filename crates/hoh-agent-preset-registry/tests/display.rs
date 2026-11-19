use hoh_agent_preset_registry::{
    PresetDisplaySource, preset_display_text, types::BuiltInPresetCopyKey,
};

fn source(id: &str, name: Option<&str>, description: Option<&str>) -> PresetDisplaySource {
    PresetDisplaySource {
        id: id.into(),
        name: name.map(str::to_owned),
        description: description.map(str::to_owned),
    }
}

fn translate(key: BuiltInPresetCopyKey) -> String {
    format!("t:{}", key.as_str())
}

#[test]
fn resolves_a_shipped_preset_through_its_dictionary_keys() {
    let text = preset_display_text(&source("standard", None, None), translate);
    assert_eq!(text.name, "t:presetStandardName");
    assert_eq!(
        text.description.as_deref(),
        Some("t:presetStandardDescription")
    );
}

#[test]
fn keeps_user_authored_metadata_untranslated() {
    let text = preset_display_text(&source("mine", Some("我的模式"), Some("自述")), translate);
    assert_eq!(text.name, "我的模式");
    assert_eq!(text.description.as_deref(), Some("自述"));
}

#[test]
fn falls_back_to_the_id_for_a_preset_publishing_no_metadata() {
    for id in ["future", "bare"] {
        let text = preset_display_text(&source(id, None, None), translate);
        assert_eq!(text.name, id);
        assert_eq!(text.description, None);
    }
}
