#!/bin/bash
# Prepare a disposable official-ID channel fixture. No real CLI or network is used.
# Usage: scripts/u4-fake-telegram.sh /mnt/toshiba/alexandrie/hoh/qa/u4-NEW
set -euo pipefail
qa=/mnt/toshiba/alexandrie/hoh/qa
run=${1:?give a fresh direct u4-* directory under the QA root}
name=${run#"$qa"/}
[[ $run == "$qa"/* && $name =~ ^u4-[A-Za-z0-9._-]+$ && ! -e $run && ! -L $run ]] || exit 2
umask 077
mkdir -p "$run/home" "$run/config/plugins/cache/claude-plugins-official/telegram/0.0.7/.claude-plugin" "$run/codex" "$run/xdg" "$run/state" "$run/project"
plugin=$run/config/plugins/cache/claude-plugins-official/telegram/0.0.7
printf '%s\n' '{"enabledPlugins":{"telegram@claude-plugins-official":true}}' > "$run/config/settings.json"
node - "$run/config/plugins/installed_plugins.json" "$plugin" <<'JS'
const fs = require('fs');
fs.writeFileSync(process.argv[2], JSON.stringify({version:2, plugins:{'telegram@claude-plugins-official':[{scope:'user', installPath:process.argv[3]}]}}));
JS
printf '%s\n' '{"name":"telegram","version":"0.0.7"}' > "$plugin/.claude-plugin/plugin.json"
printf '%s\n' '{"mcpServers":{"telegram":{"command":"node","args":["${CLAUDE_PLUGIN_ROOT}/server.js"]}}}' > "$plugin/.mcp.json"
cat > "$plugin/server.js" <<'JS'
// claude/channel: deliberately offline fake MCP server.
require('fs').writeFileSync(process.env.U4_MARKER, 'started\n');
process.stdin.resume();
JS
cat > "$run/env.sh" <<EOF2
export HOME='$run/home' CLAUDE_CONFIG_DIR='$run/config' CODEX_HOME='$run/codex'
export XDG_CONFIG_HOME='$run/xdg/config' XDG_CACHE_HOME='$run/xdg/cache' XDG_DATA_HOME='$run/xdg/data' XDG_STATE_HOME='$run/xdg/state' XDG_RUNTIME_DIR='$run/xdg'
export FAKE_STATE='$run/state' U4_MARKER='$run/server-started'
EOF2
printf 'Fixture: %s\nSource %s/env.sh before the authorized isolated check.\n' "$run" "$run"
