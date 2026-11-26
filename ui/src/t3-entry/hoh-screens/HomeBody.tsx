// Главная окна HoH (HOH-41 В2б, раунд дизайна): приветствие с датой, запущенные агенты и их субагенты, «Done», «Needs your answer» над полем ввода T3.
// Подача — карточки (вариант A, выбор основателя): агент — карточка, субагенты — строки внутри неё, запросы ждут справа.
// Это не отдельный экран: vite.ts подставляет <HomeBody/> в пустой разговор T3 (ChatView, DraftHeroHeadline), так что поле ввода,
// чип модели с уровнями, Permissions, выбор проекта и отправка — настоящие, их. Здесь только содержимое над ними.
// Данные — то, что хост знает о разговорах (window.__HOH_HOME__, ui/src/hoh-host: host.home); блоков без данных нет.
import { CheckIcon, ChevronDownIcon, CoinsIcon, LayersIcon, ListTodoIcon, PlusIcon, XIcon } from 'lucide-react';
import { useEffect, useId, useLayoutEffect, useRef, useState, useSyncExternalStore, type CSSProperties, type MouseEvent } from 'react';
import { formatCount, formatTokens, formatUsd } from '@t3tools/shared/usageFormat';

import { ClaudeAI, OpenAI } from '~/components/Icons';
import { T3AppIcon } from '~/components/T3AppIcon';
import { Button } from '~/components/ui/button';
import { openCommandPalette } from '~/commandPaletteBus';
import type { HomeAgent as Agent, HomeEntry } from '../../hoh-host/threads';
import { openQuickView, QuickViewDialog } from './QuickViewDialog';
import {
  getGroupAgentsByProject,
  groupEntriesByProject,
  subscribeGroupAgentsByProject,
  type ProjectGroup,
  updateProjectDisclosure,
} from './grouping';
import './home.css';

