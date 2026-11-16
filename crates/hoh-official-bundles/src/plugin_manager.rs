//! Protected, hidden, ranked, and experimental packages for the plugin manager.

use indexmap::IndexSet;
use std::sync::LazyLock;

pub static PROTECTED_PLUGINS: LazyLock<IndexSet<&'static str>> = LazyLock::new(|| {
    IndexSet::from([
        "@deepseek-ai/dsh-plugin-manager",
        "@deepseek-ai/cordis-plugin-loader",
        "@deepseek-ai/cordis-plugin-include",
        "@deepseek-ai/dsh-api-gateway",
        "@deepseek-ai/dsh-host-webserver",
        "@deepseek-ai/dsh-client-modules",
        "@deepseek-ai/dsh-client-ui-settings-plugin-inventory",
        "@deepseek-ai/dsh-client-ui-plugin-manager",
        "@deepseek-ai/dsh-host-plugin-inventory",
        "@deepseek-ai/dsh-typert-registry",
        "@deepseek-ai/dsh-api-remotes",
        "@deepseek-ai/dsh-api-remote-access",
        "@deepseek-ai/cordis-plugin-timer",
        "@deepseek-ai/dsh-client-connection",
        "@deepseek-ai/dsh-host-frontend-static",
        "@deepseek-ai/dsh-tools",
        "@deepseek-ai/dsh-hmr",
    ])
});

/// Bundles that have no plugin manager page.
pub static HIDDEN_BUNDLES: LazyLock<IndexSet<&'static str>> = LazyLock::new(|| {
    IndexSet::from([
        "@deepseek-ai/dsh-base",
        "@deepseek-ai/dsh-web-app",
        "@deepseek-ai/dsh-headless",
        "@deepseek-ai/dsh-sdk-app",
        "@deepseek-ai/dsh-acp-app",
        "@deepseek-ai/dsh-sdk-minimal",
        "@deepseek-ai/dsh-experimental-terminal-bundle",
        "@deepseek-ai/dsh-experimental-tool-worktree",
        "@deepseek-ai/dsh-cua-mcp",
    ])
});

pub const EXTENSION_BUNDLES: &[&str] = &[
    "@deepseek-ai/dsh-experimental-voice-input-bundle",
    "@deepseek-ai/dsh-experimental-plugin-market-bundle",
    "@deepseek-ai/dsh-experimental-auto-review",
];

pub const MORE_BUNDLES: &[&str] = &[
    "@deepseek-ai/dsh-personalized-recommendations",
    "@deepseek-ai/dsh-experimental-inspector-profile",
    "@deepseek-ai/dsh-cua-bundle",
    "@deepseek-ai/dsh-timeline",
    "@deepseek-ai/dsh-experimental-cot-translation-bundle",
    "@deepseek-ai/dsh-experimental-session-search",
    "@deepseek-ai/dsh-experimental-session-titles-bundle",
    "@deepseek-ai/dsh-experimental-badge-skill-bundle",
    "@deepseek-ai/dsh-experimental-ralph-bundle",
    "@deepseek-ai/dsh-deepseek-chat-import",
    "@deepseek-ai/dsh-subagent-codex",
    "@deepseek-ai/dsh-subagent-claude-code",
];

pub static EXPERIMENTAL_BUNDLES: LazyLock<IndexSet<&'static str>> = LazyLock::new(|| {
    IndexSet::from([
        "@deepseek-ai/dsh-personalized-recommendations",
        "@deepseek-ai/dsh-cua-bundle",
        "@deepseek-ai/dsh-cua-mcp",
        "@deepseek-ai/dsh-timeline",
        "@deepseek-ai/dsh-experimental-voice-input-bundle",
        "@deepseek-ai/dsh-experimental-auto-review",
        "@deepseek-ai/dsh-experimental-inspector-profile",
        "@deepseek-ai/dsh-experimental-forge-bundle",
        "@deepseek-ai/dsh-experimental-tool-worktree",
        "@deepseek-ai/dsh-experimental-cot-translation-bundle",
        "@deepseek-ai/dsh-experimental-session-search",
        "@deepseek-ai/dsh-experimental-session-titles-bundle",
        "@deepseek-ai/dsh-experimental-badge-skill-bundle",
        "@deepseek-ai/dsh-experimental-ralph-bundle",
        "@deepseek-ai/dsh-experimental-plugin-market-bundle",
        "@deepseek-ai/dsh-experimental-liveshare",
        "@deepseek-ai/dsh-deepseek-chat-import",
    ])
});
