//! Bundle names the installation selects, offers, installs on demand, or retires.

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileTemplate {
    pub bundles: Vec<String>,
}

/// Shipped profiles in declaration order; the values are immutable installation data.
pub static PROFILE_TEMPLATES: LazyLock<IndexMap<&'static str, ProfileTemplate>> =
    LazyLock::new(|| {
        IndexMap::from([
            (
                "acp",
                template(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-acp-app"]),
            ),
            (
                "web",
                template(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"]),
            ),
            (
                "headless",
                template(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-headless"]),
            ),
            (
                "sdk",
                template(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"]),
            ),
            ("sdk-minimal", template(&["@deepseek-ai/dsh-sdk-minimal"])),
        ])
    });

/// Exact tuples written by earlier releases, eligible for replacement by the template.
pub static INSTALLATION_OWNED_PROFILE_TUPLES: LazyLock<IndexMap<&'static str, Vec<String>>> =
    LazyLock::new(|| {
        IndexMap::from([(
            "headless",
            vec![
                "@deepseek-ai/dsh-base".into(),
                "@deepseek-ai/dsh-web-app".into(),
                "@deepseek-ai/dsh-headless".into(),
            ],
        )])
    });

/// Earlier shipped bundles that app-boot removes from profiles, in source set order.
pub static RETIRED_BUNDLES: LazyLock<IndexSet<&'static str>> = LazyLock::new(|| {
    IndexSet::from([
        "@deepseek-ai/dsh-experimental-schedule-bundle",
        "@deepseek-ai/dsh-experimental-agent-team-profile",
    ])
});

pub const DEFAULT_PROFILE_BUNDLES: &[&str] = &["@deepseek-ai/dsh-base"];

/// Installation-carried bundles offered switched off, in the source order.
pub const OPTIONAL_BUNDLES: &[&str] = &[
    "@deepseek-ai/dsh-personalized-recommendations",
    "@deepseek-ai/dsh-cua-bundle",
    "@deepseek-ai/dsh-cua-mcp",
    "@deepseek-ai/dsh-timeline",
    "@deepseek-ai/dsh-experimental-session-search",
    "@deepseek-ai/dsh-experimental-ralph-bundle",
    "@deepseek-ai/dsh-experimental-terminal-bundle",
    "@deepseek-ai/dsh-experimental-badge-skill-bundle",
    "@deepseek-ai/dsh-experimental-session-titles-bundle",
    "@deepseek-ai/dsh-experimental-voice-input-bundle",
    "@deepseek-ai/dsh-experimental-cot-translation-bundle",
    "@deepseek-ai/dsh-experimental-auto-review",
    "@deepseek-ai/dsh-experimental-inspector-profile",
    "@deepseek-ai/dsh-experimental-liveshare",
    "@deepseek-ai/dsh-experimental-plugin-market-bundle",
    "@deepseek-ai/dsh-experimental-tool-worktree",
    "@deepseek-ai/dsh-experimental-forge-bundle",
    "@deepseek-ai/dsh-deepseek-chat-import",
];

pub const ON_DEMAND_BUNDLES: &[&str] = &[
    "@deepseek-ai/dsh-subagent-claude-code",
    "@deepseek-ai/dsh-subagent-codex",
];

fn template(bundles: &[&str]) -> ProfileTemplate {
    ProfileTemplate {
        bundles: bundles.iter().map(|bundle| (*bundle).to_owned()).collect(),
    }
}
