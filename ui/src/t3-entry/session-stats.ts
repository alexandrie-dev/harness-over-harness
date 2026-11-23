// HOH-43 Part A: the protocol/host source is wired in Part B after cx-sb/cx-pa/cx-codex land.
// Until then the panel consumes this exact snapshot shape. Rich data exists only when an explicit
// QA build sets VITE_HOH_SESSION_STATS_FIXTURE=1; normal release builds receive the empty snapshot.

export type HohProviderKind = 'claude' | 'codex';
export type HohActivityStatus = 'running' | 'finished' | 'failed' | 'unknown';

export interface HohTokenUsage {
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly cacheReadTokens?: number;
  readonly cacheWriteTokens?: number;
  readonly reasoningTokens?: number;
}

export interface HohSessionStatsSnapshot {
  readonly revision: number;
  readonly provider?: HohProviderKind;
  readonly goal?: string;
  readonly plan?: { readonly source: 'todoWrite' | 'exitPlanMode'; readonly text: string };
  readonly tokens: {
    readonly turn?: HohTokenUsage;
    readonly conversation?: HohTokenUsage;
    readonly providerTotal?: HohTokenUsage;
  };
  readonly costUsd?: number;
  readonly context?: {
    readonly usedTokens?: number;
    readonly maxTokens?: number;
    readonly autoCompactThreshold?: number;
  };
  readonly model?: string;
  readonly effort?: string;
  readonly mode?: string;
  readonly startedAtMs?: number;
  readonly updatedAtMs?: number;
  readonly runtimeMs?: number;
  readonly turnCount?: number;
  readonly toolCallCount?: number;
  readonly skills: {
    readonly available?: readonly string[];
    readonly invoked?: readonly { readonly name: string; readonly callId: string }[];
  };
  readonly mcpServers?: readonly {
    readonly name: string;
    readonly status: 'connected' | 'pending' | 'needs-auth' | 'failed' | 'unknown';
  }[];
  readonly backgroundJobs?: readonly {
    readonly id: string;
    readonly kind: 'scheduleWakeup' | 'cron' | 'bash' | 'monitor' | 'agent' | 'other';
    readonly label: string;
    readonly status: HohActivityStatus;
  }[];
  readonly subagents?: readonly {
    readonly callId: string;
    readonly kind: 'claude' | 'codex';
    readonly name: string;
    readonly description?: string;
    readonly status: HohActivityStatus;
    readonly note?: string;
  }[];
}

const EMPTY_SNAPSHOT: HohSessionStatsSnapshot = {
  revision: 0,
  tokens: {},
  skills: {},
};

const QA_FIXTURE: HohSessionStatsSnapshot = {
  revision: 43,
  provider: 'claude',
  goal: 'Подготовить панель полной статистики разговора на актуальное время.',
  plan: {
    source: 'todoWrite',
    text: 'Собрать события провайдера → проверить токены → подготовить проводку части B',
  },
  tokens: {
    turn: {
      inputTokens: 42,
      outputTokens: 1_284,
      cacheReadTokens: 38_912,
      cacheWriteTokens: 2_048,
    },
    conversation: {
      inputTokens: 318,
      outputTokens: 8_742,
      cacheReadTokens: 214_590,
      cacheWriteTokens: 18_432,
    },
    providerTotal: {
      inputTokens: 391,
      outputTokens: 10_126,
      cacheReadTokens: 249_301,
      cacheWriteTokens: 25_920,
    },
  },
  costUsd: 1.2847,
  context: { usedTokens: 118_420, maxTokens: 200_000, autoCompactThreshold: 160_000 },
  model: 'claude-opus-4-6',
  effort: 'high',
  mode: 'acceptEdits',
  runtimeMs: 1_487_000,
  turnCount: 7,
  toolCallCount: 31,
  skills: {
    available: ['do-best-no-mistakes', 'frontend-design', 'security-audit', 'simplify'],
    invoked: [
      { name: 'do-best-no-mistakes', callId: 'skill-1' },
      { name: 'frontend-design', callId: 'skill-2' },
    ],
  },
  mcpServers: [
    { name: 'Claude Docs', status: 'connected' },
    { name: 'Playwright', status: 'failed' },
  ],
  backgroundJobs: [
    { id: 'bg-1', kind: 'bash', label: 'Сборка фикстур', status: 'finished' },
    { id: 'bg-2', kind: 'monitor', label: 'Ожидание проверки', status: 'running' },
    { id: 'loop-1', kind: 'scheduleWakeup', label: 'Цикл проверки через 30 минут', status: 'unknown' },
  ],
  subagents: [
    {
      callId: 'agent-research',
      kind: 'claude',
      name: 'researcher',
      description: 'Сверяет учёт токенов Claude',
      status: 'finished',
    },
    {
      callId: 'agent-ui',
      kind: 'claude',
      name: 'ui-reviewer',
      description: 'Проверяет сетку панели',
      status: 'running',
    },
    {
      callId: 'bash-codex',
      kind: 'codex',
      name: 'Codex',
      description: 'codex exec review',
      status: 'running',
      note: 'внутренние шаги не видны',
    },
  ],
};

export function getHohSessionStatsSnapshot(): HohSessionStatsSnapshot {
  return import.meta.env.VITE_HOH_SESSION_STATS_FIXTURE === '1' ? QA_FIXTURE : EMPTY_SNAPSHOT;
}
