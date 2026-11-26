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

const STANDALONE_PLUGIN_AUTHORING = `# Author an external HoH plugin

The selected folder belongs to the author. The installed HoH binary is enough to start this workflow. Do not require Git, a HoH checkout, or a build of the app. Keep plugin files inside the selected folder. The user profile's external_plugins directory is for installed artifacts, not source projects.

## Start with an interview

Use the author's language and ask exactly one design question per turn. Offer a recommendation and a short reason. The first reply must ask a question, even if it is only to confirm the plan. Use a structured question tool when available. Before writing files, show the proposed files, permissions and checks. Wait for approval before scaffolding or implementing. If scope changes, confirm the revised plan.

## Author inside the selected folder

- For a Markdown skill draft, create a folder with SKILL.md and hoh-package.json. The manifest draft uses formatVersion 2, the plugin id and version, execution { "kind": "skill", "entry": "SKILL.md" }, requestedPermissions, and contributes.skills with { "id", "name", "path": "SKILL.md" }. Declare the required skills protocol version (2). Keep the frontmatter name aligned with the contribution. Mark the artifact as a draft until the packer computes the digest.
- A TypeScript UI builds separately to JavaScript. A Rust or Go contribution builds into a separate process for the target platform. Use a published and compatible HoH SDK when available. Keep the plugin's own tests and build independent of the host. Do not substitute the in-tree Rust templates.
- Check the requested extension point against the supported host API before proposing implementation. Microphone capture and inserting into the chat draft are not external SDK extension points in this build. Explain what host feature would be needed when an extension point or SDK is missing.
- This release does not include the external package loader. Do not claim that a draft is installed or enabled. Report the created files, checks actually run, and any missing SDK or runtime support. Never write directly into external_plugins or modify the running app.`;

export function pluginAuthoringPrefixes(skill: string, guide?: string): [string, ...string[]] {
  const instructions = skill.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '').trim();
  const separator = '\n\n## Plugin request\n\n';
  const implementation = instructions.split('\n\n## Implement the agreed plugin\n\n')[1];
  const previous = instructions + separator;
  const prefix: [string, ...string[]] = guide?.trim()
    ? [guide.trim() + '\n\n' + previous, previous]
    : [previous];
  prefix.unshift(STANDALONE_PLUGIN_AUTHORING + separator);
  // Keep the original user brief visible in chats written with earlier bundled instructions.
  if (implementation) prefix.push('# HoH plugin authoring\n\n' + implementation + separator);
  return prefix;
}

declare global {
  interface Window {
    __HOH_PLUGIN_CREATOR__?: PluginCreatorApi;
  }
}
