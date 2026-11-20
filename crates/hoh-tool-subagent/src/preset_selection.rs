use hoh_error::BoxError;
use hoh_subagent::{SubagentError, SubagentErrorCode, SubagentProvider};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetSelectionConfig {
    pub agent_preset: Option<String>,
    pub allowed_agent_presets: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct SubagentPresetSelection {
    config: PresetSelectionConfig,
}

impl SubagentPresetSelection {
    pub fn new(config: PresetSelectionConfig) -> Result<Self, BoxError> {
        if let Some(allowed) = &config.allowed_agent_presets {
            let unique: HashSet<&str> = allowed.iter().map(String::as_str).collect();
            if unique.len() != allowed.len() {
                return Err(
                    "tool-subagent: allowedAgentPresets contains duplicate preset ids".into(),
                );
            }
            if config
                .agent_preset
                .as_ref()
                .is_some_and(|preset| !allowed.contains(preset))
            {
                return Err(
                    "tool-subagent: agentPreset must be included in allowedAgentPresets".into(),
                );
            }
        }
        Ok(Self { config })
    }

    pub fn configured(&self) -> bool {
        self.config.agent_preset.is_some() || self.config.allowed_agent_presets.is_some()
    }

    pub fn choices(&self) -> Option<&[String]> {
        self.config.allowed_agent_presets.as_deref()
    }

    pub fn description(&self) -> String {
        let fallback = self.config.agent_preset.as_ref().map_or_else(
            || "Omit to inherit the parent agent's current preset.".into(),
            |preset| format!("Omit to use {preset}."),
        );
        format!("Agent preset for this child. {fallback}")
    }

    pub fn assert_provider(&self, provider: &dyn SubagentProvider) -> Result<(), BoxError> {
        if self.configured()
            && (!provider.capabilities().agent_preset || provider.inherits_parent_context())
        {
            return Err(format!(
                "tool-subagent: provider \"{}\" does not support independent agent preset selection",
                provider.name()
            )
            .into());
        }
        Ok(())
    }

    pub fn resolve(&self, value: Option<&Value>) -> Result<Option<String>, BoxError> {
        let Some(value) = value else {
            return Ok(self.config.agent_preset.clone());
        };
        let Some(choices) = &self.config.allowed_agent_presets else {
            return Err(Box::new(SubagentError::error(
                "agent_preset selection is disabled for this tool instance",
                SubagentErrorCode::PresetSelectionDisabled,
            )));
        };
        let selected = value.as_str();
        if selected.is_none_or(|selected| !choices.iter().any(|choice| choice == selected)) {
            return Err(Box::new(SubagentError::error(
                format!("agent_preset must be one of: {}", choices.join(", ")),
                SubagentErrorCode::PresetNotAllowed,
            )));
        }
        Ok(selected.map(str::to_owned))
    }
}
