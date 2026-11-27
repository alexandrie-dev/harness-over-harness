// The HoH host: T3's server methods (WsRpcGroup) answered by the HoH engine.
// Handshake: server.getConfig / subscribeServerConfig / probe / getSettings / updateSettings.
// Chats: orchestration.subscribeShell (the list), subscribeThread / getThreadProjection (one chat),
// launchThread and dispatchCommand (new chat, send, Stop, permission and question answers, mode, model),
// forceStopThread. A thread change is sent as a fresh snapshot of that thread, which T3's client applies
// whole (client-runtime/src/state/threads.ts `applyItemLocked`), plus `thread.updated` for the list.
// Projects: T3's "No project" (the engine's default folder), listed once a chat is in it or once T3 asks for it
// (`projects.ensureScratch`), so an engine without chats is a fresh workspace and T3 opens its welcome page;
// and folders the person adds ("Add project", `projects.mutate`), listed through `filesystem.browse`. A chat
// in a folder project works in that folder: the host sets it with the same engine request as the current
// window's folder choice (`session/working-directory/set`), before the first message. Folder projects and
// which project each chat is in are kept in the window's storage, so they survive a restart; a chat whose
// history names a project's folder (`working-directory/change`) is put in that project.
// The composer (HOH-41): `/` and `$` are what Claude declares for the composer's folder (`server.refreshProviders`
// with a cwd answers T3's workspace snapshot from `hoh/executor/commands`; "Add project" tells the engine the folder,
// `hoh/workspace/add`, so both answer before the project's first chat); `@` lists a folder project's files
// (`projects.searchEntries` → `hoh/workspace/entries`, nothing for "No project"); pictures are checked and kept
// (`assets.persistChatAttachments`) until their message goes to the engine with them (`session/prompt`).
import { WorkspaceGit } from './workspace-git.ts';
import {
  accessModeOf, CLAUDE_INSTANCE_ID, CODEX_INSTANCE_ID, PI_INSTANCE_ID, EXECUTOR_INSTANCES, HOH_EXECUTORS, isHohExecutor, piModel, piProvider, claudeProvider, codexEffortOf, codexProvider, defaultClaudeModel, defaultCodexModel, effortOf, listedClaudeModel, listedCodexModel, runtimeModeOf, serverConfig, serverSettings, type ClaudeDiscovery, type CodexDiscovery, type HarnessDiscovery, type HohAccessMode, type HohExecutor, type T3RuntimeMode,
  isAccessMode, KEYBINDINGS_LOCATION, presentProvider, type EnvironmentPlatform, type T3InteractionMode,
} from './config.ts';
import { usagePricing, usageSummary, type UsageSummaryInput } from './usage.ts';
import type { EngineNotification, EngineQuestion, EngineQuestionAnswer, HohEngine, LocalImageAsset } from './engine.ts';
import { base64Of, readUploads, searchAnswer, skillsAsCommands, stripAttachedImageReferences, UPLOAD_PATH, UPLOAD_TTL_MS, uploadSlot, workspaceSnapshot, type KeptImage, type UploadSlot } from './composer.ts';
import { englishError, hostFailure } from './errors.ts';
import { projectFileHandlers } from './project-files.ts';
import { folderProject, HOH_PROJECT_ID, hohProject, ThreadModel, type FolderProject, type HomeEntry, type Item, type ShownAttachment } from './threads.ts';
import type { SessionForkForEditResult } from '../protocol.ts';
import { RpcFailure, type Handlers, type StreamSink } from './wire.ts';
import { ThreadMetadataStore, threadTag, type ThreadMetadata, type ThreadOrganization } from './thread-metadata.ts';
import {
  isSecretaryThreadId, readSecretaryProposal, secretaryPromptPrefix, SECRETARY_THREAD_PREFIX,
  SECRETARY_TITLE, type SecretaryChatApi,
} from './secretary-chat.ts';
import type { FindInput, FindResult } from './thread-search.ts';
import type { CreateHohLocalSettings } from '../t3-entry/local-settings.ts';
import {
  MAX_PLUGIN_BRIEF, pluginAuthoringPrefixes,
  type PluginCreatorApi, type PluginCreatorOptions,
} from './plugin-creator.ts';

export interface HohHostOptions {
  readonly engine: HohEngine;
  readonly platform?: EnvironmentPlatform;
  readonly now?: () => number;
  /** Folder shown as the project root until the engine names one. */
  readonly workspaceRoot?: string;
  /** Delay that groups a burst of changes (stream chunks) into one snapshot. */
  readonly throttleMs?: number;
  /** Where folder projects are kept between runs (the window's localStorage by default; null: not kept). */
  readonly storage?: HohStorage | null;
  readonly createLocalSettings?: CreateHohLocalSettings;
  /** The engine may still be starting when the window asks (its handshake takes seconds): the first reads are repeated this often, and this many times, until it answers. */
  readonly loadRetryMs?: number;
  readonly loadRetries?: number;
  /** Test seam for the bounded race between optimistic media rendering and draft registration. */
  readonly localImageContextGraceMs?: number;
}

export interface HohStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface HomeDisposition {
  /** Run ordinal is stable when the engine history is replayed after a restart. */
  turnOrdinal: number;
  state: 'waiting' | 'done';
  markedAt?: number;
  /** This exact turn entered Home because its durable history ended mid-run, not because the person pressed Stop. */
  interrupted?: boolean;
  /** This exact turn was deliberately stopped by the person; it is not a crash-recovery placeholder. */
  stopped?: boolean;
}

/** The reasoning level goes to the engine as `effort` of `session/prompt` (agreed with the engine's writer). */
const effortParam = (effort: string | null) => effort ? { effort } : {};
const isInteractionMode = (value: unknown): value is T3InteractionMode => value === 'default' || value === 'plan';
const interactionModeOf = (value: unknown): T3InteractionMode => {
  if (value === undefined) return 'default';
  if (isInteractionMode(value)) return value;
  throw new Error('Unknown interaction mode.');
};

export const STORAGE_KEY = 'hoh-host.projects.v1';
const PLACEHOLDER_ROOT = '~/Documents/HoH';
const LOCAL_IMAGE_BYTES = 20 * 1024 * 1024;
const LOCAL_IMAGE_TYPES = new Set(['image/gif', 'image/jpeg', 'image/png', 'image/webp']);
const LOCAL_IMAGE_MAX_RETAINED_BYTES = 128 * 1024 * 1024;
const LOCAL_IMAGE_MAX_RETAINED_ENTRIES = 64;
const LOCAL_IMAGE_THREAD_MAX_RETAINED_BYTES = 64 * 1024 * 1024;
const LOCAL_IMAGE_THREAD_MAX_RETAINED_ENTRIES = 32;
const LOCAL_IMAGE_MAX_CANVAS_PIXELS = 160_000_000;
const LOCAL_IMAGE_THREAD_MAX_CANVAS_PIXELS = 80_000_000;
const LOCAL_IMAGE_MAX_DECODED_FRAME_PIXELS = 400_000_000;
const LOCAL_IMAGE_THREAD_MAX_DECODED_FRAME_PIXELS = 200_000_000;
const LOCAL_IMAGE_MAX_ANIMATION_PIXELS_PER_SECOND = 400_000_000;
const LOCAL_IMAGE_THREAD_MAX_ANIMATION_PIXELS_PER_SECOND = 200_000_000;
const LOCAL_IMAGE_REQUEST_CACHE_MAX = 128;
const LOCAL_IMAGE_FAILURE_TTL_MS = 5_000;
const LOCAL_IMAGE_REUSE_MS = 30_000;
const LOCAL_IMAGE_MAX_INFLIGHT = 6;
const LOCAL_IMAGE_THREAD_MAX_INFLIGHT = 3;
const LOCAL_IMAGE_MAX_PENDING_READS = 64;
const LOCAL_IMAGE_CONTEXT_GRACE_MS = 2_000;
const LOCAL_IMAGE_ATTEMPT_MAX = 64;
const LOCAL_IMAGE_GLOBAL_ATTEMPT_MAX = 512;
const LOCAL_IMAGE_ATTEMPT_WINDOW_MS = 10 * 60_000;
const LOCAL_IMAGE_ATTEMPT_THREADS_MAX = LOCAL_IMAGE_GLOBAL_ATTEMPT_MAX;
const LOCAL_IMAGE_THREAD_MAX_IDENTITIES = 256;

type LocalImageUrl = {
  url: string; revision: string; sourcePath: string; sizeBytes: number; threadId: string;
  decodeProfile: LocalImageDecodeProfile;
  requestKeys: Map<string, { cwd: string; readAt: number } | null>;
  confirmedPath?: string;
};
export type LocalImageDecodeProfile = {
  canvasPixels: number; decodedFramePixels: number;
  animationPixelsPerSecond: number; frameCount: number;
};
type LocalImageFailure = { message: string; expiresAt: number };
type LocalImageReservation = {
  threadId: string; reservedBytes: number; replacing: boolean; reservesIdentity: boolean;
};
type LocalImageContextWaiter = {
  resolve: (thread: ThreadModel | undefined) => void;
  graceTimer?: ReturnType<typeof setTimeout>;
};
type LocalImageThreadContext = {
  thread: ThreadModel;
  /** The media RPC won the race with both draft lifecycle RPCs. */
  admittedBeforeRegistration: boolean;
};

class PersistentLocalImageContextError extends Error {}
class LocalImageContextCapacityError extends Error {}
class LocalImageAttemptLimitError extends Error {}

function lruGet<K, V>(map: Map<K, V>, key: K): V | undefined {
  const value = map.get(key);
  if (value === undefined) return undefined;
  map.delete(key);
  map.set(key, value);
  return value;
}

function lruSet<K, V>(map: Map<K, V>, key: K, value: V, maximum: number) {
  map.delete(key);
  map.set(key, value);
  while (map.size > maximum) map.delete(map.keys().next().value!);
}

export function localImageRetentionFits(input: {
  readonly globalBytes: number; readonly globalEntries: number;
  readonly threadBytes: number; readonly threadEntries: number;
  readonly replacedBytes: number; readonly replacing: boolean; readonly sizeBytes: number;
}) {
  const addedEntries = input.replacing ? 0 : 1;
  return input.globalEntries + addedEntries <= LOCAL_IMAGE_MAX_RETAINED_ENTRIES
    && input.globalBytes - input.replacedBytes + input.sizeBytes <= LOCAL_IMAGE_MAX_RETAINED_BYTES
    && input.threadEntries + addedEntries <= LOCAL_IMAGE_THREAD_MAX_RETAINED_ENTRIES
    && input.threadBytes - input.replacedBytes + input.sizeBytes <= LOCAL_IMAGE_THREAD_MAX_RETAINED_BYTES;
}

export function localImageDecodeRetentionFits(input: {
  readonly global: LocalImageDecodeProfile;
  readonly thread: LocalImageDecodeProfile;
  readonly replaced: LocalImageDecodeProfile;
  readonly incoming: LocalImageDecodeProfile;
}) {
  const after = (scope: LocalImageDecodeProfile, maximum: LocalImageDecodeProfile) =>
    scope.canvasPixels - input.replaced.canvasPixels + input.incoming.canvasPixels <= maximum.canvasPixels
    && scope.decodedFramePixels - input.replaced.decodedFramePixels + input.incoming.decodedFramePixels <= maximum.decodedFramePixels
    && scope.animationPixelsPerSecond - input.replaced.animationPixelsPerSecond + input.incoming.animationPixelsPerSecond <= maximum.animationPixelsPerSecond;
  return after(input.global, {
    canvasPixels: LOCAL_IMAGE_MAX_CANVAS_PIXELS,
    decodedFramePixels: LOCAL_IMAGE_MAX_DECODED_FRAME_PIXELS,
    animationPixelsPerSecond: LOCAL_IMAGE_MAX_ANIMATION_PIXELS_PER_SECOND,
    frameCount: Number.MAX_SAFE_INTEGER,
  }) && after(input.thread, {
    canvasPixels: LOCAL_IMAGE_THREAD_MAX_CANVAS_PIXELS,
    decodedFramePixels: LOCAL_IMAGE_THREAD_MAX_DECODED_FRAME_PIXELS,
    animationPixelsPerSecond: LOCAL_IMAGE_THREAD_MAX_ANIMATION_PIXELS_PER_SECOND,
    frameCount: Number.MAX_SAFE_INTEGER,
  });
}

function windowStorage(): HohStorage | null {
  try { return (globalThis as { localStorage?: HohStorage }).localStorage ?? null; } catch { return null; }
}
/** A folder path as T3 compares them: no trailing separator. */
function folderPath(value: unknown): string {
  if (typeof value !== 'string') return '';
  return value.length > 1 ? value.replace(/\/+$/, '') || '/' : value;
}
/** Rebase T3's eagerly joined relative Markdown path once the draft's real cwd replaces its placeholder. */
function localImagePathAtRoot(path: string, requestedRoot: string, authoritativeRoot: string): string {
  const from = folderPath(requestedRoot);
  const to = folderPath(authoritativeRoot);
  if (!from || !to || from === to) return path;
  if (path === from) return to;
  if (path.startsWith(`${from}/`) || path.startsWith(`${from}\\`)) return to + path.slice(from.length);
  return path;
}
const titleOf = (root: string) => root.split('/').filter(Boolean).at(-1) ?? root;
const timeOf = (value: unknown) => typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null;
const executorOf = (instanceId: unknown): HohExecutor =>
  HOH_EXECUTORS.find(executor => EXECUTOR_INSTANCES[executor] === instanceId) ?? 'claude';
const instanceOf = (executor: HohExecutor) => EXECUTOR_INSTANCES[executor];

function requireInteraction(executor: HohExecutor, interaction: unknown) {
  if (interaction !== 'plan') return;
  if (executor === 'codex') throw new Error('Plan mode for Codex is coming soon.');
  if (executor === 'pi') throw new Error('Pi Agent does not support Plan mode in HoH.');
}

const PI_ACCESS_REQUIRED = 'Choose Full access in the composer to use Pi Agent.';
const exactMode = (value: unknown): HohAccessMode | null => value === 'plan' ? 'confirm' : value === 'acceptEdits' ? 'auto-accept-edits' : ['confirm', 'auto-accept-edits', 'auto', 'full'].includes(String(value)) ? value as HohAccessMode : null;
/** What the executor is actually running wins over a requested mode waiting for the next turn. */
const effectiveSessionMode = (session: any, fallback: HohAccessMode): HohAccessMode =>
  exactMode(session?.runningRuntimeMode) ?? exactMode(session?.running)
  ?? exactMode(session?.runtimeMode) ?? exactMode(session?.mode) ?? fallback;

const PAGE = 1000;
const TIMELINE_ROWS = 80;
const EARLIER_TIMELINE_ROWS = 160;
// Resume cursors are local to the current document. Do not recycle one if the host is re-created in it.
let threadSequence = 0;

interface StreamChange {
  readonly from: number;
  readonly items: Map<string, { item: Item; sequence: number }>;
}

interface JournalRead {
  readonly generation: number;
  afterSeq: number | undefined;
  ready: boolean;
  pending?: Promise<void>;
  notifications: { notification: EngineNotification; at: number }[];
  watermarks: { index: number; seq: number }[];
  delivered: number;
  watermark: number;
}
const THREAD_ORGANIZATION_COMMANDS = new Set([
  'thread.visit', 'thread.mark-unread', 'thread.settle', 'thread.unsettle', 'thread.auto-settle', 'thread.auto-settle.set',
  'thread.active.reorder', 'thread.tag.set', 'thread.snooze', 'thread.unsnooze',
]);
const loadThreadSearch = () => import('./thread-search.ts');

function dispatchError(command: Record<string, any>, message: string) {
  return new RpcFailure({
    _tag: 'OrchestrationV2DispatchCommandError', commandId: String(command.commandId ?? 'unknown'), commandType: String(command.type ?? 'unknown'), message,
  });
}
const errorText = (error: unknown) => englishError(error);
const localImageContextUnknown = (error: unknown) =>
  !(error instanceof PersistentLocalImageContextError)
  && /conversation project folder is unknown/i.test(errorText(error));

/** T3's answers (`{ [questionId]: value | values }`, values are option labels or typed text) in the engine's form. */
export function questionAnswer(question: EngineQuestion | undefined, answers: Record<string, unknown>): EngineQuestionAnswer {
  return {
    answers: Object.entries(answers).map(([id, value]) => {
      const labels = new Set((question?.questions.find(entry => entry.id === id)?.options ?? []).map(option => option.label));
      const values = (Array.isArray(value) ? value : [value]).filter((entry): entry is string => typeof entry === 'string' && entry.length > 0);
      const selected = values.filter(entry => labels.has(entry));
      const custom = values.filter(entry => !labels.has(entry)).join('\n');
      return { id, selected, ...(custom ? { custom } : {}) };
    }),
  };
}

export class HohHost {
  private readonly engine: HohEngine;
  private readonly platform: EnvironmentPlatform | undefined;
  private readonly now: () => number;
  private readonly throttleMs: number;
  private readonly loadRetryMs: number;
  private readonly loadRetries: number;
  private readonly localImageContextGraceMs: number;
  private stopped = false;
  private readonly threads = new Map<string, ThreadModel>();
  private readonly secretaryContexts = new Map<string, { revision: string; prefix: string }>();
  private pluginAuthoringPrefixes: readonly string[] = [];
  private readonly projectTrustRevisions = new Map<string, number>();
  /** Rows restored before the engine answers; its list/history replaces their cached metadata once available. */
  private readonly restored = new Set<string>();
  /** Histories already peeked successfully during startup; a cached row is retried until its log is readable. */
  private readonly peeked = new Set<string>();
  private peekInFlight: Promise<void> | undefined;
  private peekAgain = false;
  private peekScheduled = false;
  /** Last-known rows wait here until `access/get` supplies their real runtime mode. */
  private readonly savedSessions: any[] = [];
  private readonly loading = new Map<string, Promise<ThreadModel | undefined>>();
  private readonly journals = new WeakMap<ThreadModel, JournalRead>();
  private readonly resumedSessions = new Set<string>();
  private readonly resumingSessions = new Map<string, Promise<void>>();
  private readonly questions = new Map<string, EngineQuestion>();
  private readonly shellSinks = new Set<StreamSink>();
  private readonly shellCompletionSinks = new Set<StreamSink>();
  private readonly lifecycleSinks = new Set<StreamSink>();
  private readonly threadSinks = new Map<string, Set<StreamSink>>();
  private readonly sinkSequences = new WeakMap<StreamSink, number>();
  private readonly boundedSinks = new WeakSet<StreamSink>();
  private readonly timelineStarts = new WeakMap<ThreadModel, number>();
  private readonly threadSequences = new WeakMap<ThreadModel, number>();
  private readonly streamChanges = new Map<string, StreamChange>();
  private readonly configSinks = new Set<StreamSink>();
  private readonly dirty = new Set<string>();
  private flushTimer: ReturnType<typeof setTimeout> | undefined;
  private sequence = 0;
  private homeDirty = false;
  private homeVersion = 0;
  private sessionsKnown = false;
  private readonly homeListeners = new Set<() => void>();
  private readonly git: WorkspaceGit;
  private claudeDiscovery: ClaudeDiscovery | null = null;
  private codexDiscovery: CodexDiscovery | null = null;
  private piDiscovery: HarnessDiscovery | null = null;
  private piDiscoveryInFlight?: { generation: number; promise: Promise<void> };
  /** Undefined while the startup probe is pending; set only when its bounded retry lane ends. */
  private discoveryFailure: unknown | undefined;
  private codexDiscoveryFailure: unknown | undefined;
  private discoveryPending = true;
  private discoveryStartedAt: number;
  /** A person's Refresh supersedes the startup lane; incidental workspace reads share its in-flight probe. */
  private discoveryGeneration = 0;
  private discoveryInFlight: { generation: number; promise: Promise<boolean> } | undefined;
  /** T3's "default permissions" setting: every fresh thread records Full in the engine before its first prompt. */
  private defaultMode: HohAccessMode = 'full';
  private selectedExecutor: HohExecutor = 'claude';
  /** The model the engine runs new chats with (`hoh/executor/select`), once known. */
  private selectedModel: string | null = null;
  private mode: HohAccessMode | null = null;
  private accessError: string | null = 'Executor access mode has not been read yet.';
  /** A startup access/get captured before a person's later choice must not overwrite that particular field. */
  private modeRevision = 0;
  private readonly sessionAccessRevision = new Map<string, number>();
  private readonly sessionAccessApplied = new Map<string, number>();

  private nextSessionAccessRevision(sessionId: string): number {
    const next = (this.sessionAccessRevision.get(sessionId) ?? 0) + 1;
    this.sessionAccessRevision.set(sessionId, next);
    return next;
  }
  private modelRevision = 0;
  /** Sessions for which the engine returned a concrete executor binding and per-session mode. */
  private readonly sessionAccessKnown = new Set<string>();
  private readonly notificationAccess = new Map<string, Promise<void>>();
  /** Rows published before their provider catalog arrived are reconciled when discovery completes. */
  private readonly catalogPending = new Set<string>();
  /** Only the newest global access/get may publish either success or failure. Lost engines and
   * an explicit access/set invalidate every read begun against the older state. */
  private accessGeneration = 0;
  /** A reply from an access/set sent to a process that has since gone away is never authoritative. */
  private accessProcessGeneration = 0;
  private settingsOverrides: Record<string, unknown> = {};
  private readonly localSettings?: ReturnType<CreateHohLocalSettings>;
  private workspaceRoot: string;
  private workspaceKnown = false;
  private availableEditors: Awaited<ReturnType<NonNullable<HohEngine['availableProjectEditors']>>> = [];
  /** Whether the "No project" project is in the list (see the head of this file). */
  private projectShown = false;
  private readonly storage: HohStorage | null;
  private readonly threadMetadata: ThreadMetadataStore;
  /** Folders the person added as projects, by project id. */
  private readonly folders = new Map<string, FolderProject>();
  /** The project of each chat the host placed (thread id → project id). */
  private readonly homes = new Map<string, string>();
  /** Pictures the window gave (uploaded, or `assets.persistChatAttachments`), by id, until their message goes to the engine. */
  private readonly uploads = new Map<string, KeptImage>();
  /** Pictures T3 announced (`attachments.createUploadUrl`) whose bytes have not come yet. */
  private readonly slots = new Map<string, UploadSlot>();
  /** Process-local image addresses for the native <img> loader; sent previews outlive T3 deleting their upload slot. */
  private readonly previewUrls = new Map<string, string>();
  /** Upload ids already adopted by a local message: their preview is kept until this window closes. */
  private readonly sentPreviewIds = new Set<string>();
  /** One process-local URL per conversation/canonical path, bounded globally and per conversation. */
  private readonly localImageUrls = new Map<string, LocalImageUrl>();
  /** Repeated broken markdown must not repeatedly cross the native bridge. */
  private readonly localImageFailures = new Map<string, LocalImageFailure>();
  private readonly localImageInflight = new Map<string, Promise<LocalImageUrl>>();
  /** Actual bridge calls, successful or not, in a per-conversation sliding window. */
  private readonly localImageAttempts = new Map<string, number[]>();
  private localImageGlobalAttempts: number[] = [];
  /** Worst-case byte/entry reservations are installed before native I/O. */
  private readonly localImageReservations = new Map<string, LocalImageReservation>();
  private readonly localImageReservationWaiters = new Set<() => void>();
  private localImageBytes = 0;
  /** Optimistic T3 drafts ask for their first image before createThread has installed the cwd. */
  private readonly localImageContextWaiters = new Map<string, Set<LocalImageContextWaiter>>();
  /** Context lookup is deduplicated by raw request and bounded before it can allocate a draft waiter. */
  private readonly localImageContextInflight = new Map<string, Promise<LocalImageThreadContext | undefined>>();
  /** A failed create is terminal until its draft subscription closes or creation is retried. */
  private readonly failedLocalImageContexts = new Set<string>();
  private readonly creatingThreads = new Set<string>();
  private readonly creatingEditBranches = new Set<string>();
  /** The skills Claude declared for each folder (its last workspace snapshot): `$name` in a message goes as `/name`. */
  private readonly skillsByFolder = new Map<string, Set<string>>();
  /** The reasoning level each chat was given (T3's model options): kept across Stop, reopening and restarts of the window. */
  private readonly levels = new Map<string, { id: string; value: unknown }[]>();
  /** T3's per-thread composer interaction mode. */
  private readonly interactions = new Map<string, T3InteractionMode>();
  /** Generated or manually edited titles that supersede the engine's first-message fallback. */
  private readonly titles = new Map<string, string>();
  private readonly titleRequests = new Map<string, string>();
  /** A completed Claude turn waits for the person until they explicitly mark that exact turn done. */
  private readonly deletedThreads = new Set<string>();
  private readonly deletingThreads = new Set<string>();
  private readonly homeDispositions = new Map<string, HomeDisposition>();
  /** Last engine-history timestamp classified for Home; a session changed while the window was away is read next start. */
  private readonly homeObservedAt = new Map<string, number>();
  /** Local prompt attempts must not erase the preceding wait before the engine accepts the person's answer. */
  private readonly homePrompts = new Set<string>();
  private readonly failedHomePrompts = new Set<string>();
  private readonly startedAt: number;
  private started: Promise<void> | undefined;
  private unsubscribers: (() => void)[] = [];
  /** Every mutation of the engine's global executor/model shares the new-session critical section. */
  private selectionTail: Promise<void> = Promise.resolve();
  private readonly modeGenerations = new Map<string, number>();

