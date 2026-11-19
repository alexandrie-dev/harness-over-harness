import { useEffect, useRef, useState } from 'react';
import { Icon } from '../shared/icons';
import { MethodMissing, loadModelSettings, modelsFromProbe, pickFolder, probeModel, saveModel, saveRules, storeApiKey } from './engine';
import '../shared/primitives.css';
import './firstrun.css';

type Mode = 'local' | 'cloud';
type Probe =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'ok'; text: string; warning?: string }
  | { kind: 'bad'; text: string };

const LOCAL_ADDRESS = 'http://localhost:8000/v1';
const CLOUD_ADDRESS = 'https://';
const DEFAULT_FOLDER = 'Документы/HoH';
const STEPS = ['Модель', 'Правила', 'Знакомство'] as const;

const EXAMPLES = [
  { icon: 'chart', title: 'Следить за ценой', note: 'Раз в день смотрит цену и пишет, если она изменилась', text: 'Следи за ценой видеокарты и сообщай, когда она упадёт' },
  { icon: 'doc', title: 'Прочитать документ', note: 'Открывает файл из папки и коротко пересказывает', text: 'Прочитай документ в папке и перескажи в трёх строках' },
  { icon: 'tasks', title: 'Новая задача', note: 'Расскажите, что делать регулярно, Помощник уточнит детали', text: 'Давай заведём новую задачу' },
] as const;

function latencyText(ms: number): string {
  return ms < 1000 ? `${Math.max(1, Math.round(ms))} мс` : `${(ms / 1000).toFixed(1).replace('.', ',')} с`;
}

function problemText(error: unknown): string {
  if (error instanceof MethodMissing) return `Эта версия пока не умеет: ${error.what}.`;
  return error instanceof Error && error.message ? error.message : 'Неизвестная ошибка.';
}

export interface FirstRunProps {
  /** Первый запуск закончен; `prefill` — текст примера для поля ввода чата. */
  onFinish(prefill?: string): void;
}

