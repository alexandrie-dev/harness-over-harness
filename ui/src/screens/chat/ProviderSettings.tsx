// Смена поставщика модели после мастера: модель движка (локальная или облачная, как настроена) или Claude.
// Тот же компонент подключения Claude, что в мастере. Привилегированную `hoh/executor/select` вызывает владелец экрана.
import { useEffect, useRef, useState } from 'react';
import type { DiscoverResult } from '../../lib/executor.mjs';
import { ClaudeConnect } from '../shared/ClaudeConnect';
import { ProviderMark } from '../shared/ProviderMark';

export interface ProviderSettingsProps {
  executor: '' | 'claude';
  claudeModel: string;
  engineModel: string;
  engineIcon: string;
  busy: boolean;
  discover(): Promise<DiscoverResult>;
  onApply(executor: 'claude' | null, model: string): void;
  onClose(): void;
}

export function ProviderSettings({ executor, claudeModel, engineModel, engineIcon, busy, discover, onApply, onClose }: ProviderSettingsProps) {
  const [choice, setChoice] = useState<'engine' | 'claude'>(executor === 'claude' ? 'claude' : 'engine');
  const [claude, setClaude] = useState({ ready: false, model: '' });
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus(); }, []);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  const unchanged = choice === 'engine' ? executor === '' : false;
  const canApply = !busy && !unchanged && (choice === 'engine' || claude.ready);
  return (
    <div className="chat-dialog-backdrop" data-provider-dialog-backdrop onClick={event => { if (event.target === event.currentTarget) onClose(); }}>
      <div className="chat-dialog" role="dialog" aria-modal="true" aria-labelledby="chat-provider-title" data-provider-dialog>
        <h2 id="chat-provider-title" ref={heading} tabIndex={-1}>Поставщик модели</h2>
        <p className="chat-dialog-sub">Новые чаты пойдут через выбранного поставщика. Открытые чаты остаются как были.</p>
        <div className="chat-dialog-seg" role="group" aria-label="Поставщик">
          <button type="button" aria-pressed={choice === 'engine'} data-provider-choice="engine" onClick={() => setChoice('engine')}>Как настроено в Помощнике</button>
          <button type="button" aria-pressed={choice === 'claude'} data-provider-choice="claude" onClick={() => setChoice('claude')}><ProviderMark icon="claude" />Claude</button>
        </div>
        {choice === 'engine'
          ? <p className="chat-dialog-note" data-engine-note><ProviderMark icon={engineIcon} />{engineModel ? `Модель, настроенная в Помощнике: ${engineModel}.` : 'Модель, настроенная в Помощнике.'}</p>
          : <ClaudeConnect discover={discover} onChange={setClaude} current={executor === 'claude' ? claudeModel : undefined} />}
        <div className="chat-dialog-foot">
          <button type="button" className="btn" data-provider-cancel onClick={onClose}>Закрыть</button>
          <button type="button" className="btn btn-primary" data-provider-apply disabled={!canApply}
            onClick={() => onApply(choice === 'claude' ? 'claude' : null, choice === 'claude' ? claude.model : 'default')}>Применить</button>
        </div>
      </div>
    </div>
  );
}