  constructor(options: HohHostOptions) {
    this.engine = options.engine;
    this.platform = options.platform;
    this.git = new WorkspaceGit(async (input, query) => {
      if (!this.engine.projectGit) return null;
      await this.start();
      const cwd = folderPath(input);
      if (!cwd || (!this.folderAt(cwd) && !(this.workspaceKnown && cwd === this.workspaceRoot))) return null;
      return this.engine.projectGit({ cwd, query });
    });
    this.now = options.now ?? Date.now;
    this.throttleMs = options.throttleMs ?? 40;
    this.loadRetryMs = options.loadRetryMs ?? 500;
    this.loadRetries = options.loadRetries ?? 60;
    this.discoveryStartedAt = this.now();
    this.localImageContextGraceMs = options.localImageContextGraceMs ?? LOCAL_IMAGE_CONTEXT_GRACE_MS;
    this.workspaceRoot = options.workspaceRoot ?? PLACEHOLDER_ROOT;
    this.startedAt = this.now();
    this.storage = options.storage === undefined ? windowStorage() : options.storage;
    this.localSettings = options.createLocalSettings?.(this.storage);
    this.threadMetadata = new ThreadMetadataStore(this.storage);
    this.restore();
  }

  // ---- projects kept between runs ----

  private restore() {
    let saved: any = null;
    try { saved = JSON.parse(this.storage?.getItem(STORAGE_KEY) ?? 'null'); } catch { saved = null; }
    for (const entry of Array.isArray(saved?.projects) ? saved.projects : []) {
      const root = folderPath(entry?.workspaceRoot);
      if (typeof entry?.id !== 'string' || !entry.id || entry.id === HOH_PROJECT_ID || !root) continue;
      const createdAt = timeOf(entry.createdAt) ?? this.startedAt;
      this.folders.set(entry.id, {
        id: entry.id, title: typeof entry.title === 'string' && entry.title.trim() ? entry.title.trim() : titleOf(root), workspaceRoot: root,
        createdAt, updatedAt: timeOf(entry.updatedAt) ?? createdAt, pinnedAt: timeOf(entry.pinnedAt), archivedAt: timeOf(entry.archivedAt),
      });
    }
    const threads = saved?.threads && typeof saved.threads === 'object' ? saved.threads : {};
    for (const [threadId, projectId] of Object.entries(threads)) if (typeof projectId === 'string') this.homes.set(threadId, projectId);
    for (const [threadId, options] of Object.entries(saved?.options && typeof saved.options === 'object' ? saved.options : {})) if (Array.isArray(options) && options.length) this.levels.set(threadId, options as any);
    for (const [threadId, mode] of Object.entries(saved?.interactions && typeof saved.interactions === 'object' ? saved.interactions : {})) if (isInteractionMode(mode)) this.interactions.set(threadId, mode);
    for (const [threadId, title] of Object.entries(saved?.titles && typeof saved.titles === 'object' ? saved.titles : {})) if (typeof title === 'string' && title.trim()) this.titles.set(threadId, title.trim());
    for (const [threadId, value] of Object.entries(saved?.homeDispositions && typeof saved.homeDispositions === 'object' ? saved.homeDispositions : {})) {
      const entry = value as Record<string, unknown> | null;
      const turnOrdinal = Number(entry?.turnOrdinal);
      const markedAt = timeOf(entry?.markedAt);
      if (Number.isInteger(turnOrdinal) && turnOrdinal > 0 && (entry?.state === 'waiting' || entry?.state === 'done'))
        this.homeDispositions.set(threadId, {
          turnOrdinal, state: entry.state,
          ...(entry.state === 'done' && markedAt !== null ? { markedAt } : {}),
          ...(entry.interrupted === true ? { interrupted: true } : {}),
          ...(entry.stopped === true ? { stopped: true } : {}),
        });
    }
    for (const [threadId, value] of Object.entries(saved?.homeObservedAt && typeof saved.homeObservedAt === 'object' ? saved.homeObservedAt : {})) {
      const observedAt = Number(value);
      if (Number.isFinite(observedAt) && observedAt > 0) this.homeObservedAt.set(threadId, observedAt);
    }
    if (folderPath(saved?.defaultFolder)) this.workspaceRoot = folderPath(saved.defaultFolder);
    // Version 1 was written while Supervised was the product default. Do not let that browser
    // preference silently override the new Full default; per-session modes still come from engine.
    if (Number(saved?.version) >= 2 && exactMode(saved?.defaultMode)) this.defaultMode = saved.defaultMode;
    // Last-known-good list metadata makes the first post-restart snapshot lossless even when the
    // engine's first disk scan is temporarily incomplete. History still comes only from the engine.
    for (const entry of Array.isArray(saved?.sessions) ? saved.sessions : []) {
      const id = typeof entry?.id === 'string' ? entry.id.trim() : '';
      const title = typeof entry?.title === 'string' ? entry.title.trim() : '';
      const createdAt = timeOf(entry?.createdAt);
      const updatedAt = timeOf(entry?.updatedAt);
      if (!id || !title || createdAt === null || updatedAt === null || this.threads.has(id)) continue;
      const projectId = entry?.projectId === HOH_PROJECT_ID || (typeof entry?.projectId === 'string' && this.folders.has(entry.projectId))
        ? entry.projectId : this.homeOf(id);
      const executor: HohExecutor = isHohExecutor(entry?.executor) ? entry.executor : 'claude';
      const model = typeof entry?.model === 'string' && entry.model.trim() ? entry.model.trim() : this.model(executor);
      const restoredMode = exactMode(entry?.mode) ?? this.defaultMode;
      const thread = new ThreadModel({
        id,
        title: this.titles.get(id) ?? title,
        createdAt,
        updatedAt,
        model,
        mode: restoredMode,
        executor,
        bindingKnown: false,
        interactionMode: this.interactions.get(id),
        projectId,
      });
      thread.options = this.levels.get(id);
      thread.archivedAt = entry?.archivedAt === null ? null : timeOf(entry?.archivedAt);
      const stats = entry?.homeProjectStats;
      if (Number.isSafeInteger(stats?.prompts) && stats.prompts >= 0) {
        const { prompts, peakContextTokens, reportedCostUsd } = stats;
        thread.homeProjectStats = {
          prompts,
          ...(Number.isSafeInteger(peakContextTokens) && peakContextTokens >= 0 ? { peakContextTokens } : {}),
          ...(Number.isFinite(reportedCostUsd) && reportedCostUsd >= 0 ? { reportedCostUsd } : {}),
        };
      }
      if (typeof entry?.pluginCreator === 'boolean') thread.pluginCreator = entry.pluginCreator;
      this.applyThreadMetadata(thread);
      this.threads.set(id, thread);
      this.restored.add(id);
    }
    if ([...this.threads.values()].some(thread => thread.projectId === HOH_PROJECT_ID)) this.projectShown = true;
  }

  private readSideChat(value: unknown) {
    if (!value || typeof value !== 'object') return undefined;
    const { parentSessionId, seedEndSeq } = value as Record<string, unknown>;
    if (typeof parentSessionId !== 'string' || !parentSessionId || typeof seedEndSeq !== 'number'
      || !Number.isSafeInteger(seedEndSeq) || seedEndSeq < 0) return undefined;
    return { parentSessionId, seedEndSeq };
  }

  private applyThreadMetadata(thread: ThreadModel) {
    thread.pluginAuthoringPrefixes = this.pluginAuthoringPrefixes;
    const metadata = this.threadMetadata.get(thread.id);
    thread.sideChat ??= this.readSideChat(metadata.sideChat);
    thread.restorePromptTimes(metadata.promptTimes);
    thread.restoreStopTimes(metadata.stopTimes);
    thread.applyOrganization(metadata);
    if (typeof metadata.title === 'string' && metadata.title.trim()) thread.title = metadata.title;
    if (typeof metadata.pinnedAt === 'number' || metadata.pinnedAt === null) thread.pinnedAt = metadata.pinnedAt;
    if (typeof metadata.pinOrderKey === 'string' || metadata.pinOrderKey === null) thread.pinOrderKey = metadata.pinOrderKey;
    if (typeof metadata.archivedAt === 'number' || metadata.archivedAt === null) thread.archivedAt = metadata.archivedAt;
  }

  private save() {
    if (!this.storage) return;
    const sessions = [...this.threads.values()].flatMap(thread => {
      // A local empty draft has not reached the engine and must not become a ghost after restart.
      if (thread.loaded && thread.userMessageCount() === 0) return [];
      return [{
        id: thread.id, title: thread.titleText(), projectId: thread.projectId, executor: thread.executor, model: thread.model, mode: thread.mode,
        createdAt: thread.createdAt, updatedAt: thread.updatedAt, archivedAt: thread.archivedAt,
        homeProjectStats: thread.homeProjectStats,
        pluginCreator: thread.pluginCreator,
      }];
    });
    const value = {
      version: 2, projects: [...this.folders.values()], threads: Object.fromEntries(this.homes),
      ...(this.workspaceRoot !== PLACEHOLDER_ROOT ? { defaultFolder: this.workspaceRoot } : {}),
      defaultMode: this.defaultMode, options: Object.fromEntries(this.levels), sessions,
      interactions: Object.fromEntries(this.interactions), titles: Object.fromEntries(this.titles),
      homeDispositions: Object.fromEntries(this.homeDispositions), homeObservedAt: Object.fromEntries(this.homeObservedAt),
    };
    try { this.storage.setItem(STORAGE_KEY, JSON.stringify(value)); } catch { /* storage full or closed: kept for this run */ }
  }

  /** The chat's level: set by the person's pick; an empty list from T3 never wipes it (only an explicit pick does). */
  private setLevel(thread: ThreadModel, options: unknown, explicit = false) {
    const list = Array.isArray(options) ? options.filter((entry: any) => entry && typeof entry.id === 'string') : [];
    if (!list.length && !explicit) return;
    thread.options = list.length ? list as any : undefined;
    if (list.length) this.levels.set(thread.id, list as any); else this.levels.delete(thread.id);
    this.save();
    this.changed(thread.id);
  }

  private homeOf(threadId: string) {
    const projectId = this.homes.get(threadId);
    return projectId && this.folders.has(projectId) ? projectId : HOH_PROJECT_ID;
  }

  private folderAt(path: string): FolderProject | undefined {
    const root = folderPath(path);
    for (const folder of this.folders.values()) if (folder.workspaceRoot === root) return folder;
    return undefined;
  }

  /** A chat whose history names a project's folder belongs to that project. */
  private placeByFolder(thread: ThreadModel, inferWorkspace = true) {
    if (!thread.cwd) return;
    const folder = this.folderAt(thread.cwd);
    if (folder && thread.projectId !== folder.id) {
      thread.projectId = folder.id;
      this.homes.set(thread.id, folder.id);
      this.save();
      this.changed(thread.id);
    } else if (inferWorkspace && !folder && thread.projectId === HOH_PROJECT_ID && !this.workspaceKnown) {
      this.setWorkspaceRoot(thread.cwd);
    }
  }

  /**
   * The home screen's window into the host (HOH-41 В2; not T3's wire): what the chats are doing, who waits for the person,
   * and the actions of that card. Read with `useSyncExternalStore`: `version` changes with every change of any chat.
   */
  private homeEntry(thread: ThreadModel): HomeEntry | null {
    const disposition = this.homeDispositions.get(thread.id);
    const latest = thread.home();
    const entry = latest && disposition && latest.turnOrdinal !== disposition.turnOrdinal && this.failedHomePrompts.has(thread.id)
      ? thread.home(disposition.turnOrdinal)
      : latest;
    // Cold start must not wait for history, but a persisted human decision must remain actionable
    // while that authoritative history is still being replayed. The stable turn ordinal lets replay
    // replace this metadata-only card without caching a second copy of the conversation projection.
    if (!entry && disposition && !thread.loaded) {
      return {
        id: thread.id, title: thread.shell().title, projectId: thread.projectId, providerInstanceId: instanceOf(thread.executor),
        ...(thread.pluginCreator ? { pluginCreator: true } : {}),
        state: disposition.state, turnOrdinal: disposition.turnOrdinal,
        startedAt: thread.updatedAt, doneAt: disposition.markedAt ?? thread.updatedAt,
        steps: { done: 0, total: 0 }, agents: [],
        ...(disposition.state === 'waiting' ? { canComplete: true } : {}),
        ...(disposition.interrupted === true ? { interrupted: true } : {}),
        ...(disposition.stopped === true ? { stopped: true } : {}),
      };
    }
    const interrupted = entry?.interrupted === true || (disposition?.turnOrdinal === entry?.turnOrdinal && disposition?.interrupted === true);
    const stopped = disposition?.turnOrdinal === entry?.turnOrdinal && disposition?.stopped === true;
    const completable = entry?.state === 'done' || (entry?.state === 'ended' && (interrupted || stopped));
    if (!entry || !completable) return entry;
    return disposition?.turnOrdinal === entry.turnOrdinal && disposition.state === 'done'
      ? { ...entry, state: 'done', doneAt: disposition.markedAt ?? entry.doneAt, ...(interrupted ? { interrupted: true } : {}), ...(stopped ? { stopped: true } : {}) }
      : { ...entry, state: 'waiting', canComplete: true, ...(interrupted ? { interrupted: true } : {}), ...(stopped ? { stopped: true } : {}) };
  }

  /** Follow the latest turn without changing T3's completed run: only Home waits for human sign-off. */
  private syncHomeDisposition(thread: ThreadModel): boolean {
    const entry = thread.home();
    if (!entry) return false;
    const current = this.homeDispositions.get(thread.id);
    const interrupted = entry.interrupted === true || (current?.turnOrdinal === entry.turnOrdinal && current.interrupted === true);
    const stopped = current?.turnOrdinal === entry.turnOrdinal && current.stopped === true;
    const completable = entry.state === 'done' || (entry.state === 'ended' && (interrupted || stopped));
    if (current?.turnOrdinal === entry.turnOrdinal && current.interrupted === true) {
      // The same turn really resumed: it is no longer a crash placeholder. If it already completed before
      // this flush, keep the person's waiting/done choice but drop the stale interruption label.
      if (entry.state === 'working' || entry.state === 'waiting') {
        this.homeDispositions.delete(thread.id);
        return true;
      }
      if (entry.state === 'done') {
        const { interrupted: _, ...resolved } = current;
        this.homeDispositions.set(thread.id, resolved);
        return true;
      }
    }
    if (current && current.turnOrdinal !== entry.turnOrdinal) {
      if (this.homePrompts.has(thread.id) || (this.failedHomePrompts.has(thread.id) && entry.state === 'ended')) return false;
      this.homeDispositions.delete(thread.id);
      this.failedHomePrompts.delete(thread.id);
      if (!completable) return true;
    }
    const matched = this.homeDispositions.get(thread.id);
    if (completable && (!matched || matched.turnOrdinal !== entry.turnOrdinal)) {
      this.homeDispositions.set(thread.id, {
        turnOrdinal: entry.turnOrdinal, state: 'waiting',
        ...(interrupted ? { interrupted: true } : {}), ...(stopped ? { stopped: true } : {}),
      });
      return true;
    }
    return false;
  }

  readonly home = {
    version: () => this.homeVersion,
    subscribe: (listener: () => void) => { this.homeListeners.add(listener); return () => { this.homeListeners.delete(listener); }; },
    entries: (): HomeEntry[] => [...this.threads.values()].filter(thread => !thread.sideChat && thread.archivedAt === null).map(thread => this.homeEntry(thread)).filter((entry): entry is HomeEntry => entry !== null),
    // Grouped Home uses the same visible chats as Recent chats, even before their turns replay.
    projectEntries: (): HomeEntry[] => [...this.threads.values()].flatMap(thread => {
      const shell = thread.shell();
      if (thread.sideChat || thread.archivedAt !== null || shell.latestUserMessageAt === null) return [];
      const entry: HomeEntry = this.homeEntry(thread) ?? {
        id: thread.id, title: shell.title, projectId: thread.projectId, providerInstanceId: instanceOf(thread.executor),
        ...(thread.pluginCreator ? { pluginCreator: true } : {}),
        state: 'ended', turnOrdinal: 0, startedAt: thread.updatedAt, doneAt: null,
        steps: { done: 0, total: 0 }, agents: [],
      };
      return [{ ...entry, projectStats: thread.homeProjectStats }];
    }),
    // This count is also consumed independently by the project view.
    projectCounts: (): Record<string, number> => {
      const counts: Record<string, number> = {};
      for (const thread of this.threads.values()) if (!thread.sideChat && thread.archivedAt === null) counts[thread.projectId] = (counts[thread.projectId] ?? 0) + 1;
      return counts;
    },
    projectTitle: (projectId: string) => projectId === HOH_PROJECT_ID ? 'Chats' : this.folders.get(projectId)?.title ?? 'No project',
    /** Stable private color key for Home folders: a title can change, the workspace path does not. */
    projectKey: (projectId: string) => projectId === HOH_PROJECT_ID ? this.workspaceRoot : this.folders.get(projectId)?.workspaceRoot ?? projectId,
    /** Approve or decline a waiting permission request, as the chat's buttons do. */
    answer: async (threadId: string, requestId: string, decision: 'accept' | 'decline') => {
      await this.respondImmediately(threadId, { requestId, decision });
      this.flush();
    },
    /** A completed answer becomes Done only by the person's explicit action on that exact turn. */
    complete: async (threadId: string, turnOrdinal?: number) => {
      const thread = await this.loaded(threadId);
      const disposition = this.homeDispositions.get(threadId);
      const latest = thread?.home();
      const entry = latest && disposition && latest.turnOrdinal !== disposition.turnOrdinal && this.failedHomePrompts.has(threadId)
        ? thread?.home(disposition.turnOrdinal)
        : latest;
      const interrupted = entry?.interrupted === true || (disposition?.turnOrdinal === entry?.turnOrdinal && disposition?.interrupted === true);
      const stopped = disposition?.turnOrdinal === entry?.turnOrdinal && disposition?.stopped === true;
      if (!thread || !entry || (entry.state !== 'done' && !(entry.state === 'ended' && (interrupted || stopped)))) return false;
      if (turnOrdinal !== undefined && entry.turnOrdinal !== turnOrdinal) return false;
      if (disposition?.turnOrdinal === entry.turnOrdinal && disposition.state === 'done') return true;
      this.homeDispositions.set(threadId, {
        turnOrdinal: entry.turnOrdinal, state: 'done', markedAt: this.now(),
        ...(interrupted ? { interrupted: true } : {}), ...(stopped ? { stopped: true } : {}),
      });
      this.save();
      this.changed(threadId);
      this.flush();
      return true;
    },
    /** A new chat with this first message, in the project (or "No project"); returns its id. */
    start: async (message: string, projectId?: string) => {
      await this.start();
      const threadId = 'thread-' + (globalThis.crypto?.randomUUID?.() ?? this.now().toString(36));
      await this.launch({ threadId, commandId: 'home-' + threadId, projectId: projectId ?? HOH_PROJECT_ID, initialMessage: { messageId: 'msg-' + threadId, text: message, attachments: [] } });
      return threadId;
    },
  };

  private readonly pluginCreatorListeners = new Set<() => void>();

  private pluginCreatorChanged() {
    for (const listener of this.pluginCreatorListeners) listener();
  }

  /** This API is called by the trusted Workspace shell after native package validation. */
  readonly secretaryChat: SecretaryChatApi = {
    hasThread: async threadId => {
      if (!isSecretaryThreadId(threadId)) return false;
      await this.start();
      return this.threads.get(threadId)?.archivedAt === null
        && !this.deletingThreads.has(threadId) && !this.deletedThreads.has(threadId);
    },
    bind: (threadId, input) => {
      if (!isSecretaryThreadId(threadId)) throw new Error('Некорректный чат секретаря.');
      this.secretaryContexts.set(threadId, { revision: input.revision, prefix: secretaryPromptPrefix(input) });
    },
    getProposal: async threadId => {
      const context = this.secretaryContexts.get(threadId);
      if (!context) return null;
      await this.start();
      const thread = await this.loaded(threadId);
      if (!thread || thread.archivedAt !== null || this.deletingThreads.has(threadId)
        || this.threads.get(threadId) !== thread || this.secretaryContexts.get(threadId) !== context) return null;
      const answer = thread.lastCompletedAssistantReply();
      return answer === null ? null : readSecretaryProposal(answer, context.revision);
    },
  };

  private pluginCreatorHarnesses(): PluginCreatorOptions['harnesses'] {
    const claude = claudeProvider(this.claudeDiscovery, this.iso(), this.mode !== null, this.discoveryFailure);
    const codex = codexProvider(this.codexDiscovery, this.iso(), this.codexDiscoveryFailure);
    return (['claude', 'codex'] as const).map(executor => {
      const provider = executor === 'claude' ? claude : codex;
      const model = this.model(executor);
      return {
        executor, name: provider.displayName, model,
        available: provider.status === 'ready' && !!model,
        reason: provider.message,
      };
    });
  }

  pluginCreator(skill: string, guide?: string): PluginCreatorApi {
    const prefixes = pluginAuthoringPrefixes(skill, guide);
    const [promptPrefix] = prefixes;
    this.pluginAuthoringPrefixes = prefixes;
    for (const thread of this.threads.values()) thread.pluginAuthoringPrefixes = this.pluginAuthoringPrefixes;
    return {
      options: async () => {
        await this.start();
        return {
          projects: [...this.folders.values()]
            .filter(folder => folder.archivedAt === null)
            .map(({ id, title, workspaceRoot }) => ({ id, title, path: workspaceRoot })),
          harnesses: this.pluginCreatorHarnesses(),
        };
      },
      subscribe: listener => {
        this.pluginCreatorListeners.add(listener);
        return () => this.pluginCreatorListeners.delete(listener);
      },
      launch: async ({ projectId, executor, brief }) => {
        const task = typeof brief === 'string' ? brief.trim() : '';
        if (!task || task.length > MAX_PLUGIN_BRIEF) throw new Error('Enter a plugin description of up to 8,000 characters.');
        if (executor !== 'claude' && executor !== 'codex') throw new Error('Choose a harness.');
        await this.start();
        const project = this.folders.get(projectId);
        if (!project || project.archivedAt !== null) throw new Error('Choose a folder for the plugin.');
        const provider = this.pluginCreatorHarnesses().find(harness => harness.executor === executor);
        if (!provider?.available) throw new Error(provider?.reason ?? 'The selected harness is unavailable.');
        const threadId = 'thread-' + (globalThis.crypto?.randomUUID?.() ?? `${this.now().toString(36)}-${Math.random().toString(36).slice(2)}`);
        const result = await this.launch({
          threadId, commandId: 'plugin-creator-' + threadId, projectId,
          title: 'Plugin Creator',
          modelSelection: { instanceId: instanceOf(executor), model: provider.model },
          initialMessage: { messageId: 'msg-' + threadId, text: promptPrefix + task, attachments: [] },
        });
        return result.threadId;
      },
    };
  }

  private iso() { return new Date(this.now()).toISOString(); }
  private model(executor: HohExecutor = this.selectedExecutor) {
    if (executor === 'pi') return piModel(this.piDiscovery, this.selectedModel);
    return executor === 'codex'
      ? listedCodexModel(this.codexDiscovery, this.selectedModel) ?? defaultCodexModel(this.codexDiscovery)
      : listedClaudeModel(this.claudeDiscovery, this.selectedModel) ?? defaultClaudeModel(this.claudeDiscovery);
  }

  /** Listen to the engine and read what it already has. Safe to call more than once. */
  start(): Promise<void> {
    this.started ??= this.load();
    return this.started;
  }

  stop() {
    this.stopped = true;
    this.git.stop();
    for (const off of this.unsubscribers) off();
    this.unsubscribers = [];
    if (this.flushTimer) clearTimeout(this.flushTimer);
    for (const url of this.previewUrls.values()) URL.revokeObjectURL(url);
    this.previewUrls.clear();
    for (const image of this.localImageUrls.values()) URL.revokeObjectURL(image.url);
    this.localImageUrls.clear();
    this.localImageFailures.clear();
    this.localImageInflight.clear();
    this.localImageAttempts.clear();
    this.localImageGlobalAttempts = [];
    this.localImageReservations.clear();
    this.wakeLocalImageReaders();
    this.localImageBytes = 0;
    for (const waiters of this.localImageContextWaiters.values()) {
      for (const waiter of waiters) {
        if (waiter.graceTimer) clearTimeout(waiter.graceTimer);
        waiter.resolve(undefined);
      }
    }
    this.localImageContextWaiters.clear();
    this.localImageContextInflight.clear();
    this.failedLocalImageContexts.clear();
    this.creatingThreads.clear();
    this.sentPreviewIds.clear();
  }

  /** A CSP-local address for bytes already checked by the attachment intake. */
  private previewUrl(image: KeptImage): string {
    const existing = this.previewUrls.get(image.id);
    if (existing) return existing;
    const binary = atob(image.data);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
    const url = URL.createObjectURL(new Blob([bytes], { type: image.mimeType }));
    this.previewUrls.set(image.id, url);
    return url;
  }

