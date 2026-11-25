import { DEFAULT_ACCESS_MODE, discoverView, personalSettingsApply, projectSettingsIgnored } from '../../lib/executor.mjs';
import { errorText, hostError, hostWords, refusalWords } from '../../lib/hostError.mjs';
import { detectProvider } from '../../lib/providers.mjs';
import { numberRepeatedSteps, assistantChunks, ChatProjection, humanFile, windowText } from './projection.mjs';

// Observable behavior without React or a particular transport. The production
// frame scheduler is injectable so cancellation races can be tested deterministically.
const WINDOW_SESSION = 'window-main';
/** The native host marks only "engine has no such method"; every other failure stays a failure. */
async function unlessMissing(call, fallback) {
  try { return await call(); } catch (error) {
    if (hostError(error).code !== 'unavailable') throw error;
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

/** The folder as a person says it: the default one is «Документы → HoH», any other is its own name. */
function folderLabelOf(normalized, name) {
  if (/\/(Documents|Документы)\/HoH$/.test(normalized)) return 'Документы → HoH';
  return windowText(name === 'файл' ? 'Рабочая папка' : name, '', 'Рабочая папка');
}
/** Why the window could not reach the assistant, in words, and what to do. The host's own Russian refusals are passed on. */
function connectionProblem(error) {
  const { code } = hostError(error);
  const reason = code === 'connecting' ? 'Помощник ещё запускается.'
    : code === 'unavailable' ? 'Окно и Помощник из разных версий программы.'
    : hostWords(error) || 'Помощник не отвечает.';
  return `Не удалось связаться с Помощником. ${reason} Подождите немного и нажмите «Подключиться ещё раз»; если не помогает, закройте HoH и откройте снова.`;
}
export class ChatController {
  constructor(transportFactory, changed = () => {}, frame = () => new Promise(resolve => requestAnimationFrame(resolve))) {
    this.transportFactory = transportFactory; this.changed = changed; this.frame = frame;
    this.projection = new ChatProjection(); this.sessionId = ''; this.unsubscribe = undefined;
    this.epoch = 0; this.generation = 0; this.queue = Promise.resolve(); this.received = new Set();
    this.ignoreUntilNextTurn = false; this.allowNextTurn = false; this.cancelUnconfirmed = false;
    this.statusOff = undefined; this.statusPending = undefined; this.lifecycle = 0;
    this.active = false; this.reloading = false; this.accessSeq = 0; this.sessionPrivacy = '';
    this.state = { entries: [], connection: 'connecting', busy: false, canceling: false, error: '', privacy: 'unknown', folderLabel: 'Рабочая папка', model: '', modelIcon: '', modelScope: '', executor: '', executorModel: '', projectSettingsOff: false, personalSettings: false, accessPending: false, folderFixed: false, newExecutor: '', newExecutorModel: '', executorModels: [], executorBusy: false, permissions: '', notice: '', folderPath: '', draft: '', questions: [],
      sessions: [], currentId: '', working: [], openingChat: false };
    this.questionsOff = undefined;
    this.listRefresh = undefined;
    this.pendingPublish = undefined;
    this.historyCache = new Map();
    this.historySeq = -1;
    // `link` counts real connections of the window to the engine; opening another chat keeps the same link.
    this.link = 0; this.chosen = ''; this.listSeq = 0; this.workingIds = new Set();
  }
  update(patch) { this.state = { ...this.state, ...patch }; this.changed(this.state); }
  publish(patch = {}) {
    this.pendingPublish = undefined;
    const source = this.projection.entries;
    if (source !== this.publishedSource) {
      this.publishedSource = source;
      this.publishedEntries = numberRepeatedSteps(source);
    }
    this.update({ entries: this.publishedEntries, busy: this.projection.running || this.cancelUnconfirmed, ...patch });
  }
  publishStream() {
    if (typeof requestAnimationFrame !== 'function') { this.publish(); return; }
    if (this.pendingPublish) return;
    const pending = { generation: this.generation };
    this.pendingPublish = pending;
    requestAnimationFrame(() => {
      if (this.pendingPublish === pending && this.generation === pending.generation) this.publish();
    });
  }
  setDraft(draft) { this.update({ draft }); }
  async readHistory(transport, projection, current, afterSeq) {
    for (;;) {
      const page = await transport.request('session/events', { sessionId: this.sessionId, limit: 1000, ...(afterSeq === undefined ? {} : { afterSeq }) });
      if (!current()) return false;
      for (const event of page.events) projection.event(event);
      if (page.events.length < 1000) return page.nextSeq;
      if (afterSeq !== undefined && page.nextSeq <= afterSeq) throw new Error('History did not advance');
      afterSeq = page.nextSeq;
    }
  }
  async openHistory(transport, projection, current) {
    // Older hosts can reject the optional tail parameter. The forward path still works.
    let recent;
    const limit = 160;
    try { recent = await transport.request('session/events', { sessionId: this.sessionId, tail: true, limit }); } catch { /* use forward replay */ }
    if (!current()) return;
    if (!recent) return this.readHistory(transport, projection, current);

    const firstSeq = recent.events[0]?.seq;
    if (firstSeq === 0 || firstSeq === undefined) {
      for (const event of recent.events) projection.event(event);
      // Some legacy transports accept unknown keys but return the first page.
      if (recent.events.length === limit) return this.readHistory(transport, projection, current, recent.nextSeq);
      return recent.nextSeq;
    }
    const preview = new ChatProjection(projection.folder);
    // A completed turn is a clean boundary for presenting tool steps. If the
    // active turn alone fills the page, present just its available messages.
    const boundary = recent.events.findIndex(event => event.type === 'turn/end');
    for (const [index, event] of recent.events.entries()) {
      if (index > boundary && (boundary >= 0 || event.type === 'user/message' || event.type === 'assistant/message' || event.type === 'agent/inbox/spliced')) {
        preview.event(event);
      }
    }
    if (preview.entries.length) this.update({ entries: numberRepeatedSteps(preview.entries) });
    return this.readHistory(transport, projection, current);
  }
  cacheOpenChat() {
    if (!this.sessionId || this.state.connection !== 'ready' || this.projection.running ||
        this.cancelUnconfirmed || this.historySeq === undefined || this.projection.hasGap()) return;
    const entries = this.projection.entries;
    if (entries.length > 6000 || entries.some(entry =>
      entry.kind === 'note' || entry.pending || entry.failed || entry.incomplete || entry.streaming)) return;
    this.historyCache.set(this.sessionId, { projection: this.projection, afterSeq: this.historySeq });
    if (this.historyCache.size > 4) this.historyCache.delete(this.historyCache.keys().next().value);
  }
  scheduleSessionsRefresh() {
    if (this.listRefresh !== undefined) return;
    this.listRefresh = setTimeout(() => {
      this.listRefresh = undefined;
      void this.refreshSessions();
    }, 35);
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
      return { id: item.id, title: raw, updatedAt: Number(item.updatedAt) || 0 };
    });
    if (this.sessionId && !rows.some(row => row.id === this.sessionId)) rows.unshift({ id: this.sessionId, title: '', updatedAt: 0 });
    this.update({ sessions: rows });
  }
  trackWorking(params) {
    if (typeof params?.sessionId !== 'string') return;
    const had = this.workingIds.has(params.sessionId);
    const running = params.status === 'running';
    if (running !== had) {
      if (running) this.workingIds.add(params.sessionId); else this.workingIds.delete(params.sessionId);
      this.update({ working: [...this.workingIds] });
    }
    if (!running) this.scheduleSessionsRefresh();
  }
  /** Open an existing chat: its history is read again from the engine, nothing is shared between chats. */
  async openSession(id) {
    if (!id || id === this.state.currentId || !this.active || (this.state.connection === 'connecting' && !this.state.openingChat)) return;
    this.update({ notice: '', folderFixed: false });
    await this.connect(false, id);
  }
  /** A chat that was never written to is reused instead of piling up empty ones. */
  async newChat() {
    if (!this.active || this.state.connection === 'connecting') return;
    if (this.sessionId && this.state.connection === 'ready' && this.projection.entries.length === 0) return;
    // Another chat that was never written to already exists: go there instead of piling up empty ones.
    const empty = this.state.sessions.find(row => row.id !== this.sessionId && row.title === '');
    if (empty) { await this.openSession(empty.id); return; }
    const id = globalThis.crypto?.randomUUID ? `chat-${globalThis.crypto.randomUUID()}` : `chat-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
    this.update({ notice: '', folderFixed: false });
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
          await this.readHistory(this.transport, this.projection, () => current === this.generation && this.active, this.historySeq);
        } catch { /* the explicit incomplete marker below is the safe fallback */ }
        if (current !== this.generation || !this.active) return;
        this.projection.finishGaps(); this.publish();
      }
      if (event.type === 'turn/end') this.scheduleSessionsRefresh();
      // The mode of the next message and of the process may have changed: ask the engine again (start and end of every turn).
      if (event.type === 'turn/start' || event.type === 'turn/end') void this.readAccess(this.transport, this.epoch);
      if (event.type === 'turn/end' && event.data?.reason?.kind === 'error')
        this.update({ error: 'Ответ прервался. Попробуйте ещё раз.' });
    }
  }
  flushReceived() {
    // Receipt is authoritative; animation is only presentation. Include the
    // currently animated message AND every event already waiting behind it.
    for (const task of this.received) {
      if (task.frame) this.projection.liveFrame(task.frame);
      else if (task.approval) this.applyApproval(task.approval);
      else if (task.event) {
        if (task.accepted) this.projection.apply(task.event);
        else this.projection.event(task.event);
      } else this.projection.setRunning(task.running);
    }
    this.received.clear();
  }
  applyApproval(notification) {
    this.projection.waitForDecision(notification.method === 'approval.pending', notification.params.outcome === 'rejected');
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
    } else if ((notification.method === 'approval.pending' || notification.method === 'approval.resolved') && notification.params?.sessionId === this.sessionId) {
      task = { approval: notification };
    } else if (notification.method === 'session.status' && notification.params.sessionId === this.sessionId) {
      if (this.ignoreUntilNextTurn) return;
      task = { running: notification.params.status === 'running' };
    }
    if (!task) return;
    this.received.add(task);
    this.queue = this.queue.then(async () => {
      if (current !== this.generation) return;
      if (task.frame) { this.projection.liveFrame(task.frame); this.publishStream(); }
      else if (task.approval) { this.applyApproval(task.approval); this.publish(); }
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
    this.pendingPublish = undefined;
    this.historyCache.clear();
    this.unsubscribe?.(); this.unsubscribe = undefined;
    this.cancelUnconfirmed = false;
    if (!this.state.openingChat) { this.projection.stop(); this.publish(); }
    // The host gave up starting the engine: a final state with the reason (the button below tries again), not an endless «восстанавливаю».
    if (status.state === 'failed') {
      const reason = typeof status.message === 'string' && status.message.trim() ? status.message.trim().slice(0, 300) : 'Помощник не запускается.';
      this.update({ questions: [], connection: 'error', openingChat: false, busy: false, canceling: false, error: reason });
      return;
    }
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
    this.flushReceived();
    if (target && target !== this.sessionId) this.cacheOpenChat();
    if (!target) { this.historyCache.clear(); if (!this.state.openingChat) this.publish(); }
    const cached = this.historyCache.get(target);
    this.historyCache.delete(target);
    this.active = true; this.reloading = true;
    const ownEpoch = ++this.epoch; const loadGeneration = ++this.generation;
    this.pendingPublish = undefined;
    this.queue = Promise.resolve(); this.ignoreUntilNextTurn = false; this.allowNextTurn = false; this.cancelUnconfirmed = false;
    this.unsubscribe?.(); this.unsubscribe = undefined;
    let projection = new ChatProjection(); this.sessionId = '';
    if (target) this.projection = projection;
    const ownLink = target ? this.link : ++this.link;
    if (target) this.chosen = target;
    // A question belongs to its chat and survives switching; a new link to the engine drops them.
    this.update({ ...(target ? { entries: cached ? numberRepeatedSteps(cached.projection.entries.slice(-160)) : [], currentId: target } : { questions: [] }), openingChat: Boolean(target || this.state.openingChat), connection: recovering ? 'recovering' : 'connecting', busy: false, canceling: false,
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
      this.update({ currentId: this.sessionId, openingChat: true });
      if (!target && !recovering) void this.refreshSessions();
      // After a restart the conversation is on disk but not open in the new engine. Asking for its
      // working folder first would open it as a NEW one ("already exists") and the history would be
      // lost to the screen, so it is resumed first. A session that does not exist yet ("not found")
      // or is already open is fine: the next request creates or reuses it, and any real failure
      // surfaces there.
      try { await transport.request('session/resume', { sessionId: this.sessionId }); } catch { /* see above */ }
      if (ownEpoch !== this.epoch) return;
      const directory = await transport.request('session/working-directory/get', { sessionId: this.sessionId });
      if (ownEpoch !== this.epoch) return;
      const reusable = cached && cached.projection.folder === directory.cwd;
      if (reusable) projection = cached.projection;
      projection.folder = directory.cwd;
      const normalized = directory.cwd.replaceAll('\\', '/').replace(/\/$/, '');
      const name = humanFile(normalized, normalized.slice(0, normalized.lastIndexOf('/')));
      this.update({ folderPath: directory.cwd, folderLabel: folderLabelOf(normalized, name) });
      const stillLoading = () => ownEpoch === this.epoch && loadGeneration === this.generation;
      const [summary, historySeq] = await Promise.all([
        unlessMissing(() => transport.request('hoh/security/summary', {}), async () => {
          const settings = await transport.request('hoh/settings/get', {});
          return { model: { local: isLocalAddress(settings.model.baseUrl) } };
        }),
        reusable ? this.readHistory(transport, projection, stillLoading, cached.afterSeq) : this.openHistory(transport, projection, stillLoading),
        this.readSettings(transport, ownEpoch),
        this.readAccess(transport, ownEpoch),
      ]);
      if (!stillLoading()) return;
      // A new engine cannot still be executing the previous process's turn.
      // History may end mid-turn if termination prevented its closing event.
      if (recovering) projection.stop();
      this.projection = projection; this.historySeq = historySeq; this.reloading = false;
      this.basePrivacy = summary.model.local ? 'local' : 'remote';
      this.publish({ privacy: this.privacyNow(this.state.executor === 'claude') || this.basePrivacy, connection: 'ready', openingChat: false, error: '' });
      // Listing chats reads their logs on the engine. A switch between known chats
      // leaves the list valid; turn completion still refreshes titles and ordering.
      if (recovering || (target && !this.state.sessions.some(row => row.id === target))) void this.refreshSessions();
      loading = false;
      for (const event of buffered) this.enqueue(event);
    } catch (problem) {
      if (ownEpoch !== this.epoch) return;
      // The other requests for this load may still be in flight.
      this.epoch++;
      this.reloading = false;
      this.unsubscribe?.(); this.unsubscribe = undefined;
      this.update({ entries: [], connection: 'error', openingChat: false, busy: false, error: connectionProblem(problem) });
    }
  }
  /** Model name and permission mode for the composer line; absent data leaves the field empty (and hidden). */
  async readSettings(transport, ownEpoch) {
    try {
      const settings = await transport.request('hoh/settings/get', {});
      if (ownEpoch !== this.epoch) return;
      const security = settings.security;
      const permissions = security ? 'Спрашивать разрешение' : '';
      const found = detectProvider(settings.model?.baseUrl, settings.model?.model);
      this.update({ model: windowText(found.label, '', ''), modelIcon: found.icon ?? '', modelScope: found.scope, permissions });
    } catch { /* the line simply shows less */ }
  }
  /**
   * What the engine says about the OPEN chat (`session` of `access/get` with its `sessionId`): its executor, model, where its data
   * goes and the mode of its NEXT turn (`mode`), with the mode of the live process (`running`) and whether the change waits for the
   * next message (`pending`). `selected` is only the choice for NEW chats and is kept apart. An engine that does not name the
   * session is read as before: `selected` stands for the chat. Returns the mode shown for the chat ('' without Claude).
   */
  async readAccess(transport, ownEpoch) {
    const sid = this.sessionId; const seq = ++this.accessSeq;
    try {
      const answer = await transport.request('hoh/executor/access/get', { executor: 'claude', ...(sid ? { sessionId: sid } : {}) });
      if (ownEpoch !== undefined && ownEpoch !== this.epoch) return '';
      const newClaude = answer?.selected?.executor === 'claude';
      const own = answer?.session && typeof answer.session === 'object' ? answer.session : null;
      const claude = own ? own.executor === 'claude' : newClaude;
      const mode = (own ? own.mode : undefined) ?? answer?.effective?.mode ?? DEFAULT_ACCESS_MODE;
      const access = claude && (mode === 'full' || mode === 'confirm') ? mode : '';
      const running = own && (own.running === 'full' || own.running === 'confirm') ? own.running : '';
      const pending = access !== '' && own !== null && (own.pending === true || (running !== '' && running !== access));
      const model = claude ? String((own ? own.model : answer.selected.model) ?? '') : '';
      this.sessionPrivacy = own && (own.privacy === 'remote' || own.privacy === 'local') ? own.privacy : '';
      if (sid === this.sessionId && seq === this.accessSeq) {
        // With Claude the messages go to a service on the internet, whatever the engine's own model is.
        this.update({
          access, accessPending: pending, executor: claude ? 'claude' : '', executorModel: model,
          newExecutor: newClaude ? 'claude' : '', newExecutorModel: newClaude ? String(answer.selected.model ?? '') : '',
          projectSettingsOff: claude && projectSettingsIgnored(answer.effective?.flags), personalSettings: claude && personalSettingsApply(answer.effective?.flags),
          ...(this.privacyNow(claude) ? { privacy: this.privacyNow(claude) } : {}),
        });
        if ((claude || newClaude) && !this.state.executorModels.length) void this.loadExecutorModels(); // the names of the models for the composer line and the menu
      }
      return access;
    } catch {
      // A read that fails after the window left (disconnect, another connection) must not publish anything.
      if ((ownEpoch === undefined || ownEpoch === this.epoch) && sid === this.sessionId && seq === this.accessSeq) this.update({ access: '', accessPending: false, executor: '', executorModel: '', newExecutor: '', newExecutorModel: '', projectSettingsOff: false, personalSettings: false });
      return '';
    }
  }
  /** Where the data of the open chat goes: what the engine said about the chat, else by its executor. */
  privacyNow(claude) { return this.sessionPrivacy || (claude ? 'remote' : this.basePrivacy || ''); }
  /** `hoh/executor/discover`: a real, read-only call every time. The raw answer goes to the screen that shows it. */
  async discoverClaude() {
    if (!this.transport) throw new Error('нет связи с Помощником');
    return this.transport.request('hoh/executor/discover', { executor: 'claude' });
  }
  /** The models of the composer menu come from a fresh discover; a failure leaves the last list. */
  async loadExecutorModels() {
    try {
      const view = discoverView(await this.discoverClaude());
      this.update({ executorModels: view.models });
    } catch { /* the menu shows the model it has */ }
  }
  /** Privileged (the host asks the person): new chats run through Claude with `model`, or back on the engine's own model (`executor` null). */
  async selectExecutor(executor, model) {
    if (!this.transport || this.state.connection !== 'ready' || this.state.executorBusy) return false;
    const ownEpoch = this.epoch;
    this.update({ executorBusy: true, notice: '' });
    let refused = false;
    try { await this.transport.request('hoh/executor/select', { executor, model }); } catch { refused = true; }
    await this.readAccess(this.transport, ownEpoch);
    if (ownEpoch !== this.epoch) return false;
    const actual = this.state.newExecutor === 'claude' ? this.state.newExecutorModel : null;
    const asked = executor === 'claude' ? model : null;
    const failed = refused || actual !== asked;
    this.update({ executorBusy: false, notice: failed ? 'Поставщик не изменён: действие не разрешено или не удалось.' : '' });
    return !failed;
  }
  /** The person chose another mode in the composer menu (the only caller). The engine asks the host for approval; afterwards the mode is read back. */
  async setAccess(mode) {
    if ((mode !== 'confirm' && mode !== 'full') || !this.transport || this.state.connection !== 'ready' || this.state.accessBusy || mode === this.state.access) return;
    const ownEpoch = this.epoch;
    this.update({ accessBusy: true, notice: '' });
    let refused = false;
    try { await this.transport.request('hoh/executor/access/set', { executor: 'claude', mode }); } catch { refused = true; }
    const actual = await this.readAccess(this.transport, ownEpoch);
    if (ownEpoch !== this.epoch) return;
    const word = value => value === 'full' ? 'Полный доступ' : 'Спрашивать разрешение';
    this.update({
      accessBusy: false,
      notice: refused ? 'Режим доступа не изменён: действие не разрешено или не удалось.'
        : actual && actual !== mode ? `Помощник работает в режиме «${word(actual)}», а выбран «${word(mode)}». Показан фактический режим.` : '',
    });
  }
  /** A link of an answer: the host opens it, or says why it will not. Nothing opens without the host's own checks. */
  async openLink(kind, target) {
    const open = this.transport?.openLink;
    if (this.state.openingChat) return;
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
    this.update({ folderFixed: false });
    try {
      await this.transport.request('session/working-directory/set', { sessionId: this.sessionId, path });
      if (ownEpoch !== this.epoch) return;
      this.projection.folder = path;
      this.historySeq = undefined;
      const normalized = path.replaceAll('\\', '/').replace(/\/$/, '');
      const name = humanFile(normalized, normalized.slice(0, normalized.lastIndexOf('/')));
      this.update({ folderPath: path, folderLabel: folderLabelOf(path.replaceAll('\\', '/').replace(/\/$/, ''), name), error: '', folderFixed: false });
    } catch (error) {
      if (ownEpoch !== this.epoch) return;
      // By the code of the refusal: a chat of Claude keeps its folder (the person starts a new chat), no answer in the window is its own matter.
      const fixed = hostError(error).code === 'folder-fixed';
      this.update({ folderFixed: fixed, error: fixed ? refusalWords(error)
        : hostError(error).code === 'rejected' ? 'Вы не разрешили смену папки. Выберите папку ещё раз и нажмите «Разрешить один раз».'
        : errorText(error, 'Не удалось сменить папку. Попробуйте ещё раз.') });
    }
  }
  disconnect() {
    this.flushReceived(); this.active = false; this.reloading = false;
    clearTimeout(this.listRefresh); this.listRefresh = undefined;
    this.pendingPublish = undefined;
    this.historyCache.clear();
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
    this.projection.stop(recoverGap); this.projection.markStopped(); this.publish(); this.update({ canceling: true, error: '' });
    try {
      await this.transport.request('session/cancel', { sessionId: this.sessionId });
      if (ownEpoch !== this.epoch) return;
      if (recoverGap) {
        // Cancellation is acknowledged before the agent necessarily reaches idle.
        // Wait for its durable prefix/final message, then reconcile exactly as turn/end does.
        try { await this.transport.request('session/wait', { sessionId: this.sessionId }); } catch { /* history can still be available */ }
        if (ownEpoch !== this.epoch) return;
        try { await this.readHistory(this.transport, this.projection, () => ownEpoch === this.epoch && this.active, this.historySeq); } catch { /* finish with the marker below */ }
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
