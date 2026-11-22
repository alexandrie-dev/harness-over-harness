// The HoH host: T3's server methods (WsRpcGroup) answered by the HoH engine.
// Handshake: server.getConfig / subscribeServerConfig / probe / getSettings / updateSettings.
// Chats: orchestration.subscribeShell (the list), subscribeThread / getThreadProjection (one chat),
// launchThread and dispatchCommand (new chat, send, Stop, permission and question answers, mode, model),
// forceStopThread. A thread change is sent as a fresh snapshot of that thread, which T3's client applies
// whole (client-runtime/src/state/threads.ts `applyItemLocked`), plus `thread.updated` for the list.
import {
  accessModeOf, claudeProvider, defaultClaudeModel, serverConfig, serverSettings, type ClaudeDiscovery, type HohAccessMode, type T3RuntimeMode,
} from './config.ts';
import type { EngineNotification, EngineQuestion, EngineQuestionAnswer, HohEngine } from './engine.ts';
import { HOH_PROJECT_ID, hohProject, ThreadModel } from './threads.ts';
import { RpcFailure, type Handlers, type StreamSink } from './wire.ts';

export interface HohHostOptions {
  readonly engine: HohEngine;
  readonly now?: () => number;
  /** Folder shown as the project root until the engine names one. */
  readonly workspaceRoot?: string;
  /** Delay that groups a burst of changes (stream chunks) into one snapshot. */
  readonly throttleMs?: number;
}

const PAGE = 1000;
const NO_OP_COMMANDS = new Set([
  'thread.visit', 'thread.mark-unread', 'thread.settle', 'thread.unsettle', 'thread.auto-settle', 'thread.auto-settle.set',
  'thread.pin', 'thread.unpin', 'thread.pin.reorder', 'thread.active.reorder', 'thread.tag.set', 'thread.snooze', 'thread.unsnooze',
  'thread.interaction-mode.set',
]);

