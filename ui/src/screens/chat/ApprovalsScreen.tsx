import { tidyHuman } from '../../lib/humanText';
import { useEffect, useState } from 'react';
import { Icon } from '../shared/icons';
import { chatTransport, type ApprovalRecord } from './transport';

const OUTCOME: Record<string, string> = { 'allowed-once': 'Разрешено один раз', rejected: 'Отклонено', cancelled: 'Без ответа', expired: 'Без ответа' };
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
  return <section className="ap-history" aria-label="Подтверждения">
    <div className="ap-history-body" data-approvals>
      {failed ? <p className="ap-history-empty" role="alert">Не удалось загрузить историю решений.</p>
        : !records ? <p className="ap-history-empty">Загружаю историю…</p>
        : records.length === 0 ? <p className="ap-history-empty" data-approvals-empty>Решений пока нет. Когда Помощник попросит разрешение, оно появится здесь.</p>
        : <ol className="ap-history-list">{records.map(record => <li key={`${record.at}-${record.method}-${record.outcome}-${record.humanText}`} className="ap-history-row" data-outcome={record.outcome}>
          <Icon name={record.outcome === 'allowed-once' ? 'checkCircle' : 'shield'} className="ap-history-icon" />
          <div className="ap-history-text">
            <p className="ap-history-what">{tidyHuman(record.humanText)}</p>
            <p className="ap-history-meta">{record.asker === 'assistant' ? 'Просил Помощник' : 'Вы, в настройках'} · {when(record.at)}</p>
            {record.reason && <p className="ap-history-reason">«{record.reason}»</p>}
            {record.details && <details className="ap-history-details"><summary>Подробности</summary><pre>{record.details}</pre></details>}
          </div>
          <span className="ap-history-outcome">{OUTCOME[record.outcome] ?? 'Решение принято'}</span>
        </li>)}</ol>}
    </div>
  </section>;
}
