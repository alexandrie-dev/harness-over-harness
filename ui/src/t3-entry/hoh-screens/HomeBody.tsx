// Главная окна HoH (HOH-41 В2б, раунд дизайна): приветствие с датой, запущенные агенты и их субагенты, «Done», «Needs your answer» над полем ввода T3.
// Подача — карточки (вариант A, выбор основателя): агент — карточка, субагенты — строки внутри неё, запросы ждут справа.
// Это не отдельный экран: vite.ts подставляет <HomeBody/> в пустой разговор T3 (ChatView, DraftHeroHeadline), так что поле ввода,
// чип модели с уровнями, Permissions, выбор проекта и отправка — настоящие, их. Здесь только содержимое над ними.
// Данные — то, что хост знает о разговорах (window.__HOH_HOME__, ui/src/hoh-host: host.home); блоков без данных нет.
import { useNavigate } from '@tanstack/react-router';
import { CheckIcon, XIcon } from 'lucide-react';
import { useEffect, useState, useSyncExternalStore } from 'react';

import { ClaudeAI } from '~/components/Icons';
import { Button } from '~/components/ui/button';
import './home.css';

interface Entry {
  id: string; title: string; projectId: string; state: 'working' | 'waiting' | 'done' | 'ended';
  startedAt: number; doneAt: number | null; steps: { done: number; total: number };
  request?: { id: string; kind: 'approval' | 'question'; requestKind: string | null; text: string };
  agents: Agent[];
}
interface Agent { id: string; name: string; kind: string; state: 'working' | 'done' | 'failed'; startedAt: number; doneAt: number | null }
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


interface View { host: Home | undefined; now: number; open(threadId: string): void; where(entry: Entry): string }

/** The executor's mark: Claude's logo from T3's set (the host runs only Claude). */
const Mark = () => <ClaudeAI className="hoh-mark" role="img" aria-label="Claude" />;

function Steps({ entry }: { entry: Entry }) {
  if (entry.steps.total === 0) return null;
  return (
    <span className="hoh-steps" data-waiting={entry.state === 'waiting' || undefined} title={`${entry.steps.done} of ${entry.steps.total} steps`}>
      {Array.from({ length: Math.min(entry.steps.total, 12) }, (_, index) => (
        <i key={index} data-done={index < entry.steps.done || undefined} data-open={index >= entry.steps.done || undefined} />
      ))}
    </span>
  );
}
function StateDot({ state }: { state: Agent['state'] }) {
  return state === 'working' ? <span className="hoh-orbit" aria-hidden /> : state === 'done' ? <CheckIcon className="hoh-tick" aria-hidden /> : <XIcon className="hoh-tick" data-failed="" aria-hidden />;
}
function Sub({ agent, now }: { agent: Agent; now: number }) {
  return (
    <div className="hoh-sub" data-state={agent.state} title={`${agent.kind}: ${agent.name}`}>
      <StateDot state={agent.state} /><span className="hoh-sub__name">{agent.name}</span><small>{agent.kind}</small>
      <time>{elapsed(agent.startedAt, agent.doneAt ?? now)}</time>
    </div>
  );
}
/** What Claude asks, as T3 shows its request: the kind, the command or file itself, the chat's own Approve / Decline. */
function Ask({ entry, view }: { entry: Entry; view: View }) {
  const request = entry.request!;
  return (
    <div className="hoh-ask__item">
      <div className="hoh-ask__kind">{requestLabel(request)}</div>
      <code className="hoh-ask__text" data-approval-detail="complete">{request.text || requestLabel(request)}</code>
      {request.kind === 'approval' ? (
        <div className="hoh-ask__actions">
          <Button variant="outline" size="sm" onClick={() => void view.host?.answer(entry.id, request.id, 'decline')}>Decline</Button>
          <Button size="sm" onClick={() => void view.host?.answer(entry.id, request.id, 'accept')}>Approve</Button>
        </div>
      ) : (
        <div className="hoh-ask__actions"><Button variant="outline" size="sm" onClick={() => view.open(entry.id)}>Answer in the thread</Button></div>
      )}
    </div>
  );
}
const press = (open: () => void) => ({ role: 'button' as const, tabIndex: 0, onClick: open, onKeyDown: (event: React.KeyboardEvent) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); open(); } } });
const stateLabel = (entry: Entry) => entry.state === 'working' ? <><span className="hoh-orbit" aria-hidden />Working</> : 'Waiting for your answer';
const DoneRows = ({ done, view }: { done: Entry[]; view: View }) => done.length === 0 ? null : (
  <section>
    <h2 className="hoh-home__label">Done <span className="hoh-home__count">{done.length}</span></h2>
    {done.map((entry) => (
      <div key={entry.id} className="hoh-row">
        <time>{clock(entry.doneAt ?? 0)}</time>
        <span><Mark />{entry.title}</span>
        <small>{view.where(entry)}</small>
        <button type="button" className="hoh-link" onClick={() => view.open(entry.id)}>Open</button>
      </div>
    ))}
  </section>
);
const Count = ({ list }: { list: Entry[] }) => { const subs = list.reduce((sum, entry) => sum + entry.agents.length, 0); return <span className="hoh-home__count">{list.length}{subs > 0 ? ` · ${subs} subagent${subs === 1 ? '' : 's'}` : ''}</span>; };