  private settleLocalImageContext(threadId: string, thread: ThreadModel | undefined) {
    const waiters = this.localImageContextWaiters.get(threadId);
    if (!waiters) return;
    this.localImageContextWaiters.delete(threadId);
    for (const waiter of waiters) {
      if (waiter.graceTimer) clearTimeout(waiter.graceTimer);
      waiter.resolve(thread);
    }
  }

  /** A real draft lifecycle event promotes an early media lookup beyond its registration grace. */
  private registerLocalImageContext(threadId: string) {
    for (const waiter of this.localImageContextWaiters.get(threadId) ?? []) {
      if (waiter.graceTimer) clearTimeout(waiter.graceTimer);
      waiter.graceTimer = undefined;
    }
  }

  private waitForLocalImageContext(threadId: string, requireTransition = false, allowRegistrationGrace = false): Promise<ThreadModel | undefined> {
    const existing = this.threads.get(threadId);
    if (!requireTransition && existing?.cwd) return Promise.resolve(existing);
    if (this.stopped || this.failedLocalImageContexts.has(threadId)) return Promise.resolve(undefined);
    return new Promise(resolve => {
      let waiters = this.localImageContextWaiters.get(threadId);
      if (!waiters) this.localImageContextWaiters.set(threadId, waiters = new Set());
      const waiter: LocalImageContextWaiter = { resolve };
      if (allowRegistrationGrace && !this.threadSinks.has(threadId) && !this.creatingThreads.has(threadId)) {
        waiter.graceTimer = setTimeout(() => {
          const current = this.localImageContextWaiters.get(threadId);
          if (!current?.delete(waiter)) return;
          if (!current.size) this.localImageContextWaiters.delete(threadId);
          waiter.graceTimer = undefined;
          resolve(undefined);
        }, this.localImageContextGraceMs);
      }
      waiters.add(waiter);
    });
  }

  private async refreshLocalImageContext(threadId: string, waitForTransition: boolean) {
    const thread = this.threads.get(threadId);
    if (!thread || this.stopped) return undefined;
    try {
      await this.loaded(threadId);
      await this.ensureSessionResumed(threadId);
      const cwd = folderPath((await this.engine.request('session/working-directory/get', { sessionId: threadId }))?.cwd);
      if (cwd) {
        thread.cwd = cwd;
        this.placeByFolder(thread);
        this.settleLocalImageContext(threadId, thread);
        return thread;
      }
    } catch { /* a pending draft is completed by createThread or working-directory/change */ }
    return waitForTransition && (this.threadSinks.has(threadId) || this.creatingThreads.has(threadId))
      ? this.waitForLocalImageContext(threadId, true)
      : undefined;
  }

  /** Trust UI shares the host's resume barrier for every request scoped to a saved session. */
  async trustRequest(method: string, params: any) {
    const id = typeof params?.sessionId === 'string' ? params.sessionId : null;
    if (id) {
      await this.start();
      const thread = this.threads.get(id);
      if (!thread) throw new Error('Conversation is not ready yet.');
      await this.loaded(id);
      await this.ensureSessionResumed(id);
    }
    return this.engine.request(method, params);
  }

  /** Resolve an optimistic media-file only after its stable thread id owns an authoritative cwd. */
  private async localImageThreadContext(threadId: string, admittedBeforeRegistration: boolean) {
    await this.start();
    let thread = await this.loaded(threadId);
    if (!thread) {
      if (this.failedLocalImageContexts.has(threadId)) return undefined;
      // The built client can render its optimistic first message before either subscribeThread
      // or thread.create reaches the host. The resource lookup becomes the first draft event,
      // bounded until one of those authoritative lifecycle events registers the same id.
      thread = await this.waitForLocalImageContext(threadId, false, true);
    }
    if (!thread || this.stopped) return undefined;
    const ready = thread.cwd ? thread : await this.refreshLocalImageContext(thread.id, true);
    return ready ? { thread: ready, admittedBeforeRegistration } : undefined;
  }

  private localImageThreadContextForPath(threadId: string, path: string, admittedBeforeRegistration: boolean) {
    const thread = this.threads.get(threadId);
    if (!this.stopped && thread?.loaded && thread.cwd) {
      return Promise.resolve({ thread, admittedBeforeRegistration });
    }
    const requestKey = `${threadId}\0${path}`;
    const existing = this.localImageContextInflight.get(requestKey);
    if (existing) return existing;
    let threadInflight = 0;
    for (const key of this.localImageContextInflight.keys()) {
      if (key.startsWith(`${threadId}\0`)) threadInflight++;
    }
    if (this.localImageContextInflight.size >= LOCAL_IMAGE_MAX_INFLIGHT
      || threadInflight >= LOCAL_IMAGE_THREAD_MAX_INFLIGHT) {
      return Promise.reject(new LocalImageContextCapacityError(
        'Image unavailable: too many project images are waiting for this conversation folder.',
      ));
    }
    const pending = this.localImageThreadContext(threadId, admittedBeforeRegistration).finally(() => {
      if (this.localImageContextInflight.get(requestKey) === pending) this.localImageContextInflight.delete(requestKey);
    });
    this.localImageContextInflight.set(requestKey, pending);
    return pending;
  }

  private tryReserveLocalImage(requestKey: string, threadId: string): boolean {
    let reservedGlobalBytes = 0;
    let reservedGlobalEntries = 0;
    let reservedThreadBytes = 0;
    let reservedThreadEntries = 0;
    let reservedThreadIdentities = 0;
    let threadInflight = 0;
    for (const reservation of this.localImageReservations.values()) {
      reservedGlobalBytes += reservation.reservedBytes;
      if (!reservation.replacing) reservedGlobalEntries++;
      if (reservation.threadId !== threadId) continue;
      threadInflight++;
      reservedThreadBytes += reservation.reservedBytes;
      if (!reservation.replacing) reservedThreadEntries++;
      if (reservation.reservesIdentity) reservedThreadIdentities++;
    }
    let threadBytes = 0;
    let threadEntries = 0;
    let threadIdentities = 0;
    let existing: LocalImageUrl | undefined;
    for (const retained of this.localImageUrls.values()) {
      if (retained.threadId !== threadId) continue;
      threadBytes += retained.sizeBytes;
      threadEntries++;
      threadIdentities += retained.requestKeys.size;
      if (retained.requestKeys.has(requestKey)) existing = retained;
    }
    if (!existing && threadIdentities >= LOCAL_IMAGE_THREAD_MAX_IDENTITIES)
      throw new Error('Image unavailable: too many project image paths are already retained for this conversation.');
    const replacedBytes = existing?.sizeBytes ?? 0;
    // A known retained key is a replacement reservation: it cannot grow retention until the
    // canonical post-read size check succeeds, so refreshing it needs no speculative byte slot.
    const reservedSize = existing?.sizeBytes ?? LOCAL_IMAGE_BYTES;
    const retained = {
      globalBytes: this.localImageBytes, globalEntries: this.localImageUrls.size,
      threadBytes, threadEntries,
      replacedBytes, replacing: !!existing, sizeBytes: reservedSize,
    };
    if (!localImageRetentionFits(retained))
      throw new Error('Image unavailable: too many project images are already open in this window.');
    // Active reads may use less than their reservations, or fail and free their slots.
    if (this.localImageReservations.size >= LOCAL_IMAGE_MAX_INFLIGHT
      || threadInflight >= LOCAL_IMAGE_THREAD_MAX_INFLIGHT
      || (!existing && threadIdentities + reservedThreadIdentities >= LOCAL_IMAGE_THREAD_MAX_IDENTITIES)
      || !localImageRetentionFits({
        ...retained,
        globalBytes: retained.globalBytes + reservedGlobalBytes,
        globalEntries: retained.globalEntries + reservedGlobalEntries,
        threadBytes: threadBytes + reservedThreadBytes,
        threadEntries: threadEntries + reservedThreadEntries,
      })) return false;
    this.localImageReservations.set(requestKey, {
      threadId, reservedBytes: reservedSize - replacedBytes,
      replacing: !!existing, reservesIdentity: !existing,
    });
    return true;
  }

  private wakeLocalImageReaders() {
    for (const resolve of this.localImageReservationWaiters) resolve();
    this.localImageReservationWaiters.clear();
  }

  private async readLocalImageWhenReady(threadId: string, path: string, requestKey: string, cwd: string | null) {
    for (;;) {
      if (this.stopped) throw new Error('Image unavailable: this window has closed.');
      if (this.deletedThreads.has(threadId)) throw new Error('Image unavailable: this conversation has been deleted.');
      if (this.tryReserveLocalImage(requestKey, threadId)) break;
      await new Promise<void>(resolve => this.localImageReservationWaiters.add(resolve));
    }
    try {
      return await this.readLocalImage(threadId, path, requestKey, cwd);
    } finally {
      this.localImageReservations.delete(requestKey);
      this.wakeLocalImageReaders();
    }
  }

  private bindLocalImageRequest(requestKey: string, key: string, cwd: string | null) {
    const retained = this.localImageUrls.get(key);
    if (!retained) return;
    for (const [otherKey, other] of this.localImageUrls) {
      if (otherKey !== key && other.threadId === retained.threadId) other.requestKeys.delete(requestKey);
    }
    retained.requestKeys.set(requestKey, cwd === null ? null : { cwd, readAt: this.now() });
  }

  private consumeLocalImageAttempt(threadId: string) {
    const now = this.now();
    this.localImageGlobalAttempts = this.localImageGlobalAttempts
      .filter(timestamp => timestamp > now - LOCAL_IMAGE_ATTEMPT_WINDOW_MS);
    const previous = lruGet(this.localImageAttempts, threadId) ?? [];
    const attempts = previous.filter(timestamp => timestamp > now - LOCAL_IMAGE_ATTEMPT_WINDOW_MS);
    if (attempts.length >= LOCAL_IMAGE_ATTEMPT_MAX || this.localImageGlobalAttempts.length >= LOCAL_IMAGE_GLOBAL_ATTEMPT_MAX)
      throw new LocalImageAttemptLimitError('Image unavailable: too many project image reads were attempted for this conversation; try again later.');
    attempts.push(now);
    this.localImageGlobalAttempts.push(now);
    lruSet(this.localImageAttempts, threadId, attempts, LOCAL_IMAGE_ATTEMPT_THREADS_MAX);
    return now;
  }

  private refundLocalImageAttempt(threadId: string, timestamp: number) {
    const attempts = this.localImageAttempts.get(threadId);
    const threadIndex = attempts?.lastIndexOf(timestamp) ?? -1;
    if (attempts && threadIndex >= 0) {
      attempts.splice(threadIndex, 1);
      if (!attempts.length) this.localImageAttempts.delete(threadId);
    }
    const globalIndex = this.localImageGlobalAttempts.lastIndexOf(timestamp);
    if (globalIndex >= 0) this.localImageGlobalAttempts.splice(globalIndex, 1);
  }

  private async readLocalImage(threadId: string, path: string, requestKey: string, cwd: string | null) {
    if (!this.engine.readImageAsset) throw new Error('Image unavailable: this window cannot read project images.');
    let image: LocalImageAsset;
    let refundedContextFailure = false;
    for (;;) {
      const attempt = this.consumeLocalImageAttempt(threadId);
      try {
        image = await this.engine.readImageAsset({ sessionId: threadId, path });
        break;
      } catch (error) {
        if (!localImageContextUnknown(error)) throw error;
        if (refundedContextFailure) throw new PersistentLocalImageContextError(errorText(error));
        refundedContextFailure = true;
        // Refund only after a fresh authoritative cwd is available. An established conversation
        // whose engine still reports no folder is a persistent failure: count and cache it.
        if (!await this.refreshLocalImageContext(threadId, false)) {
          throw new PersistentLocalImageContextError(errorText(error));
        }
        this.refundLocalImageAttempt(threadId, attempt);
      }
    }
    return this.retainLocalImage(threadId, requestKey, image, cwd);
  }

  private retainLocalImage(threadId: string, requestKey: string, image: LocalImageAsset, cwd: string | null): LocalImageUrl {
    if (this.stopped) throw new Error('Image unavailable: this window has closed.');
    if (!LOCAL_IMAGE_TYPES.has(image?.mimeType) || !Number.isSafeInteger(image?.sizeBytes) || image.sizeBytes < 1 || image.sizeBytes > LOCAL_IMAGE_BYTES)
      throw new Error('Image unavailable: the native image response is invalid.');
    if (typeof image.sourcePath !== 'string' || !image.sourcePath || typeof image.fileRevision !== 'string' || !image.fileRevision)
      throw new Error('Image unavailable: the native image identity is invalid.');
    const decodeProfile = image.decodeProfile;
    if (!decodeProfile
      || !Number.isSafeInteger(decodeProfile.canvasPixels) || decodeProfile.canvasPixels < 1 || decodeProfile.canvasPixels > 40_000_000
      || !Number.isSafeInteger(decodeProfile.decodedFramePixels) || decodeProfile.decodedFramePixels < 1 || decodeProfile.decodedFramePixels > 100_000_000
      || !Number.isSafeInteger(decodeProfile.animationPixelsPerSecond) || decodeProfile.animationPixelsPerSecond < 0 || decodeProfile.animationPixelsPerSecond > 200_000_000
      || !Number.isSafeInteger(decodeProfile.frameCount) || decodeProfile.frameCount < 1 || decodeProfile.frameCount > 300
      || (decodeProfile.frameCount > 1 && decodeProfile.animationPixelsPerSecond === 0))
      throw new Error('Image unavailable: the native image decode profile is invalid.');
    const key = `${threadId}\0${image.sourcePath}`;
    const existing = this.localImageUrls.get(key);
    if (existing?.revision === image.fileRevision && existing.sizeBytes === image.sizeBytes) {
      this.bindLocalImageRequest(requestKey, key, cwd);
      this.localImageFailures.delete(requestKey);
      return existing;
    }
    let threadBytes = 0;
    let threadEntries = 0;
    const globalDecode: LocalImageDecodeProfile = { canvasPixels: 0, decodedFramePixels: 0, animationPixelsPerSecond: 0, frameCount: 0 };
    const threadDecode: LocalImageDecodeProfile = { canvasPixels: 0, decodedFramePixels: 0, animationPixelsPerSecond: 0, frameCount: 0 };
    for (const retained of this.localImageUrls.values()) {
      globalDecode.canvasPixels += retained.decodeProfile.canvasPixels;
      globalDecode.decodedFramePixels += retained.decodeProfile.decodedFramePixels;
      globalDecode.animationPixelsPerSecond += retained.decodeProfile.animationPixelsPerSecond;
      globalDecode.frameCount += retained.decodeProfile.frameCount;
      if (retained.threadId !== threadId) continue;
      threadBytes += retained.sizeBytes;
      threadEntries++;
      threadDecode.canvasPixels += retained.decodeProfile.canvasPixels;
      threadDecode.decodedFramePixels += retained.decodeProfile.decodedFramePixels;
      threadDecode.animationPixelsPerSecond += retained.decodeProfile.animationPixelsPerSecond;
      threadDecode.frameCount += retained.decodeProfile.frameCount;
    }
    const replacedBytes = existing?.sizeBytes ?? 0;
    if (!localImageRetentionFits({
      globalBytes: this.localImageBytes, globalEntries: this.localImageUrls.size,
      threadBytes, threadEntries, replacedBytes, replacing: !!existing, sizeBytes: image.sizeBytes,
    }))
      throw new Error('Image unavailable: too many project images are already open in this window.');
    const emptyDecode: LocalImageDecodeProfile = { canvasPixels: 0, decodedFramePixels: 0, animationPixelsPerSecond: 0, frameCount: 0 };
    if (!localImageDecodeRetentionFits({
      global: globalDecode, thread: threadDecode,
      replaced: existing?.decodeProfile ?? emptyDecode,
      incoming: decodeProfile,
    }))
      throw new Error('Image unavailable: project images exceed the safe decoded-image or animation budget for this window.');
    let binary: string;
    try { binary = atob(image.dataBase64); }
    catch { throw new Error('Image unavailable: the native image payload is invalid.'); }
    if (binary.length !== image.sizeBytes) throw new Error('Image unavailable: the native image size is invalid.');
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
    const url = URL.createObjectURL(new Blob([bytes], { type: image.mimeType }));
    if (existing) {
      URL.revokeObjectURL(existing.url);
      this.localImageBytes -= existing.sizeBytes;
    }
    const minted = {
      url, revision: image.fileRevision, sourcePath: image.sourcePath,
      sizeBytes: image.sizeBytes, decodeProfile, threadId,
      requestKeys: existing?.requestKeys ?? new Map(),
    };
    this.localImageUrls.set(key, minted);
    this.bindLocalImageRequest(requestKey, key, cwd);
    this.localImageBytes += image.sizeBytes;
    this.localImageFailures.delete(requestKey);
    return minted;
  }

  /** A conversation-bound Blob URL for a native image that passed the single Rust file policy. */
  private localImageUrl(threadId: string, path: string, cwd: string | null): Promise<LocalImageUrl> {
    if (this.stopped) return Promise.reject(new Error('Image unavailable: this window has closed.'));
    if (path.length > 4096) return Promise.reject(new Error('Image unavailable: the image path is too long.'));
    const requestKey = `${threadId}\0${path}`;
    // An accepted external image is a snapshot tied to this exact request and conversation.
    for (const image of this.localImageUrls.values()) {
      if (image.threadId === threadId && image.confirmedPath === path) return Promise.resolve(image);
    }
    const reusable = (fresh: boolean) => {
      for (const image of this.localImageUrls.values()) {
        const request = image.requestKeys.get(requestKey);
        if (request && request.cwd === cwd && (!fresh || this.now() - request.readAt < LOCAL_IMAGE_REUSE_MS)) {
          return image;
        }
      }
      return undefined;
    };
    const failure = lruGet(this.localImageFailures, requestKey);
    if (failure && failure.expiresAt > this.now()) return Promise.reject(new Error(failure.message));
    const pending = this.localImageInflight.get(requestKey);
    if (pending) return pending;
    if (this.localImageReservations.has(requestKey)) return Promise.reject(new Error('This image is awaiting confirmation.'));
    // Tool completions can refresh the same screenshot repeatedly while it is being displayed.
    const cached = reusable(true);
    if (cached) return Promise.resolve(cached);
    if (this.localImageInflight.size >= LOCAL_IMAGE_MAX_PENDING_READS) {
      return Promise.reject(new Error('Image unavailable: the project image queue is full.'));
    }
    const read = this.readLocalImageWhenReady(threadId, path, requestKey, cwd)
      .catch(error => {
        if (error instanceof LocalImageAttemptLimitError) {
          const shown = reusable(false);
          if (shown) return shown;
        }
        if (!this.stopped && !localImageContextUnknown(error)) lruSet(this.localImageFailures, requestKey, {
          message: errorText(error).slice(0, 512), expiresAt: this.now() + LOCAL_IMAGE_FAILURE_TTL_MS,
        }, LOCAL_IMAGE_REQUEST_CACHE_MAX);
        throw error;
      })
      .finally(() => {
        if (this.localImageInflight.get(requestKey) === read) this.localImageInflight.delete(requestKey);
      });
    this.localImageInflight.set(requestKey, read);
    return read;
  }

  async showExternalImage(threadId: string, path: string): Promise<boolean> {
    if (!this.engine.confirmExternalImage || this.stopped || path.length > 4096 ||
        !this.threads.has(threadId) || this.deletedThreads.has(threadId)) return false;
    const requestKey = `${threadId}\0${path}`;
    if (this.localImageInflight.has(requestKey) || this.localImageReservations.has(requestKey)) return false;
    if (!this.tryReserveLocalImage(requestKey, threadId)) {
      throw new Error('Image unavailable: the window is busy opening project images. Try again shortly.');
    }
    try {
      const image = await this.engine.confirmExternalImage({ sessionId: threadId, path });
      if (!image || this.deletedThreads.has(threadId) || !this.localImageReservations.has(requestKey)) return false;
      this.retainLocalImage(threadId, requestKey, image, null).confirmedPath = path;
      return true;
    } finally {
      this.localImageReservations.delete(requestKey);
      this.wakeLocalImageReaders();
    }
  }

  private async load() {
    void this.engine.availableProjectEditors?.().then(editors => {
      if (this.stopped) return;
      this.availableEditors = editors;
      this.configChanged();
    }).catch(() => undefined);
    this.unsubscribers.push(await this.engine.subscribe(notification => this.onNotification(notification)));
    if (this.engine.subscribeQuestions) this.unsubscribers.push(await this.engine.subscribeQuestions(question => this.onQuestion(question)));
    if (this.engine.subscribeStatus) this.unsubscribers.push(await this.engine.subscribeStatus(status => {
      if (status.state === 'ready') void this.onEngineReady();
      else if (status.state === 'restarting' || status.state === 'unavailable' || status.state === 'stopped') this.onEngineLost();
    }));
    // Provider probes and settings are independent of the shell. In particular, a slow CLI health check must not
    // keep T3 from showing the window. Access remains fail-closed and alone gates the authoritative
    // session inventory and prompt traffic.
    this.discoveryStartedAt = this.now();
    const discoveryGeneration = this.discoveryGeneration;
    void this.readPiDiscovery(discoveryGeneration);
    void this.loadConfiguration(discoveryGeneration).then(discovered => {
      if (!discovered && discoveryGeneration === this.discoveryGeneration) void this.retryConfiguration(discoveryGeneration);
    });
    const sessions = this.readAccess().then(async accessRead => {
      this.configChanged();
      if (!accessRead) void this.retryAccessConfiguration();
    });
    const inventory = this.loadSessions();
    // Preserve the single-snapshot fast path when session/list answers immediately, without letting a slow or
    // wedged bridge own startup: after one event-loop turn T3 receives the explicit pending placeholder instead.
    await Promise.race([Promise.all([sessions, inventory]), new Promise<void>(resolve => setTimeout(resolve, 0))]);
    // The first config, lifecycle and empty shell snapshots are usable immediately. Both engine reads reconcile
    // them in the background, so even a stuck session/list cannot hold the T3 window behind a bridge timeout.
  }

  /** Provider probes and access settings, announced whenever that independent startup lane completes. */
  private async loadConfiguration(generation: number): Promise<boolean> {
    const discovered = await this.readDiscovery(generation);
    this.configChanged();
    return discovered;
  }

  private async retryConfiguration(generation: number) {
    for (let attempt = 0; attempt < this.loadRetries && !this.stopped && generation === this.discoveryGeneration && this.discoveryPending && (this.claudeDiscovery === null || this.codexDiscovery === null); attempt++) {
      await new Promise<void>(resolve => { const timer: any = setTimeout(resolve, this.loadRetryMs); timer?.unref?.(); });
      if (this.stopped || generation !== this.discoveryGeneration) return;
      await this.loadConfiguration(generation);
    }
    if (!this.stopped && generation === this.discoveryGeneration && this.discoveryPending && (this.claudeDiscovery === null || this.codexDiscovery === null)) {
      this.finishDiscoveryFailure(this.discoveryStartedAt);
      this.configChanged();
    }
  }

  private async retryAccessConfiguration() {
    for (let attempt = 0; attempt < this.loadRetries && !this.stopped && this.mode === null; attempt++) {
      await new Promise<void>(resolve => { const timer: any = setTimeout(resolve, this.loadRetryMs); timer?.unref?.(); });
      if (this.stopped) return;
      if (await this.readAccess()) {
        this.configChanged();
        await this.loadSessions();
        return;
      }
      this.configChanged();
    }
  }

  /** Session metadata retries do not sit behind Claude discovery. */
  private async loadSessions() {
    await this.readSessions();
    // A successful first disk scan can still be incomplete while the previous process releases
    // resources. Keep reconciling for the same bounded window used while the engine is starting.
    if (!this.stopped) void this.retrySessions();
  }

  private async retrySessions() {
    for (let attempt = 0; attempt < this.loadRetries && !this.stopped; attempt++) {
      await new Promise<void>(resolve => { const timer: any = setTimeout(resolve, this.loadRetryMs); timer?.unref?.(); });
      if (this.stopped) return;
      await this.readSessions();
    }
  }

  private readDiscovery(generation = this.discoveryGeneration): Promise<boolean> {
    if (this.discoveryInFlight?.generation === generation) return this.discoveryInFlight.promise;
    const request = this.performDiscovery(generation);
    const promise = request.finally(() => {
      if (this.discoveryInFlight?.promise === promise) this.discoveryInFlight = undefined;
    });
    this.discoveryInFlight = { generation, promise };
    return promise;
  }

  private async performDiscovery(generation: number): Promise<boolean> {
    const [claude, codex] = await Promise.all([
      this.engine.request('hoh/executor/discover', { executor: 'claude' }).catch(() => null),
      this.engine.request('hoh/executor/discover', { executor: 'codex' }).catch(() => null),
    ]);
    if (generation !== this.discoveryGeneration) return this.claudeDiscovery !== null && this.codexDiscovery !== null;
    this.claudeDiscovery = claude;
    this.codexDiscovery = codex;
    if (claude !== null) this.discoveryFailure = undefined;
    if (codex !== null) this.codexDiscoveryFailure = undefined;
    if (claude !== null) this.updateCatalog('claude');
    if (codex !== null) this.updateCatalog('codex');
    if (claude !== null && codex !== null) {
      this.discoveryPending = false;
      return true;
    }
    return false;
  }

