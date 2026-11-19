//! Pure display-copy resolution shared by host and client surfaces.

use crate::types::{BuiltInPresetCopyKey, PresetDisplaySource, PresetDisplayText};

#[derive(Clone, Copy)]
struct PresetLocaleKeys {
    name: BuiltInPresetCopyKey,
    description: BuiltInPresetCopyKey,
}

fn built_in_keys(id: &str) -> Option<PresetLocaleKeys> {
    use BuiltInPresetCopyKey as Key;
    match id {
        "standard" => Some(PresetLocaleKeys {
            name: Key::PresetStandardName,
            description: Key::PresetStandardDescription,
        }),
        "ptc" => Some(PresetLocaleKeys {
            name: Key::PresetPtcName,
            description: Key::PresetPtcDescription,
        }),
        "minimal" => Some(PresetLocaleKeys {
            name: Key::PresetMinimalName,
            description: Key::PresetMinimalDescription,
        }),
        "cordis" => Some(PresetLocaleKeys {
            name: Key::PresetCordisName,
            description: Key::PresetCordisDescription,
        }),
        _ => None,
    }
}

pub fn is_built_in_preset(preset: &PresetDisplaySource) -> bool {
    preset.name.is_none() && built_in_keys(&preset.id).is_some()
}

pub fn preset_display_text(
    preset: &PresetDisplaySource,
    t: impl Fn(BuiltInPresetCopyKey) -> String,
) -> PresetDisplayText {
    if is_built_in_preset(preset) {
        // The predicate above proves the key exists.
        let keys = built_in_keys(&preset.id).expect("known built-in preset id");
        return PresetDisplayText {
            name: t(keys.name),
            description: Some(t(keys.description)),
        };
    }
    PresetDisplayText {
        name: preset.name.clone().unwrap_or_else(|| preset.id.clone()),
        description: preset.description.clone(),
    }
}
