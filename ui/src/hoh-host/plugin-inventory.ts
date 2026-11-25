import type { HohEngine } from './engine.ts';

export interface PluginInventoryItem {
  id: string;
  manifest: { name?: string; version?: string; description?: string; panel?: { title?: string; fields?: Array<{ key: string; label: string }> } };
  status: string;
  diagnostic: string | null;
  data: Record<string, unknown> | null;
}

/** A narrow window-facing method; the page cannot invoke arbitrary engine methods. */
export function pluginInventory(engine: HohEngine) {
  return {
    async list(sessionId?: string): Promise<PluginInventoryItem[]> {
      const result: unknown = await engine.request('hoh/plugins/list', sessionId ? { sessionId } : {});
      if (!Array.isArray(result)) throw new Error('Invalid plugin inventory');
      return result as PluginInventoryItem[];
    },
  };
}
