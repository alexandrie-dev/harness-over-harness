// One HoH chat as a T3 thread. The engine's session events (the same ones the current window reads:
// ui/src/screens/chat/projection.mjs) are folded into T3's `OrchestrationV2ThreadProjection` and
// `OrchestrationV2ThreadShell` (packages/contracts/src/orchestrationV2.ts), in their encoded JSON form.
// One user message is one run; answers, tool steps, permission requests and questions are turn items.
import { CLAUDE_INSTANCE_ID, CODEX_INSTANCE_ID, runtimeModeOf, type HohAccessMode, type HohExecutor, type T3InteractionMode } from './config.ts';
import type { EngineQuestion } from './engine.ts';
import { providerFailure } from './errors.ts';

export const HOH_PROJECT_ID = 'hoh-project';
const NODE_ID = 'hoh-node';

const iso = (ms: number) => new Date(ms).toISOString();
const record = (value: unknown): Record<string, any> => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, any> : {};
const text = (value: unknown) => typeof value === 'string' ? value : '';
const nonBlank = (value: string, fallback: string) => value.trim() ? value.trim() : fallback;
export function contentText(blocks: unknown): string {
  return Array.isArray(blocks) ? blocks.filter(block => block?.type === 'text').map(block => text(block.text)).join('') : '';
}
function contentReasoning(blocks: unknown): string {
  return Array.isArray(blocks) ? blocks.filter(block => block?.type === 'reasoning').map(block => text(block.text)).filter(Boolean).join('\n\n') : '';
}
function argumentsOf(value: unknown): Record<string, any> {
  try { return record(typeof value === 'string' ? JSON.parse(value) : value); } catch { return {}; }
}

type Status = 'idle' | 'pending' | 'running' | 'waiting' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
type RunStatus = 'running' | 'waiting' | 'completed' | 'interrupted' | 'failed' | 'cancelled';
interface TokenUsage { usedTokens: number; maxTokens?: number; inputTokens: number; cachedInputTokens: number; outputTokens: number; reasoningOutputTokens: number; updatedAt: string }
interface Run { id: string; ordinal: number; model: string; userMessageId: string; status: RunStatus; requestedAt: number; startedAt: number | null; completedAt: number | null; engineStartedAt?: number; tokenUsage?: TokenUsage }
type Item = Record<string, any> & { id: string; type: string; status: Status; runId: string | null; ordinal: number };
/** A picture of a message as T3 lists it (`ChatImageAttachment`): its bytes went to the engine, not into the thread. */
export interface ShownAttachment { readonly type: 'image'; readonly id: string; readonly name: string; readonly mimeType: string; readonly sizeBytes: number }
interface Message { id: string; runId: string | null; role: 'user' | 'assistant' | 'system'; text: string; streaming: boolean; createdAt: number; updatedAt: number; attachments?: ShownAttachment[] }
interface Request {
  id: string; kind: string; status: 'pending' | 'resolved' | 'expired' | 'cancelled';
  createdAt: number; resolvedAt: number | null; decision?: string; answers?: Record<string, unknown>;
}
interface Live {
  itemId: string;
  attemptId: string;
  key: string;
  kind: 'text' | 'reasoning' | null;
  raw: string;
  last: number;
  gapped: boolean;
}

/** What a tool call is, by the name the engine reports (Claude's tools and the engine's own). */
function toolKind(name: string): 'command' | 'write' | 'read' | 'search' | 'web' | 'other' {
  const lower = name.toLowerCase();
  if (['bash', 'shell', 'run_command', 'exec', 'run_code', 'bashoutput'].includes(lower)) return 'command';
  if (['write', 'edit', 'multiedit', 'write_file', 'file_write', 'str_replace_editor', 'edit_file', 'notebookedit'].includes(lower)) return 'write';
  if (['read', 'read_file', 'file_read', 'read_image'].includes(lower)) return 'read';
  if (['grep', 'glob', 'ls', 'find'].includes(lower)) return 'search';
  if (['webfetch', 'websearch', 'web_fetch', 'web_search'].includes(lower)) return 'web';
  return 'other';
}

/** Claude's own names of its tools, which T3 writes in a request (the engine reports them in lower case). */
const CLAUDE_TOOL_NAMES: Readonly<Record<string, string>> = {
  bash: 'Bash', read: 'Read', write: 'Write', edit: 'Edit', multiedit: 'MultiEdit', notebookedit: 'NotebookEdit',
  grep: 'Grep', glob: 'Glob', ls: 'LS', webfetch: 'WebFetch', websearch: 'WebSearch', todowrite: 'TodoWrite', task: 'Task',
};

/**
 * A permission request as T3 makes one for Claude (apps/server ClaudeAdapterV2.ts `canUseTool`): its kind by
 * the tool (`providerRequestKindFromClaudeTool`) and its text the command itself, else "Tool: file" or
 * "Tool: input" (`summarizeClaudeToolRequest`). T3 titles the card by the kind ("Command approval").
 */
function toolRequest(name: string, args: Record<string, any>, cwd: string | null = null): { requestKind: string; prompt?: string } {
  const kind = toolKind(name);
  const requestKind = kind === 'write' ? 'file-change' : kind === 'read' || kind === 'search' ? 'file-read' : 'command';
  const str = (value: unknown) => typeof value === 'string' && value.trim() !== '' ? value.trim() : undefined;
  const first = (keys: string[]) => keys.map(key => str(args[key])).find((value): value is string => value !== undefined);
  const inFolder = (path: string) => cwd && path.startsWith(cwd.replace(/\/+$/, '') + '/') ? path.slice(cwd.replace(/\/+$/, '').length + 1) : path;
  const lower = name.toLowerCase();
  // A subagent: what it is for, its kind small, the start of its task (two lines at most). Never the call's JSON.
  if (lower === 'task' || lower === 'agent') {
    const lines = [`Subagent: ${first(['description']) ?? 'task'}`];
    const type = first(['subagent_type']);
    if (type) lines[0] += ` (${type})`;
    const task = first(['prompt'])?.split('\n').map(line => line.trim()).filter(Boolean).slice(0, 2).join('\n');
    if (task) lines.push(task.length > 160 ? task.slice(0, 157) + '...' : task);
    return { requestKind, prompt: lines.join('\n') };
  }
  const command = first(['command']);
  if (command) return { requestKind, prompt: command };
  const tool = CLAUDE_TOOL_NAMES[lower] ?? name;
  if (!tool.trim()) return { requestKind };
  const subject = first(['cmd', 'script']) ?? first(['file_path', 'path', 'filename', 'fileName', 'notebook_path']);
  if (subject) return { requestKind, prompt: kind === 'write' || kind === 'read' ? inFolder(subject).slice(0, 400) : `${tool}: ${inFolder(subject).slice(0, 400)}` };
  // An unknown tool: its name and the key fields as lines, no JSON.
  const lines = Object.entries(args).slice(0, 4).map(([key, value]) => {
    const shown = typeof value === 'string' ? value : typeof value === 'number' || typeof value === 'boolean' ? String(value) : Array.isArray(value) ? `${value.length} items` : value && typeof value === 'object' ? '{…}' : '';
    const one = shown.replace(/\s+/g, ' ').trim();
    return `${key}: ${one.length > 120 ? one.slice(0, 117) + '...' : one}`;
  });
  return { requestKind, prompt: [tool, ...lines].join('\n') };
}

