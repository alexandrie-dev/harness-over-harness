import type { HohExecutor } from './config.ts';

export const MAX_PLUGIN_BRIEF = 8_000;

export interface PluginCreatorHarness {
  executor: HohExecutor;
  name: string;
  model: string;
  available: boolean;
  reason?: string;
}

export interface PluginCreatorOptions {
  projects: { id: string; title: string; path: string }[];
  harnesses: PluginCreatorHarness[];
}

export interface PluginCreatorInput {
  projectId: string;
  executor: HohExecutor;
  brief: string;
}

export interface PluginCreatorApi {
  options(): Promise<PluginCreatorOptions>;
  subscribe(listener: () => void): () => void;
  launch(input: PluginCreatorInput): Promise<string>;
}

const STANDALONE_INTRO = `# Author an external HoH plugin

The selected folder belongs to the author. The installed HoH binary is enough to start this workflow. Do not require Git, a HoH checkout, or a build of the app. Keep plugin files inside the selected folder. The user profile's external_plugins directory is for installed artifacts, not source projects.

## Start with an interview

Use the author's language and ask exactly one design question per turn. Offer a recommendation and a short reason. The first reply must ask a question, even if it is only to confirm the plan. Use a structured question tool when available. Before writing files, show the proposed files, permissions and checks. Wait for approval before scaffolding or implementing. If scope changes, confirm the revised plan.

## Author inside the selected folder`;

const SKILL_DRAFT = `- For a Markdown skill draft, create a folder with SKILL.md and hoh-package.json. The manifest draft uses formatVersion 2, the plugin id and version, execution { "kind": "skill", "entry": "SKILL.md" }, requestedPermissions, and contributes.skills with { "id", "name", "path": "SKILL.md" }. Declare the required skills protocol version (2). Keep the frontmatter name aligned with the contribution. Mark the artifact as a draft until the packer computes the digest.`;

const PREVIOUS_STANDALONE_PLUGIN_AUTHORING = `${STANDALONE_INTRO}

${SKILL_DRAFT}
- A TypeScript UI builds separately to JavaScript. A Rust or Go contribution builds into a separate process for the target platform. Use a published and compatible HoH SDK when available. Keep the plugin's own tests and build independent of the host. Do not substitute the in-tree Rust templates.
- Check the requested extension point against the supported host API before proposing implementation. Microphone capture and inserting into the chat draft are not external SDK extension points in this build. Explain what host feature would be needed when an extension point or SDK is missing.
- This release does not include the external package loader. Do not claim that a draft is installed or enabled. Report the created files, checks actually run, and any missing SDK or runtime support. Never write directly into external_plugins or modify the running app.`;

const STANDALONE_PLUGIN_AUTHORING = `${STANDALONE_INTRO}

- For an interactive page inside HoH, build a local UI package for Linux desktop. Create a folder named for the plugin ID with hoh-package.json and dist/ui.js. The JavaScript must be self-contained and export a synchronous mount(root) function. Use the supplied root for elements and element.style for styling.
- Include formatVersion: 2, id, version, digest, ui: { "entry": "dist/ui.js" }, requestedPermissions: { "ui": ["page"] }, contributes: { "ui": { "pages": [{ "id": "main", "name": "Your page name", "handler": "mount" }] } }, and requires: { "services": { "ui": { "protocolVersions": [1] } } }. Set digest to "sha256:" followed by the lowercase SHA-256 of the exact dist/ui.js bytes. Recompute it after each change.
- Keep the plugin UI inside its own page. This surface has no HoH API, project or chat access, direct network, or persistent storage. For data the user wants to retain, discuss the missing storage capability before promising it. Only suggest a browser app if the user asks for one.
- Send the user the package folder and explain how to open it in Plugins. Mounting the page does not require Node or rebuilding HoH. Never write directly into external_plugins without the user's explicit request.
${SKILL_DRAFT}
- Check that the requested extension point is available. Skill drafts, processes, changes to the main HoH layout, microphone capture and chat draft insertion need other host support. Report the files and checks actually completed.`;

const PANEL_AUTHORING = `- For a sidebar entry and a live adjacent chat panel, use external UI protocol version 2 on Linux desktop. Keep formatVersion, id, version, digest and ui.entry as in the page package. Set requestedPermissions to { "ui": ["sidebar", "chatPanel", "reviewQueue", "openChat"] }, contributes to { "ui": { "chatPanel": { "title": "Panel title", "sidebarLabel": "Workspace label" } } }, and requires to { "services": { "ui": { "protocolVersions": [2] } } }. Supply the chatPanel contribution instead of pages.
- Export mount(root, sdk). The plugin owns its interface inside root. sdk.reviewQueue.get() returns the latest immutable array. sdk.reviewQueue.subscribe(listener) calls listener immediately and on updates and returns an unsubscribe function. Each queue entry contains threadId, title, project, state ("question", "approval", or "review"), and startedAt (Unix milliseconds). Call sdk.openChat(threadId) synchronously from a user click or keyboard activation on an entry still in the queue. Tell the user what chat metadata the plugin reads.
- Panel scripts can use inline styles. They have no general HoH API, message body, question text, project paths, direct network or persistent storage. Open the installed panel from Workspace in the sidebar after refreshing Plugins.`;

const PANEL_STANDALONE_PLUGIN_AUTHORING = STANDALONE_PLUGIN_AUTHORING.replace(
  '## Author inside the selected folder\n\n',
  '## Author inside the selected folder\n\n' + PANEL_AUTHORING + '\n\n',
);

const WORKSPACE_AUTHORING = `- For a full-size plugin page in HoH's central area, use the page package with UI protocol version 1. Add "sidebarLabel": "Your Workspace label" to its entry in contributes.ui.pages. Use mount(root) and the page permission. HoH places the page in Workspace and the plugin owns the interface and navigation inside its isolated view. Refresh Plugins after installation and open the new sidebar entry. Keep the standalone page format when the user requests a separate window.`;
const CURRENT_STANDALONE_PLUGIN_AUTHORING = PANEL_STANDALONE_PLUGIN_AUTHORING.replace(
  '## Author inside the selected folder\n\n',
  '## Author inside the selected folder\n\n' + WORKSPACE_AUTHORING + '\n\n',
);

export function pluginAuthoringPrefixes(skill: string, guide?: string): [string, ...string[]] {
  const instructions = skill.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '').trim();
  const separator = '\n\n## Plugin request\n\n';
  const implementation = instructions.split('\n\n## Implement the agreed plugin\n\n')[1];
  const previous = instructions + separator;
  const prefix: [string, ...string[]] = guide?.trim()
    ? [guide.trim() + '\n\n' + previous, previous]
    : [previous];
  prefix.unshift(
    CURRENT_STANDALONE_PLUGIN_AUTHORING + separator,
    PANEL_STANDALONE_PLUGIN_AUTHORING + separator,
    STANDALONE_PLUGIN_AUTHORING + separator,
  );
  prefix.push(PREVIOUS_STANDALONE_PLUGIN_AUTHORING + separator);
  // Keep the original user brief visible in chats written with earlier bundled instructions.
  if (implementation) prefix.push('# HoH plugin authoring\n\n' + implementation + separator);
  return prefix;
}

declare global {
  interface Window {
    __HOH_PLUGIN_CREATOR__?: PluginCreatorApi;
  }
}