  private readPiDiscovery(generation = this.discoveryGeneration): Promise<void> {
    if (this.piDiscoveryInFlight?.generation === generation) return this.piDiscoveryInFlight.promise;
    const promise = this.engine.request('hoh/executor/discover', { executor: 'pi' })
      .catch(() => null)
      .then(found => {
        if (this.stopped || generation !== this.discoveryGeneration) return;
        this.piDiscovery = found;
        if (found !== null) this.updateCatalog('pi');
        this.configChanged();
      })
      .finally(() => {
        if (this.piDiscoveryInFlight?.promise === promise) this.piDiscoveryInFlight = undefined;
      });
    this.piDiscoveryInFlight = { generation, promise };
    return promise;
  }

  private updateCatalog(executor: HohExecutor) {
    for (const threadId of this.catalogPending) {
      const thread = this.threads.get(threadId);
      if (thread?.executor !== executor) continue;
      thread.model = this.model(executor);
      this.catalogPending.delete(threadId);
      this.changed(threadId);
    }
  }

  private finishDiscoveryFailure(startedAt: number) {
    const seconds = Math.max(1, Math.ceil((this.now() - startedAt) / 1000));
    this.discoveryPending = false;
    const waited = `${seconds} ${seconds === 1 ? 'second' : 'seconds'}`;
    if (this.claudeDiscovery === null) this.discoveryFailure = `HoH could not run the Claude health check after waiting ${waited} for the engine. Press Refresh to try again.`;
    if (this.codexDiscovery === null) this.codexDiscoveryFailure = `HoH could not run the Codex health check after waiting ${waited} for the engine. Press Refresh to try again.`;
  }

  private async readAccess(): Promise<boolean> {
    const accessGeneration = ++this.accessGeneration;
    const modeRevision = this.modeRevision;
    const modelRevision = this.modelRevision;
    try {
      // The selected executor/model is global and is returned from either registered harness. Claude
      // is the stable bootstrap query; per-session reads below recover each row's exact executor/mode.
      const access = await this.engine.request('hoh/executor/access/get', { executor: 'claude' });
      const mode = exactMode(access?.runtimeMode) ?? exactMode(access?.mode);
      if (!mode) throw new Error('The engine returned an unknown executor access mode.');
      if (accessGeneration !== this.accessGeneration) return this.mode !== null;
      if (modelRevision === this.modelRevision) {
        if (isHohExecutor(access?.selected?.executor)) this.selectedExecutor = access.selected.executor;
        if (typeof access?.selected?.model === 'string' && access.selected.model) this.selectedModel = access.selected.model;
      }
      if (modeRevision === this.modeRevision) {
        this.mode = mode;
        this.accessError = null;
      }
      return true;
    } catch (error) {
      if (accessGeneration !== this.accessGeneration || modeRevision !== this.modeRevision) return this.mode !== null;
      this.mode = null;
      this.accessError = errorText(error) || 'Executor access mode is unavailable.';
      return false;
    }
  }

  private async requireAccess(): Promise<HohAccessMode> {
    if (this.mode !== null || await this.readAccess()) return this.mode as HohAccessMode;
    throw new Error(this.accessError ?? 'Executor access mode is unavailable.');
  }

  private async onEngineReady() {
    void this.readPiDiscovery();
    // Inventory is independent of provider discovery and the global access read. In particular,
    // a slow CLI probe after the engine connects must not delay the authoritative welcome and rows.
    const discovery = this.readDiscovery().then(() => this.configChanged());
    const access = this.readAccess().then(accessRead => {
      this.configChanged();
      if (accessRead) this.flush();
    });
    await Promise.all([discovery, access, this.readSessions()]);
  }

  private async readSessions(): Promise<boolean> {
    const globalMode = this.mode ?? 'confirm';
    let sessions: any[] = [];
    try { sessions = (await this.engine.request('session/list', {}))?.sessions ?? []; } catch { return false; }
    let inventoryChanged = false;
    // Publish every row before access/get: an unanswered read must not hide other conversations
    // or an approval that arrives for one of them.
    for (const session of sessions) {
      if (typeof session?.id !== 'string' || !session.id || this.deletedThreads.has(session.id) || this.threads.has(session.id)) continue;
      const at = Number(session.updatedAt) || this.startedAt;
      const provisional = new ThreadModel({
        id: session.id, title: this.titles.get(session.id) ?? (typeof session.title === 'string' ? session.title : ''),
        createdAt: at, updatedAt: at, model: 'unknown', mode: 'confirm', executor: 'claude', bindingKnown: false,
        projectId: this.homeOf(session.id),
      });
      provisional.sideChat = this.readSideChat(session.sideChat);
      this.applyThreadMetadata(provisional);
      this.threads.set(session.id, provisional);
      inventoryChanged = true;
      if (this.homeOf(session.id) === HOH_PROJECT_ID) this.showProject();
      if (this.shellSinks.size) this.changed(session.id);
    }
    // session/list is authoritative even when an executor never answers its access query.
    // Give immediate replies a turn to fill the first snapshot, then unblock bootstrap.
    const bootstrapTimer: any = setTimeout(() => {
      if (!this.stopped) {
        this.sessionsReady();
        this.schedulePeekToday();
      }
    }, 0);
    bootstrapTimer?.unref?.();
    await Promise.all(sessions.map(async session => {
      if (typeof session?.id !== 'string' || !session.id || this.deletedThreads.has(session.id) || this.creatingEditBranches.has(session.id)) return;
      const at = Number(session.updatedAt) || this.startedAt;
      const previous = this.threads.get(session.id);
      let executor = previous?.executor ?? this.selectedExecutor;
      let model: string | undefined = previous?.model;
      let mode = previous && this.sessionAccessKnown.has(session.id) ? previous.mode : globalMode;
      let ownKnown = Boolean(previous?.bindingKnown && this.sessionAccessKnown.has(session.id));
      let builtin = previous?.builtin ?? false;
      let runningPlan = false;
      let runningInteraction: T3InteractionMode | null = null;
      const modeRevision = this.modeRevision;
      const modeGeneration = this.modeGenerations.get(session.id) ?? 0;
      try {
        const own = (await this.engine.request('hoh/executor/access/get', { executor: 'codex', sessionId: session.id }))?.session;
        const exact = exactMode(own?.runtimeMode);
        builtin = own?.builtin === true && own?.executor == null && own?.runtimeMode == null;
        if (own && !builtin && (!exact || !isHohExecutor(own.executor))) throw new Error('Conversation access is unavailable.');
        if (builtin) ownKnown = false;
        if (own && !builtin) ownKnown = true;
        if (isHohExecutor(own?.executor)) executor = own.executor;
        model = typeof own?.model === 'string' && own.model ? own.model : undefined;
        mode = effectiveSessionMode(own, mode);
        runningPlan = own?.runningRuntimeMode === 'plan';
        if (own?.runningRuntimeMode != null) runningInteraction = runningPlan ? 'plan' : 'default';
      } catch {
        if (previous) { executor = previous.executor; model = previous.model; mode = previous.mode; }
      }
      // Wait for an in-flight native catalog before publishing a default for an older binding.
      if (model === undefined) {
        if (executor === 'codex' && this.codexDiscovery === null) await this.discoveryInFlight?.promise;
        if (executor === 'pi' && this.piDiscovery === null) await this.piDiscoveryInFlight?.promise;
      }
      model ??= this.model(executor);

      if (this.deletedThreads.has(session.id)) return;
      const existing = this.threads.get(session.id);
      if (existing) {
        if (modeRevision !== this.modeRevision) return;
        if ((this.modeGenerations.get(session.id) ?? 0) !== modeGeneration) return;
        if (ownKnown) this.sessionAccessKnown.add(session.id);
        const bindingChanged = existing.bindingKnown !== ownKnown;
        existing.bindingKnown = ownKnown;
        const builtinChanged = existing.builtin !== builtin;
        existing.builtin = builtin;
        const planChanged = runningInteraction !== null && existing.runningInteractionMode !== runningInteraction;
        if (runningInteraction !== null) existing.setRunningInteractionMode(runningInteraction);
        if (bindingChanged || builtinChanged || planChanged || existing.executor !== executor || existing.model !== model || existing.mode !== mode) {
          existing.executor = executor;
          existing.model = model;
          existing.mode = mode;
          inventoryChanged = true;
          if (existing.loaded || this.shellSinks.size) this.changed(session.id);
        }
        if (this.restored.delete(session.id)) {
          // The cache is only continuity for the first snapshot. Once this session is visible on
          // disk, let its current list entry and history replace stale presentation metadata.
          existing.title = this.titles.get(session.id) ?? (typeof session.title === 'string' ? session.title : '');
          existing.sideChat = this.readSideChat(session.sideChat) ?? existing.sideChat;
          this.applyThreadMetadata(existing);
          existing.updatedAt = Math.max(existing.updatedAt, at);
          inventoryChanged = true;
          if (this.sessionsKnown) this.changed(session.id);
        }
        return;
      }
      if (ownKnown) this.sessionAccessKnown.add(session.id);
      const discovered = new ThreadModel({
        id: session.id,
        title: this.titles.get(session.id) ?? (typeof session.title === 'string' ? session.title : ''),
        createdAt: at,
        updatedAt: at,
        model,
        mode,
        executor,
        bindingKnown: ownKnown,
        interactionMode: this.interactions.get(session.id),
        projectId: this.homeOf(session.id),
      });
      if (runningInteraction !== null) discovered.setRunningInteractionMode(runningInteraction);
      discovered.sideChat = this.readSideChat(session.sideChat);
      this.applyThreadMetadata(discovered);
      this.threads.set(session.id, discovered);
      if ((executor === 'pi' ? this.piDiscovery : executor === 'codex' ? this.codexDiscovery : this.claudeDiscovery) === null) this.catalogPending.add(session.id);
      inventoryChanged = true;
      // During the first load the metadata is already in the forthcoming snapshot. A retry happens after that
      // snapshot, so it needs an update for each conversation that arrived late.
      if (this.sessionsKnown && this.shellSinks.size) this.changed(session.id);
    }));
    clearTimeout(bootstrapTimer);
    // Reconciliation is deliberately merge-only: an immediately restarted engine may return a
    // temporarily incomplete list, so neither cached rows nor their Waiting/Done state is pruned.
    if (inventoryChanged) this.save();
    if ([...this.threads.values()].some(thread => thread.projectId === HOH_PROJECT_ID)) this.showProject();
    this.sessionsReady();
    // History enriches Home and list rows, but metadata is enough to render and select a conversation. Start it
    // only after the authoritative bootstrap frames have been queued: this keeps first paint ahead of disk I/O
    // and lets an immediate Add project win over stale folder inference from an old history.
    this.schedulePeekToday();
    return true;
  }

  /** Finish T3's bootstrap only when the first authoritative session list (including an empty one) is known. */
  private sessionsReady() {
    if (this.sessionsKnown) return;
    this.sessionsKnown = true;
    for (const sink of this.lifecycleSinks) sink.emit(this.lifecycleWelcome('complete'));
    for (const sink of this.shellSinks) sink.emit(this.shellSnapshot());
    for (const sink of this.shellCompletionSinks) sink.emit({ kind: 'synchronized' });
    this.shellCompletionSinks.clear();
  }

  /** Coalesce repeated inventories while allowing history to start before all access reads finish. */
  private schedulePeekToday() {
    if (this.stopped || this.peekScheduled) return;
    this.peekScheduled = true;
    const timer: any = setTimeout(() => {
      this.peekScheduled = false;
      if (this.stopped) return;
      if (this.peekInFlight) { this.peekAgain = true; return; }
      this.peekInFlight = this.peekToday().finally(() => {
        this.peekInFlight = undefined;
        if (this.peekAgain) { this.peekAgain = false; this.schedulePeekToday(); }
      });
    }, 0);
    timer?.unref?.();
  }

