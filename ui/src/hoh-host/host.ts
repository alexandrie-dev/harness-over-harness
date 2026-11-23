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
  accessModeOf, CLAUDE_INSTANCE_ID, claudeProvider, defaultClaudeModel, effortOf, listedClaudeModel, serverConfig, serverSettings, type ClaudeDiscovery, type HohAccessMode, type T3RuntimeMode,
} from './config.ts';
import type { EngineNotification, EngineQuestion, EngineQuestionAnswer, HohEngine } from './engine.ts';
import { base64Of, readUploads, searchAnswer, skillsAsCommands, UPLOAD_PATH, UPLOAD_TTL_MS, uploadSlot, workspaceSnapshot, type KeptImage, type UploadSlot } from './composer.ts';
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
}

export interface HohStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface HomeDisposition {
  /** Run ordinal is stable when the engine history is replayed after a restart. */
  turnOrdinal: number;
  state: 'waiting' | 'done';
}

/** The reasoning level goes to the engine as `effort` of `session/prompt` (agreed with the engine's writer). */
const effortParam = (effort: string | null) => effort ? { effort } : {};

export const STORAGE_KEY = 'hoh-host.projects.v1';
const PLACEHOLDER_ROOT = '~/Documents/HoH';

function windowStorage(): HohStorage | null {
  try { return (globalThis as { localStorage?: HohStorage }).localStorage ?? null; } catch { return null; }
}
/** A folder path as T3 compares them: no trailing separator. */
function folderPath(value: unknown): string {
  if (typeof value !== 'string') return '';
  const trimmed = value.trim();
  return trimmed.length > 1 ? trimmed.replace(/\/+$/, '') || '/' : trimmed;
}
const titleOf = (root: string) => root.split('/').filter(Boolean).at(-1) ?? root;
const timeOf = (value: unknown) => typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null;