const STEP_TYPES = new Set(['command_execution', 'file_change', 'file_search', 'web_search', 'dynamic_tool']);

/** Remove harness-only hand-back control text while preserving the subagent's actual report. */
export function visibleSubagentResult(value: unknown): string {
  let result = text(value).trim().replace(/<tools-used-before-summary>[\s\S]*?<\/tools-used-before-summary>\s*/gi, '').trim();
  if (result.startsWith('[Subagent hand-back]')) {
    result = result.replace(/^\[Subagent hand-back\][\s\S]*?\bThe report follows:\s*/i, '').trim();
  }
  return result;
}

/** One chat on the home screen. `state`: working, waiting (for the person's answer), done, ended (stopped or failed). */
export interface HomeEntry {
  id: string; title: string; projectId: string;
  providerInstanceId: string;
  state: 'working' | 'waiting' | 'done' | 'ended';
  /** Stable across replay: runs are rebuilt in the same order even when their message ids change after a restart. */
  turnOrdinal: number;
  startedAt: number; doneAt: number | null;
  steps: { done: number; total: number };
  /** Home-only action: a completed executor turn still needs the person's sign-off. */
  canComplete?: boolean;
  /** The last turn has no durable ending because the window or engine disappeared, rather than the person pressing Stop. */
  interrupted?: boolean;
  /** The person explicitly stopped this turn; Home keeps it visible without presenting it as a crash. */
  stopped?: boolean;
  request?: { id: string; kind: 'approval' | 'question'; requestKind: string | null; text: string };
  /** Subagents of the run: Claude's Task (Agent) calls — name and kind from the call, state from its result. */
  agents: HomeAgent[];
}
export interface HomeAgent {
  id: string; name: string; kind: string; state: 'working' | 'done' | 'failed';
  startedAt: number; doneAt: number | null; currentStep: string | null;
}

/** Claude starts a subagent with its Task tool (named Agent in newer versions): the call is the subagent. */
const isSubagentCall = (item: Item) => item.type === 'dynamic_tool' && ['task', 'agent'].includes(String(item.toolName ?? '').toLowerCase());

export interface ThreadSeed {
  readonly id: string;
  readonly title?: string;
  readonly createdAt: number;
  readonly updatedAt?: number;
  readonly model: string;
  readonly mode: HohAccessMode;
  readonly executor?: HohExecutor;
  readonly bindingKnown?: boolean;
  readonly interactionMode?: T3InteractionMode;
  /** The T3 project the chat belongs to: "No project" unless it was started in a folder project. */
  readonly projectId?: string;
}

export class ThreadModel {
  readonly id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  model: string;
  mode: HohAccessMode;
  executor: HohExecutor;
  bindingKnown: boolean;
  /** The engine explicitly reported a built-in agent rather than an external executor. */
  builtin = false;
  /** Durable `approval/asked` can arrive before the native `approval.pending`; keep its exact id. */
  private readonly approvalDetails = new Map<string, { requestKind: string }>();
  interactionMode: T3InteractionMode;
  /** The actual interaction policy of the running process, independent of the next selection. */
  runningInteractionMode: T3InteractionMode | null = null;
  projectId: string;
  /** The folder the chat works in, as its history records it (`working-directory/change`); null until known. */
  cwd: string | null = null;
  /** History was read from the engine (or the chat is new); until then only the shell row is known. */
  loaded = false;
  /** The model options the person chose for this chat (T3's reasoning level): shown in the chip and sent with every message. */
  options: { id: string; value: unknown }[] | undefined;
  running = false;
  /** The engine itself said the chat is working (`session.status`), not just its history. */
  engineRunning = false;
  /** The person pressed Stop for the current turn: however it ends, it ends stopped. */
  stopRequested = false;
  /** The last turn was closed by `settleUnfinished`, a guess the engine may still correct. */
  private settledByGuess = false;
  archivedAt: number | null = null;
  pinnedAt: number | null = null;
  pinOrderKey: string | null = null;
  private readonly seen = new Set<number>();
  private readonly runs: Run[] = [];
  private readonly items: Item[] = [];
  private readonly messages: Message[] = [];
  private readonly requests: Request[] = [];
  private readonly plans: Record<string, any>[] = [];
  private readonly live = new Map<string, Live>();
  private readonly retired = new Set<string>();
  private readonly calls = new Map<string, string>();
  /** Name and input of each tool call by the engine's call id: a permission request names its call. */
  private readonly toolInputs = new Map<string, { name: string; args: Record<string, any>; itemId: string }>();
  /** A run `turn/start` reopened: the engine starts a turn before it records the turn's message. */
  private reopened: { run: Run; status: RunStatus; completedAt: number | null } | null = null;
  /** Messages T3 sent that the engine has not echoed yet: T3's id is kept for the echo. */
  private readonly outgoing: { text: string; messageId: string; at: number }[] = [];
  private readonly echoed = new Map<string, string>();
  private ordinal = 0;
  private promptTimes: Record<string, number> = {};
  /** Durable start from the engine; turn/start normally precedes its user/message. */
  private pendingTurnStart: number | null = null;

  restorePromptTimes(times: Record<string, number> | undefined) { this.promptTimes = times ?? {}; }
  promptTime(messageId: string): number | undefined { return this.runs.find(run => run.userMessageId === messageId)?.requestedAt; }

  constructor(seed: ThreadSeed) {
    this.id = seed.id;
    this.title = seed.title ?? '';
    this.createdAt = seed.createdAt;
    this.updatedAt = seed.updatedAt ?? seed.createdAt;
    this.model = seed.model;
    this.mode = seed.mode;
    this.executor = seed.executor ?? 'claude';
    this.bindingKnown = seed.bindingKnown ?? true;
    this.interactionMode = seed.executor === 'codex' ? 'default' : seed.interactionMode ?? 'default';
    this.projectId = seed.projectId ?? HOH_PROJECT_ID;
  }

  private touch(at: number) { if (at > this.updatedAt) this.updatedAt = at; }
  private item(id: string) { return this.items.find(item => item.id === id); }
  private currentRun() { return this.runs.at(-1); }
  private base(id: string, type: string, status: Status, at: number, runId = this.currentRun()?.id ?? null, parentItemId: string | null = null): Item {
    return {
      id, threadId: this.id, runId, nodeId: null, providerThreadId: null, providerTurnId: null, nativeItemRef: null,
      parentItemId, ordinal: this.ordinal++, status, title: null,
      startedAt: iso(at), completedAt: status === 'running' || status === 'waiting' || status === 'pending' ? null : iso(at), updatedAt: iso(at), type,
    };
  }
  private settleItem(item: Item, status: Status, at: number) {
    item.status = status; item.updatedAt = iso(at);
    item.completedAt = status === 'running' || status === 'waiting' || status === 'pending' ? null : iso(at);
    if ('streaming' in item) item.streaming = false;
  }

