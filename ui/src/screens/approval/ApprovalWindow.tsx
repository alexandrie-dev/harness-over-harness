import { tidyHuman } from '../../lib/humanText';
import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { Risk } from '../../protocol';
import { Icon } from '../shared/icons';
import { approvalHost, type ApprovalView, type WindowDecision } from './bridge';
import '../shared/primitives.css';
import './approval.css';

/** Пока кнопка «Разрешить один раз» не нажимается, мс: нельзя разрешить, не прочитав. */
export const ALLOW_DELAY_MS = 1000;
/** Счёт времени показывается только в последнюю минуту: до неё окно человека не торопит. */
export const LAST_MINUTE_S = 60;

const WHY: Record<Risk, { title: string; text: string }> = {
  high: {
    title: 'Почему это опасно',
    text: 'Это действие, возможно, нельзя будет отменить, или оно затрагивает ваши данные и права. После «Разрешить один раз» оно выполнится сразу. Если вы не просили об этом, нажмите «Отклонить».',
  },
  medium: {
    title: 'Почему это опасно',
    text: 'Действие меняет что-то на этом компьютере или выходит в интернет. Разрешайте, только если ждали именно этого.',
  },
  low: {
    title: 'Почему Помощник спрашивает',
    text: 'Действие обычное, но без вашего слова Помощник его не выполняет.',
  },
};

// ApprovalView has no structured action kind. Only explicit engine-authored
// action wording gets a specific consequence; unknown wording keeps the fallback.
export function consequence(view: ApprovalView): { title: string; text: string } {
  const text = view.humanText.toLowerCase();
  const title = 'Почему это опасно';
  if (/(?:^|[^\p{L}])(?:удалить|удаление|стереть|стирание)(?:$|[^\p{L}])/u.test(text))
    return { title, text: 'Файлы не попадут в корзину. Восстановить их будет нельзя.' };
  if (/(?:отправить|передать|отправка)\s+(?:данные|файл|файлы|документ)/u.test(text))
    return { title, text: 'Данные получит другой сервис. После отправки забрать их обратно нельзя.' };
  if (/(?:запустить|выполнить)\s+(?:программу|команду)/u.test(text))
    return { title, text: 'Программа выполнится на этом компьютере и сможет изменить доступные ей файлы.' };
  if (/(?:записать|изменить|перезаписать)\s+(?:файл|файлы|документ)/u.test(text))
    return { title, text: 'Содержимое файла изменится. Прежний вариант может быть потерян.' };
  return WHY[view.risk];
}

/** A request the person started themselves (the settings of the first run): it is not dangerous and the assistant did not ask for it. */
export const isOwnAction = (view: ApprovalView) => view.approvalId.startsWith('native-preflight-');
const OWN_WHY = 'Это вы меняете настройки. Окно нужно, чтобы никто не поменял их за вас.';

