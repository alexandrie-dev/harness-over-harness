import type { HohExecutor } from './config.ts';

export const MAX_PLUGIN_BRIEF = 8_000;
export const PLUGIN_CREATOR_CHECKOUT_FILE = 'crates/hoh-plugin-creator/Cargo.toml';

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

export function pluginAuthoringPrefixes(skill: string): [string, ...string[]] {
  const instructions = skill.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '').trim();
  const separator = '\n\n## Plugin request\n\n';
  const implementation = instructions.split('\n\n## Implement the agreed plugin\n\n')[1];
  const current = instructions + separator;
  // Preserve the short message bubble for chats written before the interview was bundled.
  return implementation ? [current, '# HoH plugin authoring\n\n' + implementation + separator] : [current];
}

declare global {
  interface Window {
    __HOH_PLUGIN_CREATOR__?: PluginCreatorApi;
  }
}
