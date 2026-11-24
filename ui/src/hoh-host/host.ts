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
import {
  accessModeOf, CLAUDE_INSTANCE_ID, CODEX_INSTANCE_ID, claudeProvider, codexEffortOf, codexProvider, defaultClaudeModel, defaultCodexModel, effortOf, listedClaudeModel, listedCodexModel, runtimeModeOf, serverConfig, serverSettings, type ClaudeDiscovery, type CodexDiscovery, type HohAccessMode, type HohExecutor, type T3RuntimeMode,
  isAccessMode, type T3InteractionMode,
} from './config.ts';
import type { EngineNotification, EngineQuestion, EngineQuestionAnswer, HohEngine, LocalImageAsset } from './engine.ts';
import { base64Of, readUploads, searchAnswer, skillsAsCommands, stripAttachedImageReferences, UPLOAD_PATH, UPLOAD_TTL_MS, uploadSlot, workspaceSnapshot, type KeptImage, type UploadSlot } from './composer.ts';
import { englishError } from './errors.ts';
import { folderProject, HOH_PROJECT_ID, hohProject, ThreadModel, type FolderProject, type HomeEntry } from './threads.ts';
import { RpcFailure, type Handlers, type StreamSink } from './wire.ts';

export interface HohHostOptions {
  readonly engine: HohEngine;
  readonly now?: () => number;
  /** Folder shown as the project root until the engine names one. */
  readonly workspaceRoot?: string;
  /** Delay that groups a burst of changes (stream chunks) into one snapshot. */
  readonly throttleMs?: number;
  /** Where folder projects are kept between runs (the window's localStorage by default; null: not kept). */
  readonly storage?: HohStorage | null;
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
const LOCAL_IMAGE_MAX_INFLIGHT = 6;
const LOCAL_IMAGE_THREAD_MAX_INFLIGHT = 3;
const LOCAL_IMAGE_CONTEXT_GRACE_MS = 2_000;
const LOCAL_IMAGE_ATTEMPT_MAX = 64;
const LOCAL_IMAGE_GLOBAL_ATTEMPT_MAX = 512;
const LOCAL_IMAGE_ATTEMPT_WINDOW_MS = 10 * 60_000;
const LOCAL_IMAGE_ATTEMPT_THREADS_MAX = LOCAL_IMAGE_GLOBAL_ATTEMPT_MAX;
const LOCAL_IMAGE_THREAD_MAX_IDENTITIES = 256;

type LocalImageUrl = {
  url: string; revision: string; sourcePath: string; sizeBytes: number; threadId: string;
  decodeProfile: LocalImageDecodeProfile;
  requestKeys: Set<string>;
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
  const trimmed = value.trim();
  return trimmed.length > 1 ? trimmed.replace(/\/+$/, '') || '/' : trimmed;
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
const executorOf = (instanceId: unknown): HohExecutor => instanceId === CODEX_INSTANCE_ID ? 'codex' : 'claude';
const instanceOf = (executor: HohExecutor) => executor === 'codex' ? CODEX_INSTANCE_ID : CLAUDE_INSTANCE_ID;
const exactMode = (value: unknown): HohAccessMode | null => ['confirm', 'auto-accept-edits', 'auto', 'full'].includes(String(value)) ? value as HohAccessMode : null;

const PAGE = 1000;
const NO_OP_COMMANDS = new Set([
  'thread.visit', 'thread.mark-unread', 'thread.settle', 'thread.unsettle', 'thread.auto-settle', 'thread.auto-settle.set',
  'thread.pin', 'thread.unpin', 'thread.pin.reorder', 'thread.active.reorder', 'thread.tag.set', 'thread.snooze', 'thread.unsnooze',
]);

/** The chat folder is not a git repository for T3: no branch, no worktree, nothing to commit. */
const NOT_A_REPO_LOCAL = {
  isRepo: false, hasPrimaryRemote: false, isDefaultRef: false, refName: null, hasWorkingTreeChanges: false,
  workingTree: { files: [], insertions: 0, deletions: 0 },
};

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
  private readonly now: () => number;
  private readonly throttleMs: number;
  private readonly loadRetryMs: number;
  private readonly loadRetries: number;
  private readonly localImageContextGraceMs: number;
  private stopped = false;
  private readonly threads = new Map<string, ThreadModel>();
  /** Rows restored before the engine answers; its list/history replaces their cached metadata once available. */
  private readonly restored = new Set<string>();
  /** Histories already peeked successfully during startup; a cached row is retried until its log is readable. */
  private readonly peeked = new Set<string>();
  /** Last-known rows wait here until `access/get` supplies their real runtime mode. */
  private readonly savedSessions: any[] = [];
  private readonly loading = new Map<string, Promise<ThreadModel | undefined>>();
  private readonly questions = new Map<string, EngineQuestion>();
  private readonly shellSinks = new Set<StreamSink>();
  private readonly shellCompletionSinks = new Set<StreamSink>();
  private readonly lifecycleSinks = new Set<StreamSink>();
  private readonly threadSinks = new Map<string, Set<StreamSink>>();
  private readonly configSinks = new Set<StreamSink>();
  private readonly dirty = new Set<string>();
  private flushTimer: ReturnType<typeof setTimeout> | undefined;
  private sequence = 0;
  private homeDirty = false;
  private homeVersion = 0;
  private sessionsKnown = false;
  private readonly homeListeners = new Set<() => void>();
  private claudeDiscovery: ClaudeDiscovery | null = null;
  private codexDiscovery: CodexDiscovery | null = null;
  /** Undefined while the startup probe is pending; set only when its bounded retry lane ends. */
  private discoveryFailure: unknown | undefined;
  private codexDiscoveryFailure: unknown | undefined;
  private discoveryPending = true;
  private discoveryStartedAt: number;
  /** A person's Refresh supersedes the startup lane; incidental workspace reads share its in-flight probe. */
  private discoveryGeneration = 0;
  private discoveryInFlight: { generation: number; promise: Promise<boolean> } | undefined;
  /** Null until the engine has answered `access/get`; never aliases the fail-closed state to confirm. */
  private mode: HohAccessMode | null = null;
  private accessError: string | null = 'Executor access mode has not been read yet.';
  /** T3's "default permissions" setting: the mode a new thread starts in. Claude asks first until the person changes it. */
  private defaultMode: HohAccessMode = 'confirm';
  private selectedExecutor: HohExecutor = 'claude';
  /** The model the engine runs new chats with (`hoh/executor/select`), once known. */
  private selectedModel: string | null = null;
  /** A startup access/get captured before a person's later choice must not overwrite that particular field. */
  private modeRevision = 0;
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
  /** Engine writes are ordered through publication; response reordering cannot split UI and engine state. */
  private accessWriteTail: Promise<void> = Promise.resolve();
  private settingsOverrides: Record<string, unknown> = {};
  private workspaceRoot: string;
  private workspaceKnown = false;
  /** Whether the "No project" project is in the list (see the head of this file). */
  private projectShown = false;
  private readonly storage: HohStorage | null;
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
  private localImageBytes = 0;
  /** Optimistic T3 drafts ask for their first image before createThread has installed the cwd. */
  private readonly localImageContextWaiters = new Map<string, Set<LocalImageContextWaiter>>();
  /** Context lookup is deduplicated by raw request and bounded before it can allocate a draft waiter. */
  private readonly localImageContextInflight = new Map<string, Promise<LocalImageThreadContext | undefined>>();
  /** A failed create is terminal until its draft subscription closes or creation is retried. */
  private readonly failedLocalImageContexts = new Set<string>();
  private readonly creatingThreads = new Set<string>();
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

  constructor(options: HohHostOptions) {
    this.engine = options.engine;
    this.now = options.now ?? Date.now;
    this.throttleMs = options.throttleMs ?? 40;
    this.loadRetryMs = options.loadRetryMs ?? 500;
    this.loadRetries = options.loadRetries ?? 60;
    this.discoveryStartedAt = this.now();
    this.localImageContextGraceMs = options.localImageContextGraceMs ?? LOCAL_IMAGE_CONTEXT_GRACE_MS;
    this.workspaceRoot = options.workspaceRoot ?? PLACEHOLDER_ROOT;
    this.startedAt = this.now();
    this.storage = options.storage === undefined ? windowStorage() : options.storage;
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
      if (Number.isInteger(turnOrdinal) && turnOrdinal > 0 && (entry?.state === 'waiting' || entry?.state === 'done'))
        this.homeDispositions.set(threadId, {
          turnOrdinal, state: entry.state,
          ...(entry.interrupted === true ? { interrupted: true } : {}),
          ...(entry.stopped === true ? { stopped: true } : {}),
        });
    }
    for (const [threadId, value] of Object.entries(saved?.homeObservedAt && typeof saved.homeObservedAt === 'object' ? saved.homeObservedAt : {})) {
      const observedAt = Number(value);
      if (Number.isFinite(observedAt) && observedAt > 0) this.homeObservedAt.set(threadId, observedAt);
    }
    if (folderPath(saved?.defaultFolder)) this.workspaceRoot = folderPath(saved.defaultFolder);
    if (isAccessMode(saved?.defaultMode)) this.defaultMode = saved.defaultMode;
    this.savedSessions.push(...(Array.isArray(saved?.sessions) ? saved.sessions : []));
  }

