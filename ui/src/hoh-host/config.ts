// The handshake answers of the HoH host, in the encoded (JSON) form of T3's contracts
// (packages/contracts/src/server.ts `ServerConfig`, `ServerProvider`; settings.ts `ServerSettings`).
// Only Claude is offered; every optional server capability is left out, which is how a T3 server
// tells the client to hide a feature. The chats' folder is T3's own "No project" (its scratch project,
// `scratchWorkspaceRoot`): T3 offers no git, branch or worktree there. What has no capability flag is
// limited by the server's managed policy, which T3 reads the same way (`managedPolicy`).

import { englishError } from './errors.ts';

export const HOH_ENVIRONMENT_ID = 'hoh-local';
export const HOH_LABEL = 'HoH';
export const CLAUDE_INSTANCE_ID = 'claudeAgent';
/** T3's orchestration protocol (packages/contracts/src/environment.ts `ORCHESTRATION_PROTOCOL_VERSION`). */
export const ORCHESTRATION_PROTOCOL_VERSION = 2;

export type T3RuntimeMode = 'approval-required' | 'auto-accept-edits' | 'auto' | 'full-access';
export type HohAccessMode = 'confirm' | 'full';

/** HoH has two modes: Claude asks first, or works on its own. T3's in-between modes ask first. */
export function accessModeOf(mode: T3RuntimeMode): HohAccessMode {
  return mode === 'full-access' ? 'full' : 'confirm';
}
export function runtimeModeOf(mode: HohAccessMode): T3RuntimeMode {
  return mode === 'full' ? 'full-access' : 'approval-required';
}

/** `hoh/executor/discover` for Claude (ui/src/lib/executor.d.mts `DiscoverResult`). */
export interface ClaudeDiscovery {
  readonly installed: boolean;
  readonly version: string | null;
  readonly auth: 'authenticated' | 'unauthenticated' | 'unknown';
  readonly models: ReadonlyArray<{ readonly id: string; readonly displayName?: string; readonly isDefault?: boolean }>;
  readonly detail?: string | null;
  readonly failure?:
    | { readonly kind: 'timeout'; readonly operation: 'version' | 'initialize'; readonly timeoutSeconds: number }
    | { readonly kind: 'exit'; readonly operation: 'version' | 'initialize'; readonly exitCode: number | null }
    | null;
}

/** Claude Code's pointer at its own default model: not a model, and T3 does not list it. */
const CLAUDE_CODE_DEFAULT = 'default';

/**
 * T3's Claude catalog, in its order: slug, name and aliases of apps/server/src/provider/model-manifest.json
 * (`providers.claudeAgent.models`). T3 names Claude's models by it (ClaudeModelCatalog.ts
 * `resolveClaudeCatalogModel`, `claudeReportedModels`); a model it does not know keeps Claude's own name.
 */
const T3_CLAUDE_CATALOG: ReadonlyArray<readonly [string, string, ReadonlyArray<string>]> = [
  ['claude-opus-5-5', 'Claude Opus 5.5', ['opus', 'opus-5.5', 'claude-opus-5.5']],
  ['claude-sonnet-5-5', 'Claude Sonnet 5.5', ['sonnet', 'sonnet-5.5', 'claude-sonnet-5.5']],
  ['claude-fable-5-2', 'Claude Fable 5.2', ['fable', 'fable-5.2', 'claude-fable-5.2']],
  ['claude-fable-5-1', 'Claude Fable 5.1', ['fable-5.1', 'claude-fable-5.1']],
  ['claude-fable-5', 'Claude Fable 5', []],
  ['claude-opus-5', 'Claude Opus 5', ['opus-5', 'claude-opus-5.0', 'claude-opus-5-0']],
  ['claude-opus-4-8', 'Claude Opus 4.8', ['opus-4.8', 'claude-opus-4.8']],
  ['claude-opus-4-7', 'Claude Opus 4.7', ['opus-4.7', 'claude-opus-4.7']],
  ['claude-opus-4-6', 'Claude Opus 4.6', ['opus-4.6', 'claude-opus-4.6', 'claude-opus-4-6-20251117']],
  ['claude-opus-4-5', 'Claude Opus 4.5', []],
  ['claude-sonnet-5', 'Claude Sonnet 5', ['sonnet-5', 'claude-sonnet-5.0', 'claude-sonnet-5-0']],
  ['claude-sonnet-4-6', 'Claude Sonnet 4.6', ['sonnet-4.6', 'claude-sonnet-4.6', 'claude-sonnet-4-6-20251117']],
  ['claude-haiku-5-5', 'Claude Haiku 5.5', ['haiku', 'haiku-5.5', 'claude-haiku-5.5']],
  ['claude-haiku-4-5', 'Claude Haiku 4.5', ['haiku-4.5', 'claude-haiku-4.5', 'claude-haiku-4-5-20251001']],
];
/**
 * Reasoning levels per catalog model (model-manifest.json `profiles.<profile>.capabilities` "effort", `adapter.claudeCode.effortMap`):
 * the same levels and default as T3. "ultracode", "ultrathink" (T3's own multi-agent and prompt tricks) and "Fast Mode" are
 * left out: the engine runs Claude Code only. `def` is the default level, `drop` a level the model lacks, `map` T3's remap.
 */
