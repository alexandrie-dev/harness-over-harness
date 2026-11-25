// Главная окна HoH (HOH-41 В2б, раунд дизайна): приветствие с датой, запущенные агенты и их субагенты, «Done», «Needs your answer» над полем ввода T3.
// Подача — карточки (вариант A, выбор основателя): агент — карточка, субагенты — строки внутри неё, запросы ждут справа.
// Это не отдельный экран: vite.ts подставляет <HomeBody/> в пустой разговор T3 (ChatView, DraftHeroHeadline), так что поле ввода,
// чип модели с уровнями, Permissions, выбор проекта и отправка — настоящие, их. Здесь только содержимое над ними.
// Данные — то, что хост знает о разговорах (window.__HOH_HOME__, ui/src/hoh-host: host.home); блоков без данных нет.
import { CheckIcon, XIcon } from 'lucide-react';
import { useEffect, useId, useLayoutEffect, useRef, useState, useSyncExternalStore, type CSSProperties } from 'react';

import { ClaudeAI, OpenAI } from '~/components/Icons';
import { Button } from '~/components/ui/button';
import { openCommandPalette } from '~/commandPaletteBus';
import { openQuickView, QuickViewDialog } from './QuickViewDialog';
import {
  getGroupAgentsByProject,
  groupEntriesByProject,
  subscribeGroupAgentsByProject,
  type ProjectGroup,
  updateProjectDisclosure,
} from './grouping';
import './home.css';

interface Entry {
  id: string; title: string; projectId: string; state: 'working' | 'waiting' | 'done' | 'ended';
  providerInstanceId: string;
  runner: 'claude' | 'codex';
  turnOrdinal: number; canComplete?: boolean; interrupted?: boolean; stopped?: boolean;
  startedAt: number; doneAt: number | null; steps: { done: number; total: number };
  request?: { id: string; kind: 'approval' | 'question'; requestKind: string | null; text: string };
  agents: Agent[];
}
interface Agent {
  id: string; name: string; kind: string; state: 'working' | 'done' | 'failed';
  startedAt: number; doneAt: number | null; currentStep: string | null;
}
interface Home {
  version(): number; subscribe(listener: () => void): () => void; entries(): Entry[]; projectEntries(): Entry[]; projectTitle(id: string): string; projectKey(id: string): string;
  answer(threadId: string, requestId: string, decision: 'accept' | 'decline'): Promise<void>;
  complete(threadId: string): Promise<void>;
}
const home = (): Home | undefined => (globalThis as { __HOH_HOME__?: Home }).__HOH_HOME__;