  /**
   * The person sent this from T3: shown at once, with its pictures; the engine's echo of it (same id, or `sent`: the text
   * as it went to the engine) is not shown again.
   */
  sendLocal(messageId: string, value: string, at: number, attachments: ShownAttachment[] = [], sent = value) {
    this.addUserMessage(messageId, value, at, attachments);
    this.outgoing.push({ text: sent, messageId, at });
    this.touch(at);
  }

  /** The engine's id of a message sent from T3 (the `session/prompt` result). */
  receipt(engineId: string, messageId: string) {
    this.echoed.set(engineId, messageId);
    const index = this.outgoing.findIndex(entry => entry.messageId === messageId);
    if (index >= 0) this.outgoing.splice(index, 1);
  }

  /** `session/prompt` failed: the run never started. Shown as T3 shows a provider failure (an `error` item). */
  sendFailed(messageId: string, at: number, error?: unknown) {
    const index = this.outgoing.findIndex(entry => entry.messageId === messageId);
    if (index >= 0) this.outgoing.splice(index, 1);
    const run = this.runs.find(entry => entry.userMessageId === messageId);
    if (run && run.status === 'running') { run.status = 'failed'; run.completedAt = at; }
    this.items.push({
      ...this.base('item-error-' + messageId, 'error', 'failed', at, run?.id ?? null), title: 'Message not sent',
      failure: providerFailure(error, 'transport_error', 'The message was not sent. Try again.'),
    });
    this.running = false;
    this.touch(at);
  }

  /** A user message from the engine: a new run starts with it, unless it is the echo of one sent from T3. */
  private userMessage(engineId: string, value: string, at: number) {
    const start = this.pendingTurnStart;
    this.pendingTurnStart = null;
    const known = this.echoed.get(engineId);
    const pending = this.outgoing.findIndex(entry => entry.text === value);
    const messageId = known ?? (pending >= 0 ? this.outgoing.splice(pending, 1)[0].messageId : 'msg-' + engineId);
    this.echoed.set(engineId, messageId);
    if (known || pending >= 0) {
      if (start !== null) {
        const run = this.runs.find(entry => entry.userMessageId === messageId);
        if (run) run.engineStartedAt = start;
        // The local send can precede process startup by tens of seconds.
        // Keep that visible start; interrupted turns use the engine start at turn/end.
      }
      return;
    }
    const saved = this.promptTimes[engineId];
    this.addUserMessage(messageId, value, typeof saved === 'number' && Number.isFinite(saved) && saved > 0 && saved <= at ? saved : start ?? at);
    if (start !== null) this.currentRun()!.engineStartedAt = start;
  }

  private addUserMessage(messageId: string, value: string, at: number, attachments: ShownAttachment[] = []) {
    this.settledByGuess = false; this.stopRequested = false;
    // One run works at a time: the run this turn's start reopened was closed, and a run the engine never
    // closed ends where the next message begins (T3 shows any open run as working).
    if (this.reopened) { this.reopened.run.status = this.reopened.status; this.reopened.run.completedAt = this.reopened.completedAt; this.reopened = null; }
    for (const run of this.runs) if (run.status === 'running' || run.status === 'waiting') { run.status = 'completed'; run.completedAt = at; }
    if (!this.title) this.title = value.split('\n')[0].trim().slice(0, 80);
    const run: Run = { id: 'run-' + messageId, ordinal: this.runs.length + 1, model: this.model, userMessageId: messageId, status: 'running', requestedAt: at, startedAt: at, completedAt: null };
    this.runs.push(run);
    this.messages.push({ id: messageId, runId: run.id, role: 'user', text: value, streaming: false, createdAt: at, updatedAt: at, ...(attachments.length ? { attachments } : {}) });
    this.items.push({
      ...this.base('item-' + messageId, 'user_message', 'completed', at, run.id),
      createdBy: 'user', creationSource: 'web', messageId, inputIntent: 'turn_start', text: value, attachments,
    });
    this.running = true;
  }

  private assistantText(key: string, messageId: string, value: string, at: number, parentItemId: string | null) {
    const found = [...this.live.entries()].find(([, live]) => live.key === key && live.kind === 'text');
    const live = found?.[1];
    if (found) { this.live.delete(found[0]); this.retired.add(live!.attemptId); }
    const id = live?.itemId ?? 'item-a-' + messageId;
    const existing = this.item(id);
    if (!value) {
      if (existing) this.items.splice(this.items.indexOf(existing), 1);
      if (parentItemId) this.refreshSubagent(parentItemId);
      return;
    }
    const msgId = 'msg-a-' + messageId;
    if (existing) {
      Object.assign(existing, { text: value, messageId: msgId });
      this.settleItem(existing, 'completed', at);
    } else if (!this.item('item-a-' + messageId)) {
      this.items.push({ ...this.base(id, 'assistant_message', 'completed', at, undefined, parentItemId), messageId: msgId, text: value, streaming: false });
    }
    // A child assistant message belongs to the Task inspector. Putting it in `messages` as well would make
    // T3 render it as a second, top-level answer instead of work performed inside the subagent.
    if (!parentItemId) {
      const message = this.messages.find(entry => entry.id === msgId);
      if (message) Object.assign(message, { text: value, streaming: false, updatedAt: at });
      else this.messages.push({ id: msgId, runId: this.currentRun()?.id ?? null, role: 'assistant', text: value, streaming: false, createdAt: at, updatedAt: at });
    }
    if (parentItemId) this.refreshSubagent(parentItemId);
  }

  private assistantReasoning(key: string, messageId: string, value: string, at: number, parentItemId: string | null) {
    const found = [...this.live.entries()].filter(([, live]) => live.key === key && live.kind === 'reasoning');
    const live = found[0]?.[1];
    for (const [attempt, entry] of found) {
      this.live.delete(attempt); this.retired.add(entry.attemptId);
      if (entry !== live) {
        const duplicate = this.item(entry.itemId);
        if (duplicate) this.items.splice(this.items.indexOf(duplicate), 1);
      }
    }
    const id = live?.itemId ?? 'item-r-' + messageId;
    const existing = this.item(id);
    if (!value) {
      if (existing) this.items.splice(this.items.indexOf(existing), 1);
      return;
    }
    if (existing) {
      Object.assign(existing, { text: value });
      this.settleItem(existing, 'completed', at);
    } else if (!this.item('item-r-' + messageId)) {
      this.items.push({ ...this.base(id, 'reasoning', 'completed', at, undefined, parentItemId), text: value, streaming: false });
    }
  }