  /**
   * The home screen lists today's chats, persisted human waits, and every session whose engine timestamp changed since its
   * last classification. Thus both old histories at upgrade and replies completed while the window was away are recovered.
   * Histories are read as events only: the chat is not opened (`session/resume`) or marked loaded.
   */
  private async peekToday() {
    const today = new Date(this.now()).toDateString();
    const waiting = new Set([...this.homeDispositions].filter(([, value]) => value.state === 'waiting').map(([threadId]) => threadId));
    const candidates = [...this.threads.values()].filter(thread => !thread.loaded && !this.peeked.has(thread.id));
    const todayThreads = candidates.filter(thread => new Date(thread.updatedAt).toDateString() === today)
      .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 12);
    const changedWhileAway = candidates.filter(thread => (this.homeObservedAt.get(thread.id) ?? 0) < thread.updatedAt);
    const authoringEnabled = this.pluginAuthoringPrefixes.length > 0;
    const recent = [...new Map([...changedWhileAway, ...candidates.filter(thread => waiting.has(thread.id)), ...todayThreads].map(thread => [thread.id, thread])).values()]
      .sort((a, b) => b.updatedAt - a.updatedAt);
    const recentIds = new Set(recent.map(thread => thread.id));
    // Backfill older sessions from their journals, keeping the current Home work first.
    recent.push(...candidates.filter(thread => !recentIds.has(thread.id)
      && ((authoringEnabled && thread.pluginCreator === undefined) || (thread.archivedAt === null && !thread.homeProjectStats))));
    let dispositionChanged = false;
    let cursor = 0;
    const inspect = async (thread: ThreadModel) => {
      if (this.stopped) return;
      try {
        const previousPluginCreator = thread.pluginCreator;
        const journal = this.journalFor(thread);
        await this.replayJournal(thread, journal);
        if (!this.currentJournal(thread, journal) || thread.loaded) return;
        // The engine's own word on whether the chat is working (`access/get` of the chat), when it gives one.
        try {
          const generation = this.modeGenerations.get(thread.id) ?? 0;
          const processGeneration = this.accessProcessGeneration;
          const own = (await this.engine.request('hoh/executor/access/get', { executor: thread.executor, sessionId: thread.id }))?.session;
          if (own && this.currentJournal(thread, journal) && !thread.loaded
            && generation === (this.modeGenerations.get(thread.id) ?? 0) && processGeneration === this.accessProcessGeneration) {
            const boundMode = exactMode(own.runtimeMode);
            if (isHohExecutor(own.executor) && boundMode && (!thread.bindingKnown || thread.executor === own.executor)) {
              thread.executor = own.executor;
              thread.bindingKnown = true;
              thread.builtin = false;
              this.sessionAccessKnown.add(thread.id);
              thread.mode = effectiveSessionMode(own, thread.mode);
            }
            if (own.running) {
              thread.setRunning(true, this.now());
              // Replaying an earlier completed turn clears its live mode; restore only what
              // this process reports, never the pending choice for a later turn.
              const runningMode = exactMode(own.runningRuntimeMode) ?? exactMode(own.running);
              if (runningMode && thread.bindingKnown && own.executor === thread.executor) {
                thread.mode = runningMode;
                thread.setRunningInteractionMode(own.runningRuntimeMode === 'plan' ? 'plan' : 'default');
              }
            }
          }
        } catch { /* unknown: the history decides */ }
        if (!this.currentJournal(thread, journal) || thread.loaded) return;
        this.applyThreadMetadata(thread);
        thread.settleUnfinished(this.now());
        this.peeked.add(thread.id);
        if (!thread.homeProjectStats) dispositionChanged = true;
        thread.homeProjectStats = thread.projectStats();
        if (authoringEnabled) thread.pluginCreator ??= false;
        if (thread.pluginCreator !== previousPluginCreator) dispositionChanged = true;
        if (this.syncHomeDisposition(thread)) dispositionChanged = true;
        if (this.homeObservedAt.get(thread.id) !== thread.updatedAt) { this.homeObservedAt.set(thread.id, thread.updatedAt); dispositionChanged = true; }
        // Startup classification may discover an old arbitrary folder, but it must not race a person's Add project
        // by adopting that folder as the scratch workspace. It can still attach to an already known project. A bare
        // pre-turn history in an unknown folder has no Home state to announce; opening it performs the authoritative
        // load and emits its row after any project the person just added has been reconciled.
        const knownFolder = thread.cwd ? this.folderAt(thread.cwd) : undefined;
        if (thread.cwd) this.placeByFolder(thread, false);
        if (!thread.cwd || knownFolder) this.changed(thread.id);
        else this.homeChanged();
      } catch { /* no observed timestamp is recorded: a deleted/unavailable chat is retried on the next start */ }
    };
    // Keep history classification off startup's critical path and cap shared-disk pressure while still allowing
    // a slow session to overlap with the remaining Home recovery work.
    await Promise.all(Array.from({ length: Math.min(3, recent.length) }, async () => {
      while (!this.stopped && cursor < recent.length) await inspect(recent[cursor++]);
    }));
    if (this.stopped) return;
    if (dispositionChanged) this.save();
  }

  /** Only a successful resume or a live engine notification proves a cached session is open. */
  private async ensureSessionResumed(threadId: string): Promise<void> {
    if (this.deletedThreads.has(threadId)) throw new Error('This chat is being deleted.');
    if (this.resumedSessions.has(threadId)) return;
    let pending = this.resumingSessions.get(threadId);
    if (!pending) {
      const generation = this.accessProcessGeneration;
      pending = this.engine.request('session/resume', { sessionId: threadId }).catch(error => {
        // The SDK distinguishes a reserved live/opening session from a disk create collision.
        // Its folder lookup joins the reserved creation instead of starting a second one.
        if (hostFailure(error).message !== `SDK session is already open or opening: ${threadId}`) throw error;
      }).then(() => {
        if (this.stopped || generation !== this.accessProcessGeneration) throw new Error('The engine changed while resuming the conversation.');
        this.resumedSessions.add(threadId);
      }).finally(() => { if (this.resumingSessions.get(threadId) === pending) this.resumingSessions.delete(threadId); });
      this.resumingSessions.set(threadId, pending);
    }
    await pending;
  }

  private journalFor(thread: ThreadModel) {
    let journal = this.journals.get(thread);
    if (!journal || journal.generation !== this.accessProcessGeneration) {
      journal = {
        generation: this.accessProcessGeneration, afterSeq: journal?.afterSeq ?? thread.sideChat?.seedEndSeq, ready: false,
        notifications: [], watermarks: [], delivered: 0, watermark: 0,
      };
      this.journals.set(thread, journal);
    }
    return journal;
  }

  private currentJournal(thread: ThreadModel, journal: JournalRead) {
    return !this.stopped && journal.generation === this.accessProcessGeneration && this.threads.get(thread.id) === thread;
  }

  private deliverJournalNotifications(journal: JournalRead, end: number) {
    while (journal.delivered < end) {
      const { notification, at } = journal.notifications[journal.delivered++];
      this.onNotification(notification, at, true);
    }
  }

  /** Home and the open chat share completed pages and the same in-flight read. */
  private replayJournal(thread: ThreadModel, journal: JournalRead) {
    if (journal.pending) return journal.pending;
    journal.ready = false;
    const pending = (async () => {
      for (;;) {
        if (!this.currentJournal(thread, journal)) throw new Error('The conversation changed while reading history.');
        const afterSeq = journal.afterSeq;
        const page = await this.engine.request('session/events', { sessionId: thread.id, limit: PAGE, ...(afterSeq === undefined ? {} : { afterSeq }) });
        if (!this.currentJournal(thread, journal)) throw new Error('The conversation changed while reading history.');
        const events = page?.events;
        if (!Array.isArray(events)) throw new Error('Invalid conversation history.');
        let nextSeq = afterSeq ?? -1;
        for (const event of events) {
          if (!Number.isSafeInteger(event?.seq) || event.seq <= nextSeq) throw new Error('Conversation history did not advance.');
          nextSeq = event.seq;
        }
        if (nextSeq !== page.nextSeq) throw new Error('Invalid conversation history cursor.');
        // A live message may arrive before the historical turn it belongs to. Merge ordered
        // notifications at their durable sequence, including the frames preceding that event.
        for (const event of events) {
          while (journal.watermarks[journal.watermark]?.seq <= event.seq) {
            this.deliverJournalNotifications(journal, journal.watermarks[journal.watermark++].index + 1);
          }
          thread.applyEvent(event);
        }
        journal.afterSeq = nextSeq;
        if (events.length === PAGE) continue;
        journal.ready = true;
        this.deliverJournalNotifications(journal, journal.notifications.length);
        journal.notifications = [];
        journal.watermarks = [];
        journal.delivered = 0;
        journal.watermark = 0;
        return;
      }
    })().finally(() => { journal.pending = undefined; });
    journal.pending = pending;
    return pending;
  }

  /** A cached conversation can be opened before the desktop transport finishes connecting. */
  private async readThreadHistory(thread: ThreadModel) {
    const retryable = (error: unknown) => {
      const { code } = hostFailure(error);
      // The desktop bridge reports queue saturation and send failures as `failed`.
      return code === 'connecting' || code === 'failed';
    };
    const sameThread = () => !this.stopped && this.threads.get(thread.id) === thread;
    for (let attempt = 0; ; attempt++) {
      const journal = this.journalFor(thread);
      try {
        const joiningBeforeResume = !this.resumedSessions.has(thread.id) && journal.pending !== undefined;
        try { await this.ensureSessionResumed(thread.id); }
        catch (error) {
          if (retryable(error)) throw error;
          // Already open, or recoverable as a read-only log.
        }
        await this.replayJournal(thread, journal);
        // A slow read that began before resume may still have returned the pre-resume log.
        if (joiningBeforeResume) await this.replayJournal(thread, journal);
        return journal;
      } catch (error) {
        const { code } = hostFailure(error);
        // A reconnecting bridge can reject the read after its journal was invalidated.
        // Keep the retry budget across generations while the same conversation exists.
        const canRetry = code === 'connecting' || (code === 'failed' && this.currentJournal(thread, journal));
        if (!canRetry || !sameThread() || attempt >= this.loadRetries) throw error;
        await new Promise<void>(resolve => setTimeout(resolve, this.loadRetryMs));
        if (!sameThread()) throw error;
      }
    }
  }

  /** The chat's history, read once from the engine (after a restart too: the engine keeps it on disk). */
  private async loaded(threadId: string): Promise<ThreadModel | undefined> {
    const thread = this.threads.get(threadId);
    if (thread && !thread.options && this.levels.has(threadId)) thread.options = this.levels.get(threadId);
    if (!thread || thread.loaded) return thread;
    let pending = this.loading.get(threadId);
    if (!pending) {
      pending = (async () => {
        const journal = await this.readThreadHistory(thread);
        if (!this.currentJournal(thread, journal)) return undefined;
        if (thread.loaded) return thread;
        thread.settleUnfinished(this.now());
        // The model this chat is bound to (a chat that has had a turn keeps its own).
        const modeGeneration = this.modeGenerations.get(threadId) ?? 0;
        const processGeneration = this.accessProcessGeneration;
        try {
          const own = (await this.engine.request('hoh/executor/access/get', { executor: thread.executor, sessionId: threadId }))?.session;
          if ((this.modeGenerations.get(threadId) ?? 0) === modeGeneration && this.accessProcessGeneration === processGeneration) {
            if (isHohExecutor(own?.executor) && exactMode(own?.runtimeMode)) {
              thread.executor = own.executor;
              thread.bindingKnown = true;
              thread.builtin = false;
              this.sessionAccessKnown.add(threadId);
            } else if (own?.builtin === true && own.executor == null && own.runtimeMode == null) {
              thread.builtin = true;
              this.sessionAccessKnown.add(threadId);
            }
            if (typeof own?.model === 'string' && own.model) thread.model = own.model;
            thread.mode = effectiveSessionMode(own, thread.mode);
            if (own?.runningRuntimeMode != null) thread.setRunningInteractionMode(own.runningRuntimeMode === 'plan' ? 'plan' : 'default');
          }
        } catch { /* the chat follows the choice for new chats */ }
        if (!this.currentJournal(thread, journal)) return undefined;
        // Place the chat before publishing what its history taught us. Otherwise the first
        // `thread.updated` briefly assigns a chat with a known cwd to "No project".
        if (thread.cwd) {
          this.placeByFolder(thread);
          this.settleLocalImageContext(threadId, thread);
        }
        else if (!this.workspaceKnown && thread.projectId === HOH_PROJECT_ID) {
          try {
            await this.ensureSessionResumed(threadId);
            const directory = await this.engine.request('session/working-directory/get', { sessionId: threadId });
            if (this.currentJournal(thread, journal)) this.setWorkspaceRoot(directory?.cwd);
          } catch { /* the placeholder stays */ }
        }
        if (!this.currentJournal(thread, journal)) return undefined;
        if (thread.loaded) return thread;
        thread.loaded = true;
        // The list row learns what the history knows (a title from the first message, the last answer).
        this.changed(threadId);
        return thread;
      })().finally(() => { if (this.loading.get(threadId) === pending) this.loading.delete(threadId); });
      this.loading.set(threadId, pending);
    }
    return pending;
  }

  private threadFor(sessionId: unknown): ThreadModel | undefined {
    if (typeof sessionId !== 'string' || !sessionId || this.deletedThreads.has(sessionId)) return undefined;
    // A notification comes from an already-open session, including one started elsewhere.
    this.resumedSessions.add(sessionId);
    let thread = this.threads.get(sessionId);
    if (!thread) {
      // A chat started elsewhere (another window, a schedule): it joins the list.
      thread = new ThreadModel({
        id: sessionId,
        title: this.titles.get(sessionId),
        createdAt: this.now(),
        model: this.model(),
        mode: 'confirm',
        executor: 'claude',
        bindingKnown: false,
        interactionMode: this.interactions.get(sessionId),
      });
      this.applyThreadMetadata(thread);
      thread.loaded = true;
      this.threads.set(sessionId, thread);
      const created = thread;
      const sessionRevision = this.nextSessionAccessRevision(sessionId);
      const modeGeneration = this.modeGenerations.get(sessionId) ?? 0;
      const processGeneration = this.accessProcessGeneration;
      const access = this.engine.request('hoh/executor/access/get', { executor: this.selectedExecutor, sessionId })
        .then(result => {
          if (processGeneration !== this.accessProcessGeneration) return;
          if (sessionRevision < (this.sessionAccessApplied.get(sessionId) ?? 0)) return;
          const own = result?.session;
          if ((this.modeGenerations.get(sessionId) ?? 0) !== modeGeneration) {
            // A newer launch owns the mode, but cannot change the conversation's immutable
            // executor binding. Keep that part of this successful answer.
            if (own && isHohExecutor(own.executor) && exactMode(own.runtimeMode)) {
              created.executor = own.executor; created.bindingKnown = true; created.builtin = false;
              if (typeof own.model === 'string' && own.model) created.model = own.model;
              this.sessionAccessApplied.set(sessionId, sessionRevision); this.sessionAccessKnown.add(sessionId); this.changed(sessionId);
            }
            return;
          }
          const mode = exactMode(own?.runningRuntimeMode) ?? exactMode(own?.runtimeMode);
          if (own?.builtin === true && own.executor == null && own.runtimeMode == null) {
            created.builtin = true;
            this.sessionAccessApplied.set(sessionId, sessionRevision); this.sessionAccessKnown.add(sessionId); this.modeRevision++; this.changed(sessionId); return;
          }
          if (!mode || !isHohExecutor(own?.executor)) throw new Error('Conversation access is unavailable.');
          created.executor = own.executor; created.mode = mode;
          created.bindingKnown = true;
          created.builtin = false;
          if (own.runningRuntimeMode != null) created.setRunningInteractionMode(own.runningRuntimeMode === 'plan' ? 'plan' : 'default');
          if (typeof own.model === 'string' && own.model) created.model = own.model;
          this.modeGenerations.set(sessionId, modeGeneration + 1);
          this.sessionAccessApplied.set(sessionId, sessionRevision); this.sessionAccessKnown.add(sessionId); this.modeRevision++; this.changed(sessionId);
        });
      // Retain the promise, including a failure, so sending cannot use a guessed mode.
      this.notificationAccess.set(sessionId, access);
      void access.catch(() => {});
      this.showProject();
    }
    return thread;
  }

  /** Record only a confirmed live launch; this also runs for chats not currently on screen. */
  private async rememberProjectTrust(thread: ThreadModel, revision: number) {
    try {
      const access = await this.engine.request('hoh/executor/access/get', { executor: 'claude', sessionId: thread.id });
      const flags = Array.isArray(access?.effective?.flags) ? access.effective.flags : [];
      const at = flags.indexOf('--setting-sources');
      const sources = at >= 0 && typeof flags[at + 1] === 'string' ? flags[at + 1].split(',') : [];
      if (access?.session?.executor !== 'claude' || !access?.session?.running || !['user', 'project', 'local'].every(source => sources.includes(source))) return;
      const directory = await this.engine.request('session/working-directory/get', { sessionId: thread.id });
      if (typeof directory?.cwd !== 'string' || !directory.cwd) return;
      const trust = await this.engine.request('hoh/executor/trust/get', { executor: 'claude', cwd: directory.cwd });
      if (this.stopped || this.projectTrustRevisions.get(thread.id) !== revision || !trust?.trusted || trust?.changed) return;
      this.threadMetadata.update(thread.id, { appliedProjectTrust: { cwd: directory.cwd, executor: 'claude' } });
    } catch { /* No proof or storage: the banner must not claim a saved launch. */ }
  }

  private onNotification(notification: EngineNotification, at = this.now(), replaying = false) {
    const params = notification.params ?? {};
    if (this.creatingEditBranches.has(params.sessionId)) return;
    const event = params.event;
    if (notification.method === 'session.event') {
      const seedEnd = this.threads.get(params.sessionId)?.sideChat?.seedEndSeq;
      if (seedEnd !== undefined && typeof event?.seq === 'number' && event.seq <= seedEnd) return;
    }
    const eventType = event?.type;
    if (!replaying && notification.method === 'session.event'
      && (eventType === 'plugin:external-harness/folder-trust-changed' || eventType === 'plugin:external-harness/launch')) {
      const thread = this.threadFor(params.sessionId);
      if (thread) {
        const revision = (this.projectTrustRevisions.get(thread.id) ?? 0) + 1;
        this.projectTrustRevisions.set(thread.id, revision);
        try { if (this.threadMetadata.get(thread.id).appliedProjectTrust) this.threadMetadata.update(thread.id, { appliedProjectTrust: null }); } catch { /* Storage is optional. */ }
        if (eventType === 'plugin:external-harness/launch') void this.rememberProjectTrust(thread, revision);
      }
    }
    if (!replaying && (notification.method === 'session.event' || notification.method === 'session.assistant-stream' || notification.method === 'session.status')) {
      const thread = this.threads.get(params.sessionId);
      if (thread && !thread.loaded) {
        const journal = this.journalFor(thread);
        if (!journal.ready) {
          this.resumedSessions.add(thread.id);
          if (notification.method === 'session.status') {
            // Inventory access reads must not supersede new information from the running process.
            this.modeGenerations.set(thread.id, (this.modeGenerations.get(thread.id) ?? 0) + 1);
            void this.refreshThreadMode(thread);
          }
          if (notification.method === 'session.event' && Number.isSafeInteger(params.event?.seq)) {
            journal.watermarks.push({ index: journal.notifications.length, seq: params.event.seq });
          }
          journal.notifications.push({ notification, at });
          if (!journal.pending) {
            this.peeked.delete(thread.id);
            this.schedulePeekToday();
          }
          return;
        }
      }
    }
    switch (notification.method) {
      case 'session.event': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.applyEvent(params.event)) {
          if (!replaying && !thread.loaded && !this.peeked.has(thread.id)) this.schedulePeekToday();
          this.changed(thread.id);
        }
        if (thread && params.event?.type === 'plugin:external-harness/launch') {
          const runningPlan = params.event?.data?.runtimeMode === 'plan';
          const mode = runningPlan ? 'confirm' : exactMode(params.event?.data?.runtimeMode);
          if (mode) {
            thread.setRunningInteractionMode(runningPlan || params.event?.data?.interactionMode === 'plan' ? 'plan' : 'default');
            thread.setRunning(true, at);
            this.sessionAccessKnown.add(thread.id);
            this.modeGenerations.set(thread.id, (this.modeGenerations.get(thread.id) ?? 0) + 1);
            thread.mode = mode;
            this.changed(thread.id);
            if (!thread.bindingKnown) void this.refreshThreadMode(thread);
          }
        }
        if (thread && params.event?.type === 'turn/start') void this.refreshThreadMode(thread);
        // The end of a turn reaches the thread list at once: the sidebar's "Working" must not wait for the next change.
        if (!replaying && thread && params.event?.type === 'turn/end') this.flush();
        if (thread && params.event?.type === 'working-directory/change') {
          this.placeByFolder(thread);
          if (thread.cwd) this.settleLocalImageContext(thread.id, thread);
        }
        return;
      }
      case 'session.assistant-stream': {
        const thread = this.threadFor(params.sessionId);
        const change = thread?.applyStreamFrame(params.frame ?? {}, at);
        if (thread && change) {
          if (thread.loaded && typeof change !== 'boolean') this.streamChanged(thread, change);
          else this.changed(thread.id);
        }
        return;
      }
      case 'session.status': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.setRunning(params.status === 'running', at)) { this.changed(thread.id); if (!replaying && params.status !== 'running') this.flush(); }
        if (thread && !replaying) void this.refreshThreadMode(thread);
        return;
      }
      case 'approval.pending': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.approvalPending(params, at)) this.changed(thread.id);
        return;
      }
      case 'approval.resolved':
        for (const thread of this.threads.values()) if (thread.approvalResolved(params, at)) this.changed(thread.id);
        return;
      case 'user-question.cancelled': {
        this.questions.delete(params.requestId);
        for (const thread of this.threads.values()) if (thread.questionClosed(params.requestId, null, at)) this.changed(thread.id);
        return;
      }
      default: return;
    }
  }

  private onQuestion(question: EngineQuestion) {
    this.questions.set(question.requestId, question);
    const thread = this.threadFor(question.sessionId);
    if (thread?.questionAsked(question, this.now())) this.changed(thread.id);
  }

  /** The engine process ended: no turn of it is still running. History is read again when shown. */
  private onEngineLost() {
    this.accessGeneration++;
    this.accessProcessGeneration++;
    this.resumedSessions.clear();
    this.resumingSessions.clear();
    this.loading.clear();
    for (const question of this.questions.values()) {
      const thread = this.threads.get(question.sessionId);
      if (thread?.pendingRequest(question.requestId)?.kind === 'secret'
        && thread.questionClosed(question.requestId, null, this.now())) this.changed(thread.id);
    }
    this.questions.clear();
    this.notificationAccess.clear();
    this.sessionAccessKnown.clear();
    this.mode = null;
    this.accessError = 'Executor access mode is unknown while the engine is unavailable.';
    // Fail closed immediately: T3 must stop showing or sending a stale mode while the
    // replacement engine has not answered access/get yet.
    this.configChanged();
    for (const thread of this.threads.values()) {
      if (thread.running || thread.activeRunId()) { thread.engineRunning = false; thread.running = false; thread.setRunningInteractionMode(null); thread.settleUnfinished(this.now()); this.changed(thread.id); }
    }
  }

  // ---- change delivery ----

  private homeChanged() {
    if (this.stopped) return;
    this.homeDirty = true;
    this.scheduleFlush();
  }

  private scheduleFlush() {
    if (this.flushTimer) return;
    this.flushTimer = setTimeout(() => this.flush(), this.throttleMs);
    // Node's test host should not stay alive solely for a deferred UI notification.
    (this.flushTimer as any).unref?.();
  }

  private changed(threadId: string) {
    if (this.stopped) return;
    const thread = this.threads.get(threadId);
    if (thread) this.threadSequences.set(thread, ++threadSequence);
    this.dirty.add(threadId);
    this.streamChanges.delete(threadId);
    this.homeChanged();
  }

  private streamChanged(thread: ThreadModel, item: Item) {
    if (this.stopped) return;
    const from = this.projectionSequence(thread);
    const sequence = ++threadSequence;
    this.threadSequences.set(thread, sequence);
    // A queued structural change will deliver a full snapshot with the latest text.
    if (this.dirty.has(thread.id)) return;
    let change = this.streamChanges.get(thread.id);
    if (!change) this.streamChanges.set(thread.id, change = { from, items: new Map() });
    change.items.set(item.id, { item, sequence });
    this.scheduleFlush();
  }

  private accessVisible(threadId: string) {
    // A failed or pending access read must not hide the conversation or its Stop control.
    return this.threads.has(threadId);
  }

  /** Send every pending change now (tests call it; the timer calls it otherwise). */
  flush() {
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.flushTimer = undefined;
    const ids = [...this.dirty]; this.dirty.clear();
    let dispositionChanged = false;
    for (const id of ids) {
      const thread = this.threads.get(id);
      if (!thread) continue;
      if (thread.loaded || this.peeked.has(id)) thread.homeProjectStats = thread.projectStats();
      if (this.syncHomeDisposition(thread)) dispositionChanged = true;
    }
    if (ids.length || dispositionChanged) this.save();
    if (this.homeDirty) { this.homeDirty = false; this.homeVersion++; for (const listener of this.homeListeners) listener(); }
    for (const id of ids) {
      const thread = this.threads.get(id);
      if (!thread || !this.accessVisible(id)) continue;
      this.sequence++;
      if (this.shellSinks.size) {
        const update = { kind: 'thread.updated', sequence: this.sequence, location: thread.archivedAt === null ? 'active' : 'archive', thread: thread.shell() };
        for (const sink of this.shellSinks) sink.emit(update);
      }
      this.publishThread(thread);
    }
    for (const [id, change] of this.streamChanges) {
      const thread = this.threads.get(id);
      if (thread) this.publishThread(thread, change);
    }
    this.streamChanges.clear();
  }

  private projectionSequence(thread: ThreadModel) {
    // A provisional snapshot can paint but cannot serve as a resume baseline.
    if (!thread.loaded) return 0;
    const existing = this.threadSequences.get(thread);
    if (existing !== undefined) return existing;
    const sequence = ++threadSequence;
    this.threadSequences.set(thread, sequence);
    return sequence;
  }

  private threadSnapshot(thread: ThreadModel, snapshotSequence = this.projectionSequence(thread), bounded = false) {
    const projection = thread.projection();
    if (!bounded || !thread.loaded) return { kind: 'snapshot', snapshotSequence, projection };
    const rows = projection.visibleTurnItems;
    const first = Math.max(0, rows.length - TIMELINE_ROWS);
    const retainedStart = this.timelineStarts.get(thread);
    const start = retainedStart === undefined ? first
      : rows.findIndex(row => row.item.ordinal >= retainedStart);
    const latestLocalTurnOrdinal = projection.turnItems.at(-1)?.ordinal ?? null;
    if (start <= 0) return { kind: 'snapshot', snapshotSequence, projection, historyCursor: null, hasMoreHistory: false, latestLocalTurnOrdinal };
    const boundary = rows[start].item.ordinal;
    this.timelineStarts.set(thread, boundary);

    // Keep unanswered decisions and their controls even if a turn has a very long tool log.
    const pending = new Set(projection.runtimeRequests.filter(request => request.status === 'pending').map(request => request.id));
    const inWindow = (item: Item) => item.ordinal >= boundary || item.streaming === true || pending.has(item.requestId);
    const visibleTurnItems = rows.filter(row => inWindow(row.item));
    const turnItems = projection.turnItems.filter(inWindow);
    const messageIds = new Set(turnItems.map(item => item.messageId));
    return {
      kind: 'snapshot', snapshotSequence,
      projection: {
        ...projection, turnItems, visibleTurnItems,
        messages: projection.messages.filter(message => messageIds.has(message.id)),
      },
      historyCursor: String(boundary), hasMoreHistory: true, latestLocalTurnOrdinal,
    };
  }

  /** The local HTTP endpoints used by T3 to open and extend its visible timeline. */
  async readTimeline(threadId: string, beforeOrdinal?: number) {
    await this.start();
    const thread = await this.loaded(threadId);
    if (!thread) return undefined;
    if (beforeOrdinal === undefined) {
      const { kind: _kind, ...snapshot } = this.threadSnapshot(thread, this.projectionSequence(thread), true);
      return snapshot;
    }
    const rows = thread.projection().visibleTurnItems;
    const found = rows.findIndex(row => row.item.ordinal >= beforeOrdinal);
    const end = found < 0 ? rows.length : found;
    let start = Math.max(0, end - EARLIER_TIMELINE_ROWS);
    // Ordinarily begin on a user message, keeping its answer and tool log with it.
    while (start > 0 && end - start < EARLIER_TIMELINE_ROWS * 2 && rows[start].item.type !== 'user_message') start--;
    const items = rows.slice(start, end);
    const first = items[0]?.item.ordinal;
    if (first !== undefined) this.timelineStarts.set(thread, Math.min(this.timelineStarts.get(thread) ?? beforeOrdinal, first));
    return {
      snapshotSequence: this.projectionSequence(thread), items,
      nextCursor: start > 0 ? String(first) : null, hasMoreHistory: start > 0,
    };
  }

  private async findInChat(payload: FindInput & { threadId: string }, onProgress?: (result: FindResult) => void, signal?: AbortSignal) {
    try {
      await this.start();
      signal?.throwIfAborted();
      const [thread, search] = await Promise.all([this.loaded(payload?.threadId), loadThreadSearch()]);
      signal?.throwIfAborted();
      if (!thread || this.threads.get(thread.id) !== thread || this.deletingThreads.has(thread.id)) {
        throw new Error('This chat does not exist.');
      }
      const cwd = thread.cwd ?? this.folders.get(thread.projectId)?.workspaceRoot ?? this.workspaceRoot;
      return await search.findInThread(thread.searchableTimeline(), cwd, this.projectionSequence(thread), payload, onProgress, signal);
    } catch (error) {
      throw new RpcFailure({ _tag: 'OrchestrationV2SearchThreadError', cause: { name: 'Error', message: errorText(error) } });
    }
  }

  private async *searchSources(projects?: ReadonlySet<string>) {
    const threads = [...this.threads.values()].filter(thread => !thread.sideChat && thread.archivedAt === null)
      .sort((left, right) => right.updatedAt - left.updatedAt);
    let read = 0;
    let failure: unknown;
    for (let index = 0; index < threads.length && !this.stopped; index += 3) {
      // Reuse pending journal reads. Today's Home and future searches can use the same pages,
      // and opening a hit is the only action that resumes its native session.
      const batch = threads.slice(index, index + 3);
      await Promise.all(batch.map(async thread => {
        if (thread.loaded || this.deletingThreads.has(thread.id)) return;
        const journal = this.journalFor(thread);
        if (!journal.ready) await this.replayJournal(thread, journal).catch(error => { failure = error; });
      }));
      for (const thread of batch) {
        if (this.threads.get(thread.id) !== thread || this.deletingThreads.has(thread.id) || thread.archivedAt !== null) continue;
        if (!thread.loaded && !this.journals.get(thread)?.ready) continue;
        read++;
        const projectId = thread.cwd ? this.folderAt(thread.cwd)?.id ?? thread.projectId : thread.projectId;
        if (projects && !projects.has(projectId)) continue;
        yield {
          threadId: thread.id, projectId, messages: thread.searchableMessages(),
          subject: { title: thread.titleText(), activityAt: thread.latestUserActivity(), opens: thread.organization.opens },
        };
      }
    }
    if (failure !== undefined && read === 0) throw failure;
  }

  private async searchChats(payload: Record<string, any>) {
    const query = typeof payload.query === 'string' ? payload.query.trim().slice(0, 200) : '';
    if (payload.titlesOnly === true || query.length < 2) return { matches: [] };
    try {
      const search = await loadThreadSearch();
      if (!search.hasMessageSearchTerms(query)) return { matches: [] };
      await this.start();
      const limit = Number.isInteger(payload.limit) ? Math.min(50, Math.max(1, payload.limit)) : 50;
      const projects = Array.isArray(payload.projectIds) ? new Set<string>(payload.projectIds) : undefined;
      const matches = await search.searchThreadMessages(this.searchSources(projects), query, limit, this.now());
      return { matches: matches.filter(hit => this.threads.get(hit.threadId)?.archivedAt === null && !this.deletingThreads.has(hit.threadId)) };
    } catch (error) {
      throw new RpcFailure({ _tag: 'OrchestrationSearchThreadsError', message: 'Could not search chats.', cause: { name: 'Error', message: errorText(error) } });
    }
  }

  private publishThread(thread: ThreadModel, change?: StreamChange) {
    const sinks = this.threadSinks.get(thread.id);
    if (!sinks?.size) return;
    const sequence = this.projectionSequence(thread);
    let snapshot: ReturnType<HohHost['threadSnapshot']> | undefined;
    let boundedSnapshot: ReturnType<HohHost['threadSnapshot']> | undefined;
    let updates: unknown[] | undefined;
    for (const sink of sinks) {
      const before = this.sinkSequences.get(sink);
      if (before === undefined || (sequence !== 0 && before === sequence)) continue;
      if (change && before === change.from) {
        updates ??= [...change.items.values()].sort((a, b) => a.sequence - b.sequence).map(({ item, sequence }) => ({
          kind: 'event', sequence, event: {
            type: 'turn-item.updated', id: 'stream-' + sequence, threadId: thread.id,
            occurredAt: item.updatedAt, payload: { ...item },
          },
        }));
        sink.emit(...updates);
      } else if (this.boundedSinks.has(sink)) {
        sink.emit(boundedSnapshot ??= this.threadSnapshot(thread, sequence, true));
      } else {
        sink.emit(snapshot ??= this.threadSnapshot(thread, sequence));
      }
      this.sinkSequences.set(sink, sequence);
    }
  }

  private config() {
    return serverConfig({
      cwd: this.workspaceRoot,
      platform: this.platform,
      availableEditors: this.availableEditors,
      claudeDiscovery: this.claudeDiscovery,
      codexDiscovery: this.codexDiscovery,
      piDiscovery: this.piDiscovery,
      discoveryFailure: this.discoveryFailure,
      codexDiscoveryFailure: this.codexDiscoveryFailure,
      defaultMode: this.defaultMode,
      defaultInstanceId: instanceOf(this.selectedExecutor),
      defaultModel: this.model(),
      accessKnown: this.mode !== null,
      settingsOverrides: this.settingsOverrides,
      windowPreferences: this.localSettings?.windowPreferences(),
      keybindings: this.localSettings?.keybindings(),
      now: this.iso(),
    });
  }

  private settings() {
    return serverSettings(
      this.model(),
      this.defaultMode,
      this.settingsOverrides,
      instanceOf(this.selectedExecutor),
      this.mode !== null,
      this.localSettings?.windowPreferences(),
    );
  }

  private configChanged() {
    this.pluginCreatorChanged();
    for (const sink of this.configSinks) sink.emit({ version: 1, type: 'snapshot', config: this.config() });
  }

  private editKeybinding(payload: unknown, operation: 'upsertKeybinding' | 'removeKeybinding') {
    try {
      if (!this.localSettings) throw new Error('Local preferences storage is unavailable.');
      const result = this.localSettings[operation](payload);
      for (const sink of this.configSinks) {
        sink.emit({ version: 1, type: 'keybindingsUpdated', payload: result });
      }
      return result;
    } catch (error) {
      throw new RpcFailure({
        _tag: 'KeybindingsConfigParseError',
        configPath: KEYBINDINGS_LOCATION,
        detail: error instanceof Error ? error.message : 'Could not change this shortcut.',
      });
    }
  }

  private lifecycleWelcome(bootstrapStatus: 'pending' | 'complete') {
    const config = this.config();
    return { version: 1, sequence: 0, type: 'welcome', payload: { environment: config.environment, cwd: this.workspaceRoot, projectName: 'HoH', bootstrapStatus } };
  }

  private shellSnapshot() {
    const threads = [...this.threads.values()].filter(thread => this.accessVisible(thread.id)).sort((a, b) => b.updatedAt - a.updatedAt);
    return {
      kind: 'snapshot',
      snapshot: {
        schemaVersion: 1,
        snapshotSequence: this.sequence,
        projects: [...(this.projectShown ? [hohProject(this.workspaceRoot, this.startedAt)] : []), ...[...this.folders.values()].map(folderProject)],
        threads: threads.filter(thread => thread.archivedAt === null).map(thread => thread.shell()),
        archivedThreads: threads.filter(thread => thread.archivedAt !== null).map(thread => thread.shell()),
      },
    };
  }

  // ---- engine actions ----

  /**
   * A new chat in the engine: a session id it has not seen is created by asking for its folder (the
   * engine's default one, "No project"). A chat in a folder project is moved to that folder before its
   * first message, by the request the current window uses for the person's folder choice; the Tauri host
   * may ask the person to allow it. Claude then runs in that folder.
   */
  private async createThread(
    threadId: string,
    title: string | undefined,
    model: string | undefined,
    mode: T3RuntimeMode | undefined,
    projectId?: unknown,
    providerInstanceId?: unknown,
    interactionMode?: unknown,
  ) {
    const interaction = interactionModeOf(interactionMode);
    await this.requireAccess();
    this.creatingThreads.add(threadId);
    this.registerLocalImageContext(threadId);
    this.failedLocalImageContexts.delete(threadId);
    try {
      return await this.withSelectionMutation(async () => {
        const executor = executorOf(providerInstanceId ?? instanceOf(this.selectedExecutor));
        requireInteraction(executor, interaction);
        if (executor === 'pi' && mode !== 'full-access') throw new Error(PI_ACCESS_REQUIRED);
        const chosenModel = model ?? this.model(executor);
        const chosenMode = accessModeOf(mode ?? runtimeModeOf(this.defaultMode));
        const already = this.threads.get(threadId);
        if (already) {
          if (typeof projectId === 'string' && projectId !== already.projectId) {
            throw new Error('A conversation cannot change its project folder. Start a new chat.');
          }
          if (already.executor !== executor || already.model !== chosenModel || already.mode !== chosenMode) {
            throw new Error('Concurrent creation requested a different executor, model, or access mode for this conversation.');
          }
          if (already.cwd) this.settleLocalImageContext(threadId, already);
          return already;
        }
        const folder = typeof projectId === 'string' && projectId !== HOH_PROJECT_ID ? this.folders.get(projectId) : undefined;
        if (typeof projectId === 'string' && projectId !== HOH_PROJECT_ID && !folder) throw new Error('This project does not exist.');
        await this.applyModelLocked(executor, chosenModel);
        // working-directory/get creates the SDK session. Keep select → create → binding check →
        // per-session access under this lock: the engine's global selection must not be changed by
        // another Claude/Codex launch in between.
        const directory = await this.engine.request('session/working-directory/get', { sessionId: threadId });
        this.resumedSessions.add(threadId);
        this.setWorkspaceRoot(directory?.cwd);
        const bound = await this.engine.request('hoh/executor/access/get', { executor, sessionId: threadId });
        if (bound?.session?.executor !== executor || bound?.session?.model !== chosenModel) {
          throw new Error('The engine bound the new conversation to a different executor or model. The message was not sent.');
        }
        if (folder) {
          const set = await this.engine.request('session/working-directory/set', { sessionId: threadId, path: folder.workspaceRoot });
          // A typed `~/…` root becomes the folder the engine resolved, so chats' recorded folders match it.
          const resolved = folderPath(set?.cwd);
          if (resolved !== folder.workspaceRoot && !folder.workspaceRoot.startsWith('~/')) {
            throw new Error('The project folder changed since it was selected. Select it again before sending a message.');
          }
          if (resolved && resolved !== folder.workspaceRoot && folder.workspaceRoot.startsWith('~') && !this.folderAt(resolved)) {
            folder.workspaceRoot = resolved;
            folder.updatedAt = this.now();
            this.emitProject(folderProject(folder));
          }
        }
        const secretary = this.secretaryContexts.has(threadId);
        const thread = new ThreadModel({
          id: threadId, title: secretary ? SECRETARY_TITLE : title,
          createdAt: this.now(), model: chosenModel, mode: this.defaultMode, executor,
          projectId: folder?.id ?? HOH_PROJECT_ID,
        });
        thread.pluginAuthoringPrefixes = this.pluginAuthoringPrefixes;
        thread.setInteractionMode(interaction);
        this.interactions.set(threadId, interaction);
        thread.cwd = folder ? folder.workspaceRoot : folderPath(directory?.cwd) || null;
        thread.loaded = true;
        await this.applyMode(thread, mode ?? runtimeModeOf(this.defaultMode));
        const verified = await this.engine.request('hoh/executor/access/get', { executor, sessionId: threadId });
        if (verified?.session?.executor !== executor || verified?.session?.model !== chosenModel || verified?.session?.runtimeMode !== thread.mode) {
          throw new Error('The engine did not retain the requested executor, model, and access mode. The message was not sent.');
        }
        this.threads.set(threadId, thread);
        this.homes.set(threadId, thread.projectId);
        if (secretary) this.titles.set(threadId, SECRETARY_TITLE);
        this.save();
        if (!folder) this.showProject();
        this.changed(threadId);
        if (thread.cwd) this.settleLocalImageContext(threadId, thread);
        return thread;
      });
    } catch (error) {
      if (this.threadSinks.has(threadId)) this.failedLocalImageContexts.add(threadId);
      this.settleLocalImageContext(threadId, undefined);
      throw error;
    } finally {
      this.creatingThreads.delete(threadId);
    }
  }

  /** A draft can be created before T3's project picker settles. Honour the explicit project on
   * launch as the authoritative choice, before the first prompt reaches Claude. */
  private validateLaunchProject(thread: ThreadModel, projectId: unknown): FolderProject | null {
    if (typeof projectId !== 'string' || projectId === thread.projectId) return null;
    if (thread.userMessageCount() > 0) throw new Error('A conversation cannot change its project folder. Start a new chat.');
    if (projectId === HOH_PROJECT_ID) {
      // Moving an existing engine session back to its implicit startup folder has no safe typed
      // operation. T3 creates scratch drafts in scratch already, so never guess a broader path.
      if (thread.projectId !== HOH_PROJECT_ID) throw new Error('An existing project chat cannot be moved to No project.');
      return null;
    }
    const folder = this.folders.get(projectId);
    if (!folder) throw new Error('This project does not exist.');
    return folder;
  }

  private async selectLaunchProject(thread: ThreadModel, folder: FolderProject | null) {
    if (!folder) return;
    const set = await this.engine.request('session/working-directory/set', { sessionId: thread.id, path: folder.workspaceRoot });
    const resolved = folderPath(set?.cwd);
    if (resolved && resolved !== folder.workspaceRoot && !folder.workspaceRoot.startsWith('~')) throw new Error('The selected project folder changed. Select it again.');
    if (resolved && resolved !== folder.workspaceRoot && folder.workspaceRoot.startsWith('~') && !this.folderAt(resolved)) {
      folder.workspaceRoot = resolved;
      folder.updatedAt = this.now();
      this.emitProject(folderProject(folder));
    }
    thread.cwd = resolved || folder.workspaceRoot;
    thread.projectId = folder.id;
    this.homes.set(thread.id, folder.id);
    this.save();
    this.changed(thread.id);
  }

  // ---- folder projects ("Add project") ----

  private emitProject(project: Record<string, unknown>) {
    this.pluginCreatorChanged();
    this.sequence++;
    for (const sink of this.shellSinks) sink.emit({ kind: 'project.updated', sequence: this.sequence, project });
  }

  /** The folder exists and can be read, when the Tauri host can list folders; otherwise the first chat in it tells. */
  private async checkFolder(root: string, fail: (message: string) => RpcFailure) {
    if (!this.engine.browseFolders) return;
    try { await this.engine.browseFolders({ partialPath: root === '/' ? '/' : root + '/' }); }
    catch (error) { throw fail(`The folder ${root} cannot be opened: ${englishError(error, 'it does not exist or cannot be read.')}`); }
  }

  /** T3's `projects.mutate` (packages/contracts/src/project.ts `ProjectMutation`); answers T3's `Project`. */
  private async mutateProject(mutation: Record<string, any>) {
    await this.start();
    const fail = (message: string) => new RpcFailure({ _tag: 'ProjectMutationError', commandId: String(mutation.commandId ?? 'unknown'), message });
    const projectId = String(mutation.projectId ?? '');
    const answer = (project: FolderProject, deletedAt: string | null = null) => ({ ...folderProject(project), deletedAt });
    switch (mutation.type) {
      case 'project.create': {
        const root = folderPath(mutation.workspaceRoot);
        if (!root.startsWith('/') && root !== '~' && !root.startsWith('~/')) throw fail('Use the full path of the folder (starting with / or ~).');
        if (this.folderAt(root)) throw fail(`${root} is already a project.`);
        if (!projectId || projectId === HOH_PROJECT_ID || this.folders.has(projectId)) throw fail('This project id is taken.');
        await this.checkFolder(root, fail);
        // The engine learns the folder (the person's "Add project" in the main window confirms it), so `@`, `/` and `$`
        // answer for it before its first chat; the engine names the folder as it resolved it.
        let added: string;
        try { added = folderPath((await this.engine.request('hoh/workspace/add', { cwd: root }))?.cwd) || root; }
        catch (error) { throw fail(`The folder ${root} cannot be a project: ${englishError(error, 'HoH refused it.')}`); }
        if (added !== root && this.folderAt(added)) throw fail(`${added} is already a project.`);
        const at = this.now();
        const title = typeof mutation.title === 'string' && mutation.title.trim() ? mutation.title.trim() : titleOf(root);
        const project: FolderProject = { id: projectId, title, workspaceRoot: added, createdAt: at, updatedAt: at, pinnedAt: null, archivedAt: null };
        this.folders.set(projectId, project);
        for (const thread of this.threads.values()) if (folderPath(thread.cwd) === added) this.placeByFolder(thread, false);
        this.save();
        this.emitProject(folderProject(project));
        return answer(project);
      }
      case 'project.update': {
        if (projectId === HOH_PROJECT_ID) return { ...hohProject(this.workspaceRoot, this.startedAt), deletedAt: null };
        const project = this.folders.get(projectId);
        if (!project) throw fail('This project does not exist.');
        if (mutation.workspaceRoot !== undefined && folderPath(mutation.workspaceRoot) !== project.workspaceRoot) throw fail('A project keeps its folder in HoH. Add the other folder as a new project.');
        if (typeof mutation.title === 'string' && mutation.title.trim()) project.title = mutation.title.trim();
        if (typeof mutation.pinned === 'boolean') project.pinnedAt = mutation.pinned ? project.pinnedAt ?? this.now() : null;
        if (typeof mutation.archived === 'boolean') project.archivedAt = mutation.archived ? project.archivedAt ?? this.now() : null;
        project.updatedAt = this.now();
        this.save();
        this.emitProject(folderProject(project));
        return answer(project);
      }
      case 'project.delete': {
        if (projectId === HOH_PROJECT_ID) throw fail('"No project" cannot be removed.');
        const project = this.folders.get(projectId);
        if (!project) throw fail('This project does not exist.');
        // HoH keeps every chat; a project with chats stays (T3 removes its threads only on a forced delete).
        if ([...this.threads.values()].some(thread => thread.projectId === projectId)) throw fail('This project still has chats.');
        this.folders.delete(projectId);
        this.pluginCreatorChanged();
        for (const [threadId, home] of this.homes) if (home === projectId) this.homes.delete(threadId);
        this.save();
        this.sequence++;
        for (const sink of this.shellSinks) sink.emit({ kind: 'project.removed', sequence: this.sequence, projectId });
        return answer(project, new Date(this.now()).toISOString());
      }
      default:
        throw fail('Not available in HoH.');
    }
  }

  /** T3's folder list for "Add project" (apps/server WorkspaceEntries.browse), from the Tauri host. */
  private async browse(payload: Record<string, any>) {
    const partialPath = typeof payload.partialPath === 'string' ? payload.partialPath : '';
    const cwd = typeof payload.cwd === 'string' && payload.cwd.trim() ? payload.cwd : undefined;
    const fail = (message: string) => new RpcFailure({
      _tag: 'FilesystemBrowseError', ...(partialPath.trim() ? { partialPath } : {}), ...(cwd ? { cwd } : {}), failure: 'read_directory_failed', message,
    });
    if (!this.engine.browseFolders) throw fail('This HoH window cannot list folders. Type the full path of the folder.');
    let listing;
    try { listing = await this.engine.browseFolders({ partialPath, ...(cwd ? { cwd } : {}) }); }
    catch (error) { throw fail(englishError(error, `Failed to browse '${partialPath}'.`)); }
    const entries = (Array.isArray(listing?.entries) ? listing.entries : [])
      .filter(entry => typeof entry?.name === 'string' && entry.name.trim() && typeof entry.fullPath === 'string' && entry.fullPath.trim())
      .map(entry => ({ name: entry.name, fullPath: entry.fullPath }));
    return { parentPath: folderPath(listing?.parentPath) || '/', entries };
  }

  /** Lists the "No project" project; its threads may follow it in the list. */
  private showProject() {
    if (this.projectShown) return;
    this.projectShown = true;
    this.projectChanged();
  }

  private projectChanged() {
    if (!this.projectShown) return;
    this.sequence++;
    for (const sink of this.shellSinks) sink.emit({ kind: 'project.updated', sequence: this.sequence, project: hohProject(this.workspaceRoot, this.startedAt) });
  }

  /** The folder chats work in, as the engine names it: the project's root in T3. */
  private setWorkspaceRoot(cwd: unknown) {
    const root = folderPath(cwd);
    if (!root) return;
    this.workspaceKnown = true;
    if (root === this.workspaceRoot) return;
    this.workspaceRoot = root;
    this.save();
    this.projectChanged();
    this.configChanged();
  }

  /**
   * The access mode is the person's choice in T3's composer; the engine applies it (and alone decides
   * what a model may do) before the turn it was chosen for starts. T3 sends the choice with the send:
   * `thread.runtime-mode.set` before the message of a started thread (ChatView.tsx
   * `persistThreadSettingsForNextTurn`), the launch's `runtimeMode` for a new one (the composer's choice,
   * else the default setting this host reports; useComposerSend.ts). The privileged call names the
   * concrete session and carries the exact four-state mode as well as its downgrade-safe projection.
   */
  private async applyMode(thread: ThreadModel, mode: T3RuntimeMode | undefined) {
    if (!mode) return;
    if (thread.builtin) return;
    if (!thread.bindingKnown) throw new Error('Conversation executor and access mode are unavailable.');
    await this.requireAccess();
    const processGeneration = this.accessProcessGeneration;
    const wanted = accessModeOf(mode);
    if (thread.executor === 'pi' && wanted !== 'full') throw new Error(PI_ACCESS_REQUIRED);
    this.modeRevision++;
    // thread.mode describes the running launch; the engine may have a different pending choice.
    // Always submit the person's choice, including a return to the currently displayed mode.
    await this.engine.request('hoh/executor/access/set', {
      executor: thread.executor,
      sessionId: thread.id,
      mode: wanted === 'full' ? 'full' : 'confirm',
      runtimeMode: wanted,
    });
    if (processGeneration !== this.accessProcessGeneration) throw new Error(this.accessError ?? 'Executor access mode is unknown while the engine is unavailable.');
    this.sessionAccessKnown.add(thread.id);
    await this.refreshThreadMode(thread, wanted);
  }

  /** Reconcile the projection with the engine's current launch, never with the browser's request. */
  private async refreshThreadMode(thread: ThreadModel, fallback = thread.mode) {
    const generation = this.modeGenerations.get(thread.id) ?? 0;
    const process = this.accessProcessGeneration;
    try {
      const own = (await this.engine.request('hoh/executor/access/get', { executor: thread.executor, sessionId: thread.id }))?.session;
      if (this.stopped || process !== this.accessProcessGeneration || (this.modeGenerations.get(thread.id) ?? 0) !== generation) return;
      // A just-created SDK session may not expose its executor binding until its first prompt.
      // access/set already accepted `fallback`, and there is no running launch to contradict it.
      if (!own) {
        if (thread.mode !== fallback) { thread.mode = fallback; this.changed(thread.id); }
        return;
      }
      let bindingChanged = false;
      if (isHohExecutor(own.executor) && exactMode(own.runtimeMode)) {
        if (thread.bindingKnown && thread.executor !== own.executor) return;
        bindingChanged = !thread.bindingKnown || thread.executor !== own.executor;
        thread.executor = own.executor;
        thread.bindingKnown = true;
      }
      this.sessionAccessKnown.add(thread.id);
      const actual = effectiveSessionMode(own, fallback);
      const runningInteraction = own.runningRuntimeMode == null ? null : own.runningRuntimeMode === 'plan' ? 'plan' : 'default';
      const planChanged = thread.runningInteractionMode !== runningInteraction;
      thread.setRunningInteractionMode(runningInteraction);
      if (thread.mode !== actual || planChanged || bindingChanged) { thread.mode = actual; this.changed(thread.id); }
    } catch { /* keep the last engine-reported value until the bridge answers again */ }
  }

  private async withSelectionMutation<T>(operation: () => Promise<T>): Promise<T> {
    let release!: () => void;
    const previous = this.selectionTail;
    this.selectionTail = new Promise<void>(resolve => { release = resolve; });
    await previous;
    try { return await operation(); } finally { release(); }
  }

  /** Caller holds `selectionTail`; createThread keeps this through session binding verification. */
  private async applyModelLocked(executor: HohExecutor, model: string | undefined) {
    if (!model || (executor === this.selectedExecutor && model === this.selectedModel)) return;
    this.modelRevision++;
    await this.engine.request('hoh/executor/select', { executor, model });
    this.selectedExecutor = executor;
    this.selectedModel = model;
    this.configChanged();
  }

  private async applyModel(executor: HohExecutor, model: string | undefined) {
    await this.withSelectionMutation(() => this.applyModelLocked(executor, model));
  }

  canEditUserMessage(threadId: string, messageId: string) {
    const thread = this.threads.get(threadId);
    return Boolean(thread?.bindingKnown && !thread.builtin && thread.editableUserMessage(messageId));
  }

  async editUserMessage(threadId: string, messageId: string, text: string): Promise<string> {
    await this.start();
    const source = await this.loaded(threadId);
    return this.withSelectionMutation(async () => {
      if (!source || !this.canEditUserMessage(threadId, messageId)
        || this.deletingThreads.has(threadId) || this.threads.get(threadId) !== source) {
        throw new Error('This message is no longer available to edit.');
      }
      if (source.running || source.activeRunId()) throw new Error('Stop the current turn before editing a message.');
      const prompt = text.trim();
      if (!prompt) throw new Error('Enter a message.');
      const original = source.editableUserMessage(messageId)!;
      await this.requireAccess();
      const id = globalThis.crypto?.randomUUID?.() ?? `edit-${this.now()}-${Math.random().toString(36).slice(2)}`;
      const sessionId = isSecretaryThreadId(source.id) ? SECRETARY_THREAD_PREFIX + id : id;
      this.creatingEditBranches.add(sessionId);
      let fork: SessionForkForEditResult;
      try {
        fork = await this.engine.request('session/fork-for-edit', {
          sessionId: threadId, messageId: original.engineId, targetSessionId: sessionId,
        });
      } finally {
        this.creatingEditBranches.delete(sessionId);
      }
      const branch = new ThreadModel({
        id: sessionId, title: source.titleText() + ' (edited)', createdAt: this.now(),
        model: source.model, mode: 'confirm', executor: source.executor,
        projectId: source.projectId, interactionMode: source.interactionMode,
      });
      branch.cwd = source.cwd;
      branch.pluginCreator = source.pluginCreator;
      branch.pluginAuthoringPrefixes = this.pluginAuthoringPrefixes;
      this.threads.set(sessionId, branch);
      this.resumedSessions.add(sessionId);
      let sending = false;
      try {
        const bound = await this.engine.request('hoh/executor/access/get', { executor: branch.executor, sessionId });
        if (bound?.session?.executor !== source.executor || bound?.session?.model !== source.model) {
          throw new Error('The edit was opened with a different executor or model.');
        }
        const journal = this.journalFor(branch);
        await this.replayJournal(branch, journal);
        branch.settleUnfinished(this.now());
        branch.loaded = true;
        await this.applyMode(branch, runtimeModeOf(source.mode));
        this.setLevel(branch, source.options);
        sending = true;
        await this.send(branch, 'msg-' + sessionId, prompt, [], branch.interactionMode, undefined, {
          blocks: fork.attachments, shown: original.attachments, authoringPrefix: original.authoringPrefix,
        });
      } catch (error) {
        const { code } = hostFailure(error);
        if (sending && (code === 'connecting' || code === 'failed')) {
          // The bridge can lose the receipt after the engine accepted the message.
          this.flush();
          throw new Error('Could not confirm the send. Check the edited chat before retrying.');
        }
        await this.dispatch({ type: 'thread.delete', threadId: sessionId, commandId: sessionId }).catch(() => {});
        throw new Error(errorText(error));
      }
      this.homes.set(sessionId, branch.projectId);
      this.interactions.set(sessionId, branch.interactionMode);
      this.titles.set(sessionId, branch.title);
      this.save();
      this.flush();
      return sessionId;
    });
  }

  /**
   * One message to the engine: its pictures first (the bytes kept from T3's upload or `assets.persistChatAttachments`; the engine's
   * attachment store checks and copies them), then the text in the syntax supported by the selected harness.
   */
  private async send(thread: ThreadModel, messageId: string, text: string, attachments: unknown = [], interactionMode?: T3InteractionMode, context?: { records?: unknown }, edit?: { blocks: unknown[]; shown: ShownAttachment[]; authoringPrefix?: string }) {
    if (!thread.builtin) await this.requireAccess();
    if (!this.sessionAccessKnown.has(thread.id)) await this.notificationAccess.get(thread.id);
    if (this.deletingThreads.has(thread.id) || this.deletedThreads.has(thread.id) || this.threads.get(thread.id) !== thread)
      throw new Error('This chat is being deleted or was deleted.');
    if (!thread.bindingKnown && !thread.builtin) throw new Error('Conversation executor and access mode are unavailable.');
    const firstMessage = thread.userMessageCount() === 0;
    const images = this.keptImages(attachments);
    // T3 releases the completed upload immediately after send. Promote its process-local preview
    // first, so the timeline can keep rendering it without retaining upload bytes or opening CSP.
    for (const image of images) {
      this.previewUrl(image);
      this.sentPreviewIds.add(image.id);
    }
    const commandFolder = folderPath(thread.cwd) || (thread.projectId === HOH_PROJECT_ID ? this.workspaceRoot : '');
    const prompt = (edit?.authoringPrefix ?? this.secretaryContexts.get(thread.id)?.prefix ?? '') + skillsAsCommands(
      stripAttachedImageReferences(text, images.map(image => image.id), context),
      this.skillsByFolder.get(`${thread.executor}\0${commandFolder}`) ?? new Set(),
      thread.executor,
    );
    const sentAt = this.now();
    const organization = thread.organization;
    const wake: Partial<ThreadOrganization> = {};
    if (organization.lastVisitedAt === null) wake.lastVisitedAt = sentAt - 1;
    if (organization.settledOverride !== null) {
      wake.settledOverride = null;
      wake.settledAt = null;
      if (organization.settledOverride === 'settled') wake.unsettledAt = sentAt;
    }
    if (organization.snoozedUntil !== null) {
      wake.snoozedUntil = null;
      wake.snoozedAt = null;
    }
    if (Object.keys(wake).length) {
      try { this.threadMetadata.update(thread.id, wake); } catch { /* Sending still works without window storage. */ }
      thread.applyOrganization(wake);
    }
    this.homePrompts.add(thread.id);
    thread.sendLocal(messageId, text, sentAt, edit?.shown ?? images.map(({ data: _data, ...shown }) => ({ type: 'image' as const, ...shown })), prompt);
    this.changed(thread.id);
    const contentBlocks = [
      ...(edit?.blocks ?? []),
      ...images.map(image => ({ type: 'image', data: image.data, mimeType: image.mimeType, name: image.name })),
      ...(prompt || !images.length ? [{ type: 'text', text: prompt }] : []),
    ];
    try {
      const effort = thread.executor === 'claude' ? effortOf(thread.model, thread.options) : codexEffortOf(thread.options);
      const result = await this.engine.request('session/prompt', {
        sessionId: thread.id,
        contentBlocks,
        interactionMode: thread.executor !== 'claude' ? 'default' : interactionMode ?? thread.interactionMode,
        ...effortParam(effort),
      });
      this.homePrompts.delete(thread.id);
      this.failedHomePrompts.delete(thread.id);
      if (this.homeDispositions.delete(thread.id)) this.save();
      for (const image of images) this.uploads.delete(image.id);
      if (typeof result?.messageId === 'string') {
        thread.receipt(result.messageId, messageId);
        const startedAt = thread.promptTime(messageId);
        if (startedAt !== undefined) {
          try {
            const metadata = this.threadMetadata.get(thread.id);
            this.threadMetadata.update(thread.id, { promptTimes: { ...metadata.promptTimes, [result.messageId]: startedAt } });
          } catch { /* the engine history remains usable if window storage is unavailable */ }
        }
      }
      if (firstMessage && !edit && !thread.builtin && !isSecretaryThreadId(thread.id) && thread.executor === 'claude') {
        void this.generateTitle(thread, text);
      }
    } catch (error) {
      this.homePrompts.delete(thread.id);
      if (this.homeDispositions.has(thread.id)) this.failedHomePrompts.add(thread.id);
      thread.sendFailed(messageId, this.now(), error);
      this.changed(thread.id);
      throw error;
    }
  }

  /** The kept pictures a message names; a picture this host does not hold (a file, or one given before a restart) is refused. */
  private keptImages(attachments: unknown): KeptImage[] {
    return (Array.isArray(attachments) ? attachments : []).map((attachment: any) => {
      const kept = typeof attachment?.id === 'string' ? this.uploads.get(attachment.id) : undefined;
      if (kept) return kept;
      throw new Error(attachment?.type === 'image' ? `The picture ${String(attachment?.name ?? '')} is no longer here. Attach it again.` : 'This server does not support file attachments.');
    });
  }

  /** The pictures kept here and not yet sent (announced or with their bytes): their count and bytes. */
  private keptSize() {
    let bytes = 0;
    for (const image of this.uploads.values()) bytes += image.sizeBytes;
    for (const slot of this.slots.values()) bytes += slot.sizeBytes;
    return { count: this.uploads.size + this.slots.size, bytes };
  }

  /** T3's `attachments.createUploadUrl`: an address for one picture's bytes, checked as T3's server checks it. */
  private createUploadUrl(payload: Record<string, any>) {
    const now = this.now();
    for (const [id, slot] of this.slots) if (slot.expiresAt <= now) this.slots.delete(id);
    const minted = uploadSlot(payload, this.keptSize(), now + UPLOAD_TTL_MS);
    if ('error' in minted) throw new RpcFailure({ _tag: 'EnvironmentAuthorizationError', message: minted.error, requiredScope: 'orchestration:operate' });
    const attachmentId = 'img-' + (globalThis.crypto?.randomUUID?.() ?? `${now.toString(36)}-${this.slots.size}`);
    this.slots.set(attachmentId, minted.slot);
    return { attachmentId, relativeUrl: `/api/attachments/upload/${attachmentId}`, expiresAt: minted.slot.expiresAt };
  }

  /**
   * The bytes T3 sends to an upload address (its XMLHttpRequest, answered in the window, `installHohXhr`): kept until
   * the message goes when they are the picture announced there — its type and its size; an address is used once.
   */
  async receiveUpload(pathname: string, contentType: string, body: unknown): Promise<{ status: number }> {
    const id = UPLOAD_PATH.exec(pathname)?.[1];
    const slot = id ? this.slots.get(id) : undefined;
    if (!id || !slot || slot.expiresAt <= this.now()) return { status: 404 };
    this.slots.delete(id);
    if (contentType.split(';')[0].trim().toLowerCase() !== slot.mimeType) return { status: 415 };
    const blob = body instanceof Blob ? body : null;
    if (!blob || blob.size !== slot.sizeBytes) return { status: 400 };
    const bytes = new Uint8Array(await blob.arrayBuffer());
    if (bytes.length !== slot.sizeBytes) return { status: 400 };
    this.uploads.set(id, { id, name: slot.name, mimeType: slot.mimeType, sizeBytes: slot.sizeBytes, data: base64Of(bytes) });
    return { status: 204 };
  }

  /** T3's `assets.persistChatAttachments`: pictures checked as T3's server checks them, kept here until sent. */
  private persistAttachments(payload: Record<string, any>) {
    const read = readUploads(payload.attachments, () => 'img-' + (globalThis.crypto?.randomUUID?.() ?? `${this.now().toString(36)}-${this.uploads.size}`));
    if ('error' in read) throw new RpcFailure({ _tag: 'PersistChatAttachmentsError', message: read.error });
    for (const image of read.images) this.uploads.set(image.id, image);
    return { attachments: read.images.map(({ data: _data, ...shown }) => ({ type: 'image', ...shown })) };
  }

  /**
   * T3's `@` (`projects.searchEntries`): the files of a folder project, by relative path, from the engine (which answers
   * only for a folder of its conversations). "No project" and a folder the engine cannot search give an empty list.
   */
  private async searchEntries(payload: Record<string, any>) {
    await this.start();
    const empty = { entries: [], truncated: false };
    const cwd = folderPath(payload.cwd);
    if (!cwd || !this.folderAt(cwd)) return empty;
    const limit = Math.min(Math.max(Math.trunc(Number(payload.limit) || 1), 1), 200);
    const query = typeof payload.query === 'string' ? payload.query.trim().slice(0, 256) : '';
    try { return searchAnswer(await this.engine.request('hoh/workspace/entries', { cwd, query, limit }), payload.kind, payload.imageOnly); }
    catch { return empty; }
  }

  /**
   * T3's workspace snapshot of the composer's folder: the selected harness's commands and skills, read without a turn.
   * "No project" whose folder is not known here yet is asked without a folder: the engine answers for its default one,
   * unless that is its own data (the desktop) — then Claude is asked once a chat names the folder.
   */
  private async workspaceSnapshot(cwd: string, executor: HohExecutor) {
    try {
      const noProject = !this.folderAt(cwd) && folderPath(cwd) === this.workspaceRoot && !this.workspaceKnown;
      const found = await this.engine.request('hoh/executor/commands', noProject ? { executor } : { executor, cwd });
      const snapshot = workspaceSnapshot(cwd, this.iso(), found);
      const skills = new Set(snapshot.skills.map(skill => skill.name));
      for (const folder of [folderPath(cwd), folderPath(found?.cwd)]) if (folder) this.skillsByFolder.set(`${executor}\0${folder}`, skills);
      if (noProject) this.setWorkspaceRoot(found?.cwd);
      return snapshot;
    } catch {
      // The executor could not be asked here (not installed, signed out, a folder with no conversation yet): T3 asks again later.
      return { cwd, checkedAt: this.iso(), slashCommands: [], skills: [], slashCommandsPending: true };
    }
  }

  private async requestTitle(message: string, currentTitle?: string) {
    const prompt = `Generate a title that will help the user recognize this HoH chat weeks later.
Return JSON with key title.

Editorial rules:
- 3-8 words, fewer than 40 characters.
- Use a compact noun phrase or clear action phrase.
- Title the durable subject and desired outcome, not the plan, model, tools, tests, or workflow.
- Do not claim the work is complete, copy and truncate the message, use quotes, filler, or trailing punctuation.

USER MESSAGES:
${message.slice(0, 8000)}`;
    const result = await this.engine.request('hoh/executor/title/generate', {
      executor: 'claude',
      prompt: currentTitle ? `${prompt}\n\nCURRENT TITLE:\n${currentTitle}\nKeep it if it already fits.` : prompt,
    });
    const title = typeof result?.title === 'string' ? result.title.trim() : '';
    if (!title || title.length > 120) throw new Error('Claude returned an invalid chat title.');
    return title;
  }

  private isCurrentTitleRequest(thread: ThreadModel, requestId: string) {
    return !this.stopped && this.threads.get(thread.id) === thread && this.titleRequests.get(thread.id) === requestId;
  }

  private async generateTitle(thread: ThreadModel, message: string) {
    // A title explicitly saved by the person takes precedence over automatic naming.
    if (thread.titleRegeneration || this.threadMetadata.get(thread.id).title) return;
    const baseline = thread.title;
    const requestId = globalThis.crypto?.randomUUID?.() ?? `${this.now()}-${Math.random()}`;
    this.titleRequests.set(thread.id, requestId);
    try {
      const title = await this.requestTitle(message);
      if (!this.isCurrentTitleRequest(thread, requestId) || thread.title !== baseline || thread.archivedAt !== null) return;
      thread.title = title;
      this.titles.set(thread.id, title);
      this.save(); this.changed(thread.id); this.flush();
    } catch { /* A title is optional: the first-message fallback stays. */ }
    finally { if (this.isCurrentTitleRequest(thread, requestId)) this.titleRequests.delete(thread.id); }
  }

  private armTitleRegeneration(thread: ThreadModel, requestId: string) {
    if (!requestId.trim()) throw new Error('Invalid chat title request.');
    if (thread.archivedAt !== null) throw new Error('This chat is archived.');
    if (thread.lastTitleRegeneration?.requestId === requestId || thread.titleRegeneration?.requestId === requestId) return;
    if (thread.titleRegeneration) throw new Error('A title request is already running for this chat.');
    this.titleRequests.set(thread.id, requestId);
    thread.titleRegeneration = { requestId, startedAt: this.iso() };
    this.changed(thread.id);
    void this.regenerateTitle(thread, requestId);
  }

  private async regenerateTitle(thread: ThreadModel, requestId: string) {
    let status: 'renamed' | 'unchanged' | 'failed' = 'unchanged';
    let message: string | undefined;
    try {
      await this.loaded(thread.id);
      if (!this.isCurrentTitleRequest(thread, requestId)) return;
      if (!thread.bindingKnown || thread.builtin || thread.executor !== 'claude') {
        throw new Error('Select a Claude chat to generate its title.');
      }
      const prompts: string[] = [];
      for (const item of thread.searchableMessages()) {
        if (item.role !== 'user' || !item.text.trim()) continue;
        prompts.push(item.text.slice(0, 1200));
        if (prompts.length === 6) break;
      }
      if (!prompts.length) throw new Error('Send a message before generating a title.');
      let title: string;
      try { title = await this.requestTitle(prompts.reverse().join('\n\n'), thread.titleText()); }
      catch { throw new Error('Claude could not generate a title. Try again.'); }
      if (!this.isCurrentTitleRequest(thread, requestId)) return;
      if (title !== thread.titleText()) {
        this.threadMetadata.update(thread.id, { title });
        this.titles.set(thread.id, title);
        thread.title = title;
        status = 'renamed';
      }
    } catch (error) {
      status = 'failed';
      message = errorText(error);
    }
    if (!this.isCurrentTitleRequest(thread, requestId)) return;
    this.titleRequests.delete(thread.id);
    thread.titleRegeneration = null;
    thread.lastTitleRegeneration = { requestId, status, completedAt: this.iso(), ...(message ? { message } : {}) };
    this.changed(thread.id);
    this.flush();
  }

  /** «Стоп»: the same cancel as the current window; the engine ends Claude and its commands. */
  private async cancel(thread: ThreadModel) {
    const stoppedOrdinal = thread.activeRunId() ? thread.home()?.turnOrdinal : undefined;
    const disposition = this.homeDispositions.get(thread.id);
    const replacedCrashDisposition = stoppedOrdinal !== undefined && disposition?.turnOrdinal === stoppedOrdinal && disposition.interrupted === true;
    const stopAt = this.now();
    if (stoppedOrdinal !== undefined) {
      thread.markStopTime(stopAt);
      try {
        const metadata = this.threadMetadata.get(thread.id);
        this.threadMetadata.update(thread.id, { stopTimes: { ...metadata.stopTimes, [String(stoppedOrdinal)]: stopAt } });
      } catch { /* Engine history remains authoritative when storage is unavailable. */ }
      thread.stopRequested = true;
      // Persist the person's intent before crossing the engine boundary. Closing the window while cancel is
      // still in flight must not turn an explicit Stop back into crash recovery on the next launch.
      this.homeDispositions.set(thread.id, { turnOrdinal: stoppedOrdinal, state: 'waiting', stopped: true });
      this.save();
    }
    try {
      await this.engine.request('session/cancel', { sessionId: thread.id });
    } catch (error) {
      // A live window can report an actual refusal and continue the run. Once the host is closing, preserve the
      // already-durable user intent: the new window is authoritative and will reconcile the engine history.
      const current = this.homeDispositions.get(thread.id);
      if (!this.stopped && stoppedOrdinal !== undefined && thread.activeRunId()
        && current?.turnOrdinal === stoppedOrdinal && current.state === 'waiting' && current.stopped === true) {
        thread.stopRequested = false;
        thread.clearStopTime(stoppedOrdinal);
        try { const times = { ...this.threadMetadata.get(thread.id).stopTimes }; delete times[String(stoppedOrdinal)]; this.threadMetadata.update(thread.id, { stopTimes: times }); } catch { /* Storage is optional. */ }
        if (disposition) this.homeDispositions.set(thread.id, disposition); else this.homeDispositions.delete(thread.id);
        this.save();
        this.changed(thread.id);
        this.flush();
      }
      throw error;
    }
    // The turn is stopped now: its run closes and the thread list stops saying "Working" without waiting for the engine's closing event.
    if (thread.interrupt(stopAt) || replacedCrashDisposition || stoppedOrdinal !== undefined) { this.changed(thread.id); this.flush(); }
  }

  private async startSideChat(command: Record<string, any>) {
    const sourceId = typeof command.sourceThreadId === 'string' ? command.sourceThreadId : '';
    const targetId = typeof command.targetThreadId === 'string' ? command.targetThreadId : '';
    if (command.sideChat !== true || command.sourcePoint?.type !== 'latest_stable') {
      throw new Error('Only side chats can be opened from this command.');
    }
    if (!targetId || targetId === sourceId || this.threads.has(targetId) || this.creatingEditBranches.has(targetId)) {
      throw new Error('This side chat id is already in use.');
    }
    this.creatingEditBranches.add(targetId);
    let created = false;
    try {
      const source = await this.loaded(sourceId);
      if (!source || !source.bindingKnown || source.builtin || source.sideChat || source.archivedAt !== null
        || source.userMessageCount() === 0 || this.deletingThreads.has(sourceId)) {
        throw new Error('Open a side chat from a started main conversation.');
      }
      await this.ensureSessionResumed(source.id);
      // The engine snapshots its own stable boundary, even while the parent is streaming.
      const result = await this.engine.request('session/fork-side-chat', { sessionId: source.id, targetSessionId: targetId });
      created = result?.sessionId === targetId;
      const sideChat = this.readSideChat({ parentSessionId: source.id, seedEndSeq: result?.seedEndSeq });
      if (!created || !sideChat) throw new Error('The engine did not confirm the side chat.');

      const child = new ThreadModel({
        id: targetId, title: typeof command.title === 'string' ? command.title : 'Side chat',
        createdAt: this.now(), model: source.model, executor: source.executor, mode: 'confirm', projectId: source.projectId,
      });
      child.sideChat = sideChat;
      child.cwd = source.cwd;
      child.loaded = true;
      this.resumedSessions.add(targetId);
      await this.withSelectionMutation(async () => {
        await this.applyMode(child, runtimeModeOf(source.mode));
        const own = (await this.engine.request('hoh/executor/access/get', {
          executor: child.executor, sessionId: child.id,
        }))?.session;
        if (own?.executor !== source.executor || typeof own.model !== 'string' || own.runtimeMode !== child.mode) {
          throw new Error('The side chat did not retain the main chat’s harness and permissions.');
        }
        child.model = own.model;
      });
      this.setLevel(child, source.options);
      try { this.threadMetadata.update(child.id, { sideChat }); } catch { /* Engine inventory also carries the lineage. */ }
      this.homes.set(child.id, child.projectId);
      this.threads.set(child.id, child);
      this.save();
      this.changed(child.id);
    } catch (error) {
      if (created) {
        this.resumedSessions.delete(targetId);
        await this.engine.request('session/delete', { sessionId: targetId }).catch(() => {});
      }
      throw error;
    } finally {
      this.creatingEditBranches.delete(targetId);
    }
  }

  private async respond(thread: ThreadModel, command: Record<string, any>) {
    const requestId = String(command.requestId ?? '');
    const request = thread.pendingRequest(requestId);
    if (!request) throw dispatchError(command, 'This request is no longer waiting for an answer.');
    if (request.kind === 'secret') throw dispatchError(command, 'Use the private credential form.');
    if (request.kind === 'user_input') {
      const answers = command.answers && typeof command.answers === 'object' ? command.answers as Record<string, unknown> : {};
      if (!this.engine.answerQuestion) throw dispatchError(command, 'Questions cannot be answered here.');
      await this.engine.answerQuestion(requestId, questionAnswer(this.questions.get(requestId), answers));
      this.questions.delete(requestId);
      thread.questionClosed(requestId, answers, this.now());
      this.changed(thread.id);
      return;
    }
    if (!this.engine.decideApproval) throw dispatchError(command, 'Permission requests cannot be answered here.');
    const decision = command.decision;
    const allowed = decision === 'accept' || decision === 'acceptForSession' || decision === 'acceptAlways';
    await this.engine.decideApproval(requestId, allowed ? 'allowed-once' : 'rejected');
    thread.approvalResolved({ approvalId: requestId, outcome: allowed ? 'allowed-once' : 'rejected' }, this.now());
    this.changed(thread.id);
  }

  private async respondToSecret(input: Record<string, any>) {
    const fail = (reason: string): never => { throw new RpcFailure({ _tag: 'SecretRequestError', reason }); };
    const thread = this.threads.get(input.threadId);
    const id = typeof input.turnItemId === 'string' && input.turnItemId.startsWith('item-question-')
      ? input.turnItemId.slice('item-question-'.length) : '';
    if (!thread || thread.pendingRequest(id)?.kind !== 'secret' || this.questions.get(id)?.sessionId !== thread.id) {
      return fail('not_found');
    }
    const answer = input.answer;
    const save = answer?.type === 'save' && typeof answer.secret === 'string';
    if (!this.engine.answerSecret || (!save && answer?.type !== 'decline')) return fail('store_failed');
    try {
      await this.engine.answerSecret(thread.id, id, save ? answer.secret : null);
    } catch { return fail('store_failed'); }
    this.questions.delete(id);
    if (thread.questionClosed(id, null, this.now(), save ? 'saved' : 'declined')) this.changed(thread.id);
    this.flush();
  }

  private async respondImmediately(threadId: string, command: Record<string, any>) {
    const thread = this.threads.get(threadId);
    const requestId = String(command.requestId ?? '');
    if (thread?.pendingRequest(requestId)) return this.respond(thread, command);
    if (this.questions.get(requestId)?.questions.some(question => question.intent?.kind === 'secret-file'))
      throw dispatchError(command, 'Use the private credential form.');
    if (command.answers && this.engine.answerQuestion) {
      await this.engine.answerQuestion(requestId, questionAnswer(this.questions.get(requestId), command.answers));
    } else if (this.engine.decideApproval) {
      const allowed = ['accept', 'acceptForSession', 'acceptAlways'].includes(command.decision);
      await this.engine.decideApproval(requestId, allowed ? 'allowed-once' : 'rejected');
    } else throw dispatchError(command, 'This request is no longer available.');
  }

  private updateThreadOrganization(thread: ThreadModel, patch: ThreadMetadata) {
    this.threadMetadata.update(thread.id, patch);
    thread.applyOrganization(patch);
    if (patch.pinnedAt !== undefined) thread.pinnedAt = patch.pinnedAt;
    if (patch.pinOrderKey !== undefined) thread.pinOrderKey = patch.pinOrderKey;
    this.changed(thread.id);
  }

  private async organizeThread(thread: ThreadModel, command: Record<string, any>) {
    const type = command.type;
    const visits = type === 'thread.visit' && command.opened === true ? await loadThreadSearch() : null;
    if (!thread.loaded && ['thread.mark-unread', 'thread.settle', 'thread.auto-settle', 'thread.snooze'].includes(type)) {
      const journal = this.journalFor(thread);
      if (!journal.ready) await this.replayJournal(thread, journal);
    }
    if (this.threads.get(thread.id) !== thread || this.deletingThreads.has(thread.id)) {
      throw new Error('This chat is being deleted or was deleted.');
    }
    if (thread.archivedAt !== null && !['thread.visit', 'thread.mark-unread', 'thread.tag.set'].includes(type)) {
      throw new Error('This chat is archived.');
    }
    const now = this.now();
    const state = thread.organization;
    let patch: ThreadMetadata;
    switch (type) {
      case 'thread.visit': {
        const visitedAt = Date.parse(command.visitedAt);
        if (!Number.isFinite(visitedAt)) throw new Error('Invalid chat visit time.');
        if (state.lastVisitedAt !== null && visitedAt <= state.lastVisitedAt && !visits) return;
        patch = {
          lastVisitedAt: Math.max(state.lastVisitedAt ?? visitedAt, visitedAt),
          ...(visits ? { opens: visits.recordThreadOpen(state.opens, now,
            typeof command.searchQuery === 'string' ? command.searchQuery : undefined) } : {}),
        };
        // The visit tracker follows shell rows. A read receipt should not republish a large transcript
        // or move its search sequence while a response is streaming.
        this.threadMetadata.update(thread.id, patch);
        thread.applyOrganization(patch);
        const sequence = ++this.sequence;
        for (const sink of this.shellSinks) sink.emit({
          kind: 'thread.updated', sequence, location: thread.archivedAt === null ? 'active' : 'archive', thread: thread.shell(),
        });
        return;
      }
      case 'thread.mark-unread': {
        const completedAt = thread.latestRunCompletion();
        if (completedAt === null) throw new Error('This chat has no completed run to mark unread.');
        patch = { lastVisitedAt: completedAt - 1 };
        if (state.snoozedUntil !== null && state.snoozedUntil <= now) {
          Object.assign(patch, { snoozedUntil: null, snoozedAt: null, lastSnoozeWakeAt: state.snoozedUntil });
        }
        break;
      }
      case 'thread.auto-settle': case 'thread.settle': {
        if (type === 'thread.auto-settle' && (state.settledOverride !== null || state.autoSettleDisabledAt !== null
          || !(Date.parse(command.snapshotAt) >= thread.updatedAt))) {
          throw new Error('The chat changed before automatic settlement.');
        }
        if (thread.activeRunId() || thread.engineRunning || thread.hasPendingApproval()) {
          throw new Error('Wait for this chat to finish before settling it.');
        }
        if (command.expectedLatestRunId !== undefined && command.expectedLatestRunId !== thread.shell().latestRunId) {
          throw new Error('The chat received another run before settlement.');
        }
        const requested = typeof command.settledAt === 'string' ? Date.parse(command.settledAt) : NaN;
        patch = {
          settledOverride: 'settled', settledAt: state.settledAt ?? (Number.isFinite(requested) ? requested : now),
          unsettledAt: null, pinnedAt: null, pinOrderKey: null, activeOrderKey: null, snoozedUntil: null, snoozedAt: null,
        };
        break;
      }
      case 'thread.unsettle': {
        const restored = command.restoreUnsettledAt;
        const at = restored === null ? null : typeof restored === 'string' ? Date.parse(restored) : now;
        if (at !== null && !Number.isFinite(at)) throw new Error('Invalid chat unsettle time.');
        if (state.settledOverride === 'active') return;
        patch = { settledOverride: 'active', settledAt: null, unsettledAt: at };
        break;
      }
      case 'thread.snooze': {
        const until = Date.parse(command.snoozedUntil);
        if (!(until > now)) throw new Error('Choose a snooze time in the future.');
        if (thread.hasPendingApproval()) throw new Error('A chat waiting for an approval cannot be snoozed.');
        patch = { snoozedUntil: until, snoozedAt: now };
        break;
      }
      case 'thread.unsnooze':
        if (state.snoozedUntil === null) return;
        patch = { snoozedUntil: null, snoozedAt: null, lastSnoozeWakeAt: now };
        break;
      case 'thread.auto-settle.set':
        patch = { autoSettleDisabledAt: command.enabled ? null : state.autoSettleDisabledAt ?? now };
        break;
      case 'thread.active.reorder': {
        const key = typeof command.orderKey === 'string' ? command.orderKey.trim() : '';
        if (!key || thread.pinnedAt !== null || state.settledOverride === 'settled') {
          throw new Error('Only active chats can be reordered here.');
        }
        patch = { activeOrderKey: key };
        break;
      }
      case 'thread.tag.set': {
        const tag = threadTag(command.tag);
        if (!tag || typeof command.tagged !== 'boolean') throw new Error('Invalid chat tag.');
        if (command.tagged === state.tags.includes(tag)) return;
        patch = { tags: command.tagged ? [...state.tags, tag] : state.tags.filter(value => value !== tag) };
        break;
      }
      default: throw new Error('Invalid chat action.');
    }
    this.updateThreadOrganization(thread, patch);
  }

  private async dispatch(command: Record<string, any>) {
    const type = String(command.type ?? '');
    const directId = typeof command.threadId === 'string' ? command.threadId : '';
    if ((type === 'thread.stop' || type === 'run.interrupt') && directId) {
      const existing = this.threads.get(directId);
      if (existing) await this.cancel(existing); else await this.engine.request('session/cancel', { sessionId: directId });
      this.flush(); return { sequence: this.sequence };
    }
    if (type === 'runtime-request.respond' && directId) {
      await this.respondImmediately(directId, command);
      this.flush(); return { sequence: this.sequence };
    }
    await this.start();
    // Window-owned list actions do not require opening the engine history. They remain usable
    // while a cold conversation's history or access lookup is slow or unavailable.
    const listAction = THREAD_ORGANIZATION_COMMANDS.has(type)
      || ['thread.metadata.update', 'thread.pin', 'thread.unpin', 'thread.pin.reorder', 'thread.archive', 'thread.unarchive', 'thread.delete'].includes(type);
    try {
      const thread = typeof command.threadId === 'string'
        ? listAction ? this.threads.get(command.threadId) : await this.loaded(command.threadId)
        : undefined;
      const needThread = () => { if (!thread) throw dispatchError(command, 'This chat does not exist.'); return thread; };
      switch (type) {
        case 'thread.fork': await this.startSideChat(command); break;
        case 'thread.create':
          if (thread && typeof command.projectId === 'string' && command.projectId !== thread.projectId) throw new Error('A conversation cannot change its project folder. Start a new chat.');
          if (!thread) await this.createThread(command.threadId, command.title, command.modelSelection?.model, command.runtimeMode, command.projectId, command.modelSelection?.instanceId, command.interactionMode);
          break;
        case 'message.dispatch': {
          const target = needThread();
          const interaction = command.interactionMode === undefined ? undefined : interactionModeOf(command.interactionMode);
          requireInteraction(target.executor, interaction);
          if (command.modelSelection?.instanceId && executorOf(command.modelSelection.instanceId) !== target.executor) throw dispatchError(command, 'A conversation cannot change its executor. Start a new chat.');
          if (!target.builtin && command.modelSelection?.model) { await this.applyModel(target.executor, command.modelSelection.model); target.model = command.modelSelection.model; }
          this.setLevel(target, command.modelSelection?.options);
          await this.withSelectionMutation(async () => {
            await this.applyMode(target, command.runtimeMode);
            await this.send(target, String(command.messageId), String(command.text ?? ''), command.attachments, interaction, command.context);
          });
          break;
        }
        case 'thread.stop': case 'run.interrupt': await this.cancel(needThread()); break;
        case 'runtime-request.respond': await this.respond(needThread(), command); break;
        case 'thread.user-input.dismiss': {
          const target = needThread(); const requestId = String(command.requestId ?? '');
          if (target.pendingRequest(requestId) && this.engine.answerQuestion) {
            await this.engine.answerQuestion(requestId, { cancelled: true });
            this.questions.delete(requestId);
            target.questionClosed(requestId, null, this.now());
            this.changed(target.id);
          }
          break;
        }
        case 'thread.runtime-mode.set': { const target = needThread(); await this.withSelectionMutation(() => this.applyMode(target, command.runtimeMode)); break; }
        case 'thread.interaction-mode.set': {
          const target = needThread();
          const interaction = interactionModeOf(command.interactionMode);
          requireInteraction(target.executor, interaction);
          target.setInteractionMode(interaction);
          this.interactions.set(target.id, interaction); this.save(); this.changed(target.id);
          break;
        }
        case 'thread.model-selection.set': {
          const target = needThread();
          if (command.modelSelection?.instanceId && executorOf(command.modelSelection.instanceId) !== target.executor) throw dispatchError(command, 'A conversation cannot change its executor. Start a new chat.');
          await this.applyModel(target.executor, command.modelSelection?.model);
          if (command.modelSelection?.model) target.model = command.modelSelection.model;
          // The reasoning level of T3's model menu: kept for the chat, shown by the chip, sent with each message.
          if (command.modelSelection) this.setLevel(target, command.modelSelection.options, true);
          this.changed(target.id);
          break;
        }
        case 'thread.metadata.update': {
          const target = needThread();
          if (typeof command.title === 'string' && command.title.trim()) {
            const title = command.title.trim();
            this.threadMetadata.update(target.id, { title });
            this.titles.set(target.id, title);
            this.titleRequests.delete(target.id);
            target.titleRegeneration = null;
            target.title = title;
            this.changed(target.id);
          } else if (command.regenerateTitle === true) {
            this.armTitleRegeneration(target, String(command.commandId ?? ''));
          } else if (command.regenerateTitle === false && target.titleRegeneration) {
            this.titleRequests.delete(target.id);
            target.titleRegeneration = null;
            this.changed(target.id);
          }
          break;
        }
        case 'thread.pin': case 'thread.unpin': case 'thread.pin.reorder': {
          const target = needThread();
          if (target.archivedAt !== null) throw new Error('This chat is archived.');
          const orderKey = typeof command.orderKey === 'string' ? command.orderKey.trim() : '';
          if (type === 'thread.pin.reorder' && (target.pinnedAt === null || !orderKey)) {
            throw new Error('Only pinned chats can be reordered here.');
          }
          const now = this.now();
          const patch: ThreadMetadata = {
            pinnedAt: type === 'thread.unpin' ? null : target.pinnedAt ?? now,
            pinOrderKey: type === 'thread.unpin' ? null : orderKey || target.pinOrderKey,
          };
          if (type === 'thread.pin' && target.organization.settledOverride === 'settled') {
            Object.assign(patch, { settledOverride: 'active', settledAt: null, unsettledAt: now });
          }
          if (type === 'thread.pin' && target.organization.snoozedUntil !== null) {
            Object.assign(patch, { snoozedUntil: null, snoozedAt: null, lastSnoozeWakeAt: now });
          }
          this.updateThreadOrganization(target, patch);
          break;
        }
        case 'thread.delete': {
          const target = needThread();
          if (command.onlyIfUnused === true) {
            const loaded = target.loaded ? target : await this.loaded(target.id);
            if (!loaded || loaded.userMessageCount() > 0 || loaded.running || loaded.activeRunId()) break;
          }
          if (this.deletingThreads.has(target.id) || this.deletedThreads.has(target.id)) throw dispatchError(command, 'This chat is being deleted.');
          this.deletingThreads.add(target.id);
          try { await this.engine.request('session/delete', { sessionId: target.id }); }
          catch (error) { this.deletingThreads.delete(target.id); this.resumedSessions.delete(target.id); throw error; }
          this.deletedThreads.add(target.id);
          this.deletingThreads.delete(target.id);
          this.threads.delete(target.id);
          this.secretaryContexts.delete(target.id);
          this.titleRequests.delete(target.id);
          this.wakeLocalImageReaders();
          this.dirty.delete(target.id);
          this.streamChanges.delete(target.id);
          // Preserve window-owned project, title and preferences for manual recovery from trash.
          this.homePrompts.delete(target.id); this.failedHomePrompts.delete(target.id);
          this.resumedSessions.delete(target.id); this.restored.delete(target.id);
          this.save();
          this.sequence++;
          for (const sink of this.shellSinks) sink.emit({ kind: 'thread.removed', sequence: this.sequence, location: target.archivedAt === null ? 'active' : 'archive', threadId: target.id });
          this.homeChanged();
          break;
        }
        case 'thread.archive': case 'thread.unarchive': {
          const target = needThread();
          const archivedAt = type === 'thread.archive' ? this.now() : null;
          this.threadMetadata.update(target.id, { archivedAt });
          target.archivedAt = archivedAt;
          if (archivedAt !== null) {
            this.titleRequests.delete(target.id);
            target.titleRegeneration = null;
          }
          if (archivedAt === null && !target.homeProjectStats) this.schedulePeekToday();
          this.changed(target.id);
          break;
        }
        default:
          if (!THREAD_ORGANIZATION_COMMANDS.has(type)) throw dispatchError(command, 'Not available in HoH.');
          await this.organizeThread(needThread(), command);
      }
    } catch (error) {
      if (error instanceof RpcFailure) throw error;
      throw dispatchError(command, errorText(error));
    }
    this.flush();
    return { sequence: this.sequence };
  }

  private async launch(input: Record<string, any>) {
    // Validate the restrictive interaction choice before startup/loading can read or mutate the
    // engine. In particular, an invalid mode must not widen global access on an existing thread.
    const interactionMode = input.interactionMode === undefined ? undefined : interactionModeOf(input.interactionMode);
    await this.start();
    const threadId = typeof input.threadId === 'string' && input.threadId ? input.threadId : 'thread-' + (globalThis.crypto?.randomUUID?.() ?? this.now().toString(36));
    const existing = await this.loaded(threadId);
    try {
      const requestedExecutor = executorOf(input.modelSelection?.instanceId ?? instanceOf(this.selectedExecutor));
      requireInteraction(requestedExecutor, interactionMode);
      if (existing && requestedExecutor !== existing.executor) throw new Error('A conversation cannot change its executor. Start a new chat.');
      const launchProject = existing ? this.validateLaunchProject(existing, input.projectId) : null;
      const thread = existing ?? await this.createThread(threadId, input.title, input.modelSelection?.model, input.runtimeMode, input.projectId, input.modelSelection?.instanceId, interactionMode);
      if (existing) await this.selectLaunchProject(existing, launchProject);
      if (existing && interactionMode !== undefined) { thread.setInteractionMode(interactionMode); this.interactions.set(thread.id, interactionMode); this.save(); }
      const message = input.initialMessage;
      if (message && typeof message.text === 'string') {
        const messageId = typeof message.messageId === 'string' && message.messageId ? message.messageId : 'msg-' + String(input.commandId);
        this.setLevel(thread, input.modelSelection?.options);
        await this.withSelectionMutation(async () => {
          if (existing) await this.applyMode(existing, input.runtimeMode);
          await this.send(thread, messageId, message.text, message.attachments, interactionMode, message.context);
        });
      } else if (existing) {
        await this.withSelectionMutation(() => this.applyMode(existing, input.runtimeMode));
      }
      this.flush();
      return { threadId, projection: thread.projection(), resumed: !!existing, sequence: this.sequence };
    } catch (error) {
      throw new RpcFailure({ _tag: 'OrchestrationV2ThreadLaunchError', commandId: String(input.commandId ?? 'unknown'), projectId: String(input.projectId ?? HOH_PROJECT_ID), message: errorText(error) });
    }
  }

  // ---- the methods T3 calls ----

  private projectFileRoots(): readonly string[] {
    const roots = new Set([...this.folders.values()].map(project => project.workspaceRoot));
    if (this.workspaceKnown) roots.add(this.workspaceRoot);
    for (const thread of this.threads.values()) if (thread.cwd) roots.add(folderPath(thread.cwd));
    return [...roots];
  }

  handlers(): Handlers {
    return {
      unary: {
        ...projectFileHandlers(this.engine, () => this.start(), () => this.projectFileRoots()),
        'server.probe': () => ({}),
        // Focus and typing reports of the window: nothing to schedule here, accepted and dropped.
        'server.reportClientActivity': () => undefined,
        'server.getConfig': async () => { await this.start(); return this.config(); },
        'server.refreshUsageRates': () => usagePricing,
        'server.getUsageSummary': async (input: UsageSummaryInput) => {
          await this.start();
          await this.peekInFlight;
          return usageSummary(input, [...this.threads.values()].filter(thread => this.accessVisible(thread.id)), this.peeked);
        },
        'server.getSettings': async () => { await this.start(); return this.settings(); },
        'server.upsertKeybinding': payload => this.editKeybinding(payload, 'upsertKeybinding'),
        'server.removeKeybinding': payload => this.editKeybinding(payload, 'removeKeybinding'),
        'server.updateSettings': async (payload: any) => {
          await this.start();
          const patch = payload?.patch && typeof payload.patch === 'object' ? { ...payload.patch } : {};
          let windowPreferencesChanged = false;
          try {
            windowPreferencesChanged = this.localSettings?.updateWindowPreferences(patch, payload?.providerInstanceMutation) ?? false;
          } catch {
            throw new RpcFailure({
              _tag: 'ServerSettingsError',
              settingsPath: 'HoH window preferences',
              operation: 'write-file',
            });
          }
          // The default mode is where new threads start; the model is the engine's choice for new chats.
          if (typeof patch.defaultRuntimeMode === 'string') { this.defaultMode = accessModeOf(patch.defaultRuntimeMode); this.save(); }
          if (patch.defaultModelSelection?.model) await this.applyModel(executorOf(patch.defaultModelSelection.instanceId), patch.defaultModelSelection.model);
          delete patch.defaultRuntimeMode; delete patch.defaultModelSelection;
          // Only whole values are kept: a nested patch is partial and would not decode as a setting.
          const primitive = (value: unknown) => value === null || ['string', 'number', 'boolean'].includes(typeof value);
          for (const [key, value] of Object.entries(patch)) {
            if (primitive(value) || (Array.isArray(value) && value.every(primitive))) this.settingsOverrides = { ...this.settingsOverrides, [key]: value };
          }
          if (windowPreferencesChanged) this.configChanged();
          else for (const sink of this.configSinks) sink.emit({ version: 1, type: 'settingsUpdated', payload: { settings: this.settings() } });
          return this.settings();
        },
        // T3's "Start without a project": the chats' folder is that project.
        'projects.ensureScratch': async () => {
          await this.start();
          this.showProject();
          return { projectId: HOH_PROJECT_ID };
        },
        'projects.mutate': (mutation: any) => this.mutateProject(mutation ?? {}),
        'projects.searchEntries': (payload: any) => this.searchEntries(payload ?? {}),
        'assets.persistChatAttachments': (payload: any) => this.persistAttachments(payload ?? {}),
        'secrets.answerRequest': (payload: any) => this.respondToSecret(payload ?? {}),
        'attachments.createUploadUrl': (payload: any) => this.createUploadUrl(payload ?? {}),
        // T3 checks a picture it uploaded before it sends it: held here — an address; gone — T3 uploads it again.
        'assets.createUrl': async (payload: any) => {
          const resource = payload?.resource;
          if (resource?._tag === 'attachment') {
            const id = String(resource.attachmentId ?? '');
            const image = this.uploads.get(id);
            const relativeUrl = this.previewUrls.get(id) ?? (image ? this.previewUrl(image) : undefined);
            if (!relativeUrl || !UPLOAD_PATH.test(`/api/attachments/upload/${id}`)) throw new RpcFailure({ _tag: 'AssetAttachmentNotFoundError', resource });
            return { relativeUrl, expiresAt: this.now() + UPLOAD_TTL_MS };
          }
          if (resource?._tag !== 'media-file') throw new RpcFailure({ _tag: 'EnvironmentAuthorizationError', message: 'Not available in HoH.', requiredScope: 'orchestration:read' });
          const threadId = typeof resource.threadId === 'string' ? resource.threadId : '';
          const path = typeof resource.path === 'string' ? resource.path : '';
          // A relative Markdown image is joined by T3 before the optimistic draft owns its
          // authoritative cwd. Remember that root before awaiting createThread, then rebase
          // only hostOnly (originally relative) paths after the folder becomes known.
          const requestedWorkspaceRoot = this.workspaceRoot;
          // Snapshot the live RPC order before start()/loaded() can yield to the later
          // subscribe/launch calls. A restored conversation has a durable home and is
          // never treated as an unregistered draft merely because startup is still slow.
          const admittedBeforeRegistration = !this.threads.has(threadId)
            && !this.homes.has(threadId)
            && !this.threadSinks.has(threadId)
            && !this.creatingThreads.has(threadId);
          if (!threadId || !path) throw new RpcFailure({ _tag: 'AssetWorkspaceContextNotFoundError', resource });
          if (path.length > 4096) throw new RpcFailure({
            _tag: 'AssetWorkspaceAssetNotFoundError', resource,
            detail: 'Image unavailable: the image path is too long.',
          });
          let context: LocalImageThreadContext | undefined;
          try { context = await this.localImageThreadContextForPath(threadId, path, admittedBeforeRegistration); }
          catch (error) {
            if (error instanceof LocalImageContextCapacityError) throw new RpcFailure({
              _tag: 'AssetWorkspaceAssetNotFoundError', resource, detail: error.message,
            });
            context = undefined;
          }
          if (!context) throw new RpcFailure({ _tag: 'AssetWorkspaceContextNotFoundError', resource });
          const { thread } = context;
          try {
            const imagePath = context.admittedBeforeRegistration && resource.hostOnly === true
              ? localImagePathAtRoot(path, requestedWorkspaceRoot, thread.cwd ?? '')
              : path;
            const image = await this.localImageUrl(thread.id, imagePath, thread.cwd);
            return {
              relativeUrl: image.url,
              expiresAt: this.now() + UPLOAD_TTL_MS,
              ...(image.sourcePath.length <= 1024 ? { sourcePath: image.sourcePath } : {}),
              fileRevision: image.revision,
            };
          } catch (error) {
            throw new RpcFailure({
              _tag: 'AssetWorkspaceAssetNotFoundError', resource,
              detail: englishError(error, 'Image unavailable: the file could not be read safely.').slice(0, 512),
            });
          }
        },
        'attachments.delete': (payload: any) => {
          const id = String(payload?.attachmentId ?? '');
          this.slots.delete(id); this.uploads.delete(id);
          // Removing an unsent draft frees its URL. A sent message owns its preview independently:
          // this same RPC is T3's normal post-send upload cleanup.
          if (!this.sentPreviewIds.has(id)) {
            const url = this.previewUrls.get(id);
            if (url) URL.revokeObjectURL(url);
            this.previewUrls.delete(id);
          }
          return null;
        },
        'filesystem.browse': (payload: any) => this.browse(payload ?? {}),
        // The welcome page looks for other agents' history to import; HoH imports none, and T3 then
        // leaves its Projects section out.
        'agentSessions.scan': () => ({ candidates: [], scannedAt: this.iso() }),
        'server.refreshProviders': async (payload: any) => {
          await this.start();
          const preferences = () => this.localSettings?.windowPreferences();
          const claude = () => presentProvider(
            claudeProvider(this.claudeDiscovery, this.iso(), this.mode !== null, this.discoveryFailure),
            preferences(),
          );
          const codex = () => presentProvider(
            codexProvider(this.codexDiscovery, this.iso(), this.codexDiscoveryFailure),
            preferences(),
          );
          const pi = () => presentProvider(piProvider(this.piDiscovery, this.iso(), this.mode !== null), preferences());
          // The composer's folder (T3's workspace snapshot): `/` and `$` from its selected executor.
          const cwd = typeof payload?.cwd === 'string' ? payload.cwd.trim() : '';
          if (cwd && payload?.instanceId === CODEX_INSTANCE_ID) {
            if (this.codexDiscovery === null) await this.readDiscovery();
            const workspaceSnapshot = await this.workspaceSnapshot(cwd, 'codex');
            return { providers: [{ ...codex(), workspaceSnapshots: [workspaceSnapshot] }] };
          }
          if (cwd && payload?.instanceId === CLAUDE_INSTANCE_ID) {
            if (this.claudeDiscovery === null) await this.readDiscovery();
            return { providers: [{ ...claude(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'claude')] }] };
          }
          if (cwd && payload?.instanceId === PI_INSTANCE_ID) {
            if (this.piDiscovery === null) await this.readPiDiscovery();
            return { providers: [{ ...pi(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'pi')] }] };
          }
          if (cwd && payload?.instanceId === undefined) {
            if (this.claudeDiscovery === null || this.codexDiscovery === null) await this.readDiscovery();
            return { providers: [
              { ...claude(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'claude')] },
              { ...codex(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'codex')] },
              { ...pi(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'pi')] },
            ] };
          }
          if (payload?.instanceId === PI_INSTANCE_ID) {
            await this.readPiDiscovery();
            return { providers: [pi()] };
          }
          // Welcome mounts with an automatic empty refresh. While startup is pending it is an
          // observer of that same bounded lane, not a user's request to replace it or fail early.
          const targeted = payload?.instanceId !== undefined || payload?.refreshModels === true || payload?.reloadSecrets === true;
          if (!targeted && this.discoveryPending) {
            const generation = this.discoveryGeneration;
            await this.readDiscovery(generation);
            this.configChanged();
            return { providers: [
              claude(),
              codex(),
              pi(),
            ] };
          }
          const generation = ++this.discoveryGeneration;
          const startedAt = this.now();
          this.discoveryPending = true;
          this.discoveryFailure = undefined;
          this.codexDiscoveryFailure = undefined;
          void this.readPiDiscovery(generation);
          if (!await this.readDiscovery(generation) && generation === this.discoveryGeneration) this.finishDiscoveryFailure(startedAt);
          this.configChanged();
          return { providers: [
            claude(),
            codex(),
            pi(),
          ] };
        },
        'orchestration.dispatchCommand': (command: any) => this.dispatch(command ?? {}),
        'orchestration.launchThread': (input: any) => this.launch(input ?? {}),
        'orchestration.searchThread': (input: FindInput & { threadId: string }) => this.findInChat(input),
        'orchestration.searchThreads': (input: any) => this.searchChats(input ?? {}),
        'orchestration.getThreadFamily': async ({ threadId }) => {
          await this.start();
          const target = this.threads.get(threadId);
          const root = target?.sideChat?.parentSessionId ?? target?.id;
          return {
            snapshotSequence: this.sequence,
            threads: root ? [...this.threads.values()]
              .filter(thread => thread.archivedAt === null && (thread.id === root || thread.sideChat?.parentSessionId === root))
              .map(thread => thread.shell()) : [],
          };
        },
        'orchestration.getArchivedShellSnapshot': async () => {
          await this.start();
          const { snapshot } = this.shellSnapshot();
          return { schemaVersion: snapshot.schemaVersion, snapshotSequence: snapshot.snapshotSequence, projects: snapshot.projects, threads: snapshot.archivedThreads };
        },
        'orchestration.getThreadProjection': async (payload: any) => {
          await this.start();
          const thread = await this.loaded(String(payload?.threadId ?? ''));
          if (!thread) throw new RpcFailure({ _tag: 'OrchestrationV2GetThreadProjectionError', threadId: String(payload?.threadId ?? 'unknown'), message: 'This chat does not exist.' });
          return thread.projection();
        },
        'orchestration.getTurnItem': async (payload: { threadId: string; itemId: string }) => {
          const threadId = String(payload?.threadId ?? '');
          try {
            await this.start();
            const thread = await this.loaded(threadId);
            const item = thread && this.threads.get(threadId) === thread && !this.deletingThreads.has(threadId)
              ? thread.turnItemDetail(String(payload?.itemId ?? '')) : null;
            return { item };
          } catch (error) {
            throw new RpcFailure({ _tag: 'OrchestrationV2GetThreadProjectionError', threadId, message: errorText(error) });
          }
        },
        'orchestration.forceStopThread': async (payload: any) => {
          const id = String(payload?.threadId ?? '');
          const thread = this.threads.get(id);
          if (thread) await this.cancel(thread); else await this.engine.request('session/cancel', { sessionId: id });
          return { outcome: 'killed', sharedWithThreadIds: [] };
        },
        'vcs.refreshStatus': payload => this.git.refresh(payload),
        'vcs.refreshBranch': async payload => { await this.git.refresh(payload); },
        'vcs.listRefs': payload => this.git.listRefs(payload),
        'review.getDiffPreview': payload => this.git.diffPreview(payload),
        'review.getBranchCommits': payload => this.git.branchCommits(payload),
        'review.getDiffFileContents': payload => this.git.fileContents(payload),
        'orchestration.getThreadTitles': async (payload: any) => {
          await this.start();
          const titles: Record<string, string> = {};
          for (const id of Array.isArray(payload?.threadIds) ? payload.threadIds : []) {
            const thread = this.threads.get(id);
            if (thread) titles[id] = thread.shell().title;
          }
          return titles;
        },
      },
      stream: {
        'orchestration.searchThreadStream': (input: FindInput & { threadId: string }, sink) => {
          const controller = new AbortController();
          void this.findInChat(input, result => sink.emit(result), controller.signal).then(result => {
            if (controller.signal.aborted) return;
            sink.emit(result);
            sink.end();
          }, error => { if (!controller.signal.aborted) sink.fail(error); });
          return () => controller.abort();
        },
        subscribeServerConfig: (_payload, sink) => {
          this.configSinks.add(sink);
          void this.start().then(() => { if (this.configSinks.has(sink)) sink.emit({ version: 1, type: 'snapshot', config: this.config() }); });
          return () => { this.configSinks.delete(sink); };
        },
        // T3 shows nothing until the server welcomes it (apps/web/src/components/onboarding/FirstRunGate.tsx).
        // No bootstrap project or thread: T3 decides on its welcome page from the list alone (no project and
        // no chat is a fresh workspace, apps/web/src/onboarding/firstRun.logic.ts) and its own saved
        // `onboardingCompletedAt`.
        subscribeServerLifecycle: (_payload, sink) => {
          let open = true;
          void this.start().then(() => {
            if (!open) return;
            sink.emit(this.lifecycleWelcome(this.sessionsKnown ? 'complete' : 'pending'));
            this.lifecycleSinks.add(sink);
          });
          return () => { open = false; this.lifecycleSinks.delete(sink); };
        },
        subscribeVcsStatus: (payload, sink) => this.git.subscribe(payload, sink),
        subscribeVcsBranch: (payload, sink) => this.git.subscribe(payload, sink, true),
        'orchestration.subscribeShell': (payload: any, sink) => {
          let open = true;
          void this.start().then(() => {
            if (!open) return;
            sink.emit(this.shellSnapshot());
            this.shellSinks.add(sink);
            if (payload?.requestCompletionMarker) {
              if (this.sessionsKnown) sink.emit({ kind: 'synchronized' });
              else this.shellCompletionSinks.add(sink);
            }
          });
          return () => { open = false; this.shellSinks.delete(sink); this.shellCompletionSinks.delete(sink); };
        },
        'orchestration.subscribeThread': (payload: any, sink) => {
          const threadId = String(payload?.threadId ?? '');
          let open = true;
          let sinks = this.threadSinks.get(threadId);
          if (!sinks) this.threadSinks.set(threadId, sinks = new Set());
          this.registerLocalImageContext(threadId);
          const own = sinks;
          own.add(sink);
          if (payload?.acceptBoundedSnapshot) this.boundedSinks.add(sink);
          void this.start().then(() => {
            const provisional = this.threads.get(threadId);
            if (!open) return undefined;
            if (provisional && !provisional.loaded && !provisional.bindingKnown) {
              this.sinkSequences.set(sink, 0);
              sink.emit(this.threadSnapshot(provisional));
            }
            return this.loaded(threadId);
          }).then(thread => {
            if (!open) return;
            // A thread that does not exist yet (a draft) gets its first snapshot when it is created.
            if (thread) {
              const sequence = this.projectionSequence(thread);
              const canResume = sequence !== 0 && payload?.requestCompletionMarker && payload.afterSequence === sequence;
              if (!canResume && this.sinkSequences.get(sink) !== sequence) {
                sink.emit(this.threadSnapshot(thread, sequence, this.boundedSinks.has(sink)));
              }
              if (payload?.requestCompletionMarker) sink.emit({ kind: 'synchronized' });
              this.sinkSequences.set(sink, sequence);
            } else {
              this.sinkSequences.set(sink, 0);
              const created = this.threads.get(threadId);
              if (created?.loaded) this.publishThread(created);
            }
          }, error => {
            if (!open) return;
            own.delete(sink);
            if (!own.size && this.threadSinks.get(threadId) === own) this.threadSinks.delete(threadId);
            sink.fail(new RpcFailure({ _tag: 'OrchestrationV2GetThreadProjectionError', threadId, message: errorText(error) }));
          });
          return () => {
            open = false;
            own.delete(sink);
            if (own.size || this.threadSinks.get(threadId) !== own) return;
            this.threadSinks.delete(threadId);
            const thread = this.threads.get(threadId);
            if (!thread?.cwd) this.settleLocalImageContext(threadId, undefined);
            if (thread) return;
            this.failedLocalImageContexts.delete(threadId);
          };
        },
      },
    };
  }

  /** For tests and diagnostics. */
  thread(id: string) { return this.threads.get(id); }
  get projectId() { return HOH_PROJECT_ID; }
}
