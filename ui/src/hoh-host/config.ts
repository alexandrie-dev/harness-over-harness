// The handshake answers of the HoH host, in the encoded (JSON) form of T3's contracts
// (packages/contracts/src/server.ts `ServerConfig`, `ServerProvider`; settings.ts `ServerSettings`).
// Only Claude is offered; every optional server capability is left out, which is how a T3 server
// tells the client to hide a feature. The chats' folder is T3's own "No project" (its scratch project,
// `scratchWorkspaceRoot`): T3 offers no git, branch or worktree there. What has no capability flag is
// limited by the server's managed policy, which T3 reads the same way (`managedPolicy`).

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
}

export const DEFAULT_CLAUDE_MODEL = 'default';

export function claudeModels(discovery: ClaudeDiscovery | null) {
  const models = (discovery?.models ?? []).filter(model => typeof model.id === 'string' && model.id.trim());
  const list = models.length ? models : [{ id: DEFAULT_CLAUDE_MODEL, displayName: 'Claude', isDefault: true }];
  return list.map(model => ({
    slug: model.id,
    name: (model.displayName ?? '').trim() || model.id,
    isCustom: false,
    ...(model.isDefault ? { isDefault: true } : {}),
    capabilities: null,
  }));
}

export function defaultClaudeModel(discovery: ClaudeDiscovery | null): string {
  const models = claudeModels(discovery);
  return (models.find(model => model.isDefault) ?? models[0]).slug;
}

/** What T3's welcome page and provider settings show: found or not, signed in or not, as the engine says. */
export function claudeProvider(discovery: ClaudeDiscovery | null, now: string) {
  const ready = discovery !== null && discovery.installed && discovery.auth !== 'unauthenticated';
  return {
    instanceId: CLAUDE_INSTANCE_ID,
    driver: CLAUDE_INSTANCE_ID,
    displayName: 'Claude',
    showInteractionModeToggle: false,
    supportedRuntimeModes: ['approval-required', 'full-access'],
    supportsConversationRollback: false,
    supportsTextGeneration: false,
    enabled: true,
    installed: discovery?.installed ?? false,
    version: discovery?.version?.trim() || null,
    // An engine that could not look is not "still checking" (T3 would wait for ever): Claude is not found.
    status: ready ? 'ready' : 'error',
    auth: { status: discovery?.auth ?? 'unknown' },
    checkedAt: now,
    ...(discovery === null ? { message: 'HoH could not check Claude.' } : discovery.detail?.trim() ? { message: discovery.detail.trim() } : {}),
    models: claudeModels(discovery),
    slashCommands: [],
    skills: [],
  };
}

/** Every `ServerSettings` field has a decoding default; only what HoH decides is sent. */
export function serverSettings(model: string, mode: HohAccessMode, overrides: Record<string, unknown> = {}) {
  return {
    ...overrides,
    defaultModelSelection: { instanceId: CLAUDE_INSTANCE_ID, model },
    defaultRuntimeMode: runtimeModeOf(mode),
  };
}

export interface ConfigInput {
  readonly cwd: string;
  readonly discovery: ClaudeDiscovery | null;
  readonly mode: HohAccessMode;
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
      capabilities: { repositoryIdentity: false, connectionProbe: true, threadForceStop: true, inlineMessageContext: true },
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
    providers: [claudeProvider(input.discovery, input.now)],
    availableEditors: [],
    observability: {
      logsDirectoryPath: '/hoh/logs',
      localTracingEnabled: false,
      otlpTracesEnabled: false,
      otlpMetricsEnabled: false,
      otlpLogsEnabled: false,
    },
    settings: serverSettings(model, input.mode, input.settingsOverrides),
    serverTime: input.now,
  };
}
