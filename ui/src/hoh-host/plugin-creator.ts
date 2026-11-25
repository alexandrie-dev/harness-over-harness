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

export function pluginAuthoringPrompt(skill: string, brief: string): string {
  // The shipped skill goes in the first turn for both harnesses, including sessions
  // where project-local skill discovery is disabled by the normal trust policy.
  const instructions = skill.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '').trim();
  return `${instructions}\n\n## Plugin request\n\n${brief}`;
}

declare global {
  interface Window {
    __HOH_PLUGIN_CREATOR__?: PluginCreatorApi;
  }
}
