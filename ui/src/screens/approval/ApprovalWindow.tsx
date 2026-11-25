import { tidyHuman } from '../../lib/humanText';
import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { Risk } from '../../protocol';
import { Icon } from '../shared/icons';
import { approvalHost, type ApprovalView, type WindowDecision } from './bridge';
import '../shared/primitives.css';
import './approval.css';

/** Пока кнопка «Allow once» не нажимается, мс: нельзя разрешить, не прочитав. */
export const ALLOW_DELAY_MS = 1000;
/** Счёт времени показывается только в последнюю минуту: до неё окно человека не торопит. */
export const LAST_MINUTE_S = 60;

function approvalError(error: unknown, fallback: string): string {
  const value = typeof error === 'string' ? error : error instanceof Error ? error.message :
    error && typeof error === 'object' && 'message' in error ? String(error.message) : '';
  const known: Record<string, string> = {
    'Подтверждение недоступно': 'Confirmation is unavailable.',
    'Это окно не может открыть подтверждение': 'This window cannot open the confirmation.',
    'Это окно не может ответить на подтверждение': 'This window cannot answer the confirmation.',
    'Подтверждение уже сменилось': 'The confirmation request has changed.',
    'Сначала прочитайте подтверждение': 'Read the confirmation first.',
    'Подтверждение истекло': 'The confirmation has expired.',
    'Часть команды скрыта, в ней есть вложенная команда: разрешить один раз нельзя': 'Part of the command is hidden and contains a nested command; Allow once is unavailable.',
  };
  return known[value] ?? (value && value.length <= 160 && !/[А-Яа-яЁё]/u.test(value) ? value : fallback);
}

const WHY: Record<Risk | 'unassessed', { title: string; text: string }> = {
  unassessed: {
    title: 'Why the assistant is asking',
    text: 'The assistant cannot assess this action in advance. Review what it will do in the details below. If you did not request it, select Decline.',
  },
  high: {
    title: 'Why this could be risky',
    text: 'This action may be irreversible or affect your data and permissions. It runs immediately after you select Allow once. If you did not request it, select Decline.',
  },
  medium: {
    title: 'Why this could be risky',
    text: 'This action changes something on this computer or accesses the internet. Allow it only if you expected it.',
  },
  low: {
    title: 'Why the assistant is asking',
    text: 'This is a routine action, but the assistant needs your approval to continue.',
  },
};

// ApprovalView has no structured action kind. Only explicit engine-authored
// action wording gets a specific consequence; unknown wording keeps the fallback.
export function consequence(view: ApprovalView): { title: string; text: string } {
  const text = view.humanText.toLowerCase();
  const title = 'Why this could be risky';
  if (view.risk === 'unassessed') return WHY.unassessed;
  if (/(?:\b(?:delete|remove|erase)\s+(?:a\s+)?(?:files?|documents?)\b|(?:^|[^\p{L}])(?:удалить|удаление|стереть|стирание)(?:$|[^\p{L}]))/u.test(text))
    return { title, text: 'The files will not go to the trash and cannot be restored.' };
  if (/(?:\b(?:send|upload|transmit)\s+(?:data|files?|documents?)\b|(?:отправить|передать|отправка)\s+(?:данные|файл|файлы|документ))/u.test(text))
    return { title, text: 'Another service will receive the data. It cannot be recalled after sending.' };
  if (/(?:\b(?:run|execute)\s+(?:a\s+)?(?:program|command)\b|(?:запустить|выполнить)\s+(?:программу|команду))/u.test(text))
    return { title, text: 'The program will run on this computer and can change files it can access.' };
  // «Прежний вариант» есть только у файла, который уже существует: слова «изменить» и «перезаписать» его называют, «записать» — нет.
  if (/(?:\b(?:modify|overwrite|edit)\s+(?:a\s+)?(?:files?|documents?)\b|(?:изменить|перезаписать)\s+(?:файл|файлы|документ))/u.test(text))
    return { title, text: 'The file content will change. The previous version may be lost.' };
  if (/(?:\bwrite\s+(?:a\s+)?(?:files?|documents?)\b|записать\s+(?:файл|файлы|документ))/u.test(text))
    return { title, text: 'The file will be written. If it already exists, its previous contents will be replaced.' };
  return WHY[view.risk];
}

/** A request the person started themselves (the settings of the first run): it is not dangerous and the assistant did not ask for it. */
export const isOwnAction = (view: ApprovalView) => view.approvalId.startsWith('native-preflight-');
const OWN_WHY = 'You are changing these settings. This window prevents someone else from changing them for you.';

const RISK_LABEL: Record<Risk | 'unassessed', string> = { high: 'High risk', medium: 'Medium risk', low: 'Low risk', unassessed: 'Risk not assessed' };