  /** Restore list metadata only after the engine has supplied the mode the row must display. */
  private materializeSavedSessions() {
    const mode = this.mode;
    if (mode === null) return;
    // Last-known-good list metadata makes the first post-restart snapshot lossless even when the
    // engine's first disk scan is temporarily incomplete. History still comes only from the engine.
    for (const entry of this.savedSessions.splice(0)) {
      const id = typeof entry?.id === 'string' ? entry.id.trim() : '';
      const title = typeof entry?.title === 'string' ? entry.title.trim() : '';
      const createdAt = timeOf(entry?.createdAt);
      const updatedAt = timeOf(entry?.updatedAt);
      if (!id || !title || createdAt === null || updatedAt === null || this.threads.has(id)) continue;
      const projectId = entry?.projectId === HOH_PROJECT_ID || (typeof entry?.projectId === 'string' && this.folders.has(entry.projectId))
        ? entry.projectId : this.homeOf(id);
      const executor: HohExecutor = entry?.executor === 'codex' ? 'codex' : 'claude';
      const model = typeof entry?.model === 'string' && entry.model.trim() ? entry.model.trim() : this.model(executor);
      const restoredMode = exactMode(entry?.mode) ?? mode;
      const thread = new ThreadModel({
        id,
        title: this.titles.get(id) ?? title,
        createdAt,
        updatedAt,
        model,
        mode: restoredMode,
        executor,
        interactionMode: this.interactions.get(id),
        projectId,
      });
      thread.options = this.levels.get(id);
      thread.archivedAt = entry?.archivedAt === null ? null : timeOf(entry?.archivedAt);
      this.threads.set(id, thread);
      this.restored.add(id);
    }
    if ([...this.threads.values()].some(thread => thread.projectId === HOH_PROJECT_ID)) this.projectShown = true;
  }