const EFFORT_LABELS: Readonly<Record<string, string>> = { low: 'Low', medium: 'Medium', high: 'High', xhigh: 'Extra High', max: 'Max' };
const EFFORT_BY_SLUG: Readonly<Record<string, { def: string; drop?: string; map?: Record<string, string> } | null>> = {
  'claude-opus-5-5': { def: 'medium' }, 'claude-sonnet-5-5': { def: 'high' }, 'claude-fable-5-2': { def: 'medium' }, 'claude-fable-5-1': { def: 'medium' },
  'claude-fable-5': { def: 'medium' }, 'claude-opus-5': { def: 'high' }, 'claude-opus-4-8': { def: 'high' }, 'claude-opus-4-7': { def: 'xhigh', map: { xhigh: 'max' } },
  'claude-opus-4-6': { def: 'high', drop: 'xhigh' }, 'claude-opus-4-5': { def: 'high', drop: 'xhigh' }, 'claude-sonnet-5': { def: 'high' },
  'claude-sonnet-4-6': { def: 'high', drop: 'xhigh', map: { max: 'high' } }, 'claude-haiku-5-5': { def: 'medium' }, 'claude-haiku-4-5': null,
};

/** T3's model for a new Claude chat (model-manifest.json `providers.claudeAgent.defaults.chat`). */
const T3_DEFAULT_CLAUDE = 'claude-opus-5-5';

/** The catalog entry of a model id as Claude reports it: by slug or alias, without a `[1m]` or date suffix. */
function catalogIndex(id: string): number {
  const bare = id.trim().toLowerCase().replace(/\[[^\]]*\]$/, '').trim();
  const ids = [bare, bare.replace(/-\d{8}$/, '')];
  return T3_CLAUDE_CATALOG.findIndex(([slug, , aliases]) => ids.some(value => value === slug || aliases.includes(value)));
}

/**
 * The engine's Claude models as T3 lists Claude's: catalog names in catalog order, then models the catalog
 * does not know under Claude's own name; `default` and second ids of one catalog model are left out.
 * The slug stays the engine's id: it is what the engine selects.
 */
export function claudeModels(discovery: ClaudeDiscovery | null) {
  const reported = (discovery?.models ?? []).filter(model => typeof model.id === 'string' && model.id.trim() && model.id.trim() !== CLAUDE_CODE_DEFAULT);
  const known = new Map<number, { slug: string; name: string; effort?: string }>();
  const other: { slug: string; name: string; effort?: string }[] = [];
  // A plain id wins over its `[1m]` variant.
  for (const model of [...reported].sort((a, b) => Number(a.id.includes('[')) - Number(b.id.includes('[')))) {
    const index = catalogIndex(model.id);
    if (index < 0) other.push({ slug: model.id, name: (model.displayName ?? '').trim() || model.id });
    else if (!known.has(index)) known.set(index, { slug: model.id, name: T3_CLAUDE_CATALOG[index][1], effort: T3_CLAUDE_CATALOG[index][0] });
  }
  const list = [...[...known.entries()].sort(([a], [b]) => a - b).map(([, model]) => model), ...other];
  // Nothing to choose from (the engine could not ask Claude): Claude Code picks.
  if (!list.length) list.push({ slug: CLAUDE_CODE_DEFAULT, name: 'Claude' });
  return list.map(({ effort, ...model }) => {
    const levels = effort ? EFFORT_BY_SLUG[effort] : null;
    const options = levels ? Object.keys(EFFORT_LABELS).filter(id => id !== levels.drop).map(id => ({ id, label: EFFORT_LABELS[id], ...(id === levels.def ? { isDefault: true } : {}) })) : [];
    return { ...model, isCustom: false, capabilities: options.length ? { optionDescriptors: [{ id: 'effort', label: 'Reasoning', type: 'select', options }] } : null };
  });
}