/** Слово «исполнитель» — слово ядра, не человека: в окне защиты вместо «Исполнитель «claude»» стоит имя, как его знает человек. */
export function plainWords(text: string): string {
  return text
    .replace(/исполнител[а-яё]*\s*[«"]([^»"]*)[»"]/giu, (_, name: string) => (name.trim().toLowerCase() === 'claude' ? 'Claude' : name.trim() || 'Assistant'))
    .replace(/исполнител[а-яё]*/giu, 'Assistant');
}

/** A request of Claude (the engine's executor): its command is the whole point, so «Details» are open and nothing is said twice. */
export const fromExecutor = (view: ApprovalView) => view.risk === 'unassessed' || /^(?:исполнитель\s*[«"]?claude|claude\b)/iu.test(view.humanText.trim());

type Phase =
  | { kind: 'loading' }
  | { kind: 'error'; text: string; view?: ApprovalView }
  | { kind: 'pending'; view: ApprovalView }
  | { kind: 'done'; title: string; note: string };

function clock(seconds: number): string {
  const s = Math.max(0, Math.ceil(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

export function ApprovalWindow() {
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [now, setNow] = useState(() => Date.now());
  const [armed, setArmed] = useState(false);
  const [folderTrust, setFolderTrust] = useState(false);
  const rejectRef = useRef<HTMLButtonElement>(null);
  const total = useRef(1);
  const sent = useRef(false);
  const decideRef = useRef<(outcome: WindowDecision) => void>(() => {});
  const [sending, setSending] = useState(false);
  // «Details» of a request of Claude start open; once the person opens or closes them, their choice stays.
  const [detailsOpen, setDetailsOpen] = useState<boolean | null>(null);
  const view = phase.kind === 'pending' || phase.kind === 'error' ? phase.view ?? null : null;
  const expired = !!view && Date.now() >= view.expiresAt;
  const own = !!view && isOwnAction(view);

  useEffect(() => {
    let alive = true;
    approvalHost().current().then(
      v => { if (alive) { setFolderTrust(v.folderTrust === true); total.current = Math.max(1, v.expiresAt - Date.now()); setNow(Date.now()); setPhase({ kind: 'pending', view: v }); } },
      e => { if (alive) setPhase({ kind: 'error', text: approvalError(e, 'Could not load the request.') }); },
    );
    return () => { alive = false; };
  }, []);

  // Фокус — на безопасной кнопке; «Разрешить» оживает через секунду после показа.
  useEffect(() => {
    if (!view || expired) return;
    rejectRef.current?.focus();
    const timer = setTimeout(() => setArmed(true), ALLOW_DELAY_MS);
    return () => clearTimeout(timer);
  }, [view, expired]);

  // Секундомер до конца срока; по истечении движок сам ставит «отменено».
  useEffect(() => {
    if (!view) return;
    const tick = setInterval(() => {
      const t = Date.now();
      setNow(t);
      if (t >= view.expiresAt) clearInterval(tick);
    }, 250);
    return () => clearInterval(tick);
  }, [view]);

  async function decide(outcome: WindowDecision) {
    if (!view || sent.current) return;
    if (Date.now() >= view.expiresAt) { setNow(Date.now()); return; }
    if (outcome === 'allowed-once' && !armed) return;
    sent.current = true; setSending(true);
    try {
      await approvalHost().decide(view.approvalId, outcome);
      setPhase(outcome === 'allowed-once'
        ? { kind: 'done', title: folderTrust ? 'Confirmed' : 'Allowed once', note: folderTrust ? 'Your choice was saved.' : own ? 'The setting will be saved. The window will ask again next time.' : 'The assistant will perform this action. It will ask again next time.' }
        : { kind: 'done', title: folderTrust ? 'Declined' : 'Declined', note: folderTrust ? 'Nothing changed.' : own ? 'Nothing changed.' : 'The assistant was declined. Nothing changed.' });
    } catch (e) {
      sent.current = false;
      setPhase({ kind: 'error', view, text: folderTrust ? 'Could not send your answer.' : approvalError(e, 'Could not send your answer.') });
    } finally {
      setSending(false);
    }
  }
  decideRef.current = outcome => { void decide(outcome); };

  // Esc/pagehide retry rejection while pending, including after send failure.
  // Native-close rejection is guaranteed by the kernel, not by this page.
  useEffect(() => {
    if (!view) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.preventDefault(); decideRef.current('rejected'); } };
    const onHide = () => { decideRef.current('rejected'); };
    window.addEventListener('keydown', onKey);
    window.addEventListener('pagehide', onHide);
    return () => { window.removeEventListener('keydown', onKey); window.removeEventListener('pagehide', onHide); };
  }, [view?.approvalId]);

  const headline = view ? (view.humanText.startsWith('The folder for this chat has changed: ')
    ? view.humanText : plainWords(tidyHuman(view.humanText))) : undefined;
  const reason = view?.reason ? plainWords(view.reason.trim()) : '';
  const executorAsks = !!view && fromExecutor(view);
  // The explanation is shown when it says something the headline does not; for Claude's requests silence is not a message.
  const showSaid = !own && reason !== headline && (executorAsks ? reason !== '' : true);
  const left = view ? (view.expiresAt - now) / 1000 : 0;
  const share = view ? Math.min(1, Math.max(0, (view.expiresAt - now) / total.current)) : 0;
  const why = view ? consequence(view) : WHY.high;

  return (
    <div className="hoh-page ap-page" data-screen="approval" data-phase={expired ? 'done' : phase.kind}>
      <div className="ap-guard" role="alertdialog" aria-labelledby="ap-what" aria-describedby="ap-why">
        <div className="ap-title"><Icon name="alert" small />{folderTrust ? 'HoH confirmation' : 'HoH confirmation'}<small>{folderTrust ? 'Separate from the conversation' : 'Separate from the conversation'}</small></div>

        {phase.kind === 'loading' && <div className="ap-done" role="status">Loading request…</div>}
        {!expired && phase.kind === 'error' && <div className="ap-done" role="alert">{phase.text}<small>{folderTrust ? 'Close this window to decline.' : 'Your answer was not sent. Close this window to decline.'}</small></div>}
        {expired && <div className="ap-done" role="status">{folderTrust ? 'Confirmation expired' : 'Confirmation expired; the action was declined'}<small>{folderTrust ? 'Nothing changed.' : own ? 'Nothing changed.' : 'The assistant was declined. Nothing changed.'}</small></div>}
        {phase.kind === 'done' && <div className="ap-done" role="status">{phase.title}<small>{phase.note}</small></div>}

        {view && !expired && (
          <>
            <div className="ap-body">
              {own
                ? <p className="ap-ask" data-own-action>{folderTrust ? 'Confirm your choice' : 'Confirm your action'}</p>
                : <p className="ap-ask">The assistant is requesting permission <span className="ap-risk" data-risk={view.risk}>{RISK_LABEL[view.risk]}</span></p>}
              <h1 className="ap-what" id="ap-what">{headline}</h1>
              <div className="ap-why" id="ap-why" data-risk={own ? 'own' : view.risk}>
                <Icon name={own ? 'shield' : 'alert'} />
                {own ? <div>{folderTrust ? 'HoH asks you to confirm changes to folder trust.' : OWN_WHY}</div> : <div><h2>{why.title}</h2>{why.text}</div>}
              </div>
              {showSaid && <div className="ap-said">
                <h2>Assistant explanation</h2>
                {reason ? <p>{reason}</p> : <p className="ap-said-empty">The assistant did not explain why this is needed.</p>}
              </div>}
              {view.details?.trim() && <details className="ap-details" open={detailsOpen ?? executorAsks} data-details-open={(detailsOpen ?? executorAsks) ? 'true' : 'false'} onToggle={event => setDetailsOpen(event.currentTarget.open)}><summary>Details</summary><pre>{view.details}</pre></details>}
              {left > LAST_MINUTE_S
                ? <p className="ap-timer-note" data-calm>{folderTrust ? 'This request expires if you leave it unanswered.' : 'Take your time. The action will be declined if you do not answer.'}</p>
                : <>
                  <div className="ap-timer"><Icon name="clock" small /><span>{folderTrust ? 'Time left' : 'Time left'} <b data-testid="timer">{clock(left)}</b></span></div>
                  <div className="ap-bar" aria-hidden="true"><i style={{ '--share': share } as CSSProperties} /></div>
                  <p className="ap-timer-note">{folderTrust ? 'This request expires if you leave it unanswered.' : 'The action will be declined if you do not answer.'}</p>
                </>}
            </div>
            <div className="ap-actions">
              <button ref={rejectRef} type="button" className="btn ap-safe" disabled={sending} onClick={() => void decide('rejected')}>{folderTrust ? 'Decline' : 'Decline'}</button>
              <button type="button" className="btn ap-risky" disabled={!armed || sending} onClick={() => void decide('allowed-once')}>{folderTrust ? (view.humanText.startsWith('Revoke trust for project settings in ') ? 'Revoke trust' : 'Trust folder') : 'Allow once'}</button>
            </div>
          </>
        )}

        <div className="ap-foot"><Icon name="shield" small /><span>{folderTrust ? 'Only you can confirm this request. Closing the window declines it.' : 'This is a separate HoH confirmation window. The conversation cannot open or control it. Closing the window declines the request.'}</span></div>
      </div>
    </div>
  );
}
