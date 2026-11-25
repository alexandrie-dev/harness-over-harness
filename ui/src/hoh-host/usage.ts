import type { ThreadModel } from './threads.ts';

// Wire shape for server.getUsageSummary in the vendored T3 usage contract.
export interface UsageSummaryInput {
  sinceDay: string;
  untilDay: string;
  timeZone: string;
  resolution?: 'day' | 'hour';
  sinceTime?: string;
  untilTime?: string;
  modelFilter?: {
    include?: readonly { provider?: string; model: string }[];
    exclude?: readonly { provider?: string; model: string }[];
  };
}

interface UsageBucket {
  provider: 'claude' | 'codex';
  day: string;
  hourStart?: string;
  model: string;
  sourcePath: string;
  totals: {
    uncachedInputTokens: number;
    cachedInputTokens: number;
    cacheCreationTokens: number;
    outputTokens: number;
    reasoningTokens: number;
  };
  costUsd: number;
  cacheSavingsUsd: number;
  costSource: 'unpriced';
  records: number;
  unpricedRecords: number;
  sessions: number;
}

export const usagePricing = { status: 'unavailable', source: 'HoH', fetchedAt: null, knownModels: 0 } as const;

export function usageSummary(
  input: UsageSummaryInput,
  threads: readonly ThreadModel[],
  inspected: ReadonlySet<string>,
) {
  const started = Date.now();
  const { timeZone, sinceDay, untilDay, sinceTime, untilTime, modelFilter } = input;
  const date = new Intl.DateTimeFormat('sv-SE', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' });
  const hour = new Intl.DateTimeFormat('en-GB', { timeZone, hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
  const buckets: UsageBucket[] = [];
  const sources = [];
  for (const provider of ['claude', 'codex'] as const) {
    const sessions = threads.filter(thread => thread.executor === provider);
    const sourcePath = `hoh:chat-history:${provider}`;
    let distinctSessions = 0;
    for (const thread of sessions) {
      const byBucket = new Map<string, UsageBucket>();
      for (const { model, usage, cacheCreationTokens } of thread.usageTurns()) {
        const matches = (selector: { provider?: string; model: string }) =>
          (!selector.provider || selector.provider === provider) && selector.model === model;
        if ((modelFilter?.include && !modelFilter.include.some(matches)) || modelFilter?.exclude?.some(matches)) continue;
        const at = new Date(usage.updatedAt);
        const day = date.format(at);
        if (day < sinceDay || day > untilDay || (sinceTime && usage.updatedAt < sinceTime) || (untilTime && usage.updatedAt >= untilTime)) continue;
        const minute = Number(hour.formatToParts(at).find(part => part.type === 'minute')?.value ?? 0);
        at.setUTCSeconds(0, 0);
        at.setUTCMinutes(at.getUTCMinutes() - minute);
        const hourStart = input.resolution === 'hour' ? at.toISOString() : undefined;
        const key = JSON.stringify([hourStart ?? day, model]);
        let bucket = byBucket.get(key);
        if (!bucket) {
          bucket = {
            provider, day, ...(hourStart ? { hourStart } : {}), model, sourcePath,
            totals: { uncachedInputTokens: 0, cachedInputTokens: 0, cacheCreationTokens: 0, outputTokens: 0, reasoningTokens: 0 },
            costUsd: 0, cacheSavingsUsd: 0, costSource: 'unpriced', records: 0, unpricedRecords: 0, sessions: 1,
          };
          byBucket.set(key, bucket);
        }
        const totals = {
          uncachedInputTokens: bucket.totals.uncachedInputTokens + Math.max(0, usage.inputTokens - usage.cachedInputTokens - cacheCreationTokens),
          cachedInputTokens: bucket.totals.cachedInputTokens + usage.cachedInputTokens,
          cacheCreationTokens: bucket.totals.cacheCreationTokens + cacheCreationTokens,
          outputTokens: bucket.totals.outputTokens + usage.outputTokens,
          reasoningTokens: bucket.totals.reasoningTokens + usage.reasoningOutputTokens,
        };
        byBucket.set(key, { ...bucket, totals, records: bucket.records + 1, unpricedRecords: bucket.unpricedRecords + 1 });
      }
      if (byBucket.size) distinctSessions++;
      buckets.push(...byBucket.values());
    }
    const scannedFiles = sessions.filter(thread => thread.loaded || inspected.has(thread.id)).length;
    const skippedFiles = sessions.length - scannedFiles;
    sources.push({
      fingerprint: { hostId: 'hoh-local', provider, resolvedHomePath: sourcePath, volumeId: '' },
      status: skippedFiles ? 'partial' : 'ok', scannedFiles, skippedFiles, malformedRecords: 0,
      distinctSessions, message: skippedFiles ? `Usage history is incomplete for ${skippedFiles} HoH chat(s).` : null,
    });
  }
  return {
    contractVersion: 6, readAt: new Date().toISOString(), timeZone, sinceDay, untilDay,
    buckets, sources, pricing: usagePricing,
    scanDurationMs: Date.now() - started, ...(modelFilter ? { modelFilter } : {}),
  };
}
