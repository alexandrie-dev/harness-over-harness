import { assistantChunks, ChatProjection, humanFile, windowText } from './projection.mjs';

// Observable behavior without React or a particular transport. The production
// frame scheduler is injectable so cancellation races can be tested deterministically.
const WINDOW_SESSION = 'window-main';
const METHOD_UNAVAILABLE = 'method-unavailable';
/** The native host marks only "engine has no such method"; every other failure stays a failure. */
async function unlessMissing(call, fallback) {
  try { return await call(); } catch (error) {
    if ((error?.message ?? String(error)) !== METHOD_UNAVAILABLE) throw error;
    return fallback();
  }
}
/** Model address on this computer (loopback or the local name) means data stays here. */
function isLocalAddress(baseUrl) {
  try {
    const host = new URL(baseUrl).hostname;
    return host === 'localhost' || host === '[::1]' || /^127(\.\d{1,3}){3}$/.test(host);
  } catch { return false; }
}

export class ChatController {
  constructor(transportFactory, changed = () => {}, frame = () => new Promise(resolve => requestAnimationFrame(resolve))) {
    this.transportFactory = transportFactory; this.changed = changed; this.frame = frame;
    this.projection = new ChatProjection(); this.sessionId = ''; this.unsubscribe = undefined;
    this.epoch = 0; this.generation = 0; this.queue = Promise.resolve(); this.received = new Set();
    this.ignoreUntilNextTurn = false; this.allowNextTurn = false; this.cancelUnconfirmed = false;
    this.statusOff = undefined; this.statusPending = undefined; this.lifecycle = 0;
    this.active = false; this.reloading = false;
    this.state = { entries: [], connection: 'connecting', busy: false, canceling: false, error: '', privacy: 'unknown', folderLabel: 'Рабочая папка', draft: '', questions: [] };
    this.questionsOff = undefined;
  }
  update(patch) { this.state = { ...this.state, ...patch }; this.changed(this.state); }
  publish() { this.update({ entries: this.projection.entries, busy: this.projection.running || this.cancelUnconfirmed }); }
  setDraft(draft) { this.update({ draft }); }
  async readHistory(transport, projection, current) {
    let afterSeq;
    for (;;) {
      const page = await transport.request('session/events', { sessionId: this.sessionId, limit: 1000, ...(afterSeq === undefined ? {} : { afterSeq }) });
      if (!current()) return false;
      for (const event of page.events) projection.event(event);
      if (page.events.length < 1000) return true;
      if (afterSeq !== undefined && page.nextSeq <= afterSeq) throw new Error('History did not advance');
      afterSeq = page.nextSeq;
    }
  }
  async consume(task, current) {
    const event = task.event;
    if (current !== this.generation || !this.projection.accept(event)) return;
    task.accepted = true;
    const recoverGap = event.type === 'turn/end' && this.projection.hasUnsettledGap();
    if (event.type === 'assistant/message' && !this.projection.hasLive(event)) {
      const chunks = assistantChunks(event); let value = '';
      for (let index = 0; index < chunks.length; index++) {
        if (current !== this.generation) return;
        value += chunks[index];
        this.projection.assistant(event, value, index < chunks.length - 1); this.publish();
        // V4 carries recorded chunks in data.stream, not as new RPC methods.
        // Reasoning/call chunks are excluded by assistantChunks.
        if (chunks.length > 1) await this.frame();
      }
    } else {
      this.projection.apply(event);
      this.publish();
      if (recoverGap) {
        try {
          await this.readHistory(this.transport, this.projection, () => current === this.generation && this.active);
        } catch { /* the explicit incomplete marker below is the safe fallback */ }
        if (current !== this.generation || !this.active) return;
        this.projection.finishGaps(); this.publish();
      }
      if (event.type === 'turn/end' && event.data?.reason?.kind === 'error')
        this.update({ error: 'Ответ прервался. Попробуйте ещё раз.' });
    }
  }
  flushReceived() {
    // Receipt is authoritative; animation is only presentation. Include the
    // currently animated message AND every event already waiting behind it.
    for (const task of this.received) {
      if (task.frame) this.projection.liveFrame(task.frame);
      else if (task.event) {
        if (task.accepted) this.projection.apply(task.event);
        else this.projection.event(task.event);
      } else this.projection.setRunning(task.running);
    }
    this.received.clear();
  }
  enqueue(notification) {
    const current = this.generation;
    let task;
    if (notification.method === 'session.event' && notification.params.sessionId === this.sessionId) {
      const event = notification.params.event;
      if (this.ignoreUntilNextTurn) {
        if (event.type !== 'turn/start' || !this.allowNextTurn || this.state.canceling) return;
        this.ignoreUntilNextTurn = false; this.allowNextTurn = false;
      }
      task = { event, accepted: false };
    } else if (notification.method === 'session.assistant-stream' && notification.params.sessionId === this.sessionId) {
      // late frames of an answer that was stopped must not bring its text back
      if (this.ignoreUntilNextTurn) return;
      task = { frame: notification.params.frame };
    } else if (notification.method === 'session.status' && notification.params.sessionId === this.sessionId) {
      if (this.ignoreUntilNextTurn) return;
      task = { running: notification.params.status === 'running' };
    }
    if (!task) return;
    this.received.add(task);
    this.queue = this.queue.then(async () => {
      if (current !== this.generation) return;
      if (task.frame) { this.projection.liveFrame(task.frame); this.publish(); }
      else if (task.event) await this.consume(task, current);
      else { this.projection.setRunning(task.running); this.publish(); }
    }).finally(() => this.received.delete(task)).catch(() => {
      if (current === this.generation) {
        this.projection.stop(); this.publish(); this.update({ error: 'Не удалось получить продолжение ответа. Попробуйте ещё раз.' });
      }
    });
  }
  engineStatus(status) {
    if (!this.active) return;
    if (status.state === 'ready') {
      if (this.state.connection !== 'ready' && !this.reloading) void this.connect(true);
      return;
    }
    this.flushReceived();
    this.epoch++; this.generation++; this.queue = Promise.resolve(); this.reloading = false;
    this.unsubscribe?.(); this.unsubscribe = undefined;
    this.cancelUnconfirmed = false; this.projection.stop(); this.publish();
    this.update({ questions: [], connection: 'recovering', busy: false, canceling: false,
      error: 'Связь с Помощником прервалась, восстанавливаю…' });
  }
  async listenStatus(transport) {
    if (this.statusOff || !transport.subscribeStatus) return;
    if (!this.statusPending) {
      const lifecycle = this.lifecycle;
      this.statusPending = transport.subscribeStatus(status => {
        if (lifecycle === this.lifecycle) this.engineStatus(status);
      }).then(off => {
        if (!this.active || lifecycle !== this.lifecycle) off();
        else this.statusOff = off;
      }).finally(() => { if (lifecycle === this.lifecycle) this.statusPending = undefined; });
    }
    await this.statusPending;
  }
  async listenQuestions(transport, ownEpoch) {
    this.questionsOff?.(); this.questionsOff = undefined;
    if (!transport.subscribeQuestions) return;
    const off = await transport.subscribeQuestions(question => {
      if (ownEpoch !== this.epoch || !this.active) return;
      if (this.state.questions.some(q => q.requestId === question.requestId)) return;
      this.update({ questions: [...this.state.questions, question] });
    });
    if (ownEpoch !== this.epoch) off(); else this.questionsOff = off;
  }
  dropQuestion(requestId) {
    if (this.state.questions.some(q => q.requestId === requestId))
      this.update({ questions: this.state.questions.filter(q => q.requestId !== requestId) });
  }
  /** `answer` is {answers:[{id,selected,custom?}]} or {cancelled:true}; the host checks it against the question. */
  async answerQuestion(requestId, answer) {
    const ownEpoch = this.epoch;
    try {
      await this.transport.answerQuestion(requestId, answer);
      if (ownEpoch === this.epoch) this.dropQuestion(requestId);
    } catch {
      if (ownEpoch === this.epoch) this.update({ error: 'Ответ не отправлен. Проверьте выбор и попробуйте ещё раз.' });
    }
  }
  dismissQuestion(requestId) { return this.answerQuestion(requestId, { cancelled: true }); }
  async connect(recovering = false) {
    this.flushReceived(); this.publish(); this.active = true; this.reloading = true;
    const ownEpoch = ++this.epoch; const loadGeneration = ++this.generation;
    this.queue = Promise.resolve(); this.ignoreUntilNextTurn = false; this.allowNextTurn = false; this.cancelUnconfirmed = false;
    this.unsubscribe?.(); this.unsubscribe = undefined;
    const projection = new ChatProjection(); this.sessionId = '';
    this.update({ questions: [], connection: recovering ? 'recovering' : 'connecting', busy: false, canceling: false,
      error: recovering ? 'Связь с Помощником прервалась, восстанавливаю…' : '' });
    const buffered = []; let loading = true;
    try {
      const transport = this.transportFactory(); this.transport = transport;
      await this.listenStatus(transport);
      if (ownEpoch !== this.epoch) return;
      await this.listenQuestions(transport, ownEpoch);
      if (ownEpoch !== this.epoch) return;
      const off = await transport.subscribe(event => {
        if (ownEpoch !== this.epoch) return;
        if (event.method === 'user-question.cancelled') { this.dropQuestion(event.params.requestId); return; }
        if (loading) buffered.push(event); else this.enqueue(event);
      });
      if (ownEpoch !== this.epoch) { off(); return; }
      this.unsubscribe = off;
      // The replay stub offers per-employee conversations; the window-profile engine has no employees
      // and keeps one conversation, so a missing method falls back to its fixed session id.
      // Nothing about employees is rendered by this screen.
      const opened = await unlessMissing(() => transport.request('hoh/employee/open', { id: 'employee-1' }), () => ({ sessionId: WINDOW_SESSION }));
      if (ownEpoch !== this.epoch) return;
      this.sessionId = opened.sessionId;
      // After a restart the conversation is on disk but not open in the new engine. Asking for its
      // working folder first would open it as a NEW one ("already exists") and the history would be
      // lost to the screen, so it is resumed first. A session that does not exist yet ("not found")
      // or is already open is fine: the next request creates or reuses it, and any real failure
      // surfaces there.
      try { await transport.request('session/resume', { sessionId: this.sessionId }); } catch { /* see above */ }
      if (ownEpoch !== this.epoch) return;
      const directory = await transport.request('session/working-directory/get', { sessionId: this.sessionId });
      if (ownEpoch !== this.epoch) return;
      projection.folder = directory.cwd;
      const normalized = directory.cwd.replaceAll('\\', '/').replace(/\/$/, '');
      const name = humanFile(normalized, normalized.slice(0, normalized.lastIndexOf('/')));
      this.update({ folderLabel: windowText(name === 'файл' ? 'Рабочая папка' : name, '', 'Рабочая папка') });
      await this.readHistory(transport, projection, () => ownEpoch === this.epoch && loadGeneration === this.generation);
      if (ownEpoch !== this.epoch || loadGeneration !== this.generation) return;
      const summary = await unlessMissing(() => transport.request('hoh/security/summary', {}), async () => {
        const settings = await transport.request('hoh/settings/get', {});
        return { model: { local: isLocalAddress(settings.model.baseUrl) } };
      });
      if (ownEpoch !== this.epoch) return;
      // A new engine cannot still be executing the previous process's turn.
      // History may end mid-turn if termination prevented its closing event.
      if (recovering) projection.stop();
      this.projection = projection; this.publish(); this.reloading = false;
      this.update({ privacy: summary.model.local ? 'local' : 'remote', connection: 'ready', error: '' });
      loading = false;
      for (const event of buffered) this.enqueue(event);
    } catch {
      if (ownEpoch !== this.epoch) return;
      this.reloading = false;
      this.unsubscribe?.(); this.unsubscribe = undefined;
      this.update({ connection: 'error', busy: false, error: 'Не удалось связаться с Помощником. Попробуйте подключиться ещё раз.' });
    }
  }
  disconnect() {
    this.flushReceived(); this.active = false; this.reloading = false;
    this.epoch++; this.generation++; this.unsubscribe?.(); this.unsubscribe = undefined;
    this.questionsOff?.(); this.questionsOff = undefined;
    this.statusOff?.(); this.statusOff = undefined;
    this.lifecycle++; this.statusPending = undefined;
    this.queue = Promise.resolve();
  }
  async send() {
    const value = this.state.draft;
    if (!value.trim() || this.state.connection !== 'ready' || this.state.busy || this.state.canceling) return;
    const ownEpoch = this.epoch; const sendGeneration = this.generation;
    this.allowNextTurn = true;
    const localId = this.projection.optimistic(value);
    this.projection.setRunning(true); this.publish(); this.update({ draft: '', error: '' });
    try {
      const result = await this.transport.request('session/prompt', { sessionId: this.sessionId, contentBlocks: [{ type: 'text', text: value }] });
      if (ownEpoch !== this.epoch) return;
      this.projection.receipt(localId, result.messageId); this.publish();
    } catch {
      if (ownEpoch !== this.epoch || sendGeneration !== this.generation) return;
      this.generation++; this.ignoreUntilNextTurn = true; this.allowNextTurn = false;
      this.projection.stop(); this.projection.failed(localId); this.publish();
      this.update({ error: 'Сообщение не отправлено. Проверьте связь и попробуйте ещё раз.', ...(this.state.draft ? {} : { draft: value }) });
    }
  }
  async stop() {
    if (!this.state.busy || this.state.canceling) return;
    const ownEpoch = this.epoch;
    this.flushReceived(); this.queue = Promise.resolve();
    const recoverGap = this.projection.hasGap();
    this.generation++; this.ignoreUntilNextTurn = true; this.allowNextTurn = false; this.cancelUnconfirmed = true;
    // Preserve a gapped row while cancellation is unconfirmed so a failed Stop
    // can be retried and a successful one can still settle from durable history.
    this.projection.stop(recoverGap); this.publish(); this.update({ canceling: true, error: '' });
    try {
      await this.transport.request('session/cancel', { sessionId: this.sessionId });
      if (ownEpoch !== this.epoch) return;
      if (recoverGap) {
        // Cancellation is acknowledged before the agent necessarily reaches idle.
        // Wait for its durable prefix/final message, then reconcile exactly as turn/end does.
        try { await this.transport.request('session/wait', { sessionId: this.sessionId }); } catch { /* history can still be available */ }
        if (ownEpoch !== this.epoch) return;
        try { await this.readHistory(this.transport, this.projection, () => ownEpoch === this.epoch && this.active); } catch { /* finish with the marker below */ }
        if (ownEpoch !== this.epoch) return;
        this.projection.finishGaps();
      }
      this.cancelUnconfirmed = false; this.publish(); this.update({ canceling: false });
    } catch {
      if (ownEpoch !== this.epoch) return;
      // Preserve Stop after a failed cancellation; a new prompt would be unsafe
      // while the host may still be working on the old one.
      this.update({ canceling: false, busy: true, error: 'Не удалось остановить ответ. Нажмите «Стоп» ещё раз.' });
    }
  }
}
