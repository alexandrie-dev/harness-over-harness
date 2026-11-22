// Главная окна HoH (HOH-41 В2б): приветствие с датой, «Now», «Done», «Needs your answer» над полем ввода T3.
// Это не отдельный экран: vite.ts подставляет <HomeBody/> в пустой разговор T3 (ChatView, DraftHeroHeadline), так что поле ввода,
// чип модели с уровнями, Permissions, выбор проекта и отправка — настоящие, их. Здесь только содержимое над ними.
// Данные — то, что хост знает о разговорах (window.__HOH_HOME__, ui/src/hoh-host: host.home); блоков без данных нет.
import { useNavigate } from '@tanstack/react-router';
import { useState, useSyncExternalStore } from 'react';

import { Button } from '~/components/ui/button';
import './home.css';

interface Entry {
  id: string; title: string; projectId: string; state: 'working' | 'waiting' | 'done' | 'ended';
  startedAt: number; doneAt: number | null; steps: { done: number; total: number };
  request?: { id: string; kind: 'approval' | 'question'; requestKind: string | null; text: string };
}
interface Home {
  version(): number; subscribe(listener: () => void): () => void; entries(): Entry[]; projectTitle(id: string): string;
  answer(threadId: string, requestId: string, decision: 'accept' | 'decline'): Promise<void>;
}
const ENVIRONMENT_ID = 'hoh-local';
const home = (): Home | undefined => (globalThis as { __HOH_HOME__?: Home }).__HOH_HOME__;

export function greeting(hour: number): string {
  return hour < 5 ? 'Good evening' : hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
}
const dateLabel = (date: Date) => date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
/** Title of the home without projects (NoProjectsHero.tsx, vite.ts): greeting and the date. */
export function Greeting() {
  const date = new Date();
  return <>{greeting(date.getHours())}<span className="mt-1 block text-sm font-normal text-muted-foreground">{dateLabel(date)}</span></>;
}
export function sameDay(a: number, b: number): boolean { return new Date(a).toDateString() === new Date(b).toDateString(); }
export function elapsed(from: number, to: number): string {
  const minutes = Math.max(0, Math.floor((to - from) / 60_000));
  return minutes < 1 ? '<1 min' : minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}
// T3 titles a request by its kind (TimelinePendingApprovalRow.tsx) and shows the command or file itself as the detail.
const REQUEST_LABELS: Record<string, string> = {
  command: 'Command approval', 'file-change': 'File change approval', 'file-read': 'File read approval',
  permission: 'App permission approval', 'mcp-elicitation': 'App access approval',
};
export function requestLabel(request: { kind: string; requestKind: string | null }): string {
  return request.kind === 'question' ? 'Question' : REQUEST_LABELS[request.requestKind ?? ''] ?? 'Command approval';
}
const clock = (at: number) => new Date(at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });

export function HomeBody() {
  const host = home();
  // One snapshot per change of any chat; entries are read after it.
  useSyncExternalStore(host?.subscribe ?? (() => () => {}), host?.version ?? (() => 0));
  const [now] = useState(() => Date.now());
  const clockNow = Math.max(now, Date.now());
  const navigate = useNavigate();

  const entries = host?.entries() ?? [];
  const working = entries.filter((entry) => entry.state === 'working' || entry.state === 'waiting').sort((a, b) => b.startedAt - a.startedAt);
  const done = entries.filter((entry) => entry.state === 'done' && entry.doneAt !== null && sameDay(entry.doneAt, clockNow)).sort((a, b) => (b.doneAt ?? 0) - (a.doneAt ?? 0));
  const asking = entries.filter((entry) => entry.request);
  const open = (threadId: string) => void navigate({ to: '/$environmentId/$threadId', params: { environmentId: ENVIRONMENT_ID, threadId } });
  const where = (entry: Entry) => host?.projectTitle(entry.projectId) ?? '';
  const date = new Date(clockNow);

  return (
    <div className="hoh-home" data-hoh-home-body="">
        <div className="hoh-home__inner">
          <div className="hoh-home__main">
            <div className="hoh-home__head">
              <h1>{greeting(date.getHours())}</h1>
              <div className="hoh-home__date">{dateLabel(date)}</div>
            </div>

            {working.length > 0 ? (
              <section>
                <h2 className="hoh-home__label">Now <span className="hoh-home__count">{working.length}</span></h2>
                <div className="hoh-home__cards">
                  {working.slice(0, 4).map((entry) => (
                    <button key={entry.id} type="button" className="hoh-card" data-state={entry.state} onClick={() => open(entry.id)}>
                      <span className="hoh-card__title">{entry.title}</span>
                      <span className="hoh-card__where">{where(entry)}</span>
                      <span className="hoh-card__state">
                        <b>{entry.state === 'working' ? <><span className="hoh-orbit" aria-hidden />Working</> : 'Waiting for your answer'}</b>
                        <span>{elapsed(entry.startedAt, clockNow)}</span>
                      </span>
                      {entry.steps.total > 0 ? (
                        <span className="hoh-steps" data-waiting={entry.state === 'waiting' || undefined} title={`${entry.steps.done} of ${entry.steps.total} steps`}>
                          {Array.from({ length: Math.min(entry.steps.total, 12) }, (_, index) => (
                            <i key={index} data-done={index < entry.steps.done || undefined} data-open={index >= entry.steps.done || undefined} />
                          ))}
                        </span>
                      ) : null}
                    </button>
                  ))}
                </div>
              </section>
            ) : null}

            {done.length > 0 ? (
              <section>
                <h2 className="hoh-home__label">Done <span className="hoh-home__count">{done.length}</span></h2>
                {done.map((entry) => (
                  <div key={entry.id} className="hoh-row">
                    <time>{clock(entry.doneAt ?? 0)}</time>
                    <span>{entry.title}</span>
                    <small>{where(entry)}</small>
                    <button type="button" className="hoh-link" onClick={() => open(entry.id)}>Open</button>
                  </div>
                ))}
              </section>
            ) : null}

          </div>

          {asking.length > 0 ? (
            <aside className="hoh-home__side">
              <div className="hoh-ask">
                <div className="hoh-ask__head"><span>Needs your answer</span><span className="hoh-ask__count">{asking.length}</span></div>
                {asking.map((entry) => (
                  <div key={entry.request!.id} className="hoh-ask__item">
                    <div className="hoh-ask__where"><span>{entry.title}</span><span>{elapsed(entry.startedAt, clockNow)}</span></div>
                    <div className="hoh-ask__kind">{requestLabel(entry.request!)}</div>
                    <code className="hoh-ask__text" data-approval-detail="complete">{entry.request!.text || requestLabel(entry.request!)}</code>
                    {entry.request!.kind === 'approval' ? (
                      <div className="hoh-ask__actions">
                        <Button variant="outline" size="sm" onClick={() => void host?.answer(entry.id, entry.request!.id, 'decline')}>Decline</Button>
                        <Button size="sm" onClick={() => void host?.answer(entry.id, entry.request!.id, 'accept')}>Approve</Button>
                      </div>
                    ) : (
                      <div className="hoh-ask__actions"><Button variant="outline" size="sm" onClick={() => open(entry.id)}>Answer in the thread</Button></div>
                    )}
                  </div>
                ))}
              </div>
            </aside>
          ) : null}
        </div>
    </div>
  );
}