function dispatchError(command: Record<string, any>, message: string) {
  return new RpcFailure({
    _tag: 'OrchestrationV2DispatchCommandError', commandId: String(command.commandId ?? 'unknown'), commandType: String(command.type ?? 'unknown'), message,
  });
}
const errorText = (error: unknown) => error instanceof Error ? error.message : typeof error === 'object' && error && 'message' in error ? String((error as any).message) : String(error);

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
  private readonly threads = new Map<string, ThreadModel>();
  private readonly loading = new Map<string, Promise<ThreadModel | undefined>>();
  private readonly questions = new Map<string, EngineQuestion>();
  private readonly shellSinks = new Set<StreamSink>();
  private readonly threadSinks = new Map<string, Set<StreamSink>>();
  private readonly configSinks = new Set<StreamSink>();
  private readonly dirty = new Set<string>();
  private flushTimer: ReturnType<typeof setTimeout> | undefined;
  private sequence = 0;
  private discovery: ClaudeDiscovery | null = null;
  private mode: HohAccessMode = 'confirm';
  /** The model the engine runs new chats with (`hoh/executor/select`), once known. */
  private selectedModel: string | null = null;
  private settingsOverrides: Record<string, unknown> = {};
  private workspaceRoot: string;
  private workspaceKnown = false;
  private readonly startedAt: number;
  private started: Promise<void> | undefined;
  private unsubscribers: (() => void)[] = [];

  constructor(options: HohHostOptions) {
    this.engine = options.engine;
    this.now = options.now ?? Date.now;
    this.throttleMs = options.throttleMs ?? 40;
    this.workspaceRoot = options.workspaceRoot ?? '~/Documents/HoH';
    this.startedAt = this.now();
  }

  private iso() { return new Date(this.now()).toISOString(); }
  private model() { return this.selectedModel ?? defaultClaudeModel(this.discovery); }

  /** Listen to the engine and read what it already has. Safe to call more than once. */
  start(): Promise<void> {
    this.started ??= this.load();
    return this.started;
  }

  stop() {
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
    await Promise.all([this.readDiscovery(), this.readAccess(), this.readSessions()]);
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

  private async readSessions() {
    let sessions: any[] = [];
    try { sessions = (await this.engine.request('session/list', {}))?.sessions ?? []; } catch { return; }
    for (const session of sessions) {
      if (typeof session?.id !== 'string' || !session.id || this.threads.has(session.id)) continue;
      const at = Number(session.updatedAt) || this.startedAt;
      this.threads.set(session.id, new ThreadModel({ id: session.id, title: typeof session.title === 'string' ? session.title : '', createdAt: at, updatedAt: at, model: this.model(), mode: this.mode }));
    }
  }

  /** The chat's history, read once from the engine (after a restart too: the engine keeps it on disk). */
  private async loaded(threadId: string): Promise<ThreadModel | undefined> {
    const thread = this.threads.get(threadId);
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
        thread.loaded = true;
        if (!this.workspaceKnown) {
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
    }
    return thread;
  }

  private onNotification(notification: EngineNotification) {
    const params = notification.params ?? {};
    switch (notification.method) {
      case 'session.event': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.applyEvent(params.event)) this.changed(thread.id);
        return;
      }
      case 'session.assistant-stream': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.applyStreamFrame(params.frame ?? {}, this.now())) this.changed(thread.id);
        return;
      }
      case 'session.status': {
        const thread = this.threadFor(params.sessionId);
        if (thread?.setRunning(params.status === 'running', this.now())) this.changed(thread.id);
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
    if (this.flushTimer) return;
    this.flushTimer = setTimeout(() => this.flush(), this.throttleMs);
  }

  /** Send every pending change now (tests call it; the timer calls it otherwise). */
  flush() {
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.flushTimer = undefined;
    const ids = [...this.dirty]; this.dirty.clear();
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
    return serverConfig({ cwd: this.workspaceRoot, discovery: this.discovery, mode: this.mode, settingsOverrides: this.settingsOverrides, now: this.iso() });
  }

  private settings() { return serverSettings(this.model(), this.mode, this.settingsOverrides); }

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
        projects: [hohProject(this.workspaceRoot, this.startedAt)],
        threads: threads.filter(thread => thread.archivedAt === null).map(thread => thread.shell()),
        archivedThreads: threads.filter(thread => thread.archivedAt !== null).map(thread => thread.shell()),
      },
    };
  }

  // ---- engine actions ----

  /** A new chat in the engine: a session id it has not seen is created by asking for its folder. */
  private async createThread(threadId: string, title: string | undefined, model: string | undefined, mode: T3RuntimeMode | undefined) {
    await this.applyMode(mode);
    await this.applyModel(model);
    const directory = await this.engine.request('session/working-directory/get', { sessionId: threadId });
    this.setWorkspaceRoot(directory?.cwd);
    const thread = new ThreadModel({ id: threadId, title, createdAt: this.now(), model: model ?? this.model(), mode: this.mode });
    thread.loaded = true;
    this.threads.set(threadId, thread);
    this.changed(threadId);
    return thread;
  }

  /** The folder chats work in, as the engine names it: the project's root in T3. */
  private setWorkspaceRoot(cwd: unknown) {
    if (typeof cwd !== 'string' || !cwd.trim() || cwd === this.workspaceRoot) return;
    this.workspaceRoot = cwd;
    this.workspaceKnown = true;
    this.sequence++;
    for (const sink of this.shellSinks) sink.emit({ kind: 'project.updated', sequence: this.sequence, project: hohProject(this.workspaceRoot, this.startedAt) });
    this.configChanged();
  }

  /** The access mode is the person's choice in T3's composer; the engine applies it (and alone decides what a model may do). */
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
    thread.sendLocal(messageId, text, this.now());
    this.changed(thread.id);
    try {
      const result = await this.engine.request('session/prompt', { sessionId: thread.id, contentBlocks: [{ type: 'text', text }] });
      if (typeof result?.messageId === 'string') thread.receipt(result.messageId, messageId);
    } catch (error) {
      thread.sendFailed(messageId, this.now());
      this.changed(thread.id);
      throw error;
    }
  }

  /** «Стоп»: the same cancel as the current window; the engine ends Claude and its commands. */
  private async cancel(thread: ThreadModel) {
    await this.engine.request('session/cancel', { sessionId: thread.id });
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
          if (!thread) await this.createThread(command.threadId, command.title, command.modelSelection?.model, command.runtimeMode);
          break;
        case 'message.dispatch': {
          const target = needThread();
          if (command.modelSelection?.model) { await this.applyModel(command.modelSelection.model); target.model = command.modelSelection.model; }
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
        case 'thread.model-selection.set': {
          const target = needThread();
          await this.applyModel(command.modelSelection?.model);
          if (command.modelSelection?.model) target.model = command.modelSelection.model;
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
      const thread = existing ?? await this.createThread(threadId, input.title, input.modelSelection?.model, input.runtimeMode);
      if (existing) await this.applyMode(input.runtimeMode);
      const message = input.initialMessage;
      if (message && typeof message.text === 'string') {
        const messageId = typeof message.messageId === 'string' && message.messageId ? message.messageId : 'msg-' + String(input.commandId);
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
        'server.getConfig': async () => { await this.start(); return this.config(); },
        'server.getSettings': async () => { await this.start(); return this.settings(); },
        'server.updateSettings': async (payload: any) => {
          await this.start();
          const patch = payload?.patch && typeof payload.patch === 'object' ? { ...payload.patch } : {};
          // The mode is HoH's access mode; the model is the engine's choice for new chats.
          if (typeof patch.defaultRuntimeMode === 'string') await this.applyMode(patch.defaultRuntimeMode);
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
