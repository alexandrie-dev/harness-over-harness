import { detectProvider } from '../../lib/providers.mjs';
import { ProviderMark } from '../shared/ProviderMark';
import { Emblem } from '../shared/Emblem';
import { keyNote } from '../../lib/keyNote';
import { useEffect, useRef, useState } from 'react';
import { Icon } from '../shared/icons';
import { confirmDescription, discoverView } from '../../lib/executor.mjs';
import { errorText, refusalWords } from '../../lib/hostError.mjs';
import { ClaudeConnect } from '../shared/ClaudeConnect';
import { MethodMissing, claudeLaunchFlags, discoverClaude, loadModelSettings, loadWorkFolder, modelsFromProbe, pickFolder, probeModel, saveModel, saveRules, selectClaude, storeApiKey } from './engine';
import '../shared/primitives.css';
import './firstrun.css';

type Mode = 'local' | 'cloud' | 'claude';
type Probe =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'ok'; text: string; warning?: string }
  | { kind: 'bad'; text: string };

const LOCAL_ADDRESS = 'http://localhost:8000/v1';
const CLOUD_ADDRESS = 'https://';
const DEFAULT_FOLDER = 'Документы/HoH';
const STEPS = ['Модель', 'Правила'] as const;



type Doing = 'check' | 'save';
/** What the person sees when the protection window said no: what happened and what to press, by the CODE the host gave (`lib/hostError`). Never «unknown error». */
function refusalText(error: unknown, doing: Doing): string {
  return refusalWords(error, { what: doing === 'check' ? 'проверку' : 'сохранение', button: 'Дальше' });
}
function problemText(error: unknown): string {
  if (error instanceof MethodMissing) return `Эта версия пока не умеет: ${error.what}.`;
  return errorText(error, 'Что-то пошло не так. Попробуйте ещё раз.');
}

export interface FirstRunProps {
  /** Мастер закончен: открывается чат. */
  onFinish(): void;
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
  const [claudeState, setClaudeState] = useState({ ready: false, model: '' });
  const [claudeRun, setClaudeRun] = useState(0);
  // How the engine will start Claude (`effective.flags`): the caption of «Спрашивать разрешение» follows it, as in the chat.
  const [claudeFlags, setClaudeFlags] = useState<unknown>(undefined);
  const [internet, setInternet] = useState<'ask' | 'public'>('ask');
  const [folder, setFolder] = useState(DEFAULT_FOLDER);
  // The default folder is shown as the real path on this computer; until it is known (or if it cannot be) no folder name is invented.
  const [workFolder, setWorkFolder] = useState<string | null>(null);
  useEffect(() => { void loadWorkFolder().then(setWorkFolder); }, []);
  const detected = detectProvider(address, manualModel.trim() || model);
  // The engine's default model name «local» says nothing about a cloud service, so it is not shown beside its name.
  const shownModel = [manualModel.trim() || model].find(name => name && name.toLowerCase() !== 'local') ?? '';
  const folderShown = folder === DEFAULT_FOLDER ? (workFolder ?? 'Папка HoH') : folder;
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

  const cloud = mode === 'cloud';
  const claude = mode === 'claude';
  useEffect(() => { if (claude) void claudeLaunchFlags().then(setClaudeFlags); }, [claude, claudeRun]);
  const verified = claude ? claudeState.ready : probe.kind === 'ok';

  function invalidate() {
    checkId.current++;
    setProbe({ kind: 'idle' });
    setProblem('');
  }

  function chooseMode(next: Mode) {
    if (next === mode) return;
    setMode(next);
    if (next !== 'claude') setAddress(next === 'cloud' ? CLOUD_ADDRESS : LOCAL_ADDRESS);
    setClaudeState({ ready: false, model: '' });
    if (keyInput.current) keyInput.current.value = '';
    setKeyStored(false);
    invalidate();
  }

