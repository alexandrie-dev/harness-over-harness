// Durable window-owned thread metadata: the engine exposes no session mutation RPCs yet.
import type { HohStorage } from './host.ts';

export const THREAD_METADATA_KEY = 'hoh-host.thread-metadata.v1';
export interface ThreadMetadata { title?: string; pinnedAt?: number | null; pinOrderKey?: string | null; archivedAt?: number | null; promptTimes?: Record<string, number> }

export class ThreadMetadataStore {
  constructor(private readonly storage: HohStorage | null) {}
  private read(): Record<string, ThreadMetadata> {
    if (!this.storage) throw new Error('Conversation settings cannot be saved on this device.');
    const raw = this.storage.getItem(THREAD_METADATA_KEY);
    const value = raw === null ? {} : JSON.parse(raw);
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid conversation settings.');
    return Object.assign(Object.create(null), value);
  }
  get(id: string): ThreadMetadata {
    try { return this.read()[id] ?? {}; } catch { return {}; }
  }
  update(id: string, patch: ThreadMetadata): ThreadMetadata {
    try {
      const all = this.read();
      const next = { ...all[id], ...patch };
      all[id] = next;
      this.storage!.setItem(THREAD_METADATA_KEY, JSON.stringify(all));
      return next;
    } catch { throw new Error('Conversation settings could not be saved on this device.'); }
  }
}