/** The level for the engine: the person's choice in T3's model menu, as T3 maps it for that model (`effortMap`); null when none or unknown. */
export function effortOf(model: string | undefined, selection: unknown): string | null {
  const chosen = Array.isArray(selection) ? selection.find((entry: any) => entry?.id === 'effort')?.value : undefined;
  if (typeof chosen !== 'string' || !(chosen in EFFORT_LABELS)) return null;
  const index = model ? catalogIndex(model) : -1;
  const levels = index < 0 ? null : EFFORT_BY_SLUG[T3_CLAUDE_CATALOG[index][0]];
  if (!levels || levels.drop === chosen) return null;
  return levels.map?.[chosen] ?? chosen;
}

/** T3's model for a new chat when the engine offers it, else the first one. */
export function defaultClaudeModel(discovery: ClaudeDiscovery | null): string {
  const models = claudeModels(discovery);
  return (models.find(model => T3_CLAUDE_CATALOG[catalogIndex(model.slug)]?.[0] === T3_DEFAULT_CLAUDE) ?? models[0]).slug;
}

/** The id when T3 lists it; Claude Code's `default` and ids the engine no longer offers are not. */
export function listedClaudeModel(discovery: ClaudeDiscovery | null, id: unknown): string | null {
  return typeof id === 'string' && claudeModels(discovery).some(model => model.slug === id) ? id : null;
}

/** T3's own words for Claude's state (apps/server/src/provider/ClaudeProvider.ts); the engine's are not English. */
function probeFailureMessage(failure: ClaudeDiscovery['failure']): string | undefined {
  if (!failure) return undefined;
  const operation = failure.operation === 'version' ? 'running `claude --version`' : 'waiting for `initialize`';
  if (failure.kind === 'timeout') {
    const seconds = Math.max(0, Math.round(failure.timeoutSeconds));
    return `Claude Agent CLI health check timed out after ${seconds} ${seconds === 1 ? 'second' : 'seconds'} while ${operation}.`;
  }
  const ended = failure.exitCode === null ? 'was terminated by a signal' : `exited with code ${failure.exitCode}`;
  return `Claude Agent CLI health check failed: ${failure.operation === 'version' ? '`claude --version`' : '`initialize`'} ${ended}.`;
}

function claudeMessage(discovery: ClaudeDiscovery | null, requestFailure?: unknown): string | undefined {
  if (discovery === null) return requestFailure === undefined ? undefined : englishError(requestFailure, 'Failed to execute Claude Agent CLI health check.');
  if (!discovery.installed) return 'Claude Agent CLI (`claude`) was not found on PATH.';
  if (discovery.auth === 'unauthenticated') return 'Claude Code is not authenticated. Run `claude auth login` and try again.';
  const failure = probeFailureMessage(discovery.failure);
  if (failure) return failure;
  const detail = discovery.detail?.trim();
  return detail ? englishError(detail, 'Claude Agent CLI is installed but failed to run.') : undefined;
}

