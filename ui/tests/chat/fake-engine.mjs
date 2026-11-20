// A small in-process engine with several chats, speaking the same methods the window uses
// (session/list, session/resume, session/working-directory/get, session/events, session/prompt).
// It models what the real engine does for chats: a session id the engine has never seen is
// created by session/working-directory/get, history is per session, titles come from the first
// user message. It has no models or tools; a prompt is answered with a fixed echo.
export class FakeEngine {
  constructor({ unavailable = [] } = {}) {
    this.sessions = new Map(); this.listeners = new Set(); this.requests = []; this.clock = Date.now() - 3_600_000;
    this.unavailable = new Set(unavailable); this.counter = 0; this.pending = new Set();
  }
  tick() { return this.clock += 60_000; }
  /** A chat that already exists with a finished exchange. */
  seed(id, { title, user, assistant }) {
    const session = this.open(id, title);
    if (user) this.append(session, 'user/message', { id: `${id}-u`, source: { kind: 'user' }, content: [{ type: 'text', text: user }] });
    if (assistant) this.append(session, 'assistant/message', { message: { id: `${id}-a`, content: [{ type: 'text', text: assistant }] } });
    return session;
  }
  open(id, title = '') {
    if (!this.sessions.has(id)) this.sessions.set(id, { id, title, events: [], updatedAt: this.tick() });
    return this.sessions.get(id);
  }
  append(session, type, data) {
    const event = { seq: session.events.length, time: this.tick(), type, data };
    session.events.push(event); session.updatedAt = event.time;
    return event;
  }
  emit(method, params) { for (const listener of this.listeners) listener({ method, params }); }
  subscribe(listener) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  titleOf(session) {
    if (session.title) return session.title;
    const first = session.events.find(e => e.type === 'user/message');
    return first ? first.data.content.map(b => b.text ?? '').join('').split('\n')[0].slice(0, 60) : '';
  }
  async request(method, params) {
    this.requests.push({ method, params });
    if (this.unavailable.has(method)) throw new Error('method-unavailable');
    switch (method) {
      case 'hoh/employee/open': case 'hoh/security/summary': throw new Error('method-unavailable');
      case 'hoh/settings/get': return { model: { baseUrl: this.baseUrl ?? 'http://localhost:8000/v1', model: 'local' }, security: { internet: 'ask', domains: [], folders: [], writeFiles: false, programs: false, programNetwork: [], actAsUser: false } };
      case 'session/resume': if (!this.sessions.has(params.sessionId)) throw new Error('session not found'); return {};
      case 'session/working-directory/get': this.open(params.sessionId); return { cwd: this.cwd ?? '/workspace/chat' };
      case 'session/events': {
        const session = this.sessions.get(params.sessionId); if (!session) throw new Error('unknown SDK session');
        const after = params.afterSeq ?? -1;
        const events = session.events.filter(e => e.seq > after).slice(0, params.limit);
        return { events, nextSeq: events.at(-1)?.seq ?? after };
      }
      case 'session/list': return { sessions: [...this.sessions.values()].sort((a, b) => b.updatedAt - a.updatedAt)
        .map(s => ({ id: s.id, title: this.titleOf(s), employeeId: 'employee-1', updatedAt: s.updatedAt })) };
      case 'session/prompt': {
        const session = this.sessions.get(params.sessionId); if (!session) throw new Error('unknown SDK session');
        const text = params.contentBlocks.map(b => b.text ?? '').join(''); const messageId = `m-${++this.counter}`;
        this.pending.add(new Promise(resolve => setTimeout(() => { this.answer(session, messageId, text); resolve(); }, 20)));
        return { messageId };
      }
      case 'session/working-directory/set': this.cwd = params.path; return {};
      case 'session/cancel': return {};
      default: throw new Error(`FakeEngine: unsupported ${method}`);
    }
  }
  /** Delivers the whole turn as live events of that session, whichever chat the window shows. */
  answer(session, messageId, text) {
    const send = (type, data) => this.emit('session.event', { sessionId: session.id, event: this.append(session, type, data) });
    this.emit('session.status', { sessionId: session.id, status: 'running' });
    send('user/message', { id: messageId, source: { kind: 'user' }, content: [{ type: 'text', text }] });
    send('turn/start', {});
    send('assistant/message', { message: { id: `a-${messageId}`, content: [{ type: 'text', text: `Ответ на: ${text}` }] } });
    send('turn/end', { reason: { kind: 'completed' } });
    this.emit('session.status', { sessionId: session.id, status: 'idle' });
  }
  async close() { await Promise.all(this.pending); this.listeners.clear(); }
}