  /** A provider-native child names the native Task/Agent call; T3 items name their projected parent. */
  private parentItem(data: Record<string, any>): string | null {
    const parentCallId = text(data.parentCallId);
    // Claude tool-use ids are native session ids, independent of HoH's turn counter. A subagent may finish
    // after another turn was opened, so a turn-qualified lookup would silently flatten that late event.
    return parentCallId ? this.toolInputs.get(parentCallId)?.itemId ?? null : null;
  }

  /** Fold native child items into the output of T3's ordinary expandable Agent/Task tool row. */
  private refreshSubagent(parentItemId: string) {
    const parent = this.item(parentItemId);
    if (!parent || !isSubagentCall(parent)) return;
    const sections = this.items
      .filter(item => item.parentItemId === parentItemId)
      .sort((a, b) => a.ordinal - b.ordinal)
      .map(item => {
        if (item.type === 'assistant_message') return text(item.text).trim();
        const state = item.status === 'failed' ? 'failed' : item.status === 'running' || item.status === 'waiting' || item.status === 'pending' ? 'running' : 'done';
        let label = text(item.toolName);
        if (item.type === 'command_execution') label = `Command: ${text(item.input)}`;
        else if (item.type === 'file_change') label = `File: ${text(item.fileName)}`;
        else if (item.type === 'file_search') label = `Search: ${text(item.pattern)}`;
        else if (item.type === 'web_search') label = `Web: ${Array.isArray(item.patterns) ? item.patterns.join(', ') : ''}`;
        else if (!label) label = item.type.replaceAll('_', ' ');
        const result = text(item.subagentOutput ?? item.output).trim();
        return [`[${state}] ${label}`.trim(), result].filter(Boolean).join('\n');
      })
      .filter(Boolean);
    const result = visibleSubagentResult(parent.subagentResult);
    if (result) sections.push(`Result\n${result}`);
    const transcript = sections.join('\n\n');
    if (transcript.length <= 20_000) parent.output = transcript;
    else {
      // Preserve orientation plus the latest work and authoritative result. Keeping only the prefix makes
      // a long early Read/Bash erase the very information the folded row exists to expose.
      const marker = '\n\n… earlier subagent output truncated …\n\n';
      const headSize = 8_000;
      parent.output = transcript.slice(0, headSize) + marker + transcript.slice(-(20_000 - headSize - marker.length));
    }
  }

  private toolCall(data: Record<string, any>, at: number) {
    const rawId = text(data.callId); if (!rawId) return;
    const callId = String(data.turn ?? '') + ':' + rawId;
    if (this.calls.has(callId)) return;
    const name = text(data.name); const args = argumentsOf(data.arguments);
    const id = 'item-tool-' + callId.replace(/[^A-Za-z0-9_.:-]/g, '_');
    const parentItemId = this.parentItem(data);
    if (name === 'ExitPlanMode' && text(args.plan).trim()) {
      const markdown = text(args.plan).trim();
      const planId = 'plan-' + callId.replace(/[^A-Za-z0-9_.:-]/g, '_');
      const runId = this.currentRun()?.id ?? null;
      for (const plan of this.plans) {
        if (plan.status === 'active') plan.status = 'superseded';
      }
      this.plans.push({ id: planId, threadId: this.id, runId, nodeId: NODE_ID, kind: 'proposed_plan', status: 'active', markdown });
      this.items.push({ ...this.base(id, 'proposed_plan', 'completed', at, runId, parentItemId), planId, markdown, streaming: false });
      return;
    }
    const path = text(args.file_path ?? args.path ?? args.filePath ?? args.notebook_path);
    const kind = toolKind(name);
    let item: Item;
    if (kind === 'command') item = { ...this.base(id, 'command_execution', 'running', at, undefined, parentItemId), input: text(args.command ?? args.cmd) || name };
    else if (kind === 'write' && path.trim()) {
      const newStr = text(args.new_string ?? args.new_str ?? args.content ?? args.file_text);
      const oldStr = text(args.old_string ?? args.old_str);
      item = { ...this.base(id, 'file_change', 'running', at, undefined, parentItemId), fileName: path.trim(), ...(newStr ? { newStr } : {}), ...(oldStr ? { oldStr } : {}) };
    } else if (kind === 'search') {
      const pattern = text(args.pattern ?? args.query ?? args.path);
      item = { ...this.base(id, 'file_search', 'running', at, undefined, parentItemId), ...(pattern ? { pattern } : {}) };
    } else if (kind === 'web') {
      const pattern = text(args.query ?? args.url);
      item = { ...this.base(id, 'web_search', 'running', at, undefined, parentItemId), ...(pattern ? { patterns: [pattern] } : {}) };
    } else item = { ...this.base(id, 'dynamic_tool', 'running', at, undefined, parentItemId), toolName: name.trim() || null, input: args };
    this.calls.set(callId, id);
    this.toolInputs.set(rawId, { name, args, itemId: id });
    this.items.push(item);
    if (parentItemId) this.refreshSubagent(parentItemId);
  }

  private toolResult(message: Record<string, any>, turn: unknown, at: number) {
    const rawId = text(message.toolCallId);
    const id = this.calls.get(String(turn ?? '') + ':' + rawId) ?? this.toolInputs.get(rawId)?.itemId;
    const item = id ? this.item(id) : undefined;
    if (!item) return;
    const output = contentText(message.content) || text(message.content);
    const failed = message.isError === true;
    if (isSubagentCall(item)) {
      item.subagentResult = output.slice(0, 20_000);
    } else if (item.type === 'command_execution') {
      const match = Array.from(output.matchAll(/\[(?:Command finished with exit code|shell exited: code)\s+(-?\d+)\]/gi)).at(-1);
      const exitCode = match ? Number(match[1]) : undefined;
      Object.assign(item, { output: output.slice(0, 20_000), ...(exitCode === undefined ? {} : { exitCode }), ...(failed || (exitCode ?? 0) !== 0 ? { outputIndicatesFailure: true } : {}) });
    } else if (item.type === 'dynamic_tool') item.output = output.slice(0, 20_000);
    if (item.parentItemId) item.subagentOutput = output.slice(0, 20_000);
    this.settleItem(item, failed ? 'failed' : 'completed', at);
    if (isSubagentCall(item)) this.refreshSubagent(item.id);
    if (item.parentItemId) this.refreshSubagent(item.parentItemId);
  }

  private updateUsage(rawUsage: unknown, at: number) {
    const run = this.currentRun(); const usage = record(rawUsage);
    const count = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null;
    const input = count(usage.inputTokens); const created = count(usage.cacheCreationInputTokens); const cached = count(usage.cacheReadInputTokens); const output = count(usage.outputTokens); const max = count(usage.maxTokens);
    if (run && input !== null && created !== null && cached !== null && output !== null) {
      const inputTokens = input + created + cached; const maxTokens = max !== null && max > 0 ? max : run.tokenUsage?.maxTokens;
      run.tokenUsage = { usedTokens: inputTokens + output, ...(maxTokens ? { maxTokens } : {}), inputTokens, cachedInputTokens: cached, outputTokens: output, reasoningOutputTokens: 0, updatedAt: iso(at) };
    }
  }

