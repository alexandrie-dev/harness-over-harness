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
import {
  accessModeOf, claudeProvider, defaultClaudeModel, effortOf, listedClaudeModel, serverConfig, serverSettings, type ClaudeDiscovery, type HohAccessMode, type T3InteractionMode, type T3RuntimeMode,
} from './config.ts';
import type { EngineNotification, EngineQuestion, EngineQuestionAnswer, HohEngine } from './engine.ts';
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

/** The reasoning level goes to the engine as `effort` of `session/prompt` (agreed with the engine's writer). */
const effortParam = (effort: string | null) => effort ? { effort } : {};
const interactionModeOf = (value: unknown): T3InteractionMode => value === 'plan' ? 'plan' : 'default';

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
  private readonly loading = new Map<string, Promise<ThreadModel | undefined>>();
  private readonly questions = new Map<string, EngineQuestion>();
  private readonly shellSinks = new Set<StreamSink>();
  private readonly threadSinks = new Map<string, Set<StreamSink>>();
  private readonly configSinks = new Set<StreamSink>();
  private readonly dirty = new Set<string>();
  private flushTimer: ReturnType<typeof setTimeout> | undefined;
  private sequence = 0;
  private homeDirty = false;
  private homeVersion = 0;
  private readonly homeListeners = new Set<() => void>();
  private discovery: ClaudeDiscovery | null = null;
  private mode: HohAccessMode = 'confirm';
  /** T3's "default permissions" setting: the mode a new thread starts in. Claude asks first until the person changes it. */
  private defaultMode: HohAccessMode = 'confirm';
  /** The model the engine runs new chats with (`hoh/executor/select`), once known. */
  private selectedModel: string | null = null;
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
  /** The reasoning level each chat was given (T3's model options): kept across Stop, reopening and restarts of the window. */
  private readonly levels = new Map<string, { id: string; value: unknown }[]>();
  /** T3's per-thread composer interaction mode. */
  private readonly interactions = new Map<string, T3InteractionMode>();
  /** Generated or manually edited titles that supersede the engine's first-message fallback. */
  private readonly titles = new Map<string, string>();
  private readonly titleRequests = new Map<string, string>();
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
    for (const [threadId, mode] of Object.entries(saved?.interactions && typeof saved.interactions === 'object' ? saved.interactions : {})) this.interactions.set(threadId, interactionModeOf(mode));
    for (const [threadId, title] of Object.entries(saved?.titles && typeof saved.titles === 'object' ? saved.titles : {})) if (typeof title === 'string' && title.trim()) this.titles.set(threadId, title.trim());
    if (folderPath(saved?.defaultFolder)) this.workspaceRoot = folderPath(saved.defaultFolder);
    if (saved?.defaultMode === 'full' || saved?.defaultMode === 'confirm') this.defaultMode = saved.defaultMode;
  }

  private save() {
    if (!this.storage) return;
    const value = {
      version: 1, projects: [...this.folders.values()], threads: Object.fromEntries(this.homes),
      ...(this.workspaceRoot !== PLACEHOLDER_ROOT ? { defaultFolder: this.workspaceRoot } : {}),
      defaultMode: this.defaultMode, options: Object.fromEntries(this.levels), interactions: Object.fromEntries(this.interactions), titles: Object.fromEntries(this.titles),
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
  private placeByFolder(thread: ThreadModel) {
    if (!thread.cwd) return;
    const folder = this.folderAt(thread.cwd);
    if (folder && thread.projectId !== folder.id) {
      thread.projectId = folder.id;
      this.homes.set(thread.id, folder.id);
      this.save();
      this.changed(thread.id);
    } else if (!folder && thread.projectId === HOH_PROJECT_ID && !this.workspaceKnown) {
      this.setWorkspaceRoot(thread.cwd);
    }
  }

  /**
   * The home screen's window into the host (HOH-41 В2; not T3's wire): what the chats are doing, who waits for the person,
   * and the two actions of that card. Read with `useSyncExternalStore`: `version` changes with every change of any chat.
   */
  readonly home = {
    version: () => this.homeVersion,
    subscribe: (listener: () => void) => { this.homeListeners.add(listener); return () => { this.homeListeners.delete(listener); }; },
    entries: (): HomeEntry[] => [...this.threads.values()].filter(thread => thread.archivedAt === null).map(thread => thread.home()).filter((entry): entry is HomeEntry => entry !== null),
    projectTitle: (projectId: string) => projectId === HOH_PROJECT_ID ? 'No project' : this.folders.get(projectId)?.title ?? 'No project',
    /** Approve or decline a waiting permission request, as the chat's buttons do. */
    answer: async (threadId: string, requestId: string, decision: 'accept' | 'decline') => {
      const thread = await this.loaded(threadId);
      if (!thread) return;
      await this.respond(thread, { requestId, decision });
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
    const [, , sessionsRead] = await Promise.all([this.readDiscovery(), this.readAccess(), this.readSessions()]);
    // The engine that was still starting answers nothing at first (and the window never asks again): go on asking in the background.
    if (this.discovery === null || !sessionsRead) void this.retryLoad(this.discovery !== null, sessionsRead);
  }

  /** The reads of `load` that failed, repeated until the engine answers; what arrives late is announced to the window. */
  private async retryLoad(discovered: boolean, sessionsRead: boolean) {
    for (let attempt = 0; attempt < this.loadRetries && !this.stopped && !(discovered && sessionsRead); attempt++) {
      await new Promise<void>(resolve => { const timer: any = setTimeout(resolve, this.loadRetryMs); timer?.unref?.(); });
      if (this.stopped) return;
      if (!discovered) { await this.readDiscovery(); discovered = this.discovery !== null; if (discovered) { await this.readAccess(); this.configChanged(); } }
      if (!sessionsRead) { sessionsRead = await this.readSessions(); if (sessionsRead) for (const id of this.threads.keys()) this.changed(id); }
    }
  }

  private async readDiscovery() {
    try { this.discovery = await this.engine.request('hoh/executor/discover', { executor: 'claude' }); } catch { this.discovery = null; }
  }

  private async readAccess() {
    try {
      const access = await this.engine.request('hoh/executor/access/get', { executor: 'claude' });
      if (access?.mode === 'full' || access?.mode === 'confirm') this.mode = access.mode;
      if (typeof access?.selected?.model === 'string' && access.selected.model) this.selectedModel = access.selected.model;
    } catch { /* the engine has no Claude access setting: the default mode stays */ }
  }

  private async readSessions(): Promise<boolean> {
    let sessions: any[] = [];
    try { sessions = (await this.engine.request('session/list', {}))?.sessions ?? []; } catch { return false; }
    for (const session of sessions) {
      if (typeof session?.id !== 'string' || !session.id || this.threads.has(session.id)) continue;
      const at = Number(session.updatedAt) || this.startedAt;
      this.threads.set(session.id, new ThreadModel({ id: session.id, title: this.titles.get(session.id) ?? (typeof session.title === 'string' ? session.title : ''), createdAt: at, updatedAt: at, model: this.model(), mode: this.mode, interactionMode: this.interactions.get(session.id), projectId: this.homeOf(session.id) }));
    }
    if ([...this.threads.values()].some(thread => thread.projectId === HOH_PROJECT_ID)) this.showProject();
    await this.peekToday();
    return true;
  }

  /**
   * The home screen lists today's chats, so their history is read at start: events only, the chat is not opened (no
   * `session/resume`) and not marked loaded. A turn the history leaves open ends as stopped until the engine says otherwise.
   */
  private async peekToday() {
    const today = new Date(this.now()).toDateString();
    const recent = [...this.threads.values()].filter(thread => !thread.loaded && new Date(thread.updatedAt).toDateString() === today)
      .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 12);
    await Promise.all(recent.map(async thread => {
      try {
        let afterSeq: number | undefined;
        for (;;) {
          const page = await this.engine.request('session/events', { sessionId: thread.id, limit: PAGE, ...(afterSeq === undefined ? {} : { afterSeq }) });
          const events: any[] = Array.isArray(page?.events) ? page.events : [];
          for (const event of events) thread.applyEvent(event);
          if (events.length < PAGE || (afterSeq !== undefined && page.nextSeq <= afterSeq)) break;
          afterSeq = page.nextSeq;
        }
        // The engine's own word on whether the chat is working (`access/get` of the chat), when it gives one.
        try { if ((await this.engine.request('hoh/executor/access/get', { executor: 'claude', sessionId: thread.id }))?.session?.running) thread.setRunning(true, this.now()); } catch { /* unknown: the history decides */ }
        thread.settleUnfinished(this.now());
        this.homeDirty = true;
      } catch { /* the chat is not on the home screen */ }
    }));
    this.homeVersion++;
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
      thread = new ThreadModel({ id: sessionId, title: this.titles.get(sessionId), createdAt: this.now(), model: this.model(), mode: this.mode, interactionMode: this.interactions.get(sessionId) });
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

  private changed(threadId: string) {
    this.dirty.add(threadId);
    this.homeDirty = true;
    if (this.flushTimer) return;
    this.flushTimer = setTimeout(() => this.flush(), this.throttleMs);
  }

  /** Send every pending change now (tests call it; the timer calls it otherwise). */
  flush() {
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.flushTimer = undefined;
    const ids = [...this.dirty]; this.dirty.clear();
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
  private async createThread(threadId: string, title: string | undefined, model: string | undefined, mode: T3RuntimeMode | undefined, projectId?: unknown, interactionMode?: unknown) {
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
    const interaction = interactionModeOf(interactionMode);
    const thread = new ThreadModel({ id: threadId, title, createdAt: this.now(), model: model ?? this.model(), mode: this.mode, interactionMode: interaction, projectId: folder?.id ?? HOH_PROJECT_ID });
    this.interactions.set(threadId, interaction);
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
        const at = this.now();
        const title = typeof mutation.title === 'string' && mutation.title.trim() ? mutation.title.trim() : titleOf(root);
        const project: FolderProject = { id: projectId, title, workspaceRoot: root, createdAt: at, updatedAt: at, pinnedAt: null, archivedAt: null };
        this.folders.set(projectId, project);
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
    if (wanted === this.mode) return;
    await this.engine.request('hoh/executor/access/set', { executor: 'claude', mode: wanted });
    this.mode = wanted;
    for (const thread of this.threads.values()) { thread.mode = wanted; this.changed(thread.id); }
    this.configChanged();
  }

  private async applyModel(model: string | undefined) {
    if (!model || model === this.selectedModel) return;
    await this.engine.request('hoh/executor/select', { executor: 'claude', model });
    this.selectedModel = model;
  }

  private async send(thread: ThreadModel, messageId: string, text: string) {
    const firstMessage = thread.userMessageCount() === 0;
    thread.sendLocal(messageId, text, this.now());
    this.changed(thread.id);
    try {
      const result = await this.engine.request('session/prompt', { sessionId: thread.id, contentBlocks: [{ type: 'text', text }], interactionMode: thread.interactionMode, ...effortParam(effortOf(thread.model, thread.options)) });
      if (typeof result?.messageId === 'string') thread.receipt(result.messageId, messageId);
      if (firstMessage) void this.generateTitle(thread, text);
    } catch (error) {
      thread.sendFailed(messageId, this.now(), error);
      this.changed(thread.id);
      throw error;
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
          if (!thread) await this.createThread(command.threadId, command.title, command.modelSelection?.model, command.runtimeMode, command.projectId, command.interactionMode);
          break;
        case 'message.dispatch': {
          const target = needThread();
          if (command.modelSelection?.model) { await this.applyModel(command.modelSelection.model); target.model = command.modelSelection.model; }
          this.setLevel(target, command.modelSelection?.options);
          await this.send(target, String(command.messageId), String(command.text ?? ''));
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
        case 'thread.interaction-mode.set': {
          const target = needThread();
          target.setInteractionMode(interactionModeOf(command.interactionMode));
          this.interactions.set(target.id, target.interactionMode);
          this.save(); this.changed(target.id);
          break;
        }
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
    await this.start();
    const threadId = typeof input.threadId === 'string' && input.threadId ? input.threadId : 'thread-' + (globalThis.crypto?.randomUUID?.() ?? this.now().toString(36));
    const existing = await this.loaded(threadId);
    try {
      const thread = existing ?? await this.createThread(threadId, input.title, input.modelSelection?.model, input.runtimeMode, input.projectId, input.interactionMode);
      if (existing) await this.applyMode(input.runtimeMode);
      if (existing && input.interactionMode !== undefined) {
        thread.setInteractionMode(interactionModeOf(input.interactionMode));
        this.interactions.set(thread.id, thread.interactionMode); this.save();
      }
      const message = input.initialMessage;
      if (message && typeof message.text === 'string') {
        const messageId = typeof message.messageId === 'string' && message.messageId ? message.messageId : 'msg-' + String(input.commandId);
        this.setLevel(thread, input.modelSelection?.options);
        await this.send(thread, messageId, message.text);
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
        'filesystem.browse': (payload: any) => this.browse(payload ?? {}),
        // The welcome page looks for other agents' history to import; HoH imports none, and T3 then
        // leaves its Projects section out.
        'agentSessions.scan': () => ({ candidates: [], scannedAt: this.iso() }),
        'server.refreshProviders': async () => {
          await this.start();
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
            const config = this.config();
            sink.emit({ version: 1, sequence: 0, type: 'welcome', payload: { environment: config.environment, cwd: this.workspaceRoot, projectName: 'HoH', bootstrapStatus: 'complete' } });
          });
          return () => { open = false; };
        },
        // Without an answer T3 assumes a repository and asks for a base branch before sending.
        subscribeVcsStatus: (_payload, sink) => { sink.emit({ _tag: 'snapshot', local: NOT_A_REPO_LOCAL, remote: null }); },
        'orchestration.subscribeShell': (payload: any, sink) => {
          let open = true;
          void this.start().then(() => {
            if (!open) return;
            sink.emit(this.shellSnapshot());
            if (payload?.requestCompletionMarker) sink.emit({ kind: 'synchronized' });
            this.shellSinks.add(sink);
          });
          return () => { open = false; this.shellSinks.delete(sink); };
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