const PAGE = 1000;
const NO_OP_COMMANDS = new Set([
  'thread.visit', 'thread.mark-unread', 'thread.settle', 'thread.unsettle', 'thread.auto-settle', 'thread.auto-settle.set',
  'thread.pin', 'thread.unpin', 'thread.pin.reorder', 'thread.active.reorder', 'thread.tag.set', 'thread.snooze', 'thread.unsnooze',
  'thread.interaction-mode.set',
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
  private stopped = false;
  private readonly threads = new Map<string, ThreadModel>();
  /** Rows restored before the engine answers; its list/history replaces their cached metadata once available. */
  private readonly restored = new Set<string>();
  /** Histories already peeked successfully during startup; a cached row is retried until its log is readable. */
  private readonly peeked = new Set<string>();
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
  private discovery: ClaudeDiscovery | null = null;
  private mode: HohAccessMode = 'confirm';
  private accessKnown = false;
  /** T3's "default permissions" setting: the mode a new thread starts in. Claude asks first until the person changes it. */
  private defaultMode: HohAccessMode = 'confirm';
  /** The model the engine runs new chats with (`hoh/executor/select`), once known. */
  private selectedModel: string | null = null;
  /** A startup access/get captured before a person's later choice must not overwrite that particular field. */
  private modeRevision = 0;
  private modelRevision = 0;
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
  /** The skills Claude declared for each folder (its last workspace snapshot): `$name` in a message goes as `/name`. */
  private readonly skillsByFolder = new Map<string, Set<string>>();
  /** The reasoning level each chat was given (T3's model options): kept across Stop, reopening and restarts of the window. */
  private readonly levels = new Map<string, { id: string; value: unknown }[]>();
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

  constructor(options: HohHostOptions) {
    this.engine = options.engine;
    this.now = options.now ?? Date.now;
    this.throttleMs = options.throttleMs ?? 40;
    this.loadRetryMs = options.loadRetryMs ?? 500;
    this.loadRetries = options.loadRetries ?? 60;
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
    for (const [threadId, value] of Object.entries(saved?.homeDispositions && typeof saved.homeDispositions === 'object' ? saved.homeDispositions : {})) {
      const entry = value as Record<string, unknown> | null;
      const turnOrdinal = Number(entry?.turnOrdinal);
      if (Number.isInteger(turnOrdinal) && turnOrdinal > 0 && (entry?.state === 'waiting' || entry?.state === 'done'))
        this.homeDispositions.set(threadId, { turnOrdinal, state: entry.state });
    }
    for (const [threadId, value] of Object.entries(saved?.homeObservedAt && typeof saved.homeObservedAt === 'object' ? saved.homeObservedAt : {})) {
      const observedAt = Number(value);
      if (Number.isFinite(observedAt) && observedAt > 0) this.homeObservedAt.set(threadId, observedAt);
    }
    if (folderPath(saved?.defaultFolder)) this.workspaceRoot = folderPath(saved.defaultFolder);
    if (saved?.defaultMode === 'full' || saved?.defaultMode === 'confirm') this.defaultMode = saved.defaultMode;
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
      const model = typeof entry?.model === 'string' && entry.model.trim() ? entry.model.trim() : this.model();
      const thread = new ThreadModel({ id, title, createdAt, updatedAt, model, mode: this.mode, projectId });
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
        id: thread.id, title: shell.title, projectId: thread.projectId, model: thread.model,
        createdAt: thread.createdAt, updatedAt: thread.updatedAt, archivedAt: thread.archivedAt,
      }];
    });
    const value = {
      version: 1, projects: [...this.folders.values()], threads: Object.fromEntries(this.homes),
      ...(this.workspaceRoot !== PLACEHOLDER_ROOT ? { defaultFolder: this.workspaceRoot } : {}),
      defaultMode: this.defaultMode, options: Object.fromEntries(this.levels), homeDispositions: Object.fromEntries(this.homeDispositions),
      homeObservedAt: Object.fromEntries(this.homeObservedAt), sessions,
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
    if (!entry || entry.state !== 'done') return entry;
    return disposition?.turnOrdinal === entry.turnOrdinal && disposition.state === 'done'
      ? entry
      : { ...entry, state: 'waiting', canComplete: true };
  }

  /** Follow the latest turn without changing T3's completed run: only Home waits for human sign-off. */
  private syncHomeDisposition(thread: ThreadModel): boolean {
    const entry = thread.home();
    if (!entry) return false;
    const current = this.homeDispositions.get(thread.id);
    if (current && current.turnOrdinal !== entry.turnOrdinal) {
      if (this.homePrompts.has(thread.id) || (this.failedHomePrompts.has(thread.id) && entry.state === 'ended')) return false;
      this.homeDispositions.delete(thread.id);
      this.failedHomePrompts.delete(thread.id);
      if (entry.state !== 'done') return true;
    }
    const matched = this.homeDispositions.get(thread.id);
    if (entry.state === 'done' && (!matched || matched.turnOrdinal !== entry.turnOrdinal)) {
      this.homeDispositions.set(thread.id, { turnOrdinal: entry.turnOrdinal, state: 'waiting' });
      return true;
    }
    return false;
  }

  readonly home = {
    version: () => this.homeVersion,
    subscribe: (listener: () => void) => { this.homeListeners.add(listener); return () => { this.homeListeners.delete(listener); }; },
    entries: (): HomeEntry[] => [...this.threads.values()].filter(thread => thread.archivedAt === null).map(thread => this.homeEntry(thread)).filter((entry): entry is HomeEntry => entry !== null),
    projectTitle: (projectId: string) => projectId === HOH_PROJECT_ID ? 'No project' : this.folders.get(projectId)?.title ?? 'No project',
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
      if (!thread || !entry || entry.state !== 'done') return;
      this.homeDispositions.set(threadId, { turnOrdinal: entry.turnOrdinal, state: 'done' });
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
  /** The engine's choice for new chats when T3 lists it; otherwise (Claude Code's `default`) T3's default model. */
  private model() { return listedClaudeModel(this.discovery, this.selectedModel) ?? defaultClaudeModel(this.discovery); }

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
  }

  private async load() {
    this.unsubscribers.push(await this.engine.subscribe(notification => this.onNotification(notification)));
    if (this.engine.subscribeQuestions) this.unsubscribers.push(await this.engine.subscribeQuestions(question => this.onQuestion(question)));
    if (this.engine.subscribeStatus) this.unsubscribers.push(await this.engine.subscribeStatus(status => {
      if (status.state === 'restarting' || status.state === 'unavailable' || status.state === 'stopped') this.onEngineLost();
    }));
    // Claude's probe and settings are independent of the shell. In particular, a slow CLI health check must not
    // keep T3 from showing the window, the composer, or session metadata that the engine already has.
    void this.loadConfiguration().then(discovered => { if (!discovered) void this.retryConfiguration(); });
    const sessions = this.loadSessions();
    // Preserve the single-snapshot fast path when session/list answers immediately, without letting a slow or
    // wedged bridge own startup: after one event-loop turn T3 receives the explicit pending placeholder instead.
    await Promise.race([sessions, new Promise<void>(resolve => setTimeout(resolve, 0))]);
    // The first config, lifecycle and empty shell snapshots are usable immediately. Both engine reads reconcile
    // them in the background, so even a stuck session/list cannot hold the T3 window behind a bridge timeout.
  }

  /** Claude's probe and access settings, announced whenever that independent startup lane completes. */
  private async loadConfiguration(): Promise<boolean> {
    await Promise.all([this.readDiscovery(), this.readAccess()]);
    this.configChanged();
    return this.discovery !== null;
  }

  private async retryConfiguration() {
    for (let attempt = 0; attempt < this.loadRetries && !this.stopped && this.discovery === null; attempt++) {
      await new Promise<void>(resolve => { const timer: any = setTimeout(resolve, this.loadRetryMs); timer?.unref?.(); });
      if (this.stopped) return;
      await this.loadConfiguration();
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

  private async readDiscovery() {
    try { this.discovery = await this.engine.request('hoh/executor/discover', { executor: 'claude' }); } catch { this.discovery = null; }
  }

  private async readAccess() {
    const modeRevision = this.modeRevision;
    const modelRevision = this.modelRevision;
    try {
      const access = await this.engine.request('hoh/executor/access/get', { executor: 'claude' });
      if (modeRevision === this.modeRevision && (access?.mode === 'full' || access?.mode === 'confirm')) {
        const changed = this.mode !== access.mode;
        this.mode = access.mode;
        this.accessKnown = true;
        if (changed) for (const thread of this.threads.values()) { thread.mode = access.mode; this.changed(thread.id); }
      }
      if (modelRevision === this.modelRevision && typeof access?.selected?.model === 'string' && access.selected.model)
        this.selectedModel = access.selected.model;
    } catch { /* the engine has no Claude access setting: the default mode stays */ }
  }

  private async readSessions(): Promise<boolean> {
    let sessions: any[] = [];
    try { sessions = (await this.engine.request('session/list', {}))?.sessions ?? []; } catch { return false; }
    let inventoryChanged = false;
    for (const session of sessions) {
      if (typeof session?.id !== 'string' || !session.id) continue;
      const at = Number(session.updatedAt) || this.startedAt;
      const existing = this.threads.get(session.id);
      if (existing) {
        if (this.restored.delete(session.id)) {
          // The cache is only continuity for the first snapshot. Once this session is visible on
          // disk, let its current list entry and history replace stale presentation metadata.
          existing.title = typeof session.title === 'string' ? session.title : '';
          existing.updatedAt = Math.max(existing.updatedAt, at);
          inventoryChanged = true;
          if (this.sessionsKnown && this.shellSinks.size) this.changed(session.id);
        }
        continue;
      }
      this.threads.set(session.id, new ThreadModel({ id: session.id, title: typeof session.title === 'string' ? session.title : '', createdAt: at, updatedAt: at, model: this.model(), mode: this.mode, projectId: this.homeOf(session.id) }));
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
        try { if ((await this.engine.request('hoh/executor/access/get', { executor: 'claude', sessionId: thread.id }))?.session?.running) thread.setRunning(true, this.now()); } catch { /* unknown: the history decides */ }
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
          const own = (await this.engine.request('hoh/executor/access/get', { executor: 'claude', sessionId: threadId }))?.session;
          if (own?.executor === 'claude' && typeof own.model === 'string' && own.model) thread.model = own.model;
        } catch { /* the chat follows the choice for new chats */ }
        thread.loaded = true;
        // The list row learns what the history knows (a title from the first message, the last answer).
        this.changed(threadId);
        if (thread.cwd) this.placeByFolder(thread);
        else if (!this.workspaceKnown && thread.projectId === HOH_PROJECT_ID) {
          try { this.setWorkspaceRoot((await this.engine.request('session/working-directory/get', { sessionId: threadId }))?.cwd); } catch { /* the placeholder stays */ }
        }
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
      thread = new ThreadModel({ id: sessionId, createdAt: this.now(), model: this.model(), mode: this.mode });
      thread.loaded = true;
      this.threads.set(sessionId, thread);
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
        if (thread && params.event?.type === 'working-directory/change') this.placeByFolder(thread);
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
      if (!thread) continue;
      this.sequence++;
      for (const sink of this.shellSinks) sink.emit({ kind: 'thread.updated', sequence: this.sequence, location: thread.archivedAt === null ? 'active' : 'archive', thread: thread.shell() });
      if (thread.loaded) for (const sink of this.threadSinks.get(id) ?? []) sink.emit(this.threadSnapshot(thread));
    }
  }

  private threadSnapshot(thread: ThreadModel) {
    return { kind: 'snapshot', snapshotSequence: this.sequence, projection: thread.projection() };
  }

  private config() {
    return serverConfig({ cwd: this.workspaceRoot, discovery: this.discovery, defaultMode: this.defaultMode, settingsOverrides: this.settingsOverrides, now: this.iso() });
  }

  private settings() { return serverSettings(this.model(), this.defaultMode, this.settingsOverrides); }

  private configChanged() {
    for (const sink of this.configSinks) sink.emit({ version: 1, type: 'snapshot', config: this.config() });
  }

  private lifecycleWelcome(bootstrapStatus: 'pending' | 'complete') {
    const config = this.config();
    return { version: 1, sequence: 0, type: 'welcome', payload: { environment: config.environment, cwd: this.workspaceRoot, projectName: 'HoH', bootstrapStatus } };
  }

  private shellSnapshot() {
    const threads = [...this.threads.values()].sort((a, b) => b.updatedAt - a.updatedAt);
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
  private async createThread(threadId: string, title: string | undefined, model: string | undefined, mode: T3RuntimeMode | undefined, projectId?: unknown) {
    const folder = typeof projectId === 'string' && projectId !== HOH_PROJECT_ID ? this.folders.get(projectId) : undefined;
    if (typeof projectId === 'string' && projectId !== HOH_PROJECT_ID && !folder) throw new Error('This project does not exist.');
    await this.applyMode(mode);
    await this.applyModel(model);
    const directory = await this.engine.request('session/working-directory/get', { sessionId: threadId });
    this.setWorkspaceRoot(directory?.cwd);
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
    const thread = new ThreadModel({ id: threadId, title, createdAt: this.now(), model: model ?? this.model(), mode: this.mode, projectId: folder?.id ?? HOH_PROJECT_ID });
    thread.cwd = folder ? folder.workspaceRoot : folderPath(directory?.cwd) || null;
    thread.loaded = true;
    this.threads.set(threadId, thread);
    this.homes.set(threadId, thread.projectId);
    this.save();
    if (!folder) this.showProject();
    this.changed(threadId);
    return thread;
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
   * else the default setting this host reports; useComposerSend.ts). The engine's mode is one for all
   * chats, so every thread shows the mode in effect.
   */
  private async applyMode(mode: T3RuntimeMode | undefined) {
    if (!mode) return;
    const wanted = accessModeOf(mode);
    this.modeRevision++;
    if (wanted === this.mode && this.accessKnown) return;
    await this.engine.request('hoh/executor/access/set', { executor: 'claude', mode: wanted });
    this.mode = wanted; this.accessKnown = true;
    for (const thread of this.threads.values()) { thread.mode = wanted; this.changed(thread.id); }
    this.configChanged();
  }

  private async applyModel(model: string | undefined) {
    if (!model) return;
    this.modelRevision++;
    if (model === this.selectedModel) return;
    await this.engine.request('hoh/executor/select', { executor: 'claude', model });
    this.selectedModel = model;
  }

  /**
   * One message to the engine: its pictures first (the bytes kept from T3's upload or `assets.persistChatAttachments`; the engine's
   * attachment store checks and copies them), then its text, with the skills Claude declared for the chat's folder
   * written as Claude runs them (`$name` → `/name`, T3's `planClaudeSkillDispatch`).
   */
  private async send(thread: ThreadModel, messageId: string, text: string, attachments: unknown = []) {
    const images = this.keptImages(attachments);
    const prompt = skillsAsCommands(text, this.skillsByFolder.get(folderPath(thread.cwd)) ?? new Set());
    this.homePrompts.add(thread.id);
    thread.sendLocal(messageId, text, this.now(), images.map(({ data: _data, ...shown }) => ({ type: 'image' as const, ...shown })), prompt);
    this.changed(thread.id);
    const contentBlocks = [
      ...images.map(image => ({ type: 'image', data: image.data, mimeType: image.mimeType, name: image.name })),
      ...(prompt || !images.length ? [{ type: 'text', text: prompt }] : []),
    ];
    try {
      const result = await this.engine.request('session/prompt', { sessionId: thread.id, contentBlocks, ...effortParam(effortOf(thread.model, thread.options)) });
      this.homePrompts.delete(thread.id);
      this.failedHomePrompts.delete(thread.id);
      if (this.homeDispositions.delete(thread.id)) this.save();
      for (const image of images) this.uploads.delete(image.id);
      if (typeof result?.messageId === 'string') thread.receipt(result.messageId, messageId);
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
  private async workspaceSnapshot(cwd: string) {
    try {
      const noProject = !this.folderAt(cwd) && folderPath(cwd) === this.workspaceRoot && !this.workspaceKnown;
      const found = await this.engine.request('hoh/executor/commands', noProject ? { executor: 'claude' } : { executor: 'claude', cwd });
      const snapshot = workspaceSnapshot(cwd, this.iso(), found);
      const skills = new Set(snapshot.skills.map(skill => skill.name));
      for (const folder of [folderPath(cwd), folderPath(found?.cwd)]) if (folder) this.skillsByFolder.set(folder, skills);
      if (noProject) this.setWorkspaceRoot(found?.cwd);
      return snapshot;
    } catch {
      // Claude could not be asked here (not installed, signed out, a folder with no conversation yet): T3 asks again later.
      return { cwd, checkedAt: this.iso(), slashCommands: [], skills: [], slashCommandsPending: true };
    }
  }

  /** «Стоп»: the same cancel as the current window; the engine ends Claude and its commands. */
  private async cancel(thread: ThreadModel) {
    if (thread.activeRunId()) thread.stopRequested = true;
    await this.engine.request('session/cancel', { sessionId: thread.id });
    // The turn is stopped now: its run closes and the thread list stops saying "Working" without waiting for the engine's closing event.
    if (thread.interrupt(this.now())) { this.changed(thread.id); this.flush(); }
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
          if (!thread) await this.createThread(command.threadId, command.title, command.modelSelection?.model, command.runtimeMode, command.projectId);
          break;
        case 'message.dispatch': {
          const target = needThread();
          if (command.modelSelection?.model) { await this.applyModel(command.modelSelection.model); target.model = command.modelSelection.model; }
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
        case 'thread.runtime-mode.set': needThread(); await this.applyMode(command.runtimeMode); break;
        case 'thread.model-selection.set': {
          const target = needThread();
          await this.applyModel(command.modelSelection?.model);
          if (command.modelSelection?.model) target.model = command.modelSelection.model;
          // The reasoning level of T3's model menu: kept for the chat, shown by the chip, sent with each message.
          if (command.modelSelection) this.setLevel(target, command.modelSelection.options, true);
          this.changed(target.id);
          break;
        }
        case 'thread.metadata.update': {
          const target = needThread();
          if (typeof command.title === 'string' && command.title.trim()) { target.title = command.title.trim(); this.changed(target.id); }
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
    await this.start();
    const threadId = typeof input.threadId === 'string' && input.threadId ? input.threadId : 'thread-' + (globalThis.crypto?.randomUUID?.() ?? this.now().toString(36));
    const existing = await this.loaded(threadId);
    try {
      const thread = existing ?? await this.createThread(threadId, input.title, input.modelSelection?.model, input.runtimeMode, input.projectId);
      if (existing) await this.applyMode(input.runtimeMode);
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
          if (patch.defaultModelSelection?.model) await this.applyModel(patch.defaultModelSelection.model);
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
        'assets.createUrl': (payload: any) => {
          const resource = payload?.resource;
          if (resource?._tag !== 'attachment') throw new RpcFailure({ _tag: 'EnvironmentAuthorizationError', message: 'Not available in HoH.', requiredScope: 'orchestration:read' });
          const id = String(resource.attachmentId ?? '');
          if (!this.uploads.has(id) || !UPLOAD_PATH.test(`/api/attachments/upload/${id}`)) throw new RpcFailure({ _tag: 'AssetAttachmentNotFoundError', resource });
          return { relativeUrl: `/api/assets/attachment/${id}`, expiresAt: this.now() + UPLOAD_TTL_MS };
        },
        'attachments.delete': (payload: any) => {
          const id = String(payload?.attachmentId ?? '');
          this.slots.delete(id); this.uploads.delete(id);
          return null;
        },
        'filesystem.browse': (payload: any) => this.browse(payload ?? {}),
        // The welcome page looks for other agents' history to import; HoH imports none, and T3 then
        // leaves its Projects section out.
        'agentSessions.scan': () => ({ candidates: [], scannedAt: this.iso() }),
        'server.refreshProviders': async (payload: any) => {
          await this.start();
          // The composer's folder (T3's workspace snapshot): `/` and `$` from Claude, without asking for discovery again.
          const cwd = typeof payload?.cwd === 'string' ? payload.cwd.trim() : '';
          if (cwd && (payload?.instanceId === undefined || payload.instanceId === CLAUDE_INSTANCE_ID)) {
            if (this.discovery === null) await this.readDiscovery();
            return { providers: [{ ...claudeProvider(this.discovery, this.iso()), workspaceSnapshots: [await this.workspaceSnapshot(cwd)] }] };
          }
          await this.readDiscovery();
          this.configChanged();
          return { providers: [claudeProvider(this.discovery, this.iso())] };
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
          const own = sinks;
          void this.start().then(() => this.loaded(threadId)).then(thread => {
            if (!open) return;
            // A thread that does not exist yet (a draft) gets its first snapshot when it is created.
            if (thread) sink.emit(this.threadSnapshot(thread));
            if (thread && payload?.requestCompletionMarker) sink.emit({ kind: 'synchronized' });
            own.add(sink);
          }, error => { if (open) sink.fail(new RpcFailure({ _tag: 'OrchestrationV2GetThreadProjectionError', threadId, message: errorText(error) })); });
          return () => { open = false; own.delete(sink); };
        },
      },
    };
  }

  /** For tests and diagnostics. */
  thread(id: string) { return this.threads.get(id); }
  get projectId() { return HOH_PROJECT_ID; }
}