type Entry = HomeEntry & { runner: 'claude' | 'codex' };
interface Home {
  version(): number; subscribe(listener: () => void): () => void; entries(): HomeEntry[]; projectEntries(): HomeEntry[]; projectTitle(id: string): string; projectKey(id: string): string;
  answer(threadId: string, requestId: string, decision: 'accept' | 'decline'): Promise<void>;
  complete(threadId: string, turnOrdinal: number): Promise<boolean>;
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


interface View {
  host: Home | undefined; now: number; open(threadId: string): void; where(entry: Entry): string; projectKey(projectId: string): string;
  collapsedDone: ReadonlySet<string | null>;
  toggleDone(projectId: string | null): void;
}

/** The executor's mark from T3's own provider set. */
const Mark = ({ entry }: { entry: Entry }) => entry.providerInstanceId === 'codex'
  ? <OpenAI className="hoh-mark" role="img" aria-label="Codex" />
  : <ClaudeAI className="hoh-mark" role="img" aria-label="Claude" />;

function PluginCreatorMark({ entry }: { entry: Entry }) {
  return (
    <span className="hoh-card__partnership" role="img" aria-label={`Alexandrie + ${entry.runner === 'codex' ? 'Codex' : 'Claude'}`}>
      <span className="hoh-card__emblem" aria-hidden><T3AppIcon /></span>
      <span className="hoh-card__union" aria-hidden><PlusIcon strokeWidth={2.5} /></span>
      <span className="hoh-card__worker" aria-hidden><Mark entry={entry} /></span>
    </span>
  );
}

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
type Complete = (entry: Entry, event: MouseEvent<HTMLButtonElement>) => void;
const CREATOR_STATES = { working: 'Working', ended: 'Inactive' };

function EntryCard({ entry, view, complete, grouped }: { entry: Entry; view: View; complete: Complete; grouped: boolean }) {
  const waiting = entry.state === 'waiting';
  const project = waiting && !grouped && !entry.pluginCreator ? projectLabel(entry.projectId, view.where(entry)) : null;
  const label = waiting ? waitingLabel(entry) : entry.pluginCreator && entry.state !== 'done' && CREATOR_STATES[entry.state];
  return (
    <article className={entry.pluginCreator ? 'hoh-card hoh-card--plugin-creator' : 'hoh-card'} data-state={entry.state} data-thread-id={entry.id}>
      <button type="button" className="hoh-card__open" aria-label={`Open ${entry.title}`} title={waiting ? undefined : entry.title} onClick={() => view.open(entry.id)} />
      {entry.pluginCreator ? <PluginCreatorMark entry={entry} /> : <Mark entry={entry} />}
      <span className="hoh-card__title">{entry.title}</span>
      {project ? <span className="hoh-card__where">{project}</span> : null}
      {waiting && entry.agents.length > 0 ? <span className="hoh-subs">{entry.agents.map((agent) => <Sub key={agent.id} agent={agent} now={view.now} />)}</span> : null}
      {label ? (
        <span className="hoh-card__state">
          <b>{label}</b>
          {entry.canComplete ? <Button className="hoh-card__complete" variant="ghost-muted" size="micro" onClick={(event) => complete(entry, event)}><CheckIcon aria-hidden />Mark done</Button> : null}
          {waiting || entry.state === 'working' ? <span className="hoh-card__elapsed">{elapsed(entry.startedAt, view.now)}</span> : null}
        </span>
      ) : null}
      {waiting || (entry.pluginCreator && entry.state === 'working') ? <Steps entry={entry} /> : null}
      {!entry.pluginCreator && entry.state === 'done' && entry.doneAt !== null ? <span className="hoh-card__time">{clock(entry.doneAt)}</span> : null}
    </article>
  );
}

interface Completion {
  turnOrdinal: number;
  from: DOMRect;
  source: HTMLButtonElement;
  restoreFocus: boolean;
  preview: HTMLElement | null;
}

function AgentClusters({ entries, view, projectId = null, pluginCreator = false }: {
  entries: Entry[]; view: View; projectId?: string | null; pluginCreator?: boolean;
}) {
  const root = useRef<HTMLDivElement>(null);
  const flights = useRef<HTMLDivElement>(null);
  const doneToggle = useRef<HTMLButtonElement>(null);
  const pending = useRef(new Map<string, Completion>());
  const doneId = useId();
  const expanded = pluginCreator || !view.collapsedDone.has(projectId);
  const sections = [
    { state: 'waiting', label: 'Waiting for you', items: entries.filter((entry) => entry.state === 'waiting') },
    { state: 'working', label: 'Working', items: entries.filter((entry) => entry.state === 'working') },
    { state: 'ended', label: 'Inactive', items: entries.filter((entry) => entry.state === 'ended') },
    { state: 'done', label: 'Done', items: entries.filter((entry) => entry.state === 'done').sort((a, b) => (b.doneAt ?? 0) - (a.doneAt ?? 0)) },
  ];

  const complete: Complete = (entry, event) => {
    const source = event.currentTarget;
    const card = source.closest<HTMLElement>('.hoh-card');
    if (!view.host || !card || pending.current.has(entry.id)) return;
    const restoreFocus = event.detail === 0;
    const homeRoot = pluginCreator && restoreFocus ? card.closest<HTMLElement>('[data-hoh-home-body]') : null;
    const animate = !pluginCreator && !matchMedia('(prefers-reduced-motion: reduce)').matches && typeof card.animate === 'function';
    const completion = {
      turnOrdinal: entry.turnOrdinal,
      from: card.getBoundingClientRect(),
      source,
      restoreFocus,
      preview: animate ? card.cloneNode(true) as HTMLElement : null,
    };
    pending.current.set(entry.id, completion);
    void view.host.complete(entry.id, entry.turnOrdinal).then((marked) => {
      // A stale click can be ignored by the host while the thread is loading.
      if (!marked && pending.current.get(entry.id) === completion) pending.current.delete(entry.id);
      if (marked && homeRoot && (document.activeElement === document.body || document.activeElement === source)) {
        homeRoot.focus({ preventScroll: true });
      }
    }, (error: unknown) => {
      if (pending.current.get(entry.id) === completion) pending.current.delete(entry.id);
      console.error('Could not mark the chat done', error);
    });
  };

  useLayoutEffect(() => {
    const container = root.current;
    const layer = flights.current;
    if (!container || !layer || pending.current.size === 0) return;
    const cards = expanded ? Array.from(container.querySelectorAll<HTMLElement>('.hoh-card[data-state="done"]')) : [];
    for (const [id, completion] of pending.current) {
      const entry = entries.find((item) => item.id === id);
      if (entry?.turnOrdinal !== completion.turnOrdinal) {
        pending.current.delete(id);
        continue;
      }
      if (entry.state !== 'done') continue;
      pending.current.delete(id);
      const card = cards.find((node) => node.dataset.threadId === id);
      const target = card ?? doneToggle.current;
      if (!target) continue;
      const { from, source, restoreFocus, preview } = completion;
      const focusTarget = card?.querySelector<HTMLButtonElement>('.hoh-card__open') ?? doneToggle.current;
      if (restoreFocus && (document.activeElement === document.body || document.activeElement === source)) {
        focusTarget?.focus({ preventScroll: Boolean(preview) });
      }
      if (!preview) continue;

      // The real Done card takes its place in the new layout while the inert preview leaves the old one.
      const origin = layer.getBoundingClientRect();
      const to = target.getBoundingClientRect();
      preview.classList.add('hoh-card--completing');
      preview.removeAttribute('data-thread-id');
      const status = preview.querySelector('.hoh-card__state b');
      if (status) status.textContent = 'Done';
      Object.assign(preview.style, {
        left: `${from.left - origin.left}px`, top: `${from.top - origin.top}px`, width: `${from.width}px`, height: `${from.height}px`,
      });
      layer.append(preview);

      const x = to.left + (to.width - from.width) / 2 - from.left;
      const y = to.top + (to.height - from.height) / 2 - from.top;
      const scaleX = Math.min(1, to.width / from.width);
      const scaleY = Math.min(1, to.height / from.height);
      const motion = preview.animate([
        { offset: 0, transform: 'none', opacity: 1 },
        { offset: .22, transform: 'translateY(8px) rotate(-4deg)', opacity: 1 },
        { offset: .8, opacity: 1 },
        { offset: 1, transform: `translate(${x}px, ${y}px) rotate(12deg) scale(${scaleX}, ${scaleY})`, opacity: 0 },
      ], { duration: 580, easing: 'cubic-bezier(.3, 0, .2, 1)' });
      void motion.finished.then(() => {
        preview.remove();
        if (restoreFocus && focusTarget && document.activeElement === focusTarget) focusTarget.scrollIntoView({ block: 'nearest' });
      }, () => preview.remove());
      card?.animate([{ opacity: 0 }, { opacity: 1 }], { delay: 410, duration: 170, fill: 'backwards' });
    }
  }, [entries, expanded]);

  return (
    <div ref={root} className="hoh-agent-clusters" role={pluginCreator ? 'region' : undefined} aria-label={pluginCreator ? 'Plugin Creator' : undefined}>
      {pluginCreator ? (
        <div className="hoh-home__creator-cards">
          {entries.map((entry) => <EntryCard key={entry.id} entry={entry} view={view} complete={complete} grouped={false} />)}
        </div>
      ) : sections.filter(({ items }) => items.length > 0).map(({ state, label, items }) => (
        <section key={state} className="hoh-agent-cluster" data-cluster={state}>
          <h2 className="hoh-home__label">
            {state === 'done' ? (
              <button ref={doneToggle} type="button" className="hoh-agent-cluster__toggle" aria-expanded={expanded} aria-controls={doneId} onClick={() => view.toggleDone(projectId)}>
                <ChevronDownIcon aria-hidden />{label} <span className="hoh-home__count">{items.length}</span>
              </button>
            ) : <>{label} <span className="hoh-home__count">{items.length}</span></>}
          </h2>
          <div
            id={state === 'done' ? doneId : undefined}
            hidden={state === 'done' && !expanded}
            className={`hoh-home__cards${state === 'waiting' ? '' : ' hoh-home__cards--compact'}`}
          >
            {items.map((entry) => <EntryCard key={entry.id} entry={entry} view={view} complete={complete} grouped={projectId !== null} />)}
          </div>
        </section>
      ))}
      <div ref={flights} className="hoh-agent-clusters__flights" aria-hidden="true" inert />
    </div>
  );
}

/** Cards: an agent is a card, its subagents are lines inside it; the request that waits for the person stands above them. */
function Cards({ entries, asking, creators, view }: Lists) {
  return (
    <div className="hoh-home__inner">
      <div className="hoh-home__main">
        <Head now={view.now} busy={entries.length + asking.length + creators.length > 0} />
        {creators.length > 0 ? <AgentClusters entries={creators} view={view} pluginCreator /> : null}
        {entries.length > 0 ? <AgentClusters entries={entries} view={view} /> : null}
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

function FolderStats({ entries }: { entries: Entry[] }) {
  const prompts: number[] = [];
  const context: number[] = [];
  const costs: number[] = [];
  for (const { projectStats } of entries) {
    if (!projectStats) continue;
    prompts.push(projectStats.prompts);
    if (projectStats.peakContextTokens !== undefined) context.push(projectStats.peakContextTokens);
    if (projectStats.reportedCostUsd !== undefined) costs.push(projectStats.reportedCostUsd);
  }
  const promptTotal = prompts.reduce((sum, value) => sum + value, 0);
  const peak = context.reduce((max, value) => Math.max(max, value), 0);
  const cost = costs.reduce((sum, value) => sum + value, 0);
  const costLabel = cost > 0 && cost < 0.01 ? '<$0.01' : formatUsd(cost);
  const coverage = (count: number) => `Recorded for ${formatCount(count)} of ${formatCount(entries.length)} chats shown in this project`;
  const stats = [
    { label: 'All Tasks', icon: ListTodoIcon, value: prompts.length ? formatTokens(promptTotal) : '—', detail: `Requests sent to agents. ${coverage(prompts.length)}` },
    { label: 'Summary Context', icon: LayersIcon, value: context.length ? formatTokens(peak) : '—', detail: context.length ? `Peak context: ${formatCount(peak)} tokens. ${coverage(context.length)}` : 'No context measurements recorded' },
    { label: 'Reported cost', icon: CoinsIcon, value: costs.length ? costLabel : '—', detail: costs.length ? `${costLabel} in provider-reported usage cost. ${coverage(costs.length)}` : 'No cost reported by the providers' },
  ];
  return (
    <span className="hoh-project__stats">
      {stats.map(({ label, icon: Icon, value, detail }) => (
        <span key={label} className="hoh-project__stat" title={detail}>
          <strong>{value}</strong>
          <small><Icon aria-hidden="true" />{label}</small>
        </span>
      ))}
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
  const waitingCount = group.entries.filter((entry) => entry.state === 'waiting').length;
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
        <FolderStats entries={group.entries} />
      </span>
      <span className="hoh-project__heading">
        <span className="hoh-project__counts"><span>{agentLabel}</span><i /><span>{waitingCount} waiting</span></span>
        <span className="hoh-project__title">{group.title}</span>
      </span>
      <span className="hoh-project__actions">
        <span className="hoh-project__chip"><i />{group.waiting > 0 ? 'Needs answer' : group.states.working > 0 ? 'Active' : group.ready > 0 ? 'Ready for review' : group.states.interrupted > 0 ? 'Interrupted' : group.states.stopped > 0 ? 'Stopped' : group.states.done > 0 ? 'Done' : 'Inactive'}</span>
        <span className="hoh-project__more" aria-hidden>⋯</span>
      </span>
      <span className="hoh-project__footer">
        <span className="hoh-project__metric">
          <small>Agents</small>
          <span className="hoh-project__segments" aria-label={`${group.states.working} working, ${group.states.waiting} waiting, ${group.states.ready} ready for review, ${group.states.interrupted} interrupted, ${group.states.stopped} stopped, ${group.states.done} done, ${group.states.inactive} inactive`}>
            {group.states.working > 0 ? <i data-state="working" style={{ flexGrow: group.states.working }} /> : null}
            {group.states.waiting > 0 ? <i data-state="waiting" style={{ flexGrow: group.states.waiting }} /> : null}
            {group.states.ready > 0 ? <i data-state="ready" style={{ flexGrow: group.states.ready }} /> : null}
            {group.states.interrupted > 0 ? <i data-state="interrupted" style={{ flexGrow: group.states.interrupted }} /> : null}
            {group.states.stopped > 0 ? <i data-state="stopped" style={{ flexGrow: group.states.stopped }} /> : null}
            {group.states.done > 0 ? <i data-state="done" style={{ flexGrow: group.states.done }} /> : null}
            {group.states.inactive > 0 ? <i data-state="inactive" style={{ flexGrow: group.states.inactive }} /> : null}
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
            <AgentClusters entries={openGroup.entries} view={view} projectId={openGroup.id} />
          </div>,
        );
        return items;
      })}
    </div>
  );
}

