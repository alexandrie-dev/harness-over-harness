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

/** The engine's placeholder name for the model on this computer reads as words. */
function modelLabel(name, baseUrl) {
  const clean = windowText(name, '', '');
  if (clean.toLowerCase() !== 'local') return clean;
  if (isLocalAddress(baseUrl)) return 'Локальная модель';
  // The engine's default name «local» on a model reached over the network: name the machine that really answers.
  try { return new URL(baseUrl).host; } catch { return clean; }
}
/** Why the window could not reach the assistant, in words, and what to do. The host's own Russian refusals are passed on. */
function connectionProblem(error) {
  const message = (error?.message ?? (typeof error === 'string' ? error : '')).trim();
  const reason = message === 'Помощник ещё подключается' ? 'Помощник ещё запускается.'
    : /[А-Яа-яЁё]/.test(message) && message.length < 160 ? message
    : message === METHOD_UNAVAILABLE ? 'Движок не знает нужного запроса: окно и движок разных версий.'
    : 'Движок не отвечает.';
  return `Не удалось связаться с Помощником. ${reason} Подождите немного и нажмите «Подключиться ещё раз»; если не помогает, закройте HoH и откройте снова.`;
}
export class ChatController {
  constructor(transportFactory, changed = () => {}, frame = () => new Promise(resolve => requestAnimationFrame(resolve))) {
    this.transportFactory = transportFactory; this.changed = changed; this.frame = frame;
    this.projection = new ChatProjection(); this.sessionId = ''; this.unsubscribe = undefined;
    this.epoch = 0; this.generation = 0; this.queue = Promise.resolve(); this.received = new Set();
    this.ignoreUntilNextTurn = false; this.allowNextTurn = false; this.cancelUnconfirmed = false;
    this.statusOff = undefined; this.statusPending = undefined; this.lifecycle = 0;
    this.active = false; this.reloading = false;
    this.state = { entries: [], connection: 'connecting', busy: false, canceling: false, error: '', privacy: 'unknown', folderLabel: 'Рабочая папка', model: '', permissions: '', notice: '', folderPath: '', draft: '', questions: [],
      sessions: [], currentId: '', working: [] };
    this.questionsOff = undefined;
    // `link` counts real connections of the window to the engine; opening another chat keeps the same link.
    this.link = 0; this.chosen = ''; this.listSeq = 0; this.workingIds = new Set();
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
  /** Chats of the engine for the sidebar. A missing method leaves just the open chat. */
  async refreshSessions() {
    const transport = this.transport;
    if (!transport || !this.active) return;
    const seq = ++this.listSeq;
    let listed = [];
    try { listed = (await transport.request('session/list', {})).sessions ?? []; } catch { /* the open chat is still shown */ }
    if (seq !== this.listSeq || !this.active) return;
    const rows = listed.map(item => {
      const raw = typeof item.title === 'string' ? item.title.trim() : '';
      return { id: item.id, title: raw ? windowText(raw, this.projection.folder ?? '', '') || 'Разговор' : '', updatedAt: Number(item.updatedAt) || 0 };
    });
    if (this.sessionId && !rows.some(row => row.id === this.sessionId)) rows.unshift({ id: this.sessionId, title: '', updatedAt: 0 });
    this.update({ sessions: rows });
  }
  trackWorking(params) {
    if (typeof params?.sessionId !== 'string') return;
    if (params.status === 'running') this.workingIds.add(params.sessionId); else this.workingIds.delete(params.sessionId);
    this.update({ working: [...this.workingIds] });
    if (params.status !== 'running') void this.refreshSessions();
  }
  /** Open an existing chat: its history is read again from the engine, nothing is shared between chats. */
  async openSession(id) {
    if (!id || id === this.sessionId || !this.active || this.state.connection === 'connecting') return;
    this.update({ notice: '' });
    await this.connect(false, id);
  }
  /** A chat that was never written to is reused instead of piling up empty ones. */
  async newChat() {
    if (!this.active || this.state.connection === 'connecting') return;
    if (this.sessionId && this.state.connection === 'ready' && this.projection.entries.length === 0) return;
    const id = globalThis.crypto?.randomUUID ? `chat-${globalThis.crypto.randomUUID()}` : `chat-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
    this.update({ notice: '' });
    await this.connect(false, id);
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
      if (event.type === 'turn/end') void this.refreshSessions();
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
    if (notification.method === 'session.status') this.trackWorking(notification.params);
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
  async listenQuestions(transport, ownLink) {
    this.questionsOff?.(); this.questionsOff = undefined;
    if (!transport.subscribeQuestions) return;
    const off = await transport.subscribeQuestions(question => {
      if (ownLink !== this.link || !this.active) return;
      if (this.state.questions.some(q => q.requestId === question.requestId)) return;
      this.update({ questions: [...this.state.questions, question] });
    });
    if (ownLink !== this.link) off(); else this.questionsOff = off;
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
  async connect(recovering = false, target = '') {
    this.flushReceived(); this.publish(); this.active = true; this.reloading = true;
    const ownEpoch = ++this.epoch; const loadGeneration = ++this.generation;
    this.queue = Promise.resolve(); this.ignoreUntilNextTurn = false; this.allowNextTurn = false; this.cancelUnconfirmed = false;
    this.unsubscribe?.(); this.unsubscribe = undefined;
    const projection = new ChatProjection(); this.sessionId = '';
    const ownLink = target ? this.link : ++this.link;
    // A question belongs to its chat and survives switching; a new link to the engine drops them.
    this.update({ ...(target ? { entries: [], currentId: target } : { questions: [] }), connection: recovering ? 'recovering' : 'connecting', busy: false, canceling: false,
      error: recovering ? 'Связь с Помощником прервалась, восстанавливаю…' : '' });
    const buffered = []; let loading = true;
    try {
      const transport = target && this.transport ? this.transport : this.transportFactory(); this.transport = transport;
      await this.listenStatus(transport);
      if (ownEpoch !== this.epoch) return;
      if (!target) await this.listenQuestions(transport, ownLink);
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
      // The chat the person chose stays chosen across a reconnect; the first start opens the default one.
      const wanted = target || this.chosen;
      const opened = wanted ? { sessionId: wanted }
        : await unlessMissing(() => transport.request('hoh/employee/open', { id: 'employee-1' }), () => ({ sessionId: WINDOW_SESSION }));
      if (ownEpoch !== this.epoch) return;
      this.sessionId = opened.sessionId; this.chosen = opened.sessionId;
      this.update({ currentId: this.sessionId });
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
      this.update({ folderPath: directory.cwd, folderLabel: windowText(name === 'файл' ? 'Рабочая папка' : name, '', 'Рабочая папка') });
      await this.readHistory(transport, projection, () => ownEpoch === this.epoch && loadGeneration === this.generation);
      if (ownEpoch !== this.epoch || loadGeneration !== this.generation) return;
      const summary = await unlessMissing(() => transport.request('hoh/security/summary', {}), async () => {
        const settings = await transport.request('hoh/settings/get', {});
        return { model: { local: isLocalAddress(settings.model.baseUrl) } };
      });
      if (ownEpoch !== this.epoch) return;
      await this.readSettings(transport, ownEpoch);
      if (ownEpoch !== this.epoch) return;
      // A new engine cannot still be executing the previous process's turn.
      // History may end mid-turn if termination prevented its closing event.
      if (recovering) projection.stop();
      this.projection = projection; this.publish(); this.reloading = false;
      this.update({ privacy: summary.model.local ? 'local' : 'remote', connection: 'ready', error: '' });
      void this.refreshSessions();
      loading = false;
      for (const event of buffered) this.enqueue(event);
    } catch (problem) {
      if (ownEpoch !== this.epoch) return;
      this.reloading = false;
      this.unsubscribe?.(); this.unsubscribe = undefined;
      this.update({ connection: 'error', busy: false, error: connectionProblem(problem) });
    }
  }
  /** Model name and permission mode for the composer line; absent data leaves the field empty (and hidden). */
  async readSettings(transport, ownEpoch) {
    try {
      const settings = await transport.request('hoh/settings/get', {});
      if (ownEpoch !== this.epoch) return;
      const security = settings.security;
      const permissions = security ? (security.writeFiles || security.programs ? 'Запись и программы с разрешения' : 'Только чтение') : '';
      this.update({ model: typeof settings.model?.model === 'string' ? modelLabel(settings.model.model, settings.model.baseUrl) : '', permissions });
    } catch { /* the line simply shows less */ }
  }
  /** A link of an answer: the host opens it, or says why it will not. Nothing opens without the host's own checks. */
  async openLink(kind, target) {
    const open = this.transport?.openLink;
    if (!open) { this.update({ notice: 'Открывать ссылки можно только в приложении HoH.' }); return; }
    this.update({ notice: '' });
    try { await open.call(this.transport, kind, target, this.sessionId); } catch (problem) {
      const message = (problem?.message ?? (typeof problem === 'string' ? problem : '')).trim();
      this.update({ notice: /[А-Яа-яЁё]/.test(message) ? message : 'Не удалось открыть.' });
    }
  }
  /** A link to a file outside the chat folder: nothing is opened, the person is told why and what to do. */
  refuseLink() {
    const name = this.state.folderLabel;
    this.update({ notice: `Этот файл вне папки «${name}»: Помощник и окно работают только внутри неё. Чтобы открыть его, выберите другую папку внизу поля ввода.` });
  }
  /** A link that is neither an address nor a file in the folder: nothing is opened, and the person is told so. */
  blockedLink() {
    this.update({ notice: 'Эта ссылка не открывается: открываются только адреса http и https и файлы в рабочей папке.' });
  }
  dismissNotice() { this.update({ notice: '' }); }
  /** Another working folder for the open chat (the path comes from the system folder dialog). */
  async setFolder(path) {
    if (!path || !this.transport || this.state.connection !== 'ready' || this.state.busy) return;
    const ownEpoch = this.epoch;
    try {
      await this.transport.request('session/working-directory/set', { sessionId: this.sessionId, path });
      if (ownEpoch !== this.epoch) return;
      this.projection.folder = path;
      const normalized = path.replaceAll('\\', '/').replace(/\/$/, '');
      const name = humanFile(normalized, normalized.slice(0, normalized.lastIndexOf('/')));
      this.update({ folderPath: path, folderLabel: windowText(name === 'файл' ? 'Рабочая папка' : name, '', 'Рабочая папка'), error: '' });
    } catch {
      if (ownEpoch === this.epoch) this.update({ error: 'Не удалось сменить папку. Попробуйте ещё раз.' });
    }
  }
  disconnect() {
    this.flushReceived(); this.active = false; this.reloading = false;
    this.epoch++; this.generation++; this.unsubscribe?.(); this.unsubscribe = undefined;
    this.link++; this.questionsOff?.(); this.questionsOff = undefined;
    this.statusOff?.(); this.statusOff = undefined;
    this.lifecycle++; this.statusPending = undefined;
    this.queue = Promise.resolve();
  }
  async send() {
    const value = this.state.draft;
    if (!value.trim() || this.state.connection !== 'ready' || this.state.busy || this.state.canceling) return;
    const ownEpoch = this.epoch; const sendGeneration = this.generation;
    this.allowNextTurn = true; this.state.notice && this.update({ notice: '' });
    const localId = this.projection.optimistic(value);
    this.projection.setRunning(true); this.publish(); this.update({ draft: '', error: '' });
    try {
      const result = await this.transport.request('session/prompt', { sessionId: this.sessionId, contentBlocks: [{ type: 'text', text: value }] });
      if (ownEpoch !== this.epoch) return;
      this.projection.receipt(localId, result.messageId); this.publish();
      void this.refreshSessions();
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
