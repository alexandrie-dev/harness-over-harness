// Главная окна HoH (HOH-41 В2, раунд 8): приветствие с датой, «Сейчас», «Готово», «Нужен ваш ответ», поле ввода.
// Данные — только то, что хост уже знает о разговорах (window.__HOH_HOME__, ui/src/hoh-host: host.home); блоков без данных нет.
// Компоненты и размеры — T3 (Button, Textarea, SidebarInset, WorkspacePageHeader); здесь раскладка (home.css) и состав.
import { useNavigate } from '@tanstack/react-router';
import { ArrowUpIcon } from 'lucide-react';
import { useState, useSyncExternalStore } from 'react';

import { Button } from '~/components/ui/button';
import { SidebarInset } from '~/components/ui/sidebar';
import { Textarea } from '~/components/ui/textarea';
import { WorkspacePageHeader } from '~/components/WorkspacePageHeader';
import { useProjects } from '~/state/entities';
import './home.css';

interface Entry {
  id: string; title: string; projectId: string; state: 'working' | 'waiting' | 'done' | 'ended';
  startedAt: number; doneAt: number | null; steps: { done: number; total: number };
  request?: { id: string; kind: 'approval' | 'question'; requestKind: string | null; text: string };
}
interface Home {
  version(): number; subscribe(listener: () => void): () => void; entries(): Entry[]; projectTitle(id: string): string;
  answer(threadId: string, requestId: string, decision: 'accept' | 'decline'): Promise<void>;
  start(message: string, projectId?: string): Promise<string>;
}
const ENVIRONMENT_ID = 'hoh-local';
const home = (): Home | undefined => (globalThis as { __HOH_HOME__?: Home }).__HOH_HOME__;
const NO_PROJECT = 'hoh-project';

export function greeting(hour: number): string {
  return hour < 5 ? 'Good evening' : hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
}
export function sameDay(a: number, b: number): boolean { return new Date(a).toDateString() === new Date(b).toDateString(); }
export function elapsed(from: number, to: number): string {
  const minutes = Math.max(0, Math.floor((to - from) / 60_000));
  return minutes < 1 ? '<1 min' : minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}
const clock = (at: number) => new Date(at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });

export function HomeScreen() {
  const host = home();
  // One snapshot per change of any chat; entries are read after it.
  useSyncExternalStore(host?.subscribe ?? (() => () => {}), host?.version ?? (() => 0));
  const [now] = useState(() => Date.now());
  const clockNow = Math.max(now, Date.now());
  const navigate = useNavigate();
  const projects = useProjects();
  const [text, setText] = useState('');
  const [projectId, setProjectId] = useState(NO_PROJECT);
  const [busy, setBusy] = useState(false);

  const entries = host?.entries() ?? [];
  const working = entries.filter((entry) => entry.state === 'working' || entry.state === 'waiting').sort((a, b) => b.startedAt - a.startedAt);
  const done = entries.filter((entry) => entry.state === 'done' && entry.doneAt !== null && sameDay(entry.doneAt, clockNow)).sort((a, b) => (b.doneAt ?? 0) - (a.doneAt ?? 0));
  const asking = entries.filter((entry) => entry.request);
  const open = (threadId: string) => void navigate({ to: '/$environmentId/$threadId', params: { environmentId: ENVIRONMENT_ID, threadId } });
  const where = (entry: Entry) => host?.projectTitle(entry.projectId) ?? '';
  const send = async () => {
    const message = text.trim();
    if (!message || !host || busy) return;
    setBusy(true);
    try { open(await host.start(message, projectId)); setText(''); } finally { setBusy(false); }
  };
  const date = new Date(clockNow);

  return (
    <SidebarInset role="main" className="h-dvh min-h-0 overflow-hidden overscroll-y-none">
      <WorkspacePageHeader electron />
      <div className="hoh-home">
        <div className="hoh-home__inner">
          <div className="hoh-home__main">
            <div className="hoh-home__head">
              <h1>{greeting(date.getHours())}</h1>
              <div className="hoh-home__date">{date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })}</div>
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

            <div className="hoh-home__input">
              <div className="hoh-home__compose">
                <Textarea
                  size="lg"
                  rows={2}
                  value={text}
                  placeholder="Ask anything. Start a new thread."
                  aria-label="New thread"
                  onChange={(event) => setText(event.target.value)}
                  onKeyDown={(event) => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); void send(); } }}
                />
                <Button size="icon" aria-label="Send" disabled={!text.trim() || busy} onClick={() => void send()}><ArrowUpIcon /></Button>
              </div>
              <select className="hoh-home__where" aria-label="Project" value={projectId} onChange={(event) => setProjectId(event.target.value)}>
                <option value={NO_PROJECT}>No project</option>
                {projects.filter((project) => project.archivedAt == null && project.id !== NO_PROJECT).map((project) => (
                  <option key={project.id} value={project.id}>{project.title}</option>
                ))}
              </select>
            </div>
          </div>

          {asking.length > 0 ? (
            <aside className="hoh-home__side">
              <div className="hoh-ask">
                <div className="hoh-ask__head"><span>Needs your answer</span><span className="hoh-ask__count">{asking.length}</span></div>
                {asking.map((entry) => (
                  <div key={entry.request!.id} className="hoh-ask__item">
                    <div className="hoh-ask__where"><span>{entry.title}</span><span>{elapsed(entry.startedAt, clockNow)}</span></div>
                    <div className="hoh-ask__text">{entry.request!.text || (entry.request!.kind === 'approval' ? 'Permission request' : 'Question')}</div>
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
    </SidebarInset>
  );
}
