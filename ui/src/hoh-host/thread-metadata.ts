// Durable window-owned thread metadata: the engine exposes no session mutation RPCs yet.
import type { HohStorage } from './host.ts';
import type { OrchestrationV2ThreadOpens } from '@t3tools/contracts';
import type { SessionListResult } from '../../../protocol/core.ts';
type SessionSideChat = NonNullable<SessionListResult['sessions'][number]['sideChat']>;

export const THREAD_METADATA_KEY = 'hoh-host.thread-metadata.v1';

export interface ThreadOrganization {
  lastVisitedAt: number | null;
  opens: OrchestrationV2ThreadOpens | null;
  settledOverride: 'settled' | 'active' | null;
  settledAt: number | null;
  unsettledAt: number | null;
  snoozedUntil: number | null;
  snoozedAt: number | null;
  lastSnoozeWakeAt: number | null;
  autoSettleDisabledAt: number | null;
  activeOrderKey: string | null;
  tags: readonly string[];
}

export interface ThreadMetadata extends Partial<ThreadOrganization> {
  title?: string;
  sideChat?: SessionSideChat;
  pinnedAt?: number | null;
  pinOrderKey?: string | null;
  archivedAt?: number | null;
  promptTimes?: Record<string, number>;
  stopTimes?: Record<string, number>;
  appliedProjectTrust?: { cwd: string; executor: string } | null;
}

export const threadTag = (value: unknown) => {
  const tag = typeof value === 'string' ? value.trim() : '';
  return tag && tag.length <= 64 && !/[\p{Cc}]/u.test(tag) ? tag : null;
};

export function readThreadOrganization(value: Partial<ThreadOrganization>): ThreadOrganization {
  const date = (field: unknown) => typeof field === 'number' && Math.abs(field) <= 8.64e15 ? field : null;
  const opens = value.opens;
  return {
    lastVisitedAt: date(value.lastVisitedAt),
    opens: opens && date(opens.openedAt) !== null && Number.isFinite(opens.score) && opens.score >= 0
      && Array.isArray(opens.picks) && opens.picks.length <= 3
      && opens.picks.every(pick => typeof pick?.query === 'string' && pick.query.length <= 32 && date(pick.at) !== null)
      ? opens : null,
    settledOverride: value.settledOverride === 'settled' || value.settledOverride === 'active' ? value.settledOverride : null,
    settledAt: date(value.settledAt),
    unsettledAt: date(value.unsettledAt),
    snoozedUntil: date(value.snoozedUntil),
    snoozedAt: date(value.snoozedAt),
    lastSnoozeWakeAt: date(value.lastSnoozeWakeAt),
    autoSettleDisabledAt: date(value.autoSettleDisabledAt),
    activeOrderKey: typeof value.activeOrderKey === 'string' ? value.activeOrderKey.trim() || null : null,
    tags: Array.isArray(value.tags) ? [...new Set(value.tags.map(threadTag).filter(tag => tag !== null))] : [],
  };
}

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