  private endTurn(reason: string, at: number, error?: unknown, cause?: unknown, rawUsage?: unknown) {
    this.runningInteractionMode = null;
    this.reopened = null;
    const run = this.currentRun();
    const status: RunStatus = reason === 'aborted' || (this.stopRequested && reason !== 'error') ? 'interrupted' : reason === 'error' ? 'failed' : 'completed';
    this.stopRequested = false;
    const open = run?.status === 'running' || run?.status === 'waiting';
    const lateCompletedGuess = (run?.status === 'interrupted' || run?.status === 'completed') && this.settledByGuess && status === 'completed';
    const closing = !!run && (open || lateCompletedGuess || (reason === 'aborted' && record(cause).kind === 'user'));
    if (run && closing) {
      if (status === 'interrupted' && run.engineStartedAt !== undefined) {
        run.startedAt = run.engineStartedAt;
        const message = this.messages.find(entry => entry.id === run.userMessageId);
        const item = this.item('item-' + run.userMessageId);
        if (message) { message.createdAt = run.engineStartedAt; message.updatedAt = run.engineStartedAt; }
        if (item) Object.assign(item, { startedAt: iso(run.engineStartedAt), completedAt: iso(run.engineStartedAt), updatedAt: iso(run.engineStartedAt) });
      }
      run.status = status; run.completedAt = at;
    }
    this.updateUsage(rawUsage, at);
    // T3 uses the final answer's update time for earlier runs. Advance it to the
    // durable turn/end, so later turns cannot change the displayed duration.
    const answer = closing ? [...this.items].reverse().find(item => item.runId === run?.id && item.type === 'assistant_message' && !item.parentItemId) : undefined;
    if (answer) answer.updatedAt = iso(at);
    const answerMessage = answer && this.messages.find(message => message.id === answer.messageId);
    if (answerMessage) answerMessage.updatedAt = at;
    for (const item of this.items) {
      if (item.status === 'running' || item.status === 'waiting' || item.status === 'pending')
        this.settleItem(item, item.type === 'assistant_message' ? 'completed' : status === 'completed' ? 'completed' : 'interrupted', at);
    }
    for (const item of this.items) if (isSubagentCall(item)) this.refreshSubagent(item.id);
    for (const live of this.live.values()) this.retired.add(live.attemptId);
    this.live.clear();
    for (const request of this.requests) if (request.status === 'pending') { request.status = 'cancelled'; request.resolvedAt = at; }
    // This is durable provenance from the engine, not the browser's Stop click. T3 already uses
    // run_interrupt_result to remember which run was deliberately stopped; replaying the journal
    // reconstructs the same marker for Claude and Codex after any window restart.
    if (reason === 'aborted' && record(cause).kind === 'user' && run && !this.item('item-stop-' + run.id)) {
      this.items.push({
        ...this.base('item-stop-' + run.id, 'run_interrupt_result', 'completed', at, run.id),
        message: 'Stopped by user',
      });
    }
    // T3's own form and words for a failed turn (packages/provider-core/src/server/failure.ts `makeProviderFailureTurnItem`).
    if (reason === 'error') this.items.push({ ...this.base('item-error-' + this.ordinal, 'error', 'failed', at), title: 'Provider error', failure: providerFailure(error, 'provider_error', 'Provider turn failed.') });
    this.running = false;
  }

  /** One durable session event of the engine (`session/events` or `session.event`). Returns whether it changed the thread. */
  applyEvent(event: { seq?: unknown; type?: unknown; time?: unknown; data?: unknown }): boolean {
    if (!Number.isSafeInteger(event?.seq) || this.seen.has(event.seq as number)) return false;
    this.seen.add(event.seq as number);
    const at = typeof event.time === 'number' && event.time > 0 ? event.time : Date.now();
    const data = record(event.data);
    // A start precedes its user message. Do not reopen the previous run until an actual
    // continuation (rather than a new prompt) arrives.
    if (this.reopened && ['assistant/message', 'tool/call', 'turn/end'].includes(String(event.type))) {
      this.reopened.run.status = 'running'; this.reopened.run.completedAt = null; this.reopened = null;
    }
    switch (event.type) {
      // Not shown in the chat: the folder decides which T3 project the chat belongs to.
      case 'working-directory/change':
        if (text(data.cwd)) this.cwd = text(data.cwd);
        return false;
      case 'agent/inbox/spliced':
        for (const message of Array.isArray(data.inserted) ? data.inserted : []) {
          const inserted = record(message);
          if (inserted.source?.kind === 'user' && text(inserted.id)) this.userMessage(text(inserted.id), contentText(inserted.content), at);
        }
        break;
      case 'user/message':
        if (data.source?.kind !== 'user' || !text(data.id)) return false;
        this.userMessage(text(data.id), contentText(data.content), at);
        break;
      case 'turn/start': {
        this.pendingTurnStart = at;
        // A turn without a message of its own continues the last run; a message, when it follows, starts its own.
        const run = this.currentRun();
        if (run && run.status !== 'running' && run.status !== 'waiting') {
          this.reopened = { run, status: run.status, completedAt: run.completedAt };
        }
        this.running = true; this.settledByGuess = false;
        break;
      }
      case 'turn/usage':
      case 'plugin:hoh/context-usage': this.updateUsage(data.usage, at); break;
      case 'turn/end': {
        this.pendingTurnStart = null;
        const reason = record(data.reason);
        this.endTurn(text(reason.kind), at, reason.error, reason.reason, data.usage);
        this.engineRunning = false; this.settledByGuess = false;
        break;
      }
      case 'assistant/message': {
        const message = record(data.message);
        if (!text(message.id)) return false;
        const parentItemId = this.parentItem(data);
        const key = String(data.turn ?? '') + ':' + String(data.step ?? '') + (text(data.parentCallId) ? ':' + text(data.parentCallId) : '');
        this.assistantReasoning(key, text(message.id), contentReasoning(message.content), at, parentItemId);
        this.assistantText(key, text(message.id), contentText(message.content), at, parentItemId);
        break;
      }
      case 'tool/call': this.toolCall(data, at); break;
      // The permission request of a tool call: the card shows that call as T3 shows a Claude request.
      case 'approval/asked': {
        const id = text(data.id);
        if (!id) return false;
        const tool = this.toolInputs.get(text(data.callId));
        const request = tool
          ? toolRequest(tool.name, tool.args, this.cwd)
          : toolRequest(text(data.toolName), {}, this.cwd);
        this.approvalDetails.set(id, { requestKind: request.requestKind });
        const item = this.item('item-approval-' + id);
        if (item) item.requestKind = request.requestKind;
        const entry = this.requests.find(candidate => candidate.id === id);
        if (entry) entry.kind = request.requestKind;
        break;
      }
      case 'tool/result': this.toolResult(record(data.message), data.turn, at); break;
      default: return false;
    }
    this.touch(at);
    return true;
  }