  /** Проверка адреса и ключа (её делает «Дальше»). Возвращает выбранную модель или null, если проверка не прошла. */
  async function check(): Promise<string | null> {
    if (pending.current) return null;
    const url = address.trim();
    const name = manualModel.trim();
    const bad = (text: string) => { setProbe({ kind: 'bad', text }); return null; };
    if (!/^https?:\/\/.{3,}/.test(url)) return bad('Проверьте адрес: он должен начинаться с http:// или https://');
    if (cloud && !keyStored && !keyInput.current?.value.trim()) return bad('Для облачного сервиса нужен ключ доступа');
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
        } catch (error) {
          throw new Error(refusalText(error, 'check') || 'Не удалось сохранить ключ. Попробуйте ещё раз.');
        } finally {
          key = '';
        }
        if (id !== checkId.current) return null;
      }
      const result = await probeModel(url, name);
      if (id !== checkId.current) return null;
      if (!result.ok) return bad(cloud ? 'По этому адресу никто не отвечает. Проверьте адрес и ключ.' : 'По этому адресу никто не отвечает. Если вы сами не запускали модель на этом компьютере, выберите другой способ или попросите того, кто ставил программу.');
      const selected = manualModel.trim() || modelsFromProbe(result)[0] || model.trim();
      if (!selected) return bad('Сервис не сообщил доступные модели. Укажите название в «Дополнительно» и проверьте ещё раз.');
      setModel(selected);
      setProbe({
        kind: 'ok',
        text: 'Отвечает. Можно продолжать',
        warning: result.toolCalls ? undefined : 'Помощник сможет только отвечать текстом: читать файлы и что-то делать на компьютере не будет.',
      });
      return selected;
    } catch (error) {
      if (id === checkId.current) return bad(refusalText(error, 'check') || (error instanceof Error && /^Вы не разрешили|^Время на ответ|^Не удалось сохранить ключ/.test(error.message) ? error.message : `Не удалось проверить. ${problemText(error)}`));
      return null;
    } finally {
      pending.current = false; setBusy(false);
    }
  }

  async function next() {
    if (pending.current || step > 2) return;
    setProblem('');
    // «Дальше» сама проверяет: отдельной кнопки проверки на этом пути нет.
    let checked = model.trim();
    let claudeModel = claudeState.model;
    if (step === 1 && !claude && !verified) {
      const found = await check();
      if (!found) return;
      checked = found;
    }
    if (step === 1 && claude && !claudeState.ready) {
      try {
        const view = discoverView(await discoverClaude());
        setClaudeRun(run => run + 1);
        if (!view.ready) { setProblem(view.headline); return; }
        claudeModel = view.defaultModel;
      } catch (error) { setProblem(`Не удалось проверить Claude. ${problemText(error)}`); return; }
    }
    pending.current = true;
    setBusy(true);
    try {
      if (step === 1) {
        if (claude) await selectClaude(claudeModel);
        else await saveModel(address.trim(), checked);
      } else if (step === 2) {
        const defaults = internet === 'ask' && folder === DEFAULT_FOLDER;
        try {
          await saveRules({ internet, folder });
        } catch (error) {
          // Выбор, отличный от обычного, молча терять нельзя.
          if (!(error instanceof MethodMissing) || !defaults) throw error;
        }
      }
      if (step === 2) onFinish(); else setStep(step + 1);
    } catch (error) {
      setProblem(refusalText(error, 'save') || `Не удалось сохранить. ${problemText(error)}`);
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
      setFolderNote(error instanceof MethodMissing ? `Выбор папки пока недоступен, работаем в ${folderShown}.` : problemText(error));
    } finally {
      pending.current = false; setBusy(false);
    }
  }

  const probeLine = probe.kind === 'checking' ? 'Проверяю…' : probe.kind === 'ok' ? probe.text : probe.kind === 'bad' ? probe.text : '';
  const probeIcon = probe.kind === 'checking' ? <Icon name="spinner" small spin /> : probe.kind === 'ok' ? <Icon name={probe.warning ? 'alert' : 'checkCircle'} small /> : probe.kind === 'bad' ? <Icon name="alert" small /> : null;
  const probeTone = probe.kind === 'ok' ? (probe.warning ? 'warn' : 'ok') : probe.kind === 'bad' ? 'bad' : '';
  const nextNote = problem;

  const addressField = (
        <div className="field">
          <label htmlFor="fr-address">{cloud ? 'Адрес сервиса' : 'Адрес модели'}</label>
          <input id="fr-address" type="text" value={address} spellCheck={false} autoComplete="off" onChange={e => { setAddress(e.target.value); invalidate(); }} />
          <span className="help">{cloud ? 'Адрес сервиса. Его даёт сервис или тот, кто его настраивал.' : (detected.scope === 'private' ? 'Модель в вашей сети: данные уходят с этого компьютера на другой в вашей сети, но не в интернет.' : 'Модель на этом компьютере: данные никуда не уходят.')}</span>
        </div>
  );

  return (
    <div className="hoh-page fr-page" data-screen="firstrun" data-step={step}>
      <div className="fr-wiz">
        <div className="fr-brand"><Emblem /><b>HoH</b></div>
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
              <h2 id="fr-h" ref={heading} tabIndex={-1}>Кто будет отвечать</h2>
              <p className="fr-sub">Помощнику нужна программа, которая понимает текст и отвечает.</p>
              <div className="fr-seg" role="group" aria-label="Кто будет отвечать">
                <button type="button" aria-pressed={mode === 'local'} onClick={() => chooseMode('local')}>Модель на этом компьютере</button>
                <button type="button" aria-pressed={cloud} onClick={() => chooseMode('cloud')}>Сервис в интернете (по ключу)</button>
                <button type="button" aria-pressed={claude} data-mode-claude onClick={() => chooseMode('claude')}><ProviderMark icon="claude" />Claude</button>
              </div>
              {claude ? <ClaudeConnect key={claudeRun} discover={discoverClaude} onChange={setClaudeState} /> : <>
              {cloud && addressField}
              <p className="fr-provider" data-provider-line data-provider={detected.id}><ProviderMark icon={detected.icon ?? ''} /><span>{detected.scope === 'cloud' || detected.scope === 'unknown' ? detected.name : detected.label}</span>{shownModel && detected.scope === 'cloud' && <span className="fr-provider-model">{shownModel}</span>}</p>
              {cloud && <p className="fr-note" data-data-line>Ваши сообщения и то, что Помощник прочитает в файлах, отправляются в этот сервис.</p>}
              <details className="fr-advanced">
                <summary>Дополнительно</summary>
                {!cloud && addressField}
                <div className="field">
                  <label htmlFor="fr-model">Название модели</label>
                  <input id="fr-model" type="text" value={manualModel} spellCheck={false} autoComplete="off" placeholder={model || 'Выбирается автоматически'} onChange={e => { setManualModel(e.target.value); invalidate(); }} />
                </div>
              </details>
              {cloud && <div className="field">
                <label htmlFor="fr-key">Ключ доступа</label>
                <input id="fr-key" type="password" ref={keyInput} autoComplete="off" placeholder={keyStored ? 'Ключ сохранён. Оставьте пустым, чтобы не менять' : ''} onChange={invalidate} />
                <span className="help" data-key-note>Длинная строка из личного кабинета сервиса; храните как пароль. {keyNote()}</span>
              </div>}
              <div className="fr-verify">
                <span className="fr-result" data-tone={probeTone} role="status">
                  {probeIcon}{probeLine}
                </span>
              </div>
              {probe.kind === 'ok' && probe.warning && <p className="fr-warning" role="status">{probe.warning}</p>}
              </>}
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
                  <div className="fr-path" aria-labelledby="fr-folder-label"><Icon name="folder" small />{folderShown}</div>
                  <button type="button" className="btn btn-lg" onClick={() => void choose()}>Выбрать</button>
                </div>
                {folderNote && <p className="fr-note" role="status">{folderNote}</p>}
              </div>
              <div className="fr-lock">
                <Icon name="alert" />
                <div><b>Опасное подтверждаю в отдельном окне</b>Удаление, запуск программ и отправка данных наружу идут только после вашего «Разрешить» в окне защиты. Так будет, пока вы сами не включили «Полный доступ».{claude && <> {confirmDescription(claudeFlags)}</>}</div>
              </div>
            </section>
          )}

          <div className="fr-foot">
            <button type="button" className={step === 1 ? 'btn fr-invisible' : 'btn'} disabled={busy || step === 1} onClick={() => { if (!pending.current) { setProblem(''); setStep(step - 1); } }}><Icon name="back" small />Назад</button>
            <span className="fr-foot-note" role={problem ? 'alert' : undefined}>
              {nextNote}
            </span>
            <button type="button" className="btn btn-primary btn-lg" disabled={busy} onClick={() => void next()}>{step === 2 ? 'Открыть чат' : 'Дальше'}</button>
          </div>
        </fieldset>
      </div>
    </div>
  );
}
