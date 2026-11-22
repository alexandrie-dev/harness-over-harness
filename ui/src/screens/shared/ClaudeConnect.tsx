// Подключение Claude: найден ли `claude`, выполнен ли вход, какие модели. Всё — из `hoh/executor/discover`;
// окно не читает файлов Claude и не просит ключ. Один компонент для мастера и для смены поставщика в чате.
import { useCallback, useEffect, useRef, useState } from 'react';
import { errorText } from '../../lib/hostError.mjs';
import { discoverView, type DiscoverResult, type DiscoverView } from '../../lib/executor.mjs';
import { ProviderMark } from './ProviderMark';
import { Icon } from './icons';
import './claude-connect.css';

type Check = { kind: 'idle' } | { kind: 'checking' } | { kind: 'done'; view: DiscoverView } | { kind: 'failed'; text: string };

/** Куда уходят данные: человек читает это до первой просьбы. */
export const DATA_LINE = 'Ваши сообщения и то, что Помощник прочитает в файлах, отправляются в сервис Claude (компания Anthropic).';

export interface ClaudeConnectProps {
  discover(): Promise<DiscoverResult>;
  /** Выбор готов (Claude найден и вход выполнен) и выбранная модель; иначе ready=false. */
  onChange(state: { ready: boolean; model: string }): void;
  /** Модель, выбранная сейчас (для смены поставщика): подставляется, если она есть в списке. */
  current?: string;
}

export function ClaudeConnect({ discover, onChange, current }: ClaudeConnectProps) {
  const [check, setCheck] = useState<Check>({ kind: 'idle' });
  const [model, setModel] = useState('');
  const run = useRef(0);
  const report = useRef(onChange);
  report.current = onChange;

  const verify = useCallback(async () => {
    const id = ++run.current;
    setCheck({ kind: 'checking' });
    report.current({ ready: false, model: '' });
    try {
      const view = discoverView(await discover());
      if (id !== run.current) return;
      const picked = view.models.some(m => m.id === current) ? (current as string) : view.defaultModel;
      setModel(picked);
      setCheck({ kind: 'done', view });
      report.current({ ready: view.ready && !!picked, model: picked });
    } catch (error) {
      if (id !== run.current) return;
      const text = errorText(error, 'Попробуйте ещё раз.');
      setCheck({ kind: 'failed', text: `Не удалось проверить Claude. ${text}` });
    }
  }, [discover, current]);

  // Проверка только читает: её можно выполнять сразу, как только человек выбрал Claude.
  useEffect(() => { void verify(); return () => { run.current++; }; }, [verify]);

  const view = check.kind === 'done' ? check.view : null;
  const line = check.kind === 'checking' ? 'Проверяю…' : view ? view.headline : check.kind === 'failed' ? check.text : '';
  const tone = view ? view.tone : check.kind === 'failed' ? 'bad' : '';
  const icon = check.kind === 'checking' ? <Icon name="spinner" small spin /> : view ? <Icon name={view.tone === 'ok' ? 'checkCircle' : 'alert'} small /> : check.kind === 'failed' ? <Icon name="alert" small /> : null;
  return (
    <div className="cc" data-claude-connect data-state={check.kind === 'done' ? (view?.ready ? 'ready' : 'blocked') : check.kind}>
      <p className="cc-line" data-provider-line data-provider="claude"><ProviderMark icon="claude" /><span>Claude</span></p>
      <p className="cc-sub">Ключ вводить не нужно: вход выполняется в самом Claude.</p>
      <p className="cc-sub" data-data-line>{DATA_LINE}</p>
      <div className="cc-verify">
        {!(view && view.ready) && <button type="button" className="btn" data-claude-check disabled={check.kind === 'checking'} onClick={() => void verify()}>
          {check.kind === 'idle' || check.kind === 'checking' ? 'Проверить' : 'Проверить ещё раз'}
        </button>}
        <span className="cc-result" data-claude-result data-tone={tone} role="status">{icon}{line}</span>
      </div>
      {view && view.ready && view.version && <p className="cc-sub" data-claude-version>Claude, версия {view.version}</p>}
      {view && view.warning && <p className="cc-warning" data-claude-warning role="status">{view.warning}</p>}
      {view && view.ready && view.models.length > 0 && (
        <div className="cc-field">
          <label htmlFor="cc-model">Модель Claude</label>
          <select id="cc-model" data-claude-model value={model} onChange={event => { setModel(event.target.value); report.current({ ready: true, model: event.target.value }); }}>
            {view.models.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
          </select>
        </div>
      )}
    </div>
  );
}