  /** Live reasoning/text (`session.assistant-stream`), replaced by durable `assistant/message`. */
  applyStreamFrame(frame: Record<string, any>, at = Date.now()): boolean {
    const attempt = text(frame.attemptId);
    if (!attempt || this.retired.has(attempt)) return false;
    if (frame.type === 'start') {
      const key = String(frame.turn ?? '') + ':' + String(frame.step ?? '');
      if (this.live.has(attempt)) return false;
      this.live.set(attempt, { itemId: 'item-live-' + attempt, attemptId: attempt, key, kind: null, raw: '', last: -1, gapped: false });
      return false;
    }
    const live = this.live.get(attempt);
    if (!live) return false;
    if (frame.type === 'gap') { live.gapped = true; return false; }
    if (frame.type === 'end') {
      this.retired.add(attempt);
      const item = this.item(live.itemId);
      if (item && !live.gapped) { item.streaming = false; item.updatedAt = iso(at); return true; }
      return false;
    }
    const chunk = record(frame.chunk);
    const kind = chunk.type === 'text-delta' ? 'text' : chunk.type === 'reasoning-delta' ? 'reasoning' : null;
    if (live.gapped || frame.type !== 'chunk' || !kind || typeof chunk.text !== 'string') return false;
    if (live.kind !== null && live.kind !== kind) return false;
    if (!Number.isSafeInteger(frame.index) || frame.index <= live.last) return false;
    live.kind = kind; live.last = frame.index; live.raw += chunk.text;
    const item = this.item(live.itemId);
    if (item) Object.assign(item, { text: live.raw, updatedAt: iso(at) });
    else if (kind === 'text') this.items.push({ ...this.base(live.itemId, 'assistant_message', 'running', at), messageId: 'msg-live-' + attempt, text: live.raw, streaming: true });
    else this.items.push({ ...this.base(live.itemId, 'reasoning', 'running', at), text: live.raw, streaming: true });
    this.touch(at);
    return true;
  }

  setRunning(running: boolean, at = Date.now()): boolean {
    const engineChanged = this.engineRunning !== running;
    this.engineRunning = running;
    const run = this.currentRun();
    if (running && this.settledByGuess && run && (run.status === 'interrupted' || run.status === 'completed')) {
      // The history ended mid-turn because the turn is still going: it was not stopped.
      run.status = 'running'; run.completedAt = null; this.settledByGuess = false; this.running = true;
      return true;
    }
    // The engine says idle: no run stays open, whatever this flag held (the sidebar lists a thread with an open run as working).
    // Some harnesses report the process as idle while it is synchronously waiting for the person's
    // answer. The typed pending request is stronger evidence that the turn is still open; closing it
    // here would cancel the card before the native Broker's one-second read delay has elapsed.
    const open = !running && !!run && (run.status === 'running' || run.status === 'waiting')
      && this.outgoing.length === 0 && !this.requests.some(request => request.status === 'pending');
    if (this.running === running && !open) return engineChanged;
    this.running = running;
    if (open) { this.endTurn('completed', at); this.settledByGuess = true; }
    return true;
  }

  /** «Стоп» went through: an open run ends as stopped. Returns whether anything changed. */
  interrupt(at: number): boolean {
    const run = this.currentRun();
    if (!run || (run.status !== 'running' && run.status !== 'waiting')) return false;
    this.stopRequested = true;
    this.endTurn('aborted', at);
    this.settledByGuess = false;
    return true;
  }

  /** History ended without the closing event of its last turn and the engine is not working on it. */
  settleUnfinished(at = Date.now()) {
    if (this.engineRunning) return;
    const run = this.currentRun();
    if (run && (run.status === 'running' || run.status === 'waiting')) { this.endTurn('aborted', at); this.settledByGuess = true; }
  }

  /**
   * A permission request of the assistant (`approval.pending`), answered in the chat. Its visible text is
   * the engine's `humanText` for this approval id; the durable `approval/asked` only supplies the card kind.
   */
  approvalPending(params: { approvalId: string; humanText?: string; risk?: string }, at = Date.now()) {
    if (!text(params.approvalId) || this.requests.some(request => request.id === params.approvalId)) return false;
    const run = this.currentRun();
    const request = this.approvalDetails.get(params.approvalId) ?? { requestKind: 'command' };
    this.requests.push({ id: params.approvalId, kind: request.requestKind, status: 'pending', createdAt: at, resolvedAt: null });
    this.items.push({
      ...this.base('item-approval-' + params.approvalId, 'approval_request', 'waiting', at),
      requestId: params.approvalId, requestKind: request.requestKind,
      ...(text(params.humanText) ? { prompt: text(params.humanText) } : {}),
      // T3's own set for a Claude request without a standing grant (apps/server ClaudeAdapterV2.ts
      // `CLAUDE_NO_STANDING_GRANT_OPTIONS`): HoH allows one action at a time.
      options: [{ decision: 'decline', label: 'Decline' }, { decision: 'accept', label: 'Approve' }],
    });
    if (run && run.status === 'running') run.status = 'waiting';
    this.touch(at);
    return true;
  }

  approvalResolved(params: { approvalId: string; outcome?: string }, at = Date.now()) {
    const request = this.requests.find(entry => entry.id === params.approvalId);
    if (!request || request.status !== 'pending') return false;
    const allowed = params.outcome === 'allowed-once';
    request.status = allowed || params.outcome === 'rejected' ? 'resolved' : 'cancelled';
    request.resolvedAt = at;
    if (request.status === 'resolved') request.decision = allowed ? 'accept' : 'decline';
    const item = this.item('item-approval-' + params.approvalId);
    if (item) this.settleItem(item, request.status === 'resolved' ? 'completed' : 'cancelled', at);
    this.approvalDetails.delete(params.approvalId);
    const run = this.currentRun();
    if (run && run.status === 'waiting' && !this.requests.some(entry => entry.status === 'pending')) run.status = 'running';
    this.touch(at);
    return true;
  }

  /** A question of the assistant (`user-question/ask`), shown as T3's question card. */
  questionAsked(question: EngineQuestion, at = Date.now()) {
    if (this.requests.some(request => request.id === question.requestId)) return false;
    this.requests.push({ id: question.requestId, kind: 'user_input', status: 'pending', createdAt: at, resolvedAt: null });
    this.items.push({
      ...this.base('item-question-' + question.requestId, 'user_input_request', 'waiting', at),
      requestId: question.requestId,
      questions: question.questions.map((entry, index) => ({
        id: nonBlank(entry.id, 'q' + index),
        header: nonBlank(text(entry.header), 'Question'),
        question: nonBlank([entry.question, entry.detail].filter(Boolean).join('\n\n'), 'Question'),
        options: (entry.options ?? []).filter(option => text(option.label).trim()).map(option => ({
          label: option.label.trim(), description: nonBlank(text(option.description), option.label.trim()), value: option.label.trim(),
        })),
        multiSelect: entry.multiSelect === true,
        allowCustomAnswer: true,
      })),
    });
    const run = this.currentRun();
    if (run && run.status === 'running') run.status = 'waiting';
    this.touch(at);
    return true;
  }