  private save() {
    if (!this.storage) return;
    const sessions = [...this.threads.values()].flatMap(thread => {
      const shell = thread.shell();
      // A local empty draft has not reached the engine and must not become a ghost after restart.
      if (thread.loaded && shell.latestUserMessageAt === null) return [];
      return [{
        id: thread.id, title: shell.title, projectId: thread.projectId, executor: thread.executor, model: thread.model, mode: thread.mode,
        createdAt: thread.createdAt, updatedAt: thread.updatedAt, archivedAt: thread.archivedAt,
      }];
    });
    const value = {
      version: 1, projects: [...this.folders.values()], threads: Object.fromEntries(this.homes),
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
        state: disposition.state, turnOrdinal: disposition.turnOrdinal,
        startedAt: thread.updatedAt, doneAt: thread.updatedAt,
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
      ? { ...entry, state: 'done', ...(interrupted ? { interrupted: true } : {}), ...(stopped ? { stopped: true } : {}) }
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
        this.homeDispositions.set(thread.id, { turnOrdinal: entry.turnOrdinal, state: current.state });
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
    entries: (): HomeEntry[] => [...this.threads.values()].filter(thread => thread.archivedAt === null).map(thread => this.homeEntry(thread)).filter((entry): entry is HomeEntry => entry !== null),
    projectTitle: (projectId: string) => projectId === HOH_PROJECT_ID ? 'No project' : this.folders.get(projectId)?.title ?? 'No project',
    /** Stable private color key for Home folders: a title can change, the workspace path does not. */
    projectKey: (projectId: string) => projectId === HOH_PROJECT_ID ? this.workspaceRoot : this.folders.get(projectId)?.workspaceRoot ?? projectId,
    /** Approve or decline a waiting permission request, as the chat's buttons do. */
    answer: async (threadId: string, requestId: string, decision: 'accept' | 'decline') => {
      const thread = await this.loaded(threadId);
      if (!thread) return;
      await this.respond(thread, { requestId, decision });
      this.flush();
    },
    /** A completed answer becomes Done only by the person's explicit action on that exact turn. */
    complete: async (threadId: string) => {
      const thread = await this.loaded(threadId);
      const disposition = this.homeDispositions.get(threadId);
      const latest = thread?.home();
      const entry = latest && disposition && latest.turnOrdinal !== disposition.turnOrdinal && this.failedHomePrompts.has(threadId)
        ? thread?.home(disposition.turnOrdinal)
        : latest;
      const interrupted = entry?.interrupted === true || (disposition?.turnOrdinal === entry?.turnOrdinal && disposition?.interrupted === true);
      const stopped = disposition?.turnOrdinal === entry?.turnOrdinal && disposition?.stopped === true;
      if (!thread || !entry || (entry.state !== 'done' && !(entry.state === 'ended' && (interrupted || stopped)))) return;
      this.homeDispositions.set(threadId, {
        turnOrdinal: entry.turnOrdinal, state: 'done',
        ...(interrupted ? { interrupted: true } : {}), ...(stopped ? { stopped: true } : {}),
      });
      this.save();
      this.changed(threadId);
      this.flush();
    },
    /** A new chat with this first message, in the project (or "No project"); returns its id. */
    start: async (message: string, projectId?: string) => {
      await this.start();
      const threadId = 'thread-' + (globalThis.crypto?.randomUUID?.() ?? this.now().toString(36));
      await this.launch({ threadId, commandId: 'home-' + threadId, projectId: projectId ?? HOH_PROJECT_ID, initialMessage: { messageId: 'msg-' + threadId, text: message, attachments: [] } });
      return threadId;
    },
  };

  private iso() { return new Date(this.now()).toISOString(); }
  private model(executor: HohExecutor = this.selectedExecutor) {
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

  private reserveLocalImage(requestKey: string, threadId: string) {
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
    if (this.localImageReservations.size >= LOCAL_IMAGE_MAX_INFLIGHT || threadInflight >= LOCAL_IMAGE_THREAD_MAX_INFLIGHT)
      throw new Error('Image unavailable: too many project images are already being opened in this window.');
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
    if (!existing && threadIdentities + reservedThreadIdentities >= LOCAL_IMAGE_THREAD_MAX_IDENTITIES)
      throw new Error('Image unavailable: too many project image paths are already retained for this conversation.');
    const replacedBytes = existing?.sizeBytes ?? 0;
    // A known retained key is a replacement reservation: it cannot grow retention until the
    // canonical post-read size check succeeds, so refreshing it needs no speculative byte slot.
    const reservedSize = existing?.sizeBytes ?? LOCAL_IMAGE_BYTES;
    if (!localImageRetentionFits({
      globalBytes: this.localImageBytes + reservedGlobalBytes,
      globalEntries: this.localImageUrls.size + reservedGlobalEntries,
      threadBytes: threadBytes + reservedThreadBytes,
      threadEntries: threadEntries + reservedThreadEntries,
      replacedBytes, replacing: !!existing, sizeBytes: reservedSize,
    })) throw new Error('Image unavailable: too many project images are already open in this window.');
    this.localImageReservations.set(requestKey, {
      threadId, reservedBytes: reservedSize - replacedBytes,
      replacing: !!existing, reservesIdentity: !existing,
    });
  }

  private bindLocalImageRequest(requestKey: string, key: string) {
    const retained = this.localImageUrls.get(key);
    if (!retained) return;
    for (const [otherKey, other] of this.localImageUrls) {
      if (otherKey !== key && other.threadId === retained.threadId) other.requestKeys.delete(requestKey);
    }
    retained.requestKeys.add(requestKey);
  }

  private consumeLocalImageAttempt(threadId: string) {
    const now = this.now();
    this.localImageGlobalAttempts = this.localImageGlobalAttempts
      .filter(timestamp => timestamp > now - LOCAL_IMAGE_ATTEMPT_WINDOW_MS);
    const previous = lruGet(this.localImageAttempts, threadId) ?? [];
    const attempts = previous.filter(timestamp => timestamp > now - LOCAL_IMAGE_ATTEMPT_WINDOW_MS);
    if (attempts.length >= LOCAL_IMAGE_ATTEMPT_MAX || this.localImageGlobalAttempts.length >= LOCAL_IMAGE_GLOBAL_ATTEMPT_MAX)
      throw new Error('Image unavailable: too many project image reads were attempted for this conversation; try again later.');
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

  private async readLocalImage(threadId: string, path: string, requestKey: string) {
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
      this.bindLocalImageRequest(requestKey, key);
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
      requestKeys: existing?.requestKeys ?? new Set<string>(),
    };
    this.localImageUrls.set(key, minted);
    this.bindLocalImageRequest(requestKey, key);
    this.localImageBytes += image.sizeBytes;
    this.localImageFailures.delete(requestKey);
    return minted;
  }

  /** A conversation-bound Blob URL for a native image that passed the single Rust file policy. */
  private localImageUrl(threadId: string, path: string): Promise<LocalImageUrl> {
    if (this.stopped) return Promise.reject(new Error('Image unavailable: this window has closed.'));
    if (path.length > 4096) return Promise.reject(new Error('Image unavailable: the image path is too long.'));
    const requestKey = `${threadId}\0${path}`;
    const failure = lruGet(this.localImageFailures, requestKey);
    if (failure && failure.expiresAt > this.now()) return Promise.reject(new Error(failure.message));
    const pending = this.localImageInflight.get(requestKey);
    if (pending) return pending;
    try {
      this.reserveLocalImage(requestKey, threadId);
    } catch (error) {
      const message = errorText(error).slice(0, 512);
      lruSet(this.localImageFailures, requestKey, {
        message, expiresAt: this.now() + LOCAL_IMAGE_FAILURE_TTL_MS,
      }, LOCAL_IMAGE_REQUEST_CACHE_MAX);
      return Promise.reject(error);
    }
    const read = this.readLocalImage(threadId, path, requestKey)
      .catch(error => {
        if (!this.stopped && !localImageContextUnknown(error)) lruSet(this.localImageFailures, requestKey, {
          message: errorText(error).slice(0, 512), expiresAt: this.now() + LOCAL_IMAGE_FAILURE_TTL_MS,
        }, LOCAL_IMAGE_REQUEST_CACHE_MAX);
        throw error;
      })
      .finally(() => {
        this.localImageReservations.delete(requestKey);
        if (this.localImageInflight.get(requestKey) === read) this.localImageInflight.delete(requestKey);
      });
    this.localImageInflight.set(requestKey, read);
    return read;
  }

  private async load() {
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
    void this.loadConfiguration(discoveryGeneration).then(discovered => {
      if (!discovered && discoveryGeneration === this.discoveryGeneration) void this.retryConfiguration(discoveryGeneration);
    });
    const sessions = this.readAccess().then(async accessRead => {
      this.configChanged();
      if (!accessRead) { void this.retryAccessConfiguration(); return; }
      await this.loadSessions();
    });
    // Preserve the single-snapshot fast path when session/list answers immediately, without letting a slow or
    // wedged bridge own startup: after one event-loop turn T3 receives the explicit pending placeholder instead.
    await Promise.race([sessions, new Promise<void>(resolve => setTimeout(resolve, 0))]);
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
    for (const threadId of [...this.catalogPending]) {
      const thread = this.threads.get(threadId);
      const ready = thread?.executor === 'codex' ? codex !== null : claude !== null;
      if (!thread || !ready) continue;
      thread.model = this.model(thread.executor);
      this.catalogPending.delete(threadId);
      this.changed(threadId);
    }
    if (claude !== null && codex !== null) {
      this.discoveryPending = false;
      return true;
    }
    return false;
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
        if (access?.selected?.executor === 'codex' || access?.selected?.executor === 'claude') this.selectedExecutor = access.selected.executor;
        if (typeof access?.selected?.model === 'string' && access.selected.model) this.selectedModel = access.selected.model;
      }
      if (modeRevision === this.modeRevision) {
        this.mode = mode;
        this.accessError = null;
        this.materializeSavedSessions();
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
    await this.readDiscovery();
    const accessRead = await this.readAccess();
    if (accessRead) await this.readSessions();
    this.configChanged();
    if (accessRead) this.flush();
  }

  private async readSessions(): Promise<boolean> {
    const globalMode = this.mode;
    if (globalMode === null) return false;
    let sessions: any[] = [];
    try { sessions = (await this.engine.request('session/list', {}))?.sessions ?? []; } catch { return false; }
    let inventoryChanged = false;
    for (const session of sessions) {
      if (typeof session?.id !== 'string' || !session.id) continue;
      const at = Number(session.updatedAt) || this.startedAt;
      let executor = this.selectedExecutor;
      let model: string | undefined;
      // An unbound history row follows the authoritative engine access setting.
      // `defaultMode` is only the composer default for a new draft.
      let mode = globalMode;
      const wasKnown = this.sessionAccessKnown.has(session.id);
      let ownKnown = false;
      const modeRevision = this.modeRevision;
      try {
        const own = (await this.engine.request('hoh/executor/access/get', { executor: 'codex', sessionId: session.id }))?.session;
        const exact = exactMode(own?.runtimeMode);
        if (own && (!exact || (own.executor !== 'codex' && own.executor !== 'claude'))) continue;
        if (own) ownKnown = true;
        if (own?.executor === 'codex' || own?.executor === 'claude') executor = own.executor;
        model = typeof own?.model === 'string' && own.model ? own.model : undefined;
        mode = exact ?? mode;
      } catch { /* a session without an external executor follows the current composer */ }
      // A Codex row with no persisted model must not publish the synthetic `default` model while
      // app-server's native catalog is already in flight. Claude rows stay non-blocking for cold start.
      if (executor === 'codex' && model === undefined && this.codexDiscovery === null && this.discoveryInFlight)
        await this.discoveryInFlight.promise;
      model ??= this.model(executor);

      const existing = this.threads.get(session.id);
      if (existing) {
        if (modeRevision !== this.modeRevision) continue;
        if (ownKnown) this.sessionAccessKnown.add(session.id);
        if (this.restored.delete(session.id)) {
          // The cache is only continuity for the first snapshot. Once this session is visible on
          // disk, let its current list entry and history replace stale presentation metadata.
          existing.title = this.titles.get(session.id) ?? (typeof session.title === 'string' ? session.title : '');
          existing.updatedAt = Math.max(existing.updatedAt, at);
          existing.executor = executor;
          existing.model = model;
          existing.mode = mode;
          inventoryChanged = true;
          if (this.sessionsKnown) this.changed(session.id);
        } else if ((ownKnown || !wasKnown || !this.sessionAccessKnown.has(session.id))
          && (existing.mode !== mode || existing.executor !== executor || existing.model !== model)) {
          // Until a session reports its own binding it follows the global selection/access
          // returned by the engine. Refresh those provisional rows on every ready event.
          existing.mode = mode;
          existing.executor = executor;
          existing.model = model;
          inventoryChanged = true;
          if (this.sessionsKnown) this.changed(session.id);
        }
        continue;
      }
      if (modeRevision !== this.modeRevision) continue;
      if (ownKnown) this.sessionAccessKnown.add(session.id);
      this.threads.set(session.id, new ThreadModel({
        id: session.id,
        title: this.titles.get(session.id) ?? (typeof session.title === 'string' ? session.title : ''),
        createdAt: at,
        updatedAt: at,
        model,
        mode,
        executor,
        interactionMode: this.interactions.get(session.id),
        projectId: this.homeOf(session.id),
      }));
      if ((executor === 'codex' ? this.codexDiscovery : this.claudeDiscovery) === null) this.catalogPending.add(session.id);
      inventoryChanged = true;
      // During the first load the metadata is already in the forthcoming snapshot. A retry happens after that
      // snapshot, so it needs an update for each conversation that arrived late.
      if (this.sessionsKnown && this.shellSinks.size) this.changed(session.id);
    }
    // Reconciliation is deliberately merge-only: an immediately restarted engine may return a
    // temporarily incomplete list, so neither cached rows nor their Waiting/Done state is pruned.
    if (inventoryChanged) this.save();
    if ([...this.threads.values()].some(thread => thread.projectId === HOH_PROJECT_ID)) this.showProject();
    this.sessionsReady();
    // History enriches Home and list rows, but metadata is enough to render and select a conversation. Start it
    // only after the authoritative bootstrap frames have been queued: this keeps first paint ahead of disk I/O
    // and lets an immediate Add project win over stale folder inference from an old history.
    const timer: any = setTimeout(() => { if (!this.stopped) void this.peekToday(); }, 0);
    timer?.unref?.();
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
    const recent = [...new Map([...changedWhileAway, ...candidates.filter(thread => waiting.has(thread.id)), ...todayThreads].map(thread => [thread.id, thread])).values()]
      .sort((a, b) => b.updatedAt - a.updatedAt);
    let dispositionChanged = false;
    let cursor = 0;
    const inspect = async (thread: ThreadModel) => {
      if (this.stopped) return;
      try {
        let afterSeq: number | undefined;
        for (;;) {
          const page = await this.engine.request('session/events', { sessionId: thread.id, limit: PAGE, ...(afterSeq === undefined ? {} : { afterSeq }) });
          if (this.stopped) return;
          const events: any[] = Array.isArray(page?.events) ? page.events : [];
          for (const event of events) thread.applyEvent(event);
          if (events.length < PAGE || (afterSeq !== undefined && page.nextSeq <= afterSeq)) break;
          afterSeq = page.nextSeq;
        }
        // The engine's own word on whether the chat is working (`access/get` of the chat), when it gives one.
        try { if ((await this.engine.request('hoh/executor/access/get', { executor: thread.executor, sessionId: thread.id }))?.session?.running) thread.setRunning(true, this.now()); } catch { /* unknown: the history decides */ }
        if (this.stopped) return;
        thread.settleUnfinished(this.now());
        this.peeked.add(thread.id);
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

  /** The chat's history, read once from the engine (after a restart too: the engine keeps it on disk). */
  private async loaded(threadId: string): Promise<ThreadModel | undefined> {
    if (!this.sessionAccessKnown.has(threadId)) await this.notificationAccess.get(threadId);
    const thread = this.threads.get(threadId);
    if (thread && !thread.options && this.levels.has(threadId)) thread.options = this.levels.get(threadId);
    if (!thread || thread.loaded) return thread;
    let pending = this.loading.get(threadId);
    if (!pending) {
      pending = (async () => {
        // After a restart the chat is on disk but not open: it is resumed before anything else.
        try { await this.engine.request('session/resume', { sessionId: threadId }); } catch { /* new or already open */ }
        let afterSeq: number | undefined;
        for (;;) {
          const page = await this.engine.request('session/events', { sessionId: threadId, limit: PAGE, ...(afterSeq === undefined ? {} : { afterSeq }) });
          const events: any[] = Array.isArray(page?.events) ? page.events : [];
          for (const event of events) thread.applyEvent(event);
          if (events.length < PAGE) break;
          if (afterSeq !== undefined && page.nextSeq <= afterSeq) break;
          afterSeq = page.nextSeq;
        }
        thread.settleUnfinished(this.now());
        // The model this chat is bound to (a chat that has had a turn keeps its own).
        try {
          const own = (await this.engine.request('hoh/executor/access/get', { executor: thread.executor, sessionId: threadId }))?.session;
          if (own?.executor === 'codex' || own?.executor === 'claude') thread.executor = own.executor;
          if (typeof own?.model === 'string' && own.model) thread.model = own.model;
          thread.mode = exactMode(own?.runtimeMode) ?? exactMode(own?.mode) ?? thread.mode;
        } catch { /* the chat follows the choice for new chats */ }
        thread.loaded = true;
        // Place the chat before publishing what its history taught us. Otherwise the first
        // `thread.updated` briefly assigns a chat with a known cwd to "No project".
        if (thread.cwd) {
          this.placeByFolder(thread);
          this.settleLocalImageContext(threadId, thread);
        }
        else if (!this.workspaceKnown && thread.projectId === HOH_PROJECT_ID) {
          try { this.setWorkspaceRoot((await this.engine.request('session/working-directory/get', { sessionId: threadId }))?.cwd); } catch { /* the placeholder stays */ }
        }
        // The list row learns what the history knows (a title from the first message, the last answer).
        this.changed(threadId);
        return thread;
      })().finally(() => this.loading.delete(threadId));
      this.loading.set(threadId, pending);
    }
    return pending;
  }

  private threadFor(sessionId: unknown): ThreadModel | undefined {
    if (typeof sessionId !== 'string' || !sessionId) return undefined;
    let thread = this.threads.get(sessionId);
    if (!thread) {
      // A chat started elsewhere (another window, a schedule): it joins the list.
      if (this.mode === null) return undefined;
      thread = new ThreadModel({
        id: sessionId,
        title: this.titles.get(sessionId),
        createdAt: this.now(),
        model: this.model(),
        mode: this.mode,
        executor: this.selectedExecutor,
        interactionMode: this.interactions.get(sessionId),
      });
      thread.loaded = true;
      this.threads.set(sessionId, thread);
      const created = thread;
      const access = this.engine.request('hoh/executor/access/get', { executor: this.selectedExecutor, sessionId })
        .then(result => {
          if (this.sessionAccessKnown.has(sessionId)) return;
          const own = result?.session;
          const mode = exactMode(own?.runtimeMode);
          if (!mode || (own?.executor !== 'claude' && own?.executor !== 'codex')) throw new Error('Conversation access is unavailable.');
          created.executor = own.executor; created.mode = mode;
          if (typeof own.model === 'string' && own.model) created.model = own.model;
          this.sessionAccessKnown.add(sessionId); this.changed(sessionId);
        });
      // Retain the promise, including a failure, so sending cannot use a guessed mode.
      this.notificationAccess.set(sessionId, access);
      void access.catch(() => {});
      this.showProject();
    }
    return thread;
  }

  private onNotification(notification: EngineNotification) {
    const params = notification.params ?? {};
    switch (notification.method) {
      case 'session.event': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.applyEvent(params.event)) this.changed(thread.id);
        // The end of a turn reaches the thread list at once: the sidebar's "Working" must not wait for the next change.
        if (thread && params.event?.type === 'turn/end') this.flush();
        if (thread && params.event?.type === 'working-directory/change') {
          this.placeByFolder(thread);
          if (thread.cwd) this.settleLocalImageContext(thread.id, thread);
        }
        return;
      }
      case 'session.assistant-stream': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.applyStreamFrame(params.frame ?? {}, this.now())) this.changed(thread.id);
        return;
      }
      case 'session.status': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.setRunning(params.status === 'running', this.now())) { this.changed(thread.id); if (params.status !== 'running') this.flush(); }
        return;
      }
      case 'approval.pending': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.approvalPending(params, this.now())) this.changed(thread.id);
        return;
      }
      case 'approval.resolved':
        for (const thread of this.threads.values()) if (thread.approvalResolved(params, this.now())) this.changed(thread.id);
        return;
      case 'user-question.cancelled': {
        this.questions.delete(params.requestId);
        for (const thread of this.threads.values()) if (thread.questionClosed(params.requestId, null, this.now())) this.changed(thread.id);
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
    this.mode = null;
    this.accessError = 'Executor access mode is unknown while the engine is unavailable.';
    // Fail closed immediately: T3 must stop showing or sending a stale mode while the
    // replacement engine has not answered access/get yet.
    this.configChanged();
    for (const thread of this.threads.values()) {
      if (thread.running || thread.activeRunId()) { thread.engineRunning = false; thread.running = false; thread.settleUnfinished(this.now()); this.changed(thread.id); }
    }
  }

  // ---- change delivery ----

  private homeChanged() {
    if (this.stopped) return;
    this.homeDirty = true;
    if (this.flushTimer) return;
    this.flushTimer = setTimeout(() => this.flush(), this.throttleMs);
  }

  private changed(threadId: string) {
    if (this.stopped) return;
    this.dirty.add(threadId);
    this.homeChanged();
  }

  private accessVisible(threadId: string) {
    return !this.notificationAccess.has(threadId) || this.sessionAccessKnown.has(threadId);
  }

  /** Send every pending change now (tests call it; the timer calls it otherwise). */
  flush() {
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.flushTimer = undefined;
    const ids = [...this.dirty]; this.dirty.clear();
    let dispositionChanged = false;
    for (const id of ids) { const thread = this.threads.get(id); if (thread && this.syncHomeDisposition(thread)) dispositionChanged = true; }
    if (ids.length || dispositionChanged) this.save();
    if (this.homeDirty) { this.homeDirty = false; this.homeVersion++; for (const listener of this.homeListeners) listener(); }
    for (const id of ids) {
      const thread = this.threads.get(id);
      if (!thread || !this.accessVisible(id)) continue;
      this.sequence++;
      for (const sink of this.shellSinks) sink.emit({ kind: 'thread.updated', sequence: this.sequence, location: thread.archivedAt === null ? 'active' : 'archive', thread: thread.shell() });
      if (thread.loaded) for (const sink of this.threadSinks.get(id) ?? []) sink.emit(this.threadSnapshot(thread));
    }
  }

  private threadSnapshot(thread: ThreadModel) {
    return { kind: 'snapshot', snapshotSequence: this.sequence, projection: thread.projection() };
  }

  private config() {
    return serverConfig({
      cwd: this.workspaceRoot,
      claudeDiscovery: this.claudeDiscovery,
      codexDiscovery: this.codexDiscovery,
      discoveryFailure: this.discoveryFailure,
      codexDiscoveryFailure: this.codexDiscoveryFailure,
      defaultMode: this.defaultMode,
      defaultInstanceId: instanceOf(this.selectedExecutor),
      defaultModel: this.model(),
      accessKnown: this.mode !== null,
      settingsOverrides: this.settingsOverrides,
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
    );
  }

  private configChanged() {
    for (const sink of this.configSinks) sink.emit({ version: 1, type: 'snapshot', config: this.config() });
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
        if (executor === 'codex' && interaction === 'plan') throw new Error('Plan mode for Codex is coming soon.');
        const chosenModel = model ?? this.model(executor);
        const chosenMode = accessModeOf(mode ?? runtimeModeOf(this.defaultMode));
        const already = this.threads.get(threadId);
        if (already) {
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
        this.setWorkspaceRoot(directory?.cwd);
        const bound = await this.engine.request('hoh/executor/access/get', { executor, sessionId: threadId });
        if (bound?.session?.executor !== executor || bound?.session?.model !== chosenModel) {
          throw new Error('The engine bound the new conversation to a different executor or model. The message was not sent.');
        }
        if (folder) {
          const set = await this.engine.request('session/working-directory/set', { sessionId: threadId, path: folder.workspaceRoot });
          // A typed `~/…` root becomes the folder the engine resolved, so chats' recorded folders match it.
          const resolved = folderPath(set?.cwd);
          if (resolved && resolved !== folder.workspaceRoot && folder.workspaceRoot.startsWith('~') && !this.folderAt(resolved)) {
            folder.workspaceRoot = resolved;
            folder.updatedAt = this.now();
            this.emitProject(folderProject(folder));
          }
        }
        const thread = new ThreadModel({ id: threadId, title, createdAt: this.now(), model: chosenModel, mode: this.defaultMode, executor, projectId: folder?.id ?? HOH_PROJECT_ID });
        thread.setInteractionMode(interaction);
        this.interactions.set(threadId, interaction);
        thread.cwd = folder ? folder.workspaceRoot : folderPath(directory?.cwd) || null;
        thread.loaded = true;
        await this.applyMode(thread, mode ?? runtimeModeOf(this.defaultMode), true);
        const verified = await this.engine.request('hoh/executor/access/get', { executor, sessionId: threadId });
        if (verified?.session?.executor !== executor || verified?.session?.model !== chosenModel || verified?.session?.runtimeMode !== thread.mode) {
          throw new Error('The engine did not retain the requested executor, model, and access mode. The message was not sent.');
        }
        this.threads.set(threadId, thread);
        this.homes.set(threadId, thread.projectId);
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
        if (root === this.workspaceRoot) throw fail(`${root} is the folder of "No project". Use "Start without a project".`);
        if (!projectId || projectId === HOH_PROJECT_ID || this.folders.has(projectId)) throw fail('This project id is taken.');
        await this.checkFolder(root, fail);
        // The engine learns the folder (the person's "Add project" in the main window confirms it), so `@`, `/` and `$`
        // answer for it before its first chat; the engine names the folder as it resolved it.
        let added: string;
        try { added = folderPath((await this.engine.request('hoh/workspace/add', { cwd: root }))?.cwd) || root; }
        catch (error) { throw fail(`The folder ${root} cannot be a project: ${englishError(error, 'HoH refused it.')}`); }
        if (added !== root && (this.folderAt(added) || added === this.workspaceRoot)) throw fail(`${added} is already a project or the folder of "No project".`);
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
        if ([...this.threads.values()].some(thread => thread.projectId === projectId)) throw fail('This project still has threads.');
        this.folders.delete(projectId);
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
  private applyMode(thread: ThreadModel, mode: T3RuntimeMode | undefined, force = false): Promise<void> {
    const operation = this.accessWriteTail.then(() => this.applyModeSerialized(thread, mode, force));
    this.accessWriteTail = operation.catch(() => undefined);
    return operation;
  }

  private async applyModeSerialized(thread: ThreadModel, mode: T3RuntimeMode | undefined, force: boolean) {
    await this.requireAccess();
    if (!mode) return;
    const wanted = accessModeOf(mode);
    this.modeRevision++;
    if (!force && wanted === thread.mode && this.sessionAccessKnown.has(thread.id)) return;
    const processGeneration = this.accessProcessGeneration;
    this.accessGeneration++;
    await this.engine.request('hoh/executor/access/set', {
      executor: thread.executor,
      sessionId: thread.id,
      mode: wanted === 'full' ? 'full' : 'confirm',
      runtimeMode: wanted,
    });
    if (processGeneration !== this.accessProcessGeneration) {
      throw new Error(this.accessError ?? 'Executor access mode is unknown while the engine is unavailable.');
    }
    this.sessionAccessKnown.add(thread.id);
    thread.mode = wanted;
    this.changed(thread.id);
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

  /**
   * One message to the engine: its pictures first (the bytes kept from T3's upload or `assets.persistChatAttachments`; the engine's
   * attachment store checks and copies them), then its text, with the skills Claude declared for the chat's folder
   * written as Claude runs them (`$name` → `/name`, T3's `planClaudeSkillDispatch`).
   */
  private async send(thread: ThreadModel, messageId: string, text: string, attachments: unknown = []) {
    await this.requireAccess();
    if (!this.sessionAccessKnown.has(thread.id)) await this.notificationAccess.get(thread.id);
    const firstMessage = thread.userMessageCount() === 0;
    const images = this.keptImages(attachments);
    // T3 releases the completed upload immediately after send. Promote its process-local preview
    // first, so the timeline can keep rendering it without retaining upload bytes or opening CSP.
    for (const image of images) {
      this.previewUrl(image);
      this.sentPreviewIds.add(image.id);
    }
    const prompt = skillsAsCommands(
      stripAttachedImageReferences(text, images.map(image => image.id)),
      this.skillsByFolder.get(`${thread.executor}\0${folderPath(thread.cwd)}`) ?? new Set(),
    );
    this.homePrompts.add(thread.id);
    thread.sendLocal(messageId, text, this.now(), images.map(({ data: _data, ...shown }) => ({ type: 'image' as const, ...shown })), prompt);
    this.changed(thread.id);
    const contentBlocks = [
      ...images.map(image => ({ type: 'image', data: image.data, mimeType: image.mimeType, name: image.name })),
      ...(prompt || !images.length ? [{ type: 'text', text: prompt }] : []),
    ];
    try {
      const effort = thread.executor === 'codex' ? codexEffortOf(thread.options) : effortOf(thread.model, thread.options);
      const result = await this.engine.request('session/prompt', {
        sessionId: thread.id,
        contentBlocks,
        interactionMode: thread.executor === 'codex' ? 'default' : thread.interactionMode,
        ...effortParam(effort),
      });
      this.homePrompts.delete(thread.id);
      this.failedHomePrompts.delete(thread.id);
      if (this.homeDispositions.delete(thread.id)) this.save();
      for (const image of images) this.uploads.delete(image.id);
      if (typeof result?.messageId === 'string') thread.receipt(result.messageId, messageId);
      if (firstMessage && thread.executor === 'claude') void this.generateTitle(thread, text);
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
   * T3's workspace snapshot of the composer's folder: the commands and skills Claude declares there, asked without a turn.
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

  private async generateTitle(thread: ThreadModel, message: string) {
    const baseline = thread.title;
    const requestId = globalThis.crypto?.randomUUID?.() ?? `${this.now()}-${Math.random()}`;
    this.titleRequests.set(thread.id, requestId);
    const prompt = `Generate a title that will help the user recognize this T3 Code thread weeks later.
Return JSON with key title.

Editorial rules:
- 3-8 words, fewer than 40 characters.
- Use a compact noun phrase or clear action phrase.
- Title the durable subject and desired outcome, not the plan, model, tools, tests, or workflow.
- Do not claim the work is complete, copy and truncate the message, use quotes, filler, or trailing punctuation.

USER MESSAGE:
${message.slice(0, 8000)}`;
    try {
      const result = await this.engine.request('hoh/executor/title/generate', { executor: 'claude', prompt });
      const title = typeof result?.title === 'string' ? result.title.trim() : '';
      if (!title || title.length > 120 || this.titleRequests.get(thread.id) !== requestId || thread.title !== baseline) return;
      thread.title = title;
      this.titles.set(thread.id, title);
      this.titleRequests.delete(thread.id);
      this.save(); this.changed(thread.id); this.flush();
    } catch { /* A title is optional: the first-message fallback stays. */ }
  }

  /** «Стоп»: the same cancel as the current window; the engine ends Claude and its commands. */
  private async cancel(thread: ThreadModel) {
    const stoppedOrdinal = thread.activeRunId() ? thread.home()?.turnOrdinal : undefined;
    const disposition = this.homeDispositions.get(thread.id);
    const replacedCrashDisposition = stoppedOrdinal !== undefined && disposition?.turnOrdinal === stoppedOrdinal && disposition.interrupted === true;
    if (stoppedOrdinal !== undefined) {
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
        if (disposition) this.homeDispositions.set(thread.id, disposition); else this.homeDispositions.delete(thread.id);
        this.save();
        this.changed(thread.id);
        this.flush();
      }
      throw error;
    }
    // The turn is stopped now: its run closes and the thread list stops saying "Working" without waiting for the engine's closing event.
    if (thread.interrupt(this.now()) || replacedCrashDisposition || stoppedOrdinal !== undefined) { this.changed(thread.id); this.flush(); }
  }

  private async respond(thread: ThreadModel, command: Record<string, any>) {
    const requestId = String(command.requestId ?? '');
    const request = thread.pendingRequest(requestId);
    if (!request) throw dispatchError(command, 'This request is no longer waiting for an answer.');
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

  private async dispatch(command: Record<string, any>) {
    await this.start();
    const type = String(command.type ?? '');
    const thread = typeof command.threadId === 'string' ? await this.loaded(command.threadId) : undefined;
    const needThread = () => { if (!thread) throw dispatchError(command, 'This thread does not exist.'); return thread; };
    try {
      switch (type) {
        case 'thread.create':
          if (!thread) await this.createThread(
            command.threadId,
            command.title,
            command.modelSelection?.model,
            command.runtimeMode,
            command.projectId,
            command.modelSelection?.instanceId,
            command.interactionMode,
          );
          break;
        case 'message.dispatch': {
          const target = needThread();
          if (command.modelSelection?.instanceId && executorOf(command.modelSelection.instanceId) !== target.executor) throw dispatchError(command, 'A conversation cannot change its executor. Start a new thread.');
          if (command.modelSelection?.model) { await this.applyModel(target.executor, command.modelSelection.model); target.model = command.modelSelection.model; }
          this.setLevel(target, command.modelSelection?.options);
          await this.send(target, String(command.messageId), String(command.text ?? ''), command.attachments);
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
        case 'thread.runtime-mode.set': { const target = needThread(); await this.applyMode(target, command.runtimeMode); break; }
        case 'thread.interaction-mode.set': {
          const target = needThread();
          const interaction = interactionModeOf(command.interactionMode);
          if (target.executor === 'codex' && interaction === 'plan') throw new Error('Plan mode for Codex is coming soon.');
          target.setInteractionMode(interaction);
          this.interactions.set(target.id, target.interactionMode);
          this.save(); this.changed(target.id);
          break;
        }
        case 'thread.model-selection.set': {
          const target = needThread();
          if (command.modelSelection?.instanceId && executorOf(command.modelSelection.instanceId) !== target.executor) throw dispatchError(command, 'A conversation cannot change its executor. Start a new thread.');
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
            target.title = command.title.trim(); this.titles.set(target.id, target.title); this.titleRequests.delete(target.id); this.save(); this.changed(target.id);
          }
          break;
        }
        case 'thread.archive': case 'thread.unarchive': {
          const target = needThread();
          target.archivedAt = type === 'thread.archive' ? this.now() : null;
          this.changed(target.id);
          break;
        }
        default:
          if (!NO_OP_COMMANDS.has(type)) throw dispatchError(command, 'Not available in HoH.');
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
      if (requestedExecutor === 'codex' && interactionMode === 'plan') throw new Error('Plan mode for Codex is coming soon.');
      if (existing && requestedExecutor !== existing.executor) throw new Error('A conversation cannot change its executor. Start a new thread.');
      // Validate the complete project transition before access/set: a rejected launch must not
      // mutate this conversation's permissions even when the request also asks for a wider mode.
      const launchProject = existing ? this.validateLaunchProject(existing, input.projectId) : null;
      const thread = existing ?? await this.createThread(
        threadId,
        input.title,
        input.modelSelection?.model,
        input.runtimeMode,
        input.projectId,
        input.modelSelection?.instanceId,
        interactionMode,
      );
      if (existing) await this.applyMode(existing, input.runtimeMode);
      if (existing) await this.selectLaunchProject(thread, launchProject);
      if (existing && interactionMode !== undefined) {
        thread.setInteractionMode(interactionMode);
        this.interactions.set(thread.id, thread.interactionMode); this.save();
      }
      const message = input.initialMessage;
      if (message && typeof message.text === 'string') {
        const messageId = typeof message.messageId === 'string' && message.messageId ? message.messageId : 'msg-' + String(input.commandId);
        this.setLevel(thread, input.modelSelection?.options);
        await this.send(thread, messageId, message.text, message.attachments);
      }
      this.flush();
      return { threadId, projection: thread.projection(), resumed: !!existing, sequence: this.sequence };
    } catch (error) {
      throw new RpcFailure({ _tag: 'OrchestrationV2ThreadLaunchError', commandId: String(input.commandId ?? 'unknown'), projectId: String(input.projectId ?? HOH_PROJECT_ID), message: errorText(error) });
    }
  }

  // ---- the methods T3 calls ----

  handlers(): Handlers {
    return {
      unary: {
        'server.probe': () => ({}),
        // Focus and typing reports of the window: nothing to schedule here, accepted and dropped.
        'server.reportClientActivity': () => undefined,
        'server.getConfig': async () => { await this.start(); return this.config(); },
        'server.getSettings': async () => { await this.start(); return this.settings(); },
        'server.updateSettings': async (payload: any) => {
          await this.start();
          const patch = payload?.patch && typeof payload.patch === 'object' ? { ...payload.patch } : {};
          // The default mode is where new threads start; the model is the engine's choice for new chats.
          if (typeof patch.defaultRuntimeMode === 'string') { this.defaultMode = accessModeOf(patch.defaultRuntimeMode); this.save(); }
          if (patch.defaultModelSelection?.model) await this.applyModel(executorOf(patch.defaultModelSelection.instanceId), patch.defaultModelSelection.model);
          delete patch.defaultRuntimeMode; delete patch.defaultModelSelection;
          // Only whole values are kept: a nested patch is partial and would not decode as a setting.
          const primitive = (value: unknown) => value === null || ['string', 'number', 'boolean'].includes(typeof value);
          for (const [key, value] of Object.entries(patch)) {
            if (primitive(value) || (Array.isArray(value) && value.every(primitive))) this.settingsOverrides = { ...this.settingsOverrides, [key]: value };
          }
          for (const sink of this.configSinks) sink.emit({ version: 1, type: 'settingsUpdated', payload: { settings: this.settings() } });
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
            const image = await this.localImageUrl(thread.id, imagePath);
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
          const claude = () => claudeProvider(this.claudeDiscovery, this.iso(), this.mode !== null, this.discoveryFailure);
          const codex = () => codexProvider(this.codexDiscovery, this.iso(), this.codexDiscoveryFailure);
          // The composer's folder (T3's workspace snapshot): `/` and `$` from its selected executor.
          const cwd = typeof payload?.cwd === 'string' ? payload.cwd.trim() : '';
          if (cwd && payload?.instanceId === CODEX_INSTANCE_ID) {
            if (this.codexDiscovery === null) await this.readDiscovery();
            return { providers: [{ ...codexProvider(this.codexDiscovery, this.iso(), this.codexDiscoveryFailure), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'codex')] }] };
          }
          if (cwd && payload?.instanceId === CLAUDE_INSTANCE_ID) {
            if (this.claudeDiscovery === null) await this.readDiscovery();
            return { providers: [{ ...claude(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'claude')] }] };
          }
          if (cwd && payload?.instanceId === undefined) {
            if (this.claudeDiscovery === null || this.codexDiscovery === null) await this.readDiscovery();
            return { providers: [
              { ...claude(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'claude')] },
              { ...codex(), workspaceSnapshots: [await this.workspaceSnapshot(cwd, 'codex')] },
            ] };
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
            ] };
          }
          const generation = ++this.discoveryGeneration;
          const startedAt = this.now();
          this.discoveryPending = true;
          this.discoveryFailure = undefined;
          this.codexDiscoveryFailure = undefined;
          if (!await this.readDiscovery(generation) && generation === this.discoveryGeneration) this.finishDiscoveryFailure(startedAt);
          this.configChanged();
          return { providers: [
            claude(),
            codex(),
          ] };
        },
        'orchestration.dispatchCommand': (command: any) => this.dispatch(command ?? {}),
        'orchestration.launchThread': (input: any) => this.launch(input ?? {}),
        'orchestration.getThreadProjection': async (payload: any) => {
          await this.start();
          const thread = await this.loaded(String(payload?.threadId ?? ''));
          if (!thread) throw new RpcFailure({ _tag: 'OrchestrationV2GetThreadProjectionError', threadId: String(payload?.threadId ?? 'unknown'), message: 'This thread does not exist.' });
          return thread.projection();
        },
        'orchestration.forceStopThread': async (payload: any) => {
          await this.start();
          const thread = await this.loaded(String(payload?.threadId ?? ''));
          if (!thread) throw new RpcFailure({ _tag: 'OrchestrationV2ForceStopThreadError', threadId: String(payload?.threadId ?? 'unknown') });
          await this.cancel(thread);
          return { outcome: 'killed', sharedWithThreadIds: [] };
        },
        // Git is hidden; T3 still asks about the folder when a thread opens. It is not a repository.
        'vcs.refreshStatus': () => ({ ...NOT_A_REPO_LOCAL, hasUpstream: false, aheadCount: 0, behindCount: 0, pr: null }),
        'vcs.listRefs': () => ({ refs: [], isRepo: false, hasPrimaryRemote: false, nextCursor: null, totalCount: 0 }),
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
        // Without an answer T3 assumes a repository and asks for a base branch before sending.
        subscribeVcsStatus: (_payload, sink) => { sink.emit({ _tag: 'snapshot', local: NOT_A_REPO_LOCAL, remote: null }); },
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
          void this.start().then(() => this.loaded(threadId)).then(thread => {
            if (!open) return;
            // A thread that does not exist yet (a draft) gets its first snapshot when it is created.
            if (thread) sink.emit(this.threadSnapshot(thread));
            if (thread && payload?.requestCompletionMarker) sink.emit({ kind: 'synchronized' });
            own.add(sink);
          }, error => { if (open) sink.fail(new RpcFailure({ _tag: 'OrchestrationV2GetThreadProjectionError', threadId, message: errorText(error) })); });
          return () => {
            open = false;
            own.delete(sink);
            if (own.size) return;
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