export function greeting(hour: number): string {
  return hour < 5 ? 'Good night' : hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
}
/** The scratch project is an engine detail; cards name only projects the person chose. */
export function projectLabel(projectId: string, title: string): string | null {
  const label = title.trim();
  return projectId !== 'hoh-project' && label ? label : null;
}
const dateLabel = (date: Date) => date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
/** Title of the home without projects (NoProjectsHero.tsx, vite.ts): greeting and the date. */
export function Greeting() {
  const date = new Date();
  return <span className="hoh-home__greeting">{greeting(date.getHours())}<span className="mt-1 block text-sm font-normal text-muted-foreground">{dateLabel(date)}</span></span>;
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


interface View { host: Home | undefined; now: number; open(threadId: string): void; where(entry: Entry): string; projectKey(projectId: string): string }

/** The executor's mark from T3's own provider set. */
const Mark = ({ entry }: { entry: Entry }) => entry.providerInstanceId === 'codex'
  ? <OpenAI className="hoh-mark" role="img" aria-label="Codex" />
  : <ClaudeAI className="hoh-mark" role="img" aria-label="Claude" />;

function Steps({ entry }: { entry: Entry }) {
  if (entry.steps.total === 0 || entry.canComplete) return null;
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
    <div className="hoh-sub" data-state={agent.state} title={[`${agent.kind}: ${agent.name}`, agent.currentStep].filter(Boolean).join('\n')}>
      <StateDot state={agent.state} />
      <span className="hoh-sub__copy">
        <span className="hoh-sub__name">{agent.name}</span>
        {agent.currentStep ? <span className="hoh-sub__step">{agent.currentStep}</span> : null}
      </span>
      <small>{agent.kind}</small>
      <time>{elapsed(agent.startedAt, agent.doneAt ?? now)}</time>
    </div>
  );
}
/** What Claude asks, as T3 shows its request: the kind, the command or file itself, the chat's own Approve / Decline. */
function Ask({ entry, view }: { entry: Entry; view: View }) {
  const request = entry.request!;
  const detailId = useId();
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="hoh-ask__item">
      <div className="hoh-ask__kind">{requestLabel(request)}</div>
      <button
        type="button"
        className="hoh-ask__detail"
        aria-controls={detailId}
        aria-expanded={expanded}
        aria-label={expanded ? 'Collapse request details' : 'Expand request details'}
        title={expanded ? 'Collapse request details' : 'Expand request details'}
        onClick={() => setExpanded((value) => !value)}
      >
        <code id={detailId} className="hoh-ask__text" data-approval-detail="complete" data-expanded={expanded || undefined}>
          {request.text || requestLabel(request)}
        </code>
      </button>
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
export function waitingLabel(entry: Pick<Entry, 'stopped' | 'interrupted' | 'canComplete'>): string {
  return entry.stopped ? 'Stopped' : entry.interrupted ? 'Interrupted' : entry.canComplete ? 'Ready for review' : 'Waiting for your answer';
}
const stateLabel = (entry: Entry) => entry.state === 'working'
  ? <><span className="hoh-orbit" aria-hidden />Working</>
  : entry.state === 'ended' ? 'Inactive' : waitingLabel(entry);
/** An agent as a card (HOH-41): the executor's mark top-left, the title, where it works, its subagents inside, the state at the bottom.
 *  The glow of the border follows the state (home.css: blue — working, red — waits for the person, neutral — done). */
function Card({ entry, view, children }: { entry: Entry; view: View; children: React.ReactNode }) {
  const project = projectLabel(entry.projectId, view.where(entry));
  return (
    <article className="hoh-card" data-state={entry.state}>
      <button type="button" className="hoh-card__open" aria-label={`Open ${entry.title}`} onClick={() => view.open(entry.id)} />
      <Mark entry={entry} />
      <span className="hoh-card__title">{entry.title}</span>
      {project ? <span className="hoh-card__where">{project}</span> : null}
      {entry.agents.length > 0 ? <span className="hoh-subs">{entry.agents.map((agent) => <Sub key={agent.id} agent={agent} now={view.now} />)}</span> : null}
      {children}
    </article>
  );
}
function EntryCard({ entry, view }: { entry: Entry; view: View }) {
  if (entry.state === 'done') {
    return <Card entry={entry} view={view}><span className="hoh-card__state"><b>Done</b><span>{clock(entry.doneAt ?? 0)}</span></span></Card>;
  }
  return (
    <Card entry={entry} view={view}>
      <span className="hoh-card__state">
        <b>{stateLabel(entry)}</b>
        <span className="hoh-card__state-actions">
          {entry.canComplete ? <Button className="hoh-card__complete" variant="ghost-muted" size="micro" onClick={() => void view.host?.complete(entry.id)}><CheckIcon aria-hidden />Mark done</Button> : null}
          <span>{elapsed(entry.startedAt, view.now)}</span>
        </span>
      </span>
      <Steps entry={entry} />
    </Card>
  );
}
const DoneCards = ({ done, view }: { done: Entry[]; view: View }) => done.length === 0 ? null : (
  <section>
    <h2 className="hoh-home__label">Done <span className="hoh-home__count">{done.length}</span></h2>
    <div className="hoh-home__cards">
      {done.map((entry) => <EntryCard key={entry.id} entry={entry} view={view} />)}
    </div>
  </section>
);
const Count = ({ list }: { list: Entry[] }) => { const subs = list.reduce((sum, entry) => sum + entry.agents.length, 0); return <span className="hoh-home__count">{list.length}{subs > 0 ? ` · ${subs} subagent${subs === 1 ? '' : 's'}` : ''}</span>; };

/** Cards: an agent is a card, its subagents are lines inside it; the request that waits for the person stands above them. */
function Cards({ working, done, asking, view }: Lists) {
  return (
    <div className="hoh-home__inner">
      <div className="hoh-home__main">
        <Head now={view.now} busy={working.length + done.length + asking.length > 0} />
        {working.length > 0 ? (
          <section>
            <h2 className="hoh-home__label">Now <Count list={working} /></h2>
            <div className="hoh-home__cards">
              {working.map((entry) => <EntryCard key={entry.id} entry={entry} view={view} />)}
            </div>
          </section>
        ) : null}
        <DoneCards done={done} view={view} />
      </div>
      {asking.length > 0 ? (
        <aside className="hoh-home__side">
          <div className="hoh-ask">
            <div className="hoh-ask__head"><span>Needs your answer</span><span className="hoh-ask__count">{asking.length}</span></div>
            {asking.map((entry) => (
              <div key={entry.request!.id} className="hoh-ask__block">
                <div className="hoh-ask__where"><span><Mark entry={entry} />{entry.title}</span><span>{elapsed(entry.startedAt, view.now)}</span></div>
                <Ask entry={entry} view={view} />
              </div>
            ))}
          </div>
        </aside>
      ) : null}
    </div>
  );
}

/** The disclosure follows its folder's rendered grid row, not the viewport. */
export function projectPanelInsertAfter(activeIndex: number, groupCount: number, columnCount: number): number {
  if (activeIndex < 0 || groupCount < 1) return -1;
  const columns = Math.max(1, Math.floor(columnCount));
  return Math.min(groupCount - 1, (Math.floor(activeIndex / columns) + 1) * columns - 1);
}

function Runners({ runners }: { runners: ProjectGroup<Entry>['runners'] }) {
  const visible = runners.slice(0, 3);
  return (
    <span className="hoh-project__runners" aria-label={`Runners: ${runners.join(', ')}`}>
      {visible.map((runner, index) => (
        <span key={`${runner}-${index}`} className="hoh-project__runner" title={runner === 'codex' ? 'Codex' : 'Claude'}>
          {runner === 'codex' ? <OpenAI aria-hidden /> : <ClaudeAI aria-hidden />}
        </span>
      ))}
      {runners.length > visible.length ? <span className="hoh-project__runner-more">+{runners.length - visible.length}</span> : null}
    </span>
  );
}

function FolderCard({ group, expanded, setButton, onEnter, onLeave, onToggle }: {
  group: ProjectGroup<Entry>;
  expanded: boolean;
  setButton(node: HTMLButtonElement | null): void;
  onEnter(): void;
  onLeave(): void;
  onToggle(): void;
}) {
  const panelId = `hoh-project-agents-${group.id.replace(/[^a-zA-Z0-9_-]/g, '-')}`;
  const style = { '--hoh-project-color': `var(--hoh-project-color-${group.color})` } as CSSProperties;
  const count = group.totalChats ?? group.entries.length;
  const agentLabel = `${count} agent${count === 1 ? '' : 's'}`;
  const waitingLabel = `${group.waiting} waiting`;
  return (
    <button
      ref={setButton}
      type="button"
      className="hoh-project"
      data-color={group.color}
      data-waiting={group.waiting > 0 || undefined}
      data-working={(group.waiting === 0 && group.states.working > 0) || undefined}
      data-expanded={expanded || undefined}
      aria-expanded={expanded}
      aria-controls={panelId}
      onClick={onToggle}
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
      style={style}
    >
      <span className="hoh-project__top">
        <span className="hoh-project__icon" aria-hidden><i /><b /><em /></span>
        <span className="hoh-project__heading">
          <span className="hoh-project__counts"><span>{agentLabel}</span><i /><span>{waitingLabel}</span></span>
          <span className="hoh-project__title">{group.title}</span>
        </span>
      </span>
      <span className="hoh-project__actions">
        <span className="hoh-project__chip"><i />{group.waiting > 0 ? 'Needs answer' : group.states.working > 0 ? 'Active' : group.ready > 0 ? 'Ready for review' : group.states.interrupted > 0 ? 'Interrupted' : group.states.stopped > 0 ? 'Stopped' : 'Done'}</span>
        <span className="hoh-project__more" aria-hidden>⋯</span>
      </span>
      <span className="hoh-project__footer">
        <span className="hoh-project__metric">
          <small>Agents</small>
          <span className="hoh-project__segments" aria-label={`${group.states.working} working, ${group.states.waiting} waiting, ${group.states.ready} ready for review, ${group.states.interrupted} interrupted, ${group.states.stopped} stopped, ${group.states.done} done`}>
            {group.states.working > 0 ? <i data-state="working" style={{ flexGrow: group.states.working }} /> : null}
            {group.states.waiting > 0 ? <i data-state="waiting" style={{ flexGrow: group.states.waiting }} /> : null}
            {group.states.ready > 0 ? <i data-state="ready" style={{ flexGrow: group.states.ready }} /> : null}
            {group.states.interrupted > 0 ? <i data-state="interrupted" style={{ flexGrow: group.states.interrupted }} /> : null}
            {group.states.stopped > 0 ? <i data-state="stopped" style={{ flexGrow: group.states.stopped }} /> : null}
            {group.states.done > 0 ? <i data-state="done" style={{ flexGrow: group.states.done }} /> : null}
          </span>
        </span>
        <span className="hoh-project__metric hoh-project__metric--runners"><small>Runners</small><Runners runners={group.runners} /></span>
      </span>
    </button>
  );
}

function ProjectFolders({ groups, view }: { groups: ProjectGroup<Entry>[]; view: View }) {
  const [disclosure, setDisclosure] = useState({ hovered: null as string | null, pinned: null as string | null });
  const [columnCount, setColumnCount] = useState(1);
  const grid = useRef<HTMLDivElement>(null);
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const panel = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const openId = disclosure.pinned ?? disclosure.hovered;
  const openGroup = groups.find((group) => group.id === openId) ?? null;
  const openGroupId = openGroup?.id ?? null;
  const groupLayoutKey = groups.map((group) => group.id).join('\u0000');
  const openIndex = openGroup ? groups.indexOf(openGroup) : -1;
  const panelAfter = projectPanelInsertAfter(openIndex, groups.length, columnCount);

  const cancelClose = () => { if (closeTimer.current) clearTimeout(closeTimer.current); closeTimer.current = null; };
  const enter = (id: string) => { cancelClose(); setDisclosure((state) => updateProjectDisclosure(state, { type: 'hover', id })); };
  const leave = () => {
    cancelClose();
    if (disclosure.pinned === null) closeTimer.current = setTimeout(() => setDisclosure((state) => updateProjectDisclosure(state, { type: 'leave' })), 180);
  };

  useEffect(() => () => cancelClose(), []);
  useEffect(() => {
    if (openId && !openGroupId) setDisclosure((state) => updateProjectDisclosure(state, { type: 'close' }));
  }, [openId, openGroupId]);
  useLayoutEffect(() => {
    const node = grid.current;
    if (!node) return;
    const update = () => {
      const renderedColumns = getComputedStyle(node).gridTemplateColumns.replace(/\[[^\]]*\]/g, ' ').split(/\s+/).filter(Boolean).length;
      setColumnCount(Math.max(1, renderedColumns));
    };
    update();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update);
    observer?.observe(node);
    window.addEventListener('resize', update);
    return () => { observer?.disconnect(); window.removeEventListener('resize', update); };
  }, [groupLayoutKey]);
  useEffect(() => {
    if (!openGroupId) return;
    const frame = requestAnimationFrame(() => {
      const disclosurePanel = panel.current;
      const scroller = disclosurePanel?.closest<HTMLElement>('.hoh-home');
      if (!disclosurePanel || !scroller) return;
      const panelRect = disclosurePanel.getBoundingClientRect();
      const scrollerRect = scroller.getBoundingClientRect();
      if (panelRect.bottom > scrollerRect.bottom) scroller.scrollTo({ top: scroller.scrollTop + panelRect.bottom - scrollerRect.bottom + 8, behavior: 'smooth' });
      else if (panelRect.top < scrollerRect.top) scroller.scrollTo({ top: scroller.scrollTop - (scrollerRect.top - panelRect.top) - 8, behavior: 'smooth' });
    });
    return () => cancelAnimationFrame(frame);
  }, [openGroupId, panelAfter]);
  useEffect(() => {
    if (!openId) return;
    const close = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node) || panel.current?.contains(target) || [...buttons.current.values()].some((button) => button.contains(target))) return;
      setDisclosure((state) => updateProjectDisclosure(state, { type: 'close' }));
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      const trigger = buttons.current.get(openId);
      setDisclosure((state) => updateProjectDisclosure(state, { type: 'close' })); trigger?.focus();
    };
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', close); document.removeEventListener('keydown', escape); };
  }, [openId]);

  return (
    <div ref={grid} className="hoh-projects">
      {groups.flatMap((group, index) => {
        const items: React.ReactNode[] = [
          <FolderCard
            key={`folder-${group.id}`}
            group={group}
            expanded={openId === group.id}
            setButton={(node) => { if (node) buttons.current.set(group.id, node); else buttons.current.delete(group.id); }}
            onEnter={() => enter(group.id)}
            onLeave={leave}
            onToggle={() => {
              cancelClose();
              setDisclosure((state) => updateProjectDisclosure(state, { type: 'toggle', id: group.id }));
            }}
          />,
        ];
        if (openGroup && index === panelAfter) items.push(
          <div
            key={`panel-${openGroup.id}`}
            ref={panel}
            id={`hoh-project-agents-${openGroup.id.replace(/[^a-zA-Z0-9_-]/g, '-')}`}
            className="hoh-project-popover"
            data-project={openGroup.id}
            role="region"
            aria-label={`${openGroup.title} agents`}
            onMouseEnter={cancelClose}
            onMouseLeave={leave}
          >
            <div className="hoh-project-popover__head"><span>{openGroup.title}</span><small>{openGroup.totalChats ?? openGroup.entries.length} agent{(openGroup.totalChats ?? openGroup.entries.length) === 1 ? '' : 's'}</small></div>
            <div className="hoh-project-popover__cards">
              {openGroup.entries.map((entry, agentIndex) => (
                <div key={entry.id} className="hoh-project-popover__agent" style={{ '--hoh-agent-index': agentIndex } as CSSProperties}>
                  <EntryCard entry={entry} view={view} />
                </div>
              ))}
            </div>
          </div>,
        );
        return items;
      })}
    </div>
  );
}