  questionClosed(requestId: string, answers: Record<string, unknown> | null, at = Date.now()) {
    const request = this.requests.find(entry => entry.id === requestId);
    if (!request || request.status !== 'pending') return false;
    request.status = answers ? 'resolved' : 'cancelled'; request.resolvedAt = at;
    if (answers) request.answers = answers;
    const item = this.item('item-question-' + requestId);
    if (item) this.settleItem(item, answers ? 'completed' : 'cancelled', at);
    const run = this.currentRun();
    if (run && run.status === 'waiting' && !this.requests.some(entry => entry.status === 'pending')) run.status = 'running';
    this.touch(at);
    return true;
  }

  pendingRequest(id: string) { return this.requests.find(request => request.id === id && request.status === 'pending'); }
  activeRunId() { const run = this.currentRun(); return run && (run.status === 'running' || run.status === 'waiting') ? run.id : null; }
  userMessageCount() { return this.messages.filter(message => message.role === 'user').length; }
  setInteractionMode(mode: T3InteractionMode) { this.interactionMode = mode; }
  setRunningInteractionMode(mode: T3InteractionMode | null) { this.runningInteractionMode = mode; }
  private projectedInteractionMode(): T3InteractionMode {
    if (this.executor === 'codex') return 'default';
    return (this.running || this.engineRunning || this.activeRunId() !== null) && this.runningInteractionMode !== null
      ? this.runningInteractionMode : this.interactionMode;
  }
  private projectedRuntimeMode() {
    if (!this.bindingKnown) return 'unknown';
    // A queued turn can start before the executor publishes its actual launch. Do not label
    // that interval with either the previous process or the newly selected access mode.
    if ((this.running || this.engineRunning || this.activeRunId() !== null) && this.runningInteractionMode === null) return 'unknown';
    return runtimeModeOf(this.mode);
  }

  private instanceId() { return !this.bindingKnown ? 'unknown' : this.executor === 'codex' ? CODEX_INSTANCE_ID : CLAUDE_INSTANCE_ID; }
  private modelSelection() { return { instanceId: this.instanceId(), model: this.bindingKnown ? this.model : 'unknown', ...(this.options?.length ? { options: this.options } : {}) }; }
  private titleText() { return nonBlank(this.title, 'New thread'); }

  /** `OrchestrationV2ThreadProjection`, encoded. */
  projection() {
    const updatedAt = iso(this.updatedAt);
    const visibleItems = this.items.filter(item => item.parentItemId === null);
    return {
      thread: {
        createdBy: 'user', creationSource: 'web',
        id: this.id, projectId: this.projectId, title: this.titleText(), providerInstanceId: this.instanceId(),
        modelSelection: this.modelSelection(), runtimeMode: this.projectedRuntimeMode(), interactionMode: this.projectedInteractionMode(),
        branch: null, worktreePath: null, activeProviderThreadId: null,
        lineage: { parentThreadId: null, relationshipToParent: null, rootThreadId: this.id },
        forkedFrom: null, createdAt: iso(this.createdAt), updatedAt,
        pinnedAt: this.pinnedAt === null ? null : iso(this.pinnedAt), pinOrderKey: this.pinOrderKey,
        archivedAt: this.archivedAt === null ? null : iso(this.archivedAt), settledOverride: null, settledAt: null, lastVisitedAt: null, deletedAt: null,
      },
      runs: this.runs.map(run => ({
        id: run.id, threadId: this.id, ordinal: run.ordinal, providerInstanceId: this.instanceId(),
        modelSelection: { instanceId: this.instanceId(), model: run.model }, providerThreadId: null, userMessageId: run.userMessageId,
        rootNodeId: null, activeAttemptId: null, status: run.status, requestedAt: iso(run.requestedAt),
        startedAt: run.startedAt === null ? null : iso(run.startedAt), completedAt: run.completedAt === null ? null : iso(run.completedAt),
        checkpointId: null, contextHandoffId: null,
      })),
      attempts: [], nodes: [], subagents: [], providerSessions: [], providerThreads: [],
      providerTurns: this.runs.filter(run => run.tokenUsage).map(run => ({
        id: 'provider-turn-' + run.id, providerThreadId: 'hoh-' + this.id, nodeId: NODE_ID, runAttemptId: null,
        nativeTurnRef: null, ordinal: run.ordinal, status: run.status === 'waiting' ? 'running' : run.status, startedAt: run.startedAt === null ? null : iso(run.startedAt),
        completedAt: run.completedAt === null ? null : iso(run.completedAt), tokenUsage: { ...run.tokenUsage! }, model: run.model,
      })),
      runtimeRequests: this.requests.map(request => ({
        id: request.id, nodeId: NODE_ID, providerTurnId: null, nativeRequestRef: null, kind: request.kind, status: request.status,
        responseCapability: { type: 'live', providerSessionId: 'hoh-' + this.id },
        createdAt: iso(request.createdAt), resolvedAt: request.resolvedAt === null ? null : iso(request.resolvedAt),
        ...(request.decision ? { decision: request.decision } : {}), ...(request.answers ? { answers: request.answers } : {}),
      })),
      messages: this.messages.map(message => ({
        createdBy: message.role === 'user' ? 'user' : 'agent', creationSource: message.role === 'user' ? 'web' : 'provider',
        id: message.id, threadId: this.id, runId: message.runId, nodeId: null, role: message.role, text: message.text,
        attachments: message.attachments ?? [], streaming: message.streaming, createdAt: iso(message.createdAt), updatedAt: iso(message.updatedAt),
      })),
      plans: this.plans.map(plan => ({ ...plan })),
      turnItems: this.items.map(item => ({ ...item })),
      checkpointScopes: [], checkpoints: [], contextHandoffs: [], contextTransfers: [],
      // Native subagent items stay in turnItems for state and attribution, while T3 displays their folded
      // transcript inside the expandable Agent/Task row instead of duplicating them in the root timeline.
      visibleTurnItems: visibleItems.map((item, position) => ({ position, visibility: 'local', sourceThreadId: this.id, sourceItemId: item.id, item: { ...item } })),
      updatedAt,
    };
  }

