// A scriptable HoH engine for the host's tests: the methods and notifications of the real one
// (ui/src/screens/chat/transport.ts), with per-session history that survives a "restart"
// (a new FakeEngine over the same `store`). Tests drive the turn themselves: `emitEvent`,
// `emitFrame`, `askApproval`, `askQuestion`.
export class FakeEngine {
  constructor({ store = new Map(), discovery, access = { mode: 'confirm', selected: { executor: 'claude', model: 'claude-sonnet' } } } = {}) {
    this.store = store; // sessionId -> { title, events: [], updatedAt }
    this.listeners = new Set(); this.questionListeners = new Set(); this.statusListeners = new Set();
    this.requests = []; this.decisions = []; this.answers = []; this.counter = 0; this.clock = 1_700_000_000_000;
    this.open = new Set();
    this.discovery = discovery ?? { executor: 'claude', installed: true, version: '2.1.0', versionStatus: 'supported', auth: 'authenticated',
      models: [{ id: 'claude-sonnet', displayName: 'Claude Sonnet', isDefault: true }, { id: 'claude-opus', displayName: 'Claude Opus' }], detail: null };
    this.access = access;
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
  async request(method, params) {
    this.requests.push({ method, params });
    switch (method) {
      case 'hoh/executor/discover': return this.discovery;
      case 'hoh/executor/access/get': return { executor: 'claude', mode: this.access.mode, effective: { mode: this.access.mode, flags: [] }, selected: this.access.selected };
      case 'hoh/executor/access/set': this.access = { ...this.access, mode: params.mode }; return {};
      case 'hoh/executor/select': this.access = { ...this.access, selected: { executor: params.executor, model: params.model } }; return {};
      case 'session/list': return { sessions: [...this.store.entries()].map(([id, s]) => ({ id, title: s.title, employeeId: 'employee-1', updatedAt: s.updatedAt })) };
      case 'session/resume': if (!this.store.has(params.sessionId)) throw new Error('session not found'); this.open.add(params.sessionId); return {};
      case 'session/working-directory/get':
        if (!this.store.has(params.sessionId)) this.store.set(params.sessionId, { title: '', events: [], updatedAt: this.tick() });
        this.open.add(params.sessionId);
        return { cwd: '/home/person/Documents/HoH' };
      case 'session/events': {
        const session = this.store.get(params.sessionId); if (!session) throw new Error('unknown SDK session');
        const after = params.afterSeq ?? -1;
        const events = session.events.filter(e => e.seq > after).slice(0, params.limit);
        return { events, nextSeq: events.at(-1)?.seq ?? after };
      }
      case 'session/prompt': {
        if (!this.store.has(params.sessionId)) throw new Error('unknown SDK session');
        const text = params.contentBlocks.map(block => block.text ?? '').join('');
        const messageId = `engine-m-${++this.counter}`;
        // The engine records the message and starts the turn after answering.
        setTimeout(() => {
          this.emitEvent(params.sessionId, 'user/message', { id: messageId, source: { kind: 'user' }, content: [{ type: 'text', text }] });
          this.emitEvent(params.sessionId, 'turn/start', { turn: 1 });
        }, 0);
        return { messageId };
      }
      case 'session/cancel': this.cancelled = (this.cancelled ?? 0) + 1; return {};
      default: throw new Error('method-unavailable');
    }
  }
}