function GroupedCards({ working, done, asking, view, projectEntries }: Lists) {
  const groups = groupEntriesByProject(
    projectEntries,
    (projectId) => view.host?.projectTitle(projectId) ?? 'No project',
    view.projectKey,
  );
  return (
    <div className="hoh-home__inner">
      <div className="hoh-home__main">
        <Head now={view.now} busy={working.length + done.length + asking.length > 0} />
        {groups.length > 0 ? (
          <section>
            <h2 className="hoh-home__label">Projects <span className="hoh-home__count">{groups.length}</span></h2>
            <ProjectFolders groups={groups} view={view} />
          </section>
        ) : null}
      </div>
      {asking.length > 0 ? (
        <aside className="hoh-home__side">
          <div className="hoh-ask">
            <div className="hoh-ask__head"><span>Needs your answer</span><span className="hoh-ask__count">{asking.length}</span></div>
            {asking.map((entry) => (
              <div key={entry.request!.id} className="hoh-ask__block">
                <div className="hoh-ask__where"><span><Mark entry={entry} />{entry.title}</span><span>{elapsed(entry.startedAt, view.now)}</span></div>
                <Ask entry={entry} view={view} />
              </div>
            ))}
          </div>
        </aside>
      ) : null}
    </div>
  );
}

interface Lists { working: Entry[]; done: Entry[]; asking: Entry[]; projectEntries: Entry[]; view: View }
function Head({ now, busy }: { now: number; busy: boolean }) {
  const date = new Date(now);
  return (
    <div className="hoh-home__head">
      <h1>{greeting(date.getHours())}</h1>
      <div className="hoh-home__date">{dateLabel(date)}</div>
      {busy ? <Button size="sm" variant="outline" className="hoh-home__add-project" onClick={() => openCommandPalette({ open: 'add-project' })}>Add project</Button> : null}
    </div>
  );
}