  /**
   * What the home screen shows of this chat (HOH-41 В2), from what the host already knows: the state of the last run,
   * its real steps (tool calls), and the request that waits for the person. Nothing is estimated.
   */
  home(turnOrdinal?: number): HomeEntry | null {
    const run = turnOrdinal === undefined ? this.currentRun() : this.runs.find(candidate => candidate.ordinal === turnOrdinal);
    if (!run) return null;
    const calls = this.items.filter(item => item.runId === run.id && STEP_TYPES.has(item.type));
    const roots = calls.filter(item => item.parentItemId === null);
    const steps = roots.filter(item => !isSubagentCall(item));
    const agents: HomeAgent[] = roots.filter(isSubagentCall).map(item => {
      const input = record(item.input);
      const kind = text(input.subagent_type) || 'general-purpose';
      const child = this.items.filter(candidate => candidate.parentItemId === item.id).sort((a, b) => b.ordinal - a.ordinal)[0];
      const childTool = child ? text(child.toolName) || child.type.replaceAll('_', ' ') : '';
      const currentStep = child?.type === 'assistant_message'
        ? text(child.text).replace(/\s+/g, ' ').trim()
        : child?.type === 'command_execution' ? text(child.input).replace(/\s+/g, ' ').trim()
        : child?.type === 'file_change' ? text(child.fileName)
        : child?.type === 'file_search' ? `Search: ${text(child.pattern)}`
        : child?.type === 'web_search' ? `Web: ${Array.isArray(child.patterns) ? child.patterns.join(', ') : ''}`
        : child ? (toolRequest(childTool, record(child.input), this.cwd).prompt ?? childTool).split('\n')[0] : '';
      const children = this.items.filter(candidate => candidate.parentItemId === item.id);
      const activeChild = children.some(candidate => candidate.status === 'running' || candidate.status === 'waiting' || candidate.status === 'pending');
      const active = item.status === 'running' || item.status === 'waiting' || item.status === 'pending' || activeChild;
      // The Agent/Task result is authoritative: a subagent may recover from a failed inner command.
      const failed = item.status === 'failed';
      const completedTimes = [item, ...children].map(candidate => candidate.completedAt ? Date.parse(candidate.completedAt) : null).filter((value): value is number => value !== null);
      return {
        id: item.id, kind, name: text(input.description) || text(input.prompt).slice(0, 80) || kind,
        state: active ? 'working' : failed ? 'failed' : 'done',
        startedAt: Date.parse(item.startedAt), doneAt: active ? null : completedTimes.length ? Math.max(...completedTimes) : null,
        currentStep: currentStep ? currentStep.slice(0, 160) : null,
      };
    });
    const pending = this.requests.find(request => request.status === 'pending');
    const waitingItem = pending && this.items.find(item => item.requestId === pending.id);
    const open = run.status === 'running' || run.status === 'waiting';
    const question = waitingItem?.type === 'user_input_request' ? text(waitingItem.questions?.[0]?.question) : '';
    const interrupted = run === this.currentRun() && run.status === 'interrupted' && this.settledByGuess;
    return {
      id: this.id, title: this.titleText(), projectId: this.projectId, providerInstanceId: this.instanceId(),
      state: open ? (pending ? 'waiting' : 'working') : run.status === 'completed' ? 'done' : 'ended',
      turnOrdinal: run.ordinal,
      startedAt: run.startedAt ?? run.requestedAt, doneAt: run.completedAt,
      steps: { done: steps.filter(item => item.status !== 'running' && item.status !== 'waiting').length, total: steps.length }, agents,
      ...(interrupted ? { interrupted: true } : {}),
      ...(pending && waitingItem ? { request: { id: pending.id, kind: waitingItem.type === 'user_input_request' ? 'question' : 'approval', requestKind: waitingItem.requestKind ?? null, text: waitingItem.type === 'user_input_request' ? question : text(waitingItem.prompt) } } : {}),
    };
  }

  /** `OrchestrationV2ThreadShell`, encoded: the row of the thread list. */
  shell() {
    const run = this.currentRun();
    const active = this.activeRunId();
    const pending = this.requests.find(request => request.status === 'pending');
    const visible = [...this.messages].reverse().find(message => message.text.trim());
    const lastUser = [...this.messages].reverse().find(message => message.role === 'user');
    return {
      createdBy: 'user', creationSource: 'web',
      id: this.id, projectId: this.projectId, title: this.titleText(), providerInstanceId: this.instanceId(),
      modelSelection: this.modelSelection(), runtimeMode: this.projectedRuntimeMode(), interactionMode: this.projectedInteractionMode(),
      branch: null, worktreePath: null,
      lineage: { parentThreadId: null, relationshipToParent: null, rootThreadId: this.id },
      forkedFrom: null, activeProviderThreadId: null,
      latestRunId: run?.id ?? null, activeRunId: active,
      status: run ? (active ? (run.status === 'waiting' ? 'waiting' : 'running') : run.status) : 'idle',
      pendingRuntimeRequest: pending ? { id: pending.id, kind: pending.kind, createdAt: iso(pending.createdAt), blocking: true } : null,
      latestVisibleMessage: visible ? { id: visible.id, role: visible.role, text: visible.text.slice(0, 400), updatedAt: iso(visible.updatedAt) } : null,
      // Null only for a chat made here that has no message yet (an empty draft): the window keeps it out of «Recent chats»
      // until the first message. A chat of the engine's history not read yet (older than today) is stamped by its last activity.
      latestUserMessageAt: lastUser ? iso(lastUser.createdAt) : this.loaded ? null : iso(this.updatedAt),
      hasActionableProposedPlan: this.plans.some(plan => plan.kind === 'proposed_plan' && plan.status === 'active'),
      itemCount: this.items.length, visibleItemCount: this.items.filter(item => item.parentItemId === null).length,
      createdAt: iso(this.createdAt), updatedAt: iso(this.updatedAt),
      pinnedAt: this.pinnedAt === null ? null : iso(this.pinnedAt), pinOrderKey: this.pinOrderKey,
      archivedAt: this.archivedAt === null ? null : iso(this.archivedAt), settledOverride: null, settledAt: null, deletedAt: null,
    };
  }
}

/** HoH's chats folder as T3's own "No project" (apps/server ManagedProjectFolders.ts `ensureScratchProject`). */
export function hohProject(workspaceRoot: string, at: number) {
  return {
    id: HOH_PROJECT_ID,
    title: 'No project',
    workspaceRoot,
    projectIcon: { kind: 'lucide', name: 'message-square-dashed', color: 'gray' },
    repositoryIdentity: null,
    defaultModelSelection: null,
    scripts: [],
    createdAt: iso(at),
    updatedAt: iso(at),
  };
}

/** A folder the person added as a T3 project ("Add project"), as the host keeps it. */
export interface FolderProject {
  readonly id: string;
  title: string;
  workspaceRoot: string;
  readonly createdAt: number;
  updatedAt: number;
  pinnedAt: number | null;
  archivedAt: number | null;
}

/** A folder project as T3's `OrchestrationProjectShell`, encoded (no git identity: HoH reads no repository). */
export function folderProject(project: FolderProject) {
  return {
    id: project.id,
    title: project.title,
    workspaceRoot: project.workspaceRoot,
    repositoryIdentity: null,
    defaultModelSelection: null,
    scripts: [],
    pinnedAt: project.pinnedAt === null ? null : iso(project.pinnedAt),
    archivedAt: project.archivedAt === null ? null : iso(project.archivedAt),
    createdAt: iso(project.createdAt),
    updatedAt: iso(project.updatedAt),
  };
}