export function FirstRun({ onFinish }: FirstRunProps) {
  const [step, setStep] = useState(1);
  const [mode, setMode] = useState<Mode>('local');
  const [address, setAddress] = useState(LOCAL_ADDRESS);
  const [model, setModel] = useState('');
  const [manualModel, setManualModel] = useState('');
  const keyInput = useRef<HTMLInputElement>(null);
  const pending = useRef(false);
  const [keyStored, setKeyStored] = useState(false);
  const [probe, setProbe] = useState<Probe>({ kind: 'idle' });
  const [internet, setInternet] = useState<'ask' | 'public'>('ask');
  const [folder, setFolder] = useState(DEFAULT_FOLDER);
  const [folderNote, setFolderNote] = useState('');
  const [problem, setProblem] = useState('');
  const [busy, setBusy] = useState(false);
  const checkId = useRef(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const first = useRef(true);

  useEffect(() => {
    let alive = true;
    void loadModelSettings().then(saved => {
      if (!alive || !saved) return;
      if (saved.model) setModel(current => current || saved.model);
    });
    return () => { alive = false; };
  }, []);

  useEffect(() => {
    if (first.current) { first.current = false; return; }
    heading.current?.focus();
  }, [step]);

  const verified = probe.kind === 'ok';
  const cloud = mode === 'cloud';

  function invalidate() {
    checkId.current++;
    setProbe({ kind: 'idle' });
    setProblem('');
  }

  function chooseMode(next: Mode) {
    if (next === mode) return;
    setMode(next);
    setAddress(next === 'cloud' ? CLOUD_ADDRESS : LOCAL_ADDRESS);
    if (keyInput.current) keyInput.current.value = '';
    setKeyStored(false);
    invalidate();
  }

  async function check() {
    if (pending.current) return;
    const url = address.trim();
    const name = manualModel.trim();
    const bad = (text: string) => setProbe({ kind: 'bad', text });
    if (!/^https?:\/\/.{3,}/.test(url)) return bad('Проверьте адрес: он должен начинаться с http:// или https://');
    if (cloud && !keyStored && !keyInput.current?.value.trim()) return bad('Для облачного сервиса нужен API-ключ');
    const id = ++checkId.current;
    pending.current = true; setBusy(true);
    setProbe({ kind: 'checking' });
    setProblem('');
    try {
      if (cloud && keyInput.current?.value.trim()) {
        const input = keyInput.current;
        let key = input.value.trim();
        try {
          await storeApiKey(key, url);
          input.value = '';
          setKeyStored(true);
        } catch {
          throw new Error('Не удалось сохранить ключ. Попробуйте ещё раз.');
        } finally {
          key = '';
        }
        if (id !== checkId.current) return;
      }
      const result = await probeModel(url, name);
      if (id !== checkId.current) return;
      if (!result.ok) return bad('Модель не ответила. Проверьте адрес.');
      const selected = manualModel.trim() || modelsFromProbe(result)[0] || model.trim();
      if (!selected) return bad('Сервис не сообщил доступные модели. Укажите название в «Дополнительно» и проверьте ещё раз.');
      setModel(selected);
      setProbe({
        kind: 'ok',
        text: `Отвечает за ${latencyText(result.latencyMs)}`,
        warning: result.toolCalls ? undefined : 'Но она не умеет вызывать инструменты: Помощник сможет только отвечать текстом.',
      });
    } catch (error) {
      if (id === checkId.current) bad(`Не удалось проверить. ${problemText(error)}`);
    } finally {
      pending.current = false; setBusy(false);
    }
  }

  async function next() {
    if (pending.current || step >= 3 || (step === 1 && !verified)) return;
    pending.current = true;
    setProblem('');
    setBusy(true);
    try {
      if (step === 1) {
        await saveModel(address.trim(), model.trim());
      } else if (step === 2) {
        const defaults = internet === 'ask' && folder === DEFAULT_FOLDER;
        try {
          await saveRules({ internet, folder });
        } catch (error) {
          // Выбор, отличный от обычного, молча терять нельзя.
          if (!(error instanceof MethodMissing) || !defaults) throw error;
        }
      }
      setStep(step + 1);
    } catch (error) {
      setProblem(`Не удалось сохранить. ${problemText(error)}`);
    } finally {
      pending.current = false; setBusy(false);
    }
  }

  async function choose() {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    setFolderNote('');
    try {
      const picked = await pickFolder();
      if (picked) setFolder(picked);
    } catch (error) {
      setFolderNote(error instanceof MethodMissing ? `Выбор папки пока недоступен, работаем в ${folder}.` : problemText(error));
    } finally {
      pending.current = false; setBusy(false);
    }
  }

  const probeLine = probe.kind === 'checking' ? 'Проверяю…' : probe.kind === 'ok' ? probe.text : probe.kind === 'bad' ? probe.text : '';
  const probeIcon = probe.kind === 'checking' ? <Icon name="spinner" small spin /> : probe.kind === 'ok' ? <Icon name={probe.warning ? 'alert' : 'checkCircle'} small /> : probe.kind === 'bad' ? <Icon name="alert" small /> : null;
  const probeTone = probe.kind === 'ok' ? (probe.warning ? 'warn' : 'ok') : probe.kind === 'bad' ? 'bad' : '';
  const nextNote = problem || (step === 1 && !verified ? 'Сначала проверьте подключение' : '');

  return (
    <div className="hoh-page fr-page" data-screen="firstrun" data-step={step}>
      <div className="fr-wiz">
        <div className="fr-brand"><span className="mark">H</span><b>HoH</b></div>
        <ol className="fr-stepper" aria-label="Шаги настройки">
          {STEPS.map((name, i) => {
            const n = i + 1;
            const state = n < step ? 'done' : n === step ? 'current' : 'todo';
            return (
              <li key={name} className="fr-stepper-item" data-state={state} aria-current={state === 'current' ? 'step' : undefined}>
                <span className="fr-stepper-n">{state === 'done' ? <Icon name="check" small /> : n}</span>
                <span className="fr-stepper-name">{name}</span>
              </li>
            );
          })}
        </ol>

        <fieldset className="fr-card" aria-label="Настройка Помощника" disabled={busy}>
          {step === 1 && (
            <section aria-labelledby="fr-h">
              <h2 id="fr-h" ref={heading} tabIndex={-1}>Подключите модель</h2>
              <p className="fr-sub">Помощник думает с помощью языковой модели. Скажите, где она запущена.</p>
              <div className="fr-seg" role="group" aria-label="Где запущена модель">
                <button type="button" aria-pressed={!cloud} onClick={() => chooseMode('local')}>На этом компьютере</button>
                <button type="button" aria-pressed={cloud} onClick={() => chooseMode('cloud')}>Облачный сервис</button>
              </div>
              <div className="field">
                <label htmlFor="fr-address">Адрес</label>
                <input id="fr-address" type="text" value={address} spellCheck={false} autoComplete="off" onChange={e => { setAddress(e.target.value); invalidate(); }} />
                <span className="help">{cloud ? 'Адрес из личного кабинета сервиса, обычно заканчивается на /v1.' : 'Модель на этом компьютере: данные никуда не уходят.'}</span>
              </div>
              <details className="fr-advanced">
                <summary>Дополнительно</summary>
                <div className="field">
                  <label htmlFor="fr-model">Название модели</label>
                  <input id="fr-model" type="text" value={manualModel} spellCheck={false} autoComplete="off" placeholder={model || 'Выбирается автоматически'} onChange={e => { setManualModel(e.target.value); invalidate(); }} />
                </div>
              </details>
              <div className="field">
                <label htmlFor="fr-key">API-ключ</label>
                <input id="fr-key" type="password" ref={keyInput} autoComplete="off" disabled={!cloud} placeholder={cloud ? (keyStored ? 'Ключ сохранён. Оставьте пустым, чтобы не менять' : '') : 'Для модели на этом компьютере не нужен'} onChange={invalidate} />
                {cloud && <span className="help">Ключ из личного кабинета сервиса. Он хранится только на этом компьютере.</span>}
              </div>
              <div className="fr-verify">
                <button type="button" className="btn" disabled={probe.kind === 'checking'} onClick={() => void check()}>Проверить</button>
                <span className="fr-result" data-tone={probeTone} role="status">
                  {probeIcon}{probeLine}
                </span>
              </div>
              {probe.kind === 'ok' && probe.warning && <p className="fr-warning" role="status">{probe.warning}</p>}
            </section>
          )}

          {step === 2 && (
            <section aria-labelledby="fr-h">
              <h2 id="fr-h" ref={heading} tabIndex={-1}>Правила работы</h2>
              <p className="fr-sub">Это можно изменить позже в настройках.</p>
              <fieldset className="fr-group">
                <legend className="fr-legend">Интернет</legend>
                <label className="fr-option">
                  <input type="radio" name="internet" checked={internet === 'ask'} onChange={() => setInternet('ask')} />
                  <span><b>Спрашивать про каждый новый сайт</b><small>Помощник откроет сайт, только если вы разрешили именно его.</small></span>
                </label>
                <label className="fr-option">
                  <input type="radio" name="internet" checked={internet === 'public'} onChange={() => setInternet('public')} />
                  <span><b>Читать открытые сайты самому</b><small>Быстрее, но Помощник сам решает, какие сайты открывать.</small></span>
                </label>
              </fieldset>
              <div className="fr-group">
                <p className="fr-legend" id="fr-folder-label">Папка для работы</p>
                <div className="fr-pick">
                  <div className="fr-path" aria-labelledby="fr-folder-label"><Icon name="folder" small />{folder}</div>
                  <button type="button" className="btn btn-lg" onClick={() => void choose()}>Выбрать</button>
                </div>
                {folderNote && <p className="fr-note" role="status">{folderNote}</p>}
              </div>
              <div className="fr-lock">
                <Icon name="alert" />
                <div><b>Опасное подтверждаю в отдельном окне</b>Удаление, запуск программ и отправка данных наружу идут только после вашего «Разрешить» в окне защиты. Это правило нельзя отключить.</div>
              </div>
            </section>
          )}

          {step === 3 && (
            <section aria-labelledby="fr-h">
              <h2 id="fr-h" className="fr-hello" ref={heading} tabIndex={-1}>Я ваш Помощник.</h2>
              <p className="fr-sub">Скажите, что нужно сделать, обычными словами. Можно начать с одного из примеров.</p>
              <div className="fr-examples">
                {EXAMPLES.map(example => (
                  <button key={example.title} type="button" onClick={() => onFinish(example.text)}>
                    <span className="fr-tile"><Icon name={example.icon} /></span>
                    <span className="fr-example-text"><b>{example.title}</b><small>{example.note}</small></span>
                    <Icon name="chevron" small className="fr-go" />
                  </button>
                ))}
              </div>
            </section>
          )}

          <div className="fr-foot">
            <button type="button" className={step === 1 ? 'btn fr-invisible' : 'btn'} disabled={busy || step === 1} onClick={() => { if (!pending.current) { setProblem(''); setStep(step - 1); } }}><Icon name="back" small />Назад</button>
            <span className="fr-foot-note" role={problem ? 'alert' : undefined}>
              {step === 3 ? 'Можно и без примера: напишите своё в чате.' : nextNote}
            </span>
            {step < 3
              ? <button type="button" className="btn btn-primary btn-lg" disabled={busy || (step === 1 && !verified)} onClick={() => void next()}>Дальше</button>
              : <button type="button" className="btn btn-primary btn-lg" onClick={() => onFinish()}>Открыть чат</button>}
          </div>
        </fieldset>
      </div>
    </div>
  );
}