function GroupedCards({ entries, asking, creators, view, projectEntries }: Lists) {
  const groups = groupEntriesByProject(
    projectEntries,
    (projectId) => view.host?.projectTitle(projectId) ?? 'No project',
    view.projectKey,
  );
  return (
    <div className="hoh-home__inner">
      <div className="hoh-home__main">
        <Head now={view.now} busy={entries.length + asking.length + creators.length > 0} />
        {creators.length > 0 ? <AgentClusters entries={creators} view={view} pluginCreator /> : null}
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

interface Lists { entries: Entry[]; asking: Entry[]; creators: Entry[]; projectEntries: Entry[]; view: View }
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
  const [collapsedDone, setCollapsedDone] = useState(() => new Set<string | null>());
  const [, tick] = useState(0);
  useEffect(() => { const timer = setInterval(() => tick((n) => n + 1), 15_000); return () => clearInterval(timer); }, []);
  const now = Date.now();
  const withRunner = (entry: HomeEntry): Entry => ({
    ...entry,
    runner: entry.providerInstanceId === 'codex' ? 'codex' as const : 'claude' as const,
  });
  const entries = (host?.entries() ?? []).map(withRunner);
  const visible = entries.filter((entry) =>
    entry.state === 'working' || entry.state === 'waiting' ||
    (entry.state === 'done' && entry.doneAt !== null && sameDay(entry.doneAt, now)),
  ).sort((a, b) => b.startedAt - a.startedAt);
  const asking = entries.filter((entry) => entry.request);
  const view: View = {
    host, now, collapsedDone,
    open: openQuickView,
    where: (entry) => host?.projectTitle(entry.projectId) ?? '',
    projectKey: (projectId) => host?.projectKey(projectId) ?? projectId,
    toggleDone: (projectId) => setCollapsedDone((previous) => {
      const next = new Set(previous);
      if (next.has(projectId)) next.delete(projectId); else next.add(projectId);
      return next;
    }),
  };
  const projectEntries = grouped ? (host?.projectEntries() ?? []).map(withRunner) : [];
  const creators = (grouped ? projectEntries : visible)
    .filter((entry) => entry.pluginCreator && entry.state !== 'done')
    .sort((a, b) => b.startedAt - a.startedAt);
  const lists = {
    entries: visible.filter((entry) => !entry.pluginCreator),
    asking, creators, view,
    projectEntries: projectEntries.filter((entry) => !entry.pluginCreator),
  };
  const busy = lists.entries.length + asking.length + creators.length + lists.projectEntries.length > 0;
  return (
    <div className="hoh-home" data-hoh-home-body="" data-busy={busy || undefined} tabIndex={-1}>
      {grouped ? <GroupedCards {...lists} /> : <Cards {...lists} />}
      <QuickViewDialog />
    </div>
  );
}