const RISK_LABEL: Record<Risk, string> = { high: 'Высокий риск', medium: 'Средний риск', low: 'Низкий риск' };

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
  const rejectRef = useRef<HTMLButtonElement>(null);
  const total = useRef(1);
  const sent = useRef(false);
  const decideRef = useRef<(outcome: WindowDecision) => void>(() => {});
  const [sending, setSending] = useState(false);
  const view = phase.kind === 'pending' || phase.kind === 'error' ? phase.view ?? null : null;
  const expired = !!view && Date.now() >= view.expiresAt;
  const own = !!view && isOwnAction(view);

  useEffect(() => {
    let alive = true;
    approvalHost().current().then(
      v => { if (alive) { total.current = Math.max(1, v.expiresAt - Date.now()); setNow(Date.now()); setPhase({ kind: 'pending', view: v }); } },
      e => { if (alive) setPhase({ kind: 'error', text: e instanceof Error ? e.message : 'Не удалось получить запрос.' }); },
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
        ? { kind: 'done', title: 'Разрешено один раз', note: own ? 'Настройка будет сохранена. В следующий раз окно спросит снова.' : 'Помощник выполнит это действие. В следующий раз он спросит снова.' }
        : { kind: 'done', title: 'Отклонено', note: own ? 'Ничего не изменилось.' : 'Помощник получил отказ. Ничего не изменилось.' });
    } catch (e) {
      sent.current = false;
      setPhase({ kind: 'error', view, text: e instanceof Error ? e.message : 'Не удалось передать ответ.' });
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

  const headline = view ? tidyHuman(view.humanText) : undefined;
  const left = view ? (view.expiresAt - now) / 1000 : 0;
  const share = view ? Math.min(1, Math.max(0, (view.expiresAt - now) / total.current)) : 0;
  const why = view ? consequence(view) : WHY.high;

  return (
    <div className="hoh-page ap-page" data-screen="approval" data-phase={expired ? 'done' : phase.kind}>
      <div className="ap-guard" role="alertdialog" aria-labelledby="ap-what" aria-describedby="ap-why">
        <div className="ap-title"><Icon name="alert" small />Защита HoH<small>Отдельное окно, не часть чата</small></div>

        {phase.kind === 'loading' && <div className="ap-done" role="status">Загружаю запрос…</div>}
        {!expired && phase.kind === 'error' && <div className="ap-done" role="alert">{phase.text}<small>Ответ не отправлен. Закройте окно: это отказ.</small></div>}
        {expired && <div className="ap-done" role="status">Время вышло, действие отклонено<small>{own ? 'Ничего не изменилось.' : 'Помощник получил отказ. Ничего не изменилось.'}</small></div>}
        {phase.kind === 'done' && <div className="ap-done" role="status">{phase.title}<small>{phase.note}</small></div>}

        {view && !expired && (
          <>
            <div className="ap-body">
              {own
                ? <p className="ap-ask" data-own-action>Подтвердите своё действие</p>
                : <p className="ap-ask">Помощник просит разрешение <span className="ap-risk" data-risk={view.risk}>{RISK_LABEL[view.risk]}</span></p>}
              <h1 className="ap-what" id="ap-what">{headline}</h1>
              <div className="ap-why" id="ap-why" data-risk={own ? 'own' : view.risk}>
                <Icon name={own ? 'shield' : 'alert'} />
                {own ? <div>{OWN_WHY}</div> : <div><h2>{why.title}</h2>{why.text}</div>}
              </div>
              {!own && <div className="ap-said">
                <h2>Объяснение Помощника</h2>
                {view.reason?.trim() ? <p>{view.reason}</p> : <p className="ap-said-empty">Помощник не объяснил, зачем ему это нужно.</p>}
              </div>}
              {view.details?.trim() && <details className="ap-details"><summary>Подробности</summary><pre>{view.details}</pre></details>}
              {left > LAST_MINUTE_S
                ? <p className="ap-timer-note" data-calm>Можно не спешить. Если не ответить, действие отклонится само.</p>
                : <>
                  <div className="ap-timer"><Icon name="clock" small /><span>Осталось <b data-testid="timer">{clock(left)}</b></span></div>
                  <div className="ap-bar" aria-hidden="true"><i style={{ '--share': share } as CSSProperties} /></div>
                  <p className="ap-timer-note">Если не ответить, действие отклонится само.</p>
                </>}
            </div>
            <div className="ap-actions">
              <button ref={rejectRef} type="button" className="btn ap-safe" disabled={sending} onClick={() => void decide('rejected')}>Отклонить</button>
              <button type="button" className="btn ap-risky" disabled={!armed || sending} onClick={() => void decide('allowed-once')}>Разрешить один раз</button>
            </div>
          </>
        )}

        <div className="ap-foot"><Icon name="shield" small /><span>Это окно защиты HoH. Чат не может его открыть, нажать за вас или изменить. Закрыть окно — то же, что отклонить.</span></div>
      </div>
    </div>
  );
}
