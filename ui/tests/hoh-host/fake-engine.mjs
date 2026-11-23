// A scriptable HoH engine for the host's tests: the methods and notifications of the real one
// (ui/src/screens/chat/transport.ts), with per-session history that survives a "restart"
// (a new FakeEngine over the same `store`). Tests drive the turn themselves: `emitEvent`,
// `emitFrame`, `askApproval`, `askQuestion`.
export const HOME = '/home/person';
export const DEFAULT_FOLDER = '/home/person/Documents/HoH';
const FOLDERS = [HOME, '/home/person/Documents', DEFAULT_FOLDER, '/home/person/Projects', '/home/person/Projects/site', '/home/person/Projects/tools', '/home/person/.config'];

export class FakeEngine {
  constructor({ store = new Map(), discovery, codexDiscovery, access = { mode: 'confirm', selected: { executor: 'claude', model: 'claude-sonnet' } }, folders = FOLDERS } = {}) {
    /** Folders on the fake disk; `refuseFolder` makes the person press «Отклонить» in the protection window. */
    this.folders = new Set(folders); this.refuseFolder = false;
    /** Folders added as projects (`hoh/workspace/add`). */
    this.projectFolders = new Set();
    this.store = store; // sessionId -> { title, events: [], updatedAt }
    this.listeners = new Set(); this.questionListeners = new Set(); this.statusListeners = new Set();
    this.requests = []; this.decisions = []; this.answers = []; this.counter = 0; this.clock = 1_700_000_000_000;
    this.open = new Set();
    this.discovery = discovery ?? { executor: 'claude', installed: true, version: '2.1.0', versionStatus: 'supported', auth: 'authenticated',
      models: [{ id: 'claude-sonnet', displayName: 'Claude Sonnet', isDefault: true }, { id: 'claude-opus', displayName: 'Claude Opus' }], detail: null };
    this.codexDiscovery = codexDiscovery ?? { executor: 'codex', installed: true, version: '0.166.0', versionStatus: 'supported', auth: 'authenticated',
      models: [{ id: 'gpt-6-sol', displayName: 'GPT-6 Sol', isDefault: true, defaultEffort: 'medium', effortLevels: [{ id: 'low' }, { id: 'medium' }, { id: 'high' }] }], detail: null };
    this.access = access;
    this.sessionAccess = new Map();
  }
  tick() { return this.clock += 1000; }
  seed(id, title, events) {
    const session = { title, events: [], updatedAt: this.tick() };
    this.store.set(id, session);
    for (const [type, data] of events) this.persist(id, type, data);
    return session;
  }
  persist(sessionId, type, data) {
    const session = this.store.get(sessionId);
    const event = { seq: session.events.length, time: this.tick(), type, data };
    session.events.push(event); session.updatedAt = event.time;
    return event;
  }
  /** A durable event, as the engine stores it and announces it. */
  emitEvent(sessionId, type, data) {
    const event = this.persist(sessionId, type, data);
    this.notify('session.event', { sessionId, event });
    return event;
  }
  emitFrame(sessionId, frame) { this.notify('session.assistant-stream', { sessionId, frame }); }
  emitStatus(sessionId, status) { this.notify('session.status', { sessionId, status }); }
  askApproval(sessionId, approvalId, humanText) { this.notify('approval.pending', { approvalId, sessionId, risk: 'unassessed', humanText, expiresAt: this.clock + 60_000 }); }
  askQuestion(question) { for (const listener of this.questionListeners) listener(question); }
  notify(method, params) { for (const listener of this.listeners) listener({ method, params }); }
  async subscribe(listener) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  async subscribeQuestions(listener) { this.questionListeners.add(listener); return () => this.questionListeners.delete(listener); }
  async subscribeStatus(listener) { this.statusListeners.add(listener); return () => this.statusListeners.delete(listener); }
  setStatus(state) { for (const listener of this.statusListeners) listener({ state }); }
  async answerQuestion(requestId, answer) { this.answers.push({ requestId, answer }); }
  async decideApproval(approvalId, outcome) {
    this.decisions.push({ approvalId, outcome });
  }
  /** The Tauri host's `browse_folders`, as T3's server browses (apps/server WorkspaceEntries.browse). */
  async browseFolders({ partialPath, cwd }) {
    this.requests.push({ method: 'browse_folders', params: { partialPath, cwd } });
    const path = partialPath === '~' ? HOME : partialPath.startsWith('~/') ? HOME + partialPath.slice(1) : partialPath.startsWith('/') ? partialPath : (cwd ?? '') + '/' + partialPath;
    const ends = partialPath.endsWith('/') || partialPath === '~';
    const parent = ends ? path.replace(/\/+$/, '') || '/' : path.slice(0, path.lastIndexOf('/')) || '/';
    const prefix = ends ? '' : path.slice(path.lastIndexOf('/') + 1);
    if (!this.folders.has(parent)) throw new Error(`No such folder: ${parent}`);
    const entries = [...this.folders].filter(folder => folder.startsWith(parent === '/' ? '/' : parent + '/') && !folder.slice(parent.length + 1).includes('/'))
      .map(folder => ({ name: folder.slice(folder.lastIndexOf('/') + 1), fullPath: folder }))
      .filter(entry => entry.name.toLowerCase().startsWith(prefix.toLowerCase()) && (ends || prefix.startsWith('.') || !entry.name.startsWith('.')))
      .sort((a, b) => a.name.localeCompare(b.name));
    return { parentPath: parent, entries };
  }
  async request(method, params) {
    this.requests.push({ method, params });
    switch (method) {
      case 'hoh/executor/discover': return params.executor === 'codex' ? this.codexDiscovery : this.discovery;
      case 'hoh/executor/access/get': {
        const answer = { executor: params.executor, mode: this.access.mode, runtimeMode: this.access.mode, effective: { mode: this.access.mode, flags: [] }, selected: this.access.selected };
        const session = params.sessionId ? this.store.get(params.sessionId) : undefined;
        const chosen = params.sessionId ? (this.sessionAccess.get(params.sessionId) ?? session?.runtimeMode) : undefined;
        return session?.executor || session?.model ? { ...answer, session: { executor: session.executor ?? 'claude', model: session.model, privacy: 'remote', mode: chosen === 'full' ? 'full' : 'confirm', runtimeMode: chosen ?? 'confirm', running: null, runningRuntimeMode: null, pending: false } } : answer;
      }
      case 'hoh/executor/access/set':
        if (params.sessionId) {
          const chosen = params.runtimeMode ?? params.mode;
          this.sessionAccess.set(params.sessionId, chosen);
          if (this.store.has(params.sessionId)) this.store.get(params.sessionId).runtimeMode = chosen;
        }
        else this.access = { ...this.access, mode: params.runtimeMode ?? params.mode };
        return {};
      case 'hoh/executor/select': this.access = { ...this.access, selected: { executor: params.executor, model: params.model } }; return {};
      case 'session/list': return { sessions: [...this.store.entries()].map(([id, s]) => ({ id, title: s.title, employeeId: 'employee-1', updatedAt: s.updatedAt })) };
      case 'session/resume': if (!this.store.has(params.sessionId)) throw new Error('session not found'); this.open.add(params.sessionId); return {};
      case 'session/working-directory/get':
        if (!this.store.has(params.sessionId)) this.store.set(params.sessionId, { title: '', events: [], updatedAt: this.tick() });
        this.open.add(params.sessionId);
        return { cwd: this.store.get(params.sessionId).cwd ?? DEFAULT_FOLDER };
      // As the Tauri host and the engine answer it: the person allows the folder, the engine records it.
      case 'session/working-directory/set': {
        if (this.refuseFolder) throw { code: 'rejected', message: 'Действие отклонено' };
        if (!this.store.has(params.sessionId)) this.store.set(params.sessionId, { title: '', events: [], updatedAt: this.tick() });
        // The engine's file system resolves `~` (crates/hoh-fs-local) and answers the folder it set.
        const path = params.path === '~' || params.path.startsWith('~/') ? HOME + params.path.slice(1) : params.path;
        if (!this.folders.has(path)) throw { code: 'refused', message: 'Помощник отклонил запрос' };
        this.store.get(params.sessionId).cwd = path;
        this.emitEvent(params.sessionId, 'working-directory/change', { cwd: path });
        return { cwd: path };
      }
      case 'session/events': {
        const session = this.store.get(params.sessionId); if (!session) throw new Error('unknown SDK session');
        const after = params.afterSeq ?? -1;
        const events = session.events.filter(e => e.seq > after).slice(0, params.limit);
        return { events, nextSeq: events.at(-1)?.seq ?? after };
      }
      case 'session/prompt': {
        if (!this.store.has(params.sessionId)) throw new Error('unknown SDK session');
        const session = this.store.get(params.sessionId);
        session.executor ??= this.access.selected.executor;
        session.model ??= this.access.selected.model;
        const text = params.contentBlocks.map(block => block.text ?? '').join('');
        const messageId = `engine-m-${++this.counter}`;
        // After answering, the engine starts the turn and then records its message (session.v4 order).
        setTimeout(() => {
          this.emitEvent(params.sessionId, 'turn/start', { turn: 1 });
          this.emitEvent(params.sessionId, 'step/start', { turn: 1, step: 1 });
          this.emitEvent(params.sessionId, 'user/message', { id: messageId, source: { kind: 'user' }, content: [{ type: 'text', text }] });
        }, 0);
        return { messageId };
      }
      case 'session/cancel': this.cancelled = (this.cancelled ?? 0) + 1; return {};
      // HOH-41: the person's "Add project" (confirmed by that action in the main window): the engine resolves `~`
      // and keeps the folder as a project folder.
      case 'hoh/workspace/add': {
        if (this.refuseFolder) throw { code: 'rejected', message: 'Действие отклонено' };
        const path = params.cwd === '~' || params.cwd.startsWith('~/') ? HOME + params.cwd.slice(1) : params.cwd;
        if (!this.folders.has(path)) throw new Error(`the folder "${params.cwd}" cannot be opened: No such file or directory`);
        this.projectFolders.add(path);
        return { cwd: path };
      }
      // HOH-41: as the engine answers them, only for a folder it knows (its default one, a project's or a conversation's).
      case 'hoh/workspace/entries': case 'hoh/executor/commands': {
        const known = new Set([DEFAULT_FOLDER, ...this.projectFolders, ...[...this.store.values()].map(session => session.cwd).filter(Boolean)]);
        // `defaultIsData`: the engine runs in its own data folder (the desktop) and names no default folder.
        if (params.cwd === undefined && this.defaultIsData) throw new Error('"/home/person/.local/share/hoh/engine" holds the data of HoH');
        const cwd = params.cwd ?? DEFAULT_FOLDER;
        if (!known.has(cwd)) throw new Error(`"${cwd}" is not a folder this engine knows`);
        if (method === 'hoh/executor/commands') {
          if (this.commandsFail) throw new Error('Claude Code is not installed');
          return { executor: 'claude', cwd, commands: this.commands ?? [], skills: this.skills ?? [] };
        }
        const entries = (this.files ?? []).filter(entry => entry.path.includes(params.query));
        return { entries: entries.slice(0, params.limit), truncated: entries.length > params.limit };
      }
      default: throw new Error('method-unavailable');
    }
  }
}