/** Cards: an agent is a card, its subagents are lines inside it; the requests wait on the right. */
function Cards({ working, done, asking, view }: Lists) {
  return (
    <div className="hoh-home__inner">
      <div className="hoh-home__main">
        <Head now={view.now} />
        {working.length > 0 ? (
          <section>
            <h2 className="hoh-home__label">Now <Count list={working} /></h2>
            <div className="hoh-home__cards">
              {working.slice(0, 6).map((entry) => (
                <div key={entry.id} className="hoh-card" data-state={entry.state} {...press(() => view.open(entry.id))}>
                  <span className="hoh-card__top"><Mark /><span className="hoh-card__title">{entry.title}</span></span>
                  <span className="hoh-card__where">{view.where(entry)}</span>
                  {entry.agents.length > 0 ? <span className="hoh-subs">{entry.agents.map((agent) => <Sub key={agent.id} agent={agent} now={view.now} />)}</span> : null}
                  <span className="hoh-card__state"><b>{stateLabel(entry)}</b><span>{elapsed(entry.startedAt, view.now)}</span></span>
                  <Steps entry={entry} />
                </div>
              ))}
            </div>
          </section>
        ) : null}
        <DoneRows done={done} view={view} />
      </div>
      {asking.length > 0 ? (
        <aside className="hoh-home__side">
          <div className="hoh-ask">
            <div className="hoh-ask__head"><span>Needs your answer</span><span className="hoh-ask__count">{asking.length}</span></div>
            {asking.map((entry) => (
              <div key={entry.request!.id} className="hoh-ask__block">
                <div className="hoh-ask__where"><span><Mark />{entry.title}</span><span>{elapsed(entry.startedAt, view.now)}</span></div>
                <Ask entry={entry} view={view} />
              </div>
            ))}
          </div>
        </aside>
      ) : null}
    </div>
  );
}

interface Lists { working: Entry[]; done: Entry[]; asking: Entry[]; view: View }
function Head({ now }: { now: number }) {
  const date = new Date(now);
  return (
    <div className="hoh-home__head">
      <h1>{greeting(date.getHours())}</h1>
      <div className="hoh-home__date">{dateLabel(date)}</div>
    </div>
  );
}

export function HomeBody() {
  const host = home();
  // One snapshot per change of any chat; entries are read after it.
  useSyncExternalStore(host?.subscribe ?? (() => () => {}), host?.version ?? (() => 0));
  const [, tick] = useState(0);
  useEffect(() => { const timer = setInterval(() => tick((n) => n + 1), 15_000); return () => clearInterval(timer); }, []);
  const navigate = useNavigate();
  const now = Date.now();
  const entries = host?.entries() ?? [];
  const working = entries.filter((entry) => entry.state === 'working' || entry.state === 'waiting').sort((a, b) => b.startedAt - a.startedAt);
  const done = entries.filter((entry) => entry.state === 'done' && entry.doneAt !== null && sameDay(entry.doneAt, now)).sort((a, b) => (b.doneAt ?? 0) - (a.doneAt ?? 0));
  const asking = entries.filter((entry) => entry.request);
  const view: View = {
    host, now,
    open: (threadId) => void navigate({ to: '/$environmentId/$threadId', params: { environmentId: ENVIRONMENT_ID, threadId } }),
    where: (entry) => host?.projectTitle(entry.projectId) ?? '',
  };
  const lists = { working, done, asking, view };
  return (
    <div className="hoh-home" data-hoh-home-body="">
      <Cards {...lists} />
    </div>
  );
}