export function HomeBody() {
  const host = home();
  // One snapshot per change of any chat; entries are read after it.
  useSyncExternalStore(host?.subscribe ?? (() => () => {}), host?.version ?? (() => 0));
  const grouped = useSyncExternalStore(subscribeGroupAgentsByProject, getGroupAgentsByProject, () => false);
  const [, tick] = useState(0);
  useEffect(() => { const timer = setInterval(() => tick((n) => n + 1), 15_000); return () => clearInterval(timer); }, []);
  const now = Date.now();
  const withRunner = (entry: Entry) => ({
    ...entry,
    runner: entry.providerInstanceId === 'codex' ? 'codex' as const : 'claude' as const,
  });
  const entries = (host?.entries() ?? []).map(withRunner);
  const working = entries.filter((entry) => entry.state === 'working' || entry.state === 'waiting').sort((a, b) => b.startedAt - a.startedAt);
  const done = entries.filter((entry) => entry.state === 'done' && entry.doneAt !== null && sameDay(entry.doneAt, now)).sort((a, b) => (b.doneAt ?? 0) - (a.doneAt ?? 0));
  const asking = entries.filter((entry) => entry.request);
  const view: View = {
    host, now,
    open: openQuickView,
    where: (entry) => host?.projectTitle(entry.projectId) ?? '',
    projectKey: (projectId) => host?.projectKey(projectId) ?? projectId,
  };
  const projectEntries = grouped ? (host?.projectEntries() ?? []).map(withRunner) : [];
  const lists = { working, done, asking, projectEntries, view };
  return (
    <div className="hoh-home" data-hoh-home-body="" data-busy={working.length + done.length + asking.length + projectEntries.length > 0 || undefined}>
      {grouped ? <GroupedCards {...lists} /> : <Cards {...lists} />}
      <QuickViewDialog />
    </div>
  );
}
