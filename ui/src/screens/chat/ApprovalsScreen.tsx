import { tidyHuman } from '../../lib/humanText';
import { useEffect, useState } from 'react';
import { Icon } from '../shared/icons';
import { chatTransport, type ApprovalRecord } from './transport';

const OUTCOME: Record<string, string> = { 'allowed-once': 'Разрешено один раз', rejected: 'Не разрешено', cancelled: 'Без ответа', expired: 'Без ответа' };
/** A record as something that happened: «Вы разрешили: …», «Вы не разрешили: …». The engine's wording is a question («Удалить 2 файла?»). */
function pastTense(record: ApprovalRecord): string {
  const what = tidyHuman(record.humanText).replace(/\?\s*$/, '');
  const lower = what.charAt(0).toLowerCase() + what.slice(1);
  if (record.outcome === 'allowed-once') return `Вы разрешили: ${lower}`;
  if (record.outcome === 'rejected') return `Вы не разрешили: ${lower}`;
  return `Вы не ответили, поэтому не разрешено: ${lower}`;
}
const when = (at: number) => new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' }).format(at);

/** The decisions of the person: who asked, what, when and the answer. Deciding happens in the separate window. */
export function ApprovalsScreen() {
  const [records, setRecords] = useState<ApprovalRecord[] | undefined>();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let live = true;
    const history = chatTransport().approvalHistory;
    if (!history) { setFailed(true); return; }
    history().then(rows => { if (live) setRecords(rows); }, () => { if (live) setFailed(true); });
    return () => { live = false; };
  }, []);
  const row = (record: ApprovalRecord) => <li key={`${record.at}-${record.method}-${record.outcome}-${record.humanText}`} className="ap-history-row" data-outcome={record.outcome}>
    <Icon name={record.outcome === 'allowed-once' ? 'checkCircle' : 'shield'} className="ap-history-icon" />
    <div className="ap-history-text">
      <p className="ap-history-what">{pastTense(record)}</p>
      <p className="ap-history-meta">{record.asker === 'assistant' ? 'Просил Помощник' : 'Вы, в настройках'} · {when(record.at)}</p>
      {record.reason && <p className="ap-history-reason">«{record.reason}»</p>}
      {record.details && <details className="ap-history-details"><summary>Подробности</summary><pre>{record.details}</pre></details>}
    </div>
    <span className="ap-history-outcome">{OUTCOME[record.outcome] ?? 'Решение принято'}</span>
  </li>;
  const asked = (records ?? []).filter(record => record.asker !== 'you');
  const own = (records ?? []).filter(record => record.asker === 'you');
  return <section className="ap-history" aria-label="Разрешения">
    <div className="ap-history-body" data-approvals>
      {failed ? <p className="ap-history-empty" role="alert">Не удалось загрузить историю решений.</p>
        : !records ? <p className="ap-history-empty">Загружаю историю…</p>
        : records.length === 0 ? <p className="ap-history-empty" data-approvals-empty>Пока вы ничего не разрешали и не запрещали. Когда Помощник попросит разрешение, оно появится здесь.</p>
        : <>
          {asked.length > 0 ? <ol className="ap-history-list">{asked.map(row)}</ol> : <p className="ap-history-empty" data-approvals-none-asked>Помощник пока ничего не просил.</p>}
          {own.length > 0 && <details className="ap-history-own" data-approvals-own><summary>Ваши настройки ({own.length})</summary><ol className="ap-history-list">{own.map(row)}</ol></details>}
        </>}
    </div>
  </section>;
}