/** What T3's welcome page and provider settings show: found or not, signed in or not, as the engine says. */
export function claudeProvider(discovery: ClaudeDiscovery | null, now: string, requestFailure?: unknown) {
  const ready = discovery === null
    ? requestFailure === undefined
    : discovery.installed && discovery.auth !== 'unauthenticated' && discovery.failure == null;
  return {
    instanceId: CLAUDE_INSTANCE_ID,
    driver: CLAUDE_INSTANCE_ID,
    displayName: 'Claude',
    showInteractionModeToggle: false,
    supportedRuntimeModes: ['approval-required', 'full-access'],
    supportsConversationRollback: false,
    supportsTextGeneration: false,
    enabled: true,
    installed: discovery?.installed ?? requestFailure === undefined,
    version: discovery?.version?.trim() || null,
    // Pending startup is optimistic so T3 can render the usable shell; a bounded failure replaces it.
    status: ready ? 'ready' : 'error',
    auth: { status: discovery?.auth ?? 'unknown' },
    checkedAt: now,
    ...(claudeMessage(discovery, requestFailure) === undefined ? {} : { message: claudeMessage(discovery, requestFailure) }),
    models: claudeModels(discovery),
    slashCommands: [],
    skills: [],
  };
}

/** Every `ServerSettings` field has a decoding default; only what HoH decides is sent. */
export function serverSettings(model: string, defaultMode: HohAccessMode, overrides: Record<string, unknown> = {}) {
  return {
    ...overrides,
    // "Add project" offers no repository host to clone from (T3 turns its default host off by this setting:
    // client-runtime/src/operations/projects.ts `isSourceControlProviderOffered`). HoH adds local folders.
    offeredSourceControlProviders: { github: false },
    defaultModelSelection: { instanceId: CLAUDE_INSTANCE_ID, model },
    // The mode a new thread starts in (T3's setting), not the mode in effect: T3 shows the mode chip under
    // the composer only while a thread's mode differs from it (chat/composerControlsOwnership.ts).
    defaultRuntimeMode: runtimeModeOf(defaultMode),
  };
}

export interface ConfigInput {
  readonly cwd: string;
  readonly discovery: ClaudeDiscovery | null;
  readonly discoveryFailure?: unknown;
  readonly defaultMode: HohAccessMode;
  readonly settingsOverrides?: Record<string, unknown>;
  readonly now: string;
}

/**
 * Limits T3 reads from a managed server (packages/contracts/src/managedPolicy.ts): only Claude runs, so
 * the welcome page and the add-provider dialog offer no other agent and no ACP registry; no T3 Connect
 * (phone, guest links); no usage data is collected, which the welcome page says instead of T3's notice.
 */
export const HOH_MANAGED_POLICY = {
  organization: HOH_LABEL,
  lockedSettings: [],
  limits: {
    connect: false,
    telemetry: false,
    providers: { allowedDrivers: [CLAUDE_INSTANCE_ID] },
  },
};

export function serverConfig(input: ConfigInput) {
  const model = defaultClaudeModel(input.discovery);
  return {
    environment: {
      environmentId: HOH_ENVIRONMENT_ID,
      label: HOH_LABEL,
      platform: { os: 'linux', arch: 'x64' },
      serverVersion: '0.0.0-hoh',
      orchestrationProtocolVersion: ORCHESTRATION_PROTOCOL_VERSION,
      capabilities: { repositoryIdentity: false, connectionProbe: true, threadForceStop: true, inlineMessageContext: true, providerWorkspaceSnapshotsOnDemand: true },
    },
    auth: {
      policy: 'desktop-managed-local',
      bootstrapMethods: ['desktop-bootstrap'],
      sessionMethods: ['browser-session-cookie'],
      sessionCookieName: 'hoh_session',
    },
    cwd: input.cwd,
    // The folder HoH runs chats in is T3's "No project" (apps/server ManagedProjectFolders): no git there.
    scratchWorkspaceRoot: input.cwd,
    managedPolicy: HOH_MANAGED_POLICY,
    keybindingsConfigPath: input.cwd.replace(/\/$/, '') + '/.hoh-keybindings.json',
    keybindings: [],
    issues: [],
    providers: [claudeProvider(input.discovery, input.now, input.discoveryFailure)],
    availableEditors: [],
    observability: {
      logsDirectoryPath: '/hoh/logs',
      localTracingEnabled: false,
      otlpTracesEnabled: false,
      otlpMetricsEnabled: false,
      otlpLogsEnabled: false,
    },
    settings: serverSettings(model, input.defaultMode, input.settingsOverrides),
    serverTime: input.now,
  };
}
