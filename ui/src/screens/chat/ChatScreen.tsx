import { memo, useCallback, useMemo, useEffect, useState, type FormEvent, type KeyboardEvent } from 'react';
import { LegendList } from '@legendapp/list/react';
import { ChatMarkdown, LinkContext, type LinkActions } from '../../t3/render';
import { useChatStore } from '../../lib/store';
import { keyNote } from '../../lib/keyNote';
import { Icon } from '../shared/icons';
import { ProviderMark } from '../shared/ProviderMark';
import { EXAMPLES } from '../../lib/examples.mjs';
import { Emblem } from '../shared/Emblem';
import { pickFolder } from '../firstrun/engine';
import type { ChatEntry } from './projection.mjs';
import { ChatIcon } from './icons';
import { QuestionCard } from './QuestionCard';
import { ApprovalsScreen } from './ApprovalsScreen';
import { AccessMode } from './AccessMode';
import { ExecutorModelPicker } from './ExecutorModelPicker';
import { ProviderSettings } from './ProviderSettings';
import '../shared/primitives.css';
import './chat.css';

const FOLLOW_END = { animated: false, on: { dataChange: true, itemLayout: true, layout: true, footerLayout: false } } as const;
const keyOf = (entry: ChatEntry) => entry.id;
const ChatRow = memo(function ChatRow({ entry }: { entry: ChatEntry }) {
  if (entry.kind === 'step') return <article className="chat-entry chat-work-entry" data-chat-entry="step" data-operation={entry.operation} data-status={entry.status} data-entry-id={entry.id}>
    <details className="chat-work-step">
      <summary>
        <span className="chat-step-indicator"><Icon name={entry.status === 'running' ? 'spinner' : entry.status === 'error' ? 'alert' : entry.status === 'stopped' ? 'clock' : entry.operation === 'command' ? 'tasks' : entry.operation === 'read' || entry.operation === 'write' ? 'doc' : 'checkCircle'} spin={entry.status === 'running'} /></span>
        <span data-step-label>{entry.label}</span>
        <Icon name="chevron" small className="chat-step-chevron" />
      </summary>
      <div className="chat-step-detail" data-step-detail>
        {entry.detail ? <pre>{entry.detail}</pre> : <p>{entry.status === 'running' ? 'Результат появится здесь.' : 'Дополнительного содержимого нет.'}</p>}
      </div>
    </details>
  </article>;
  if (entry.kind === 'note') return <article className="chat-entry chat-note" data-chat-entry="note" data-entry-id={entry.id}><p data-stopped-note>{entry.text}</p></article>;
  return <article className={`chat-entry chat-message chat-message-${entry.kind}`} data-chat-entry={entry.kind} data-entry-id={entry.id} data-streaming={entry.streaming ? 'true' : 'false'}>
    <h2 className="chat-sr-only">{entry.kind === 'user' ? 'Вы' : 'Помощник'}</h2>
    {entry.kind === 'assistant' ? <ChatMarkdown text={entry.text} streaming={entry.streaming} /> : <div className="chat-user-bubble"><p className="chat-user-text">{entry.text}</p></div>}
    {entry.kind === 'user' && entry.at !== undefined && <time className="chat-message-time" dateTime={new Date(entry.at).toISOString()}>{new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' }).format(entry.at)}</time>}
    {entry.failed && <p className="chat-delivery-error" data-delivery="failed">Не отправлено</p>}
  </article>;
});
const feedStart = <div className="chat-feed-start" />;
const feedEnd = <div className="chat-feed-end" />;
function cssValue(name: string) { return getComputedStyle(document.documentElement).getPropertyValue(name).trim(); }
const dateFormat = new Intl.DateTimeFormat('ru-RU', { weekday: 'long', day: 'numeric', month: 'long' });
const timeFormat = new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' });
/** Seconds or milliseconds since the epoch, whichever the engine sends; 0 means "unknown". */
function ageLabel(updatedAt: number, now: number) {
  if (!updatedAt) return '';
  const minutes = Math.max(0, Math.floor((now - (updatedAt < 1e11 ? updatedAt * 1000 : updatedAt)) / 60000));
  if (minutes < 1) return 'сейчас';
  if (minutes < 60) return `${minutes} мин`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours} ч` : `${Math.floor(hours / 24)} д`;
}
function useNow() {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 30000); return () => clearInterval(timer); }, []);
  return now;
}

export function ChatScreen() {
  const entries = useChatStore(state => state.entries);
  const connection = useChatStore(state => state.connection);
  const busy = useChatStore(state => state.busy);
  const canceling = useChatStore(state => state.canceling);
  const error = useChatStore(state => state.error);
  const privacy = useChatStore(state => state.privacy);
  const folderLabel = useChatStore(state => state.folderLabel);
  const model = useChatStore(state => state.model);
  const permissions = useChatStore(state => state.permissions);
  const modelIcon = useChatStore(state => state.modelIcon);
  const notice = useChatStore(state => state.notice);
  const access = useChatStore(state => state.access);
  const accessBusy = useChatStore(state => state.accessBusy);
  const executor = useChatStore(state => state.executor);
  const executorModel = useChatStore(state => state.executorModel);
  const executorModels = useChatStore(state => state.executorModels);
  const executorBusy = useChatStore(state => state.executorBusy);
  const discoverClaude = useChatStore(state => state.discoverClaude);
  const loadExecutorModels = useChatStore(state => state.loadExecutorModels);
  const selectExecutor = useChatStore(state => state.selectExecutor);
  const [providerOpen, setProviderOpen] = useState(false);
  const [sendHint, setSendHint] = useState('');
  useEffect(() => { if (!busy) setSendHint(''); }, [busy]);
  const folderPath = useChatStore(state => state.folderPath);
  const draft = useChatStore(state => state.draft);
  const sessions = useChatStore(state => state.sessions);
  const currentId = useChatStore(state => state.currentId);
  const working = useChatStore(state => state.working);
  // Only the question of the open chat is shown; questions of other chats wait for their chat.
  const question = useChatStore(state => state.questions.find(item => item.sessionId === state.currentId));
  const [section, setSection] = useState<'chat' | 'approvals'>('chat');
  const [compact, setCompact] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [estimate] = useState(() => Number.parseFloat(cssValue('--chat-estimated-row')) || undefined);
  const now = useNow();
  const { connect, disconnect, send, stop, setDraft, openSession, newChat, setFolder, openLink, refuseLink, blockedLink, dismissNotice, setAccess } = useChatStore.getState();
  const links = useMemo<LinkActions>(() => ({ folder: folderPath, open: (kind, target) => void openLink(kind, target), refuse: () => refuseLink(), blocked: () => blockedLink() }), [folderPath, openLink, refuseLink, blockedLink]);
  useEffect(() => { void connect(); return disconnect; }, [connect, disconnect]);
  useEffect(() => {
    const breakpoint = cssValue('--chat-compact-breakpoint'); if (!breakpoint) return;
    const query = matchMedia(`(max-width: ${breakpoint})`);
    const changed = () => setCompact(query.matches);
    changed(); query.addEventListener('change', changed);
    return () => query.removeEventListener('change', changed);
  }, []);
  const startChat = useCallback(() => { setSection('chat'); setMenuOpen(false); void newChat(); }, [newChat]);
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === 'n') { event.preventDefault(); startChat(); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [startChat]);
  useEffect(() => {
    if (!compact || !menuOpen) return;
    const onEscape = (event: globalThis.KeyboardEvent) => { if (event.key === 'Escape') setMenuOpen(false); };
    window.addEventListener('keydown', onEscape);
    return () => window.removeEventListener('keydown', onEscape);
  }, [compact, menuOpen]);
  const renderItem = useCallback(({ item }: { item: ChatEntry }) => <ChatRow key={item.id} entry={item} />, []);
  function submit(event: FormEvent) { event.preventDefault(); if (busy) void stop(); else void send(); }
  async function chooseFolder() {
    try { const picked = await pickFolder(); if (picked) await setFolder(picked); } catch { /* no system dialog here: the folder stays as it is */ }
  }
  function keyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      // Enter while the answer is being written does nothing: say why instead of staying silent.
      if (busy) { setSendHint('Помощник ещё отвечает. Дождитесь ответа или нажмите «Стоп».'); return; }
      void send();
    }
  }
  const ready = connection === 'ready';
  const state = ready ? busy ? 'работает' : 'готов к работе' : connection === 'recovering' ? 'восстанавливаю связь' : connection === 'error' ? 'нет связи' : 'подключаюсь';
  const privacyText = privacy === 'local' ? 'Данные на этом компьютере' : privacy === 'remote' ? (executor === 'claude' ? 'Отвечает Claude, сервис в интернете' : 'Отвечает сервис в интернете') : connection === 'error' ? 'Нет связи: не знаю, где обрабатываются данные' : 'Проверяю, где обрабатываются данные…';
  const title = sessions.find(row => row.id === currentId)?.title || 'Новый чат';
  return <LinkContext.Provider value={links}><main className="chat-screen" data-chat-screen data-connection={connection} data-compact={compact ? 'true' : 'false'}>
    <aside className="chat-sidebar" data-chat-sidebar data-open={menuOpen ? 'true' : 'false'} aria-label="Навигация">
      <div className="chat-brand"><Emblem /><b>HoH</b></div>
      <nav className="chat-nav" aria-label="Разделы">
        <button type="button" className="chat-row" data-new-chat disabled={connection === 'connecting'} onClick={startChat}>
          <ChatIcon name="newChat" /><span className="chat-row-label">Новый чат</span><kbd>Ctrl N</kbd>
        </button>
        <button type="button" className="chat-row" data-nav-approvals aria-current={section === 'approvals' ? 'page' : undefined} onClick={() => { setSection('approvals'); setMenuOpen(false); }}>
          <Icon name="shield" /><span className="chat-row-label">Разрешения</span>
        </button>
      </nav>
      <div className="chat-side-mid">
        <section className="chat-group" aria-labelledby="chat-team-head">
          <div className="chat-group-head"><span id="chat-team-head">Команда</span><span className="chat-group-line" /><span className="chat-group-count">1</span></div>
          <button type="button" className="chat-row chat-member" data-nav-chat onClick={() => { setSection('chat'); setMenuOpen(false); }}>
            <span className="chat-avatar"><ChatIcon name="star" small /></span>
            <span className="chat-row-label">Помощник</span>
            <span className="chat-state" data-working={ready && busy ? 'true' : 'false'}>{ready && busy && <Icon name="spinner" small spin />}<span className="chat-state-text" title={state}>{state}</span></span>
          </button>
        </section>
        <section className="chat-group chat-group-chats" aria-labelledby="chat-recent-head">
          <div className="chat-group-head"><span id="chat-recent-head">Недавние чаты</span><span className="chat-group-line" /><span className="chat-group-count">{sessions.length}</span></div>
          <ul className="chat-list" data-chat-list>
            {sessions.map(row => <li key={row.id}>
              <button type="button" className="chat-row chat-item" data-chat-row data-chat-id={row.id} aria-current={section === 'chat' && row.id === currentId ? 'true' : undefined}
                disabled={connection === 'connecting'} onClick={() => { setSection('chat'); setMenuOpen(false); void openSession(row.id); }}>
                <span className="chat-row-label">{row.title || 'Новый чат'}</span>
                {working.includes(row.id) ? <span className="chat-state" data-working="true"><Icon name="spinner" small spin />работает</span> : <span className="chat-age">{ageLabel(row.updatedAt, now)}</span>}
              </button>
            </li>)}
          </ul>
        </section>
      </div>
    </aside>
    {compact && menuOpen && <div className="chat-scrim" data-chat-scrim aria-hidden="true" onClick={() => setMenuOpen(false)} />}
    <div className="chat-main">
      <header className="chat-topbar">
        <button type="button" className="btn chat-menu" data-chat-menu aria-expanded={menuOpen} onClick={() => setMenuOpen(open => !open)}>Чаты</button>
        <h1 className="chat-title">{section === 'approvals' ? 'Разрешения' : title}</h1>
        {section === 'chat' && <span className="chat-status" aria-hidden="true">{canceling ? 'останавливаю…' : busy ? 'отвечает…' : ''}</span>}
        <p className="chat-when">
          <span className="chat-when-date">{dateFormat.format(now)}</span><i className="chat-when-date" /><span className="chat-when-date">{timeFormat.format(now)}</span><i className="chat-when-date" />
          <span className="chat-privacy" data-privacy={privacy} title={privacyText}><ChatIcon name="lock" small /><span className="chat-privacy-text">{privacyText}</span></span>
        </p>
      </header>
      {section === 'approvals' && <ApprovalsScreen />}
      <section className="chat-conversation-area" aria-label="Чат с Помощником" hidden={section !== 'chat'}>
        <section className="chat-conversation" aria-label="Переписка" aria-busy={busy}>
          {entries.length === 0 && <div className="chat-empty" data-chat-empty><div className="chat-col"><h2>С чего <span className="chat-key">начнём</span>?</h2><p>{connection === 'connecting' ? 'Подключаюсь к Помощнику…' : ready ? 'Напишите, что нужно сделать. Ход работы появится здесь.' : 'Разговор появится после подключения.'}</p>{access === 'full' && <p className="chat-full-line" data-full-access-line>Включён полный доступ: Помощник не спрашивает разрешения.</p>}
            {ready && <div className="chat-examples" data-chat-examples>
              {EXAMPLES.map(example => <button key={example.id} type="button" data-chat-example={example.id} onClick={() => { setDraft(example.text); document.getElementById('chat-message-input')?.focus(); }}>
                <span className="chat-example-tile"><Icon name={example.icon} /></span>
                <span className="chat-example-text"><b>{example.title}</b><small>{example.note}</small></span>
              </button>)}
            </div>}
            {ready && access !== 'full' && <p className="chat-folder-line" data-folder-line>Папка Помощника: {folderLabel}. Кладите туда файлы, с которыми нужна помощь.</p>}{privacy === 'remote' && <p className="chat-key-note" data-key-note>{keyNote()}</p>}</div></div>}
          <LegendList<ChatEntry> data={entries} renderItem={renderItem} keyExtractor={keyOf} estimatedItemSize={estimate}
            initialScrollAtEnd maintainScrollAtEnd={FOLLOW_END} ListHeaderComponent={feedStart} ListFooterComponent={feedEnd}
            className="chat-timeline" tabIndex={0} aria-label="Сообщения" data-testid="chat-timeline" />
        </section>
        <output className="chat-sr-only" aria-live="polite" data-chat-status>{connection === 'recovering' ? 'Связь с Помощником прервалась, восстанавливаю…' : canceling ? 'Останавливаю…' : busy ? 'Помощник отвечает…' : ready ? 'Можно написать сообщение' : ''}</output>
        {error && <div className="chat-error chat-col" role="alert">{error}{connection === 'error' && <button className="btn" type="button" onClick={() => void connect()}>Подключиться ещё раз</button>}</div>}
        {notice && <div className="chat-notice chat-col" role="status" data-link-notice><span>{notice}</span><button className="btn" type="button" onClick={() => dismissNotice()}>Закрыть</button></div>}
        {question && <QuestionCard key={question.requestId} question={question} />}
        <form className="chat-composer" onSubmit={submit} aria-label="Написать Помощнику"><div className="chat-col">
          {sendHint && <p className="chat-send-hint" role="status" data-send-hint>{sendHint}</p>}
          <div className="chat-input-box">
          <label className="chat-sr-only" htmlFor="chat-message-input">Сообщение Помощнику</label>
          <textarea id="chat-message-input" className="chat-input" data-chat-input rows={1} value={draft}
            onChange={event => setDraft(event.target.value)} onKeyDown={keyDown} disabled={!ready} placeholder="Напишите, что нужно сделать" />
          <div className="chat-composer-actions">
            {executor === 'claude'
              ? <ExecutorModelPicker model={executorModel} models={executorModels} busy={executorBusy} disabled={!ready || busy}
                  onOpen={() => void loadExecutorModels()} onPick={id => void selectExecutor('claude', id)} onChangeProvider={() => setProviderOpen(true)} />
              : model && <button type="button" className="chat-pick chat-pick-text chat-pick-model" data-chat-model-chip title={`${model}. Сменить поставщика`} disabled={!ready || busy} onClick={() => setProviderOpen(true)}><ProviderMark icon={modelIcon} /><span data-chat-model>{model}</span></button>}
            {access ? <AccessMode mode={access} busy={accessBusy} disabled={!ready || busy} onChange={mode => void setAccess(mode)} />
              : permissions && <span className="chat-pick chat-pick-text" data-chat-permissions><Icon name="shield" small /><span>{permissions}</span></span>}
            <button className="chat-pick" type="button" data-chat-folder disabled={!ready || busy} onClick={() => void chooseFolder()} title="Выбрать рабочую папку"><Icon name="folder" small /><span>{folderLabel}</span></button>
            {busy
              ? <button className="chat-send" type="submit" data-chat-stop aria-label="Стоп" disabled={canceling}><ChatIcon name="stop" /></button>
              : <button className="chat-send" type="submit" data-chat-send aria-label="Отправить" disabled={!ready || canceling || !draft.trim()}><ChatIcon name="up" /></button>}
          </div>
        </div></div></form>
      </section>
      {providerOpen && <ProviderSettings executor={executor} claudeModel={executorModel} engineModel={model} engineIcon={modelIcon} busy={executorBusy} discover={discoverClaude}
        onClose={() => setProviderOpen(false)} onApply={(next, chosen) => { void selectExecutor(next, chosen).finally(() => setProviderOpen(false)); }} />}
    </div>
  </main></LinkContext.Provider>;
}
