import { memo, useCallback, useEffect, useState, type FormEvent, type KeyboardEvent } from 'react';
import { LegendList } from '@legendapp/list/react';
import { Markdown } from '../../lib/markdown';
import { useChatStore } from '../../lib/store';
import { Icon } from '../shared/icons';
import type { ChatEntry } from './projection.mjs';
import { ChatIcon } from './icons';
import '../shared/primitives.css';
import './chat.css';

const FOLLOW_END = { animated: false, on: { dataChange: true, itemLayout: true, layout: true, footerLayout: false } } as const;
const keyOf = (entry: ChatEntry) => entry.id;
const ChatRow = memo(function ChatRow({ entry }: { entry: ChatEntry }) {
  if (entry.kind === 'step') return <article className="chat-entry chat-work-entry" data-chat-entry="step" data-operation={entry.operation} data-status={entry.status} data-entry-id={entry.id}>
    <details className="chat-work-step">
      <summary>
        <span className="chat-step-indicator"><Icon name={entry.status === 'running' ? 'spinner' : entry.status === 'error' ? 'alert' : entry.status === 'stopped' ? 'clock' : 'checkCircle'} spin={entry.status === 'running'} /></span>
        <span data-step-label>{entry.label}</span>
        <Icon name="chevron" small className="chat-step-chevron" />
      </summary>
      <div className="chat-step-detail" data-step-detail>
        {entry.detail ? <pre>{entry.detail}</pre> : <p>{entry.status === 'running' ? 'Результат появится здесь.' : 'Дополнительного содержимого нет.'}</p>}
      </div>
    </details>
  </article>;
  return <article className={`chat-entry chat-message chat-message-${entry.kind}`} data-chat-entry={entry.kind} data-entry-id={entry.id} data-streaming={entry.streaming ? 'true' : 'false'}>
    <h2 className="chat-sr-only">{entry.kind === 'user' ? 'Вы' : 'Помощник'}</h2>
    {entry.kind === 'assistant' ? <Markdown text={entry.text} /> : <div className="chat-user-bubble"><p className="chat-user-text">{entry.text}</p></div>}
    {entry.kind === 'user' && entry.at !== undefined && <time className="chat-message-time" dateTime={new Date(entry.at).toISOString()}>{new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' }).format(entry.at)}</time>}
    {entry.failed && <p className="chat-delivery-error" data-delivery="failed">Не отправлено</p>}
  </article>;
});
const feedStart = <div className="chat-feed-start" />;
const feedEnd = <div className="chat-feed-end" />;
function cssValue(name: string) { return getComputedStyle(document.documentElement).getPropertyValue(name).trim(); }

export function ChatScreen() {
  const entries = useChatStore(state => state.entries);
  const connection = useChatStore(state => state.connection);
  const busy = useChatStore(state => state.busy);
  const canceling = useChatStore(state => state.canceling);
  const error = useChatStore(state => state.error);
  const privacy = useChatStore(state => state.privacy);
  const folderLabel = useChatStore(state => state.folderLabel);
  const draft = useChatStore(state => state.draft);
  const [compact, setCompact] = useState(false);
  const [estimate] = useState(() => Number.parseFloat(cssValue('--chat-estimated-row')) || undefined);
  const { connect, disconnect, send, stop, setDraft } = useChatStore.getState();
  useEffect(() => { void connect(); return disconnect; }, [connect, disconnect]);
  useEffect(() => {
    const breakpoint = cssValue('--chat-compact-breakpoint'); if (!breakpoint) return;
    const query = matchMedia(`(max-width: ${breakpoint})`);
    const changed = () => setCompact(query.matches);
    changed(); query.addEventListener('change', changed);
    return () => query.removeEventListener('change', changed);
  }, []);
  const renderItem = useCallback(({ item }: { item: ChatEntry }) => <ChatRow key={item.id} entry={item} />, []);
  function submit(event: FormEvent) { event.preventDefault(); void send(); }
  function keyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void send(); }
  }
  const ready = connection === 'ready';
  return <main className="chat-screen" data-chat-screen data-connection={connection} data-compact={compact ? 'true' : 'false'}>
    <div className="chat-privacy" data-privacy={privacy}><ChatIcon name="lock" small />{privacy === 'local' ? 'Данные на этом компьютере' : privacy === 'remote' ? 'Ответы готовятся на другом компьютере' : 'Проверяю, где обрабатываются данные…'}</div>
    <div className="chat-shell">
      <aside className="chat-sidebar" data-chat-sidebar aria-label="Помощник">
        <div className="chat-brand"><span className="mark">H</span><b>HoH</b></div>
        <div className="chat-assistant-nav"><div className="chat-assistant" aria-current="page">
          <span className="chat-avatar"><ChatIcon name="star" small /></span>
          <span className="chat-assistant-name">Помощник<small>{ready ? busy ? 'работает' : 'готов к работе' : connection === 'recovering' ? 'восстанавливаю связь' : connection === 'error' ? 'нет связи' : 'подключаюсь'}</small></span>
        </div></div>
        <div className="chat-sidebar-footer"><Icon name="folder" small /><span>{folderLabel}</span></div>
      </aside>
      <section className="chat-main" aria-label="Чат с Помощником">
        <header className="chat-header"><h1>Помощник</h1><span className="chat-status" aria-hidden="true">{canceling ? 'останавливаю…' : busy ? 'отвечает…' : ''}</span><span className="chat-folder-chip"><Icon name="folder" small />{folderLabel}</span></header>
        <section className="chat-conversation" aria-label="Переписка" aria-busy={busy}>
          {entries.length === 0 && <div className="chat-empty" data-chat-empty><h2>С чего начнём?</h2><p>{connection === 'connecting' ? 'Подключаюсь к Помощнику…' : ready ? 'Напишите, что нужно сделать. Ход работы появится здесь.' : 'Разговор появится после подключения.'}</p></div>}
          <LegendList<ChatEntry> data={entries} renderItem={renderItem} keyExtractor={keyOf} estimatedItemSize={estimate}
            initialScrollAtEnd maintainScrollAtEnd={FOLLOW_END} ListHeaderComponent={feedStart} ListFooterComponent={feedEnd}
            className="chat-timeline" tabIndex={0} aria-label="Сообщения" data-testid="chat-timeline" />
        </section>
        <output className="chat-sr-only" aria-live="polite" data-chat-status>{connection === 'recovering' ? 'Связь с Помощником прервалась, восстанавливаю…' : canceling ? 'Останавливаю…' : busy ? 'Помощник отвечает…' : ready ? 'Можно написать сообщение' : ''}</output>
        {error && <div className="chat-error" role="alert">{error}{connection === 'error' && <button className="btn" type="button" onClick={() => void connect()}>Подключиться ещё раз</button>}</div>}
        <form className="chat-composer" onSubmit={submit} aria-label="Написать Помощнику"><div className="chat-composer-column"><div className="chat-input-box">
          <label className="chat-sr-only" htmlFor="chat-message-input">Сообщение Помощнику</label>
          <textarea id="chat-message-input" className="chat-input" data-chat-input rows={1} value={draft}
            onChange={event => setDraft(event.target.value)} onKeyDown={keyDown} disabled={!ready} placeholder="Напишите, что нужно сделать" />
          <div className="chat-composer-actions"><span className="chat-composer-hint">Enter отправляет, Shift+Enter переносит строку</span>
            {<button className="btn" type="button" data-chat-stop aria-label="Стоп" disabled={!busy || canceling} onClick={() => void stop()}><ChatIcon name="stop" small />{canceling ? 'Останавливаю…' : 'Стоп'}</button>}
            <button className="btn btn-primary" type="submit" data-chat-send disabled={!ready || busy || canceling || !draft.trim()}><ChatIcon name="up" small />Отправить</button>
          </div>
        </div></div></form>
      </section>
    </div>
  </main>;
}
