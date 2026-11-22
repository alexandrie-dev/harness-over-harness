// One HoH chat as a T3 thread. The engine's session events (the same ones the current window reads:
// ui/src/screens/chat/projection.mjs) are folded into T3's `OrchestrationV2ThreadProjection` and
// `OrchestrationV2ThreadShell` (packages/contracts/src/orchestrationV2.ts), in their encoded JSON form.
// One user message is one run; answers, tool steps, permission requests and questions are turn items.
import { CLAUDE_INSTANCE_ID, runtimeModeOf, type HohAccessMode } from './config.ts';
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
function argumentsOf(value: unknown): Record<string, any> {
  try { return record(typeof value === 'string' ? JSON.parse(value) : value); } catch { return {}; }
}

type Status = 'idle' | 'pending' | 'running' | 'waiting' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
type RunStatus = 'running' | 'waiting' | 'completed' | 'interrupted' | 'failed' | 'cancelled';
interface Run { id: string; ordinal: number; model: string; userMessageId: string; status: RunStatus; requestedAt: number; startedAt: number | null; completedAt: number | null }
type Item = Record<string, any> & { id: string; type: string; status: Status; runId: string | null; ordinal: number };
interface Message { id: string; runId: string | null; role: 'user' | 'assistant' | 'system'; text: string; streaming: boolean; createdAt: number; updatedAt: number }
interface Request {
  id: string; kind: string; status: 'pending' | 'resolved' | 'expired' | 'cancelled';
  createdAt: number; resolvedAt: number | null; decision?: string; answers?: Record<string, unknown>;
}
interface Live { itemId: string; attemptId: string; raw: string; last: number; gapped: boolean }

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
function toolRequest(name: string, args: Record<string, any>): { requestKind: string; prompt?: string } {
  const kind = toolKind(name);
  const requestKind = kind === 'write' ? 'file-change' : kind === 'read' || kind === 'search' ? 'file-read' : 'command';
  const first = (keys: string[]) => keys.map(key => args[key]).find((value): value is string => typeof value === 'string' && value.trim() !== '');
  const command = first(['command']);
  if (command) return { requestKind, prompt: command };
  const tool = CLAUDE_TOOL_NAMES[name.toLowerCase()] ?? name;
  if (!tool.trim()) return { requestKind };
  const subject = first(['cmd', 'script']) ?? first(['file_path', 'path', 'filename', 'fileName']);
  if (subject) return { requestKind, prompt: `${tool}: ${subject.slice(0, 400)}` };
  const input = JSON.stringify(args);
  return { requestKind, prompt: `${tool}: ${input.length <= 400 ? input : input.slice(0, 397) + '...'}` };
}

const STEP_TYPES = new Set(['command_execution', 'file_change', 'file_search', 'web_search', 'dynamic_tool']);

/** One chat on the home screen. `state`: working, waiting (for the person's answer), done, ended (stopped or failed). */
export interface HomeEntry {
  id: string; title: string; projectId: string;
  state: 'working' | 'waiting' | 'done' | 'ended';
  startedAt: number; doneAt: number | null;
  steps: { done: number; total: number };
  request?: { id: string; kind: 'approval' | 'question'; requestKind: string | null; text: string };
}

export interface ThreadSeed {
  readonly id: string;
  readonly title?: string;
  readonly createdAt: number;
  readonly updatedAt?: number;
  readonly model: string;
  readonly mode: HohAccessMode;
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
  private readonly seen = new Set<number>();
  private readonly runs: Run[] = [];
  private readonly items: Item[] = [];
  private readonly messages: Message[] = [];
  private readonly requests: Request[] = [];
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

  constructor(seed: ThreadSeed) {
    this.id = seed.id;
    this.title = seed.title ?? '';
    this.createdAt = seed.createdAt;
    this.updatedAt = seed.updatedAt ?? seed.createdAt;
    this.model = seed.model;
    this.mode = seed.mode;
    this.projectId = seed.projectId ?? HOH_PROJECT_ID;
  }

  private touch(at: number) { if (at > this.updatedAt) this.updatedAt = at; }
  private item(id: string) { return this.items.find(item => item.id === id); }
  private currentRun() { return this.runs.at(-1); }
  private base(id: string, type: string, status: Status, at: number, runId = this.currentRun()?.id ?? null): Item {
    return {
      id, threadId: this.id, runId, nodeId: null, providerThreadId: null, providerTurnId: null, nativeItemRef: null,
      parentItemId: null, ordinal: this.ordinal++, status, title: null,
      startedAt: iso(at), completedAt: status === 'running' || status === 'waiting' || status === 'pending' ? null : iso(at), updatedAt: iso(at), type,
    };
  }
  private settleItem(item: Item, status: Status, at: number) {
    item.status = status; item.updatedAt = iso(at);
    item.completedAt = status === 'running' || status === 'waiting' || status === 'pending' ? null : iso(at);
    if ('streaming' in item) item.streaming = false;
  }

  /** The person sent this from T3: shown at once; the engine's echo of it (same id or same text) is not shown again. */
  sendLocal(messageId: string, value: string, at: number) {
    this.addUserMessage(messageId, value, at);
    this.outgoing.push({ text: value, messageId, at });
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
    if (this.echoed.has(engineId)) return;
    const pending = this.outgoing.findIndex(entry => entry.text === value);
    if (pending >= 0) { this.echoed.set(engineId, this.outgoing.splice(pending, 1)[0].messageId); return; }
    const messageId = 'msg-' + engineId;
    this.echoed.set(engineId, messageId);
    this.addUserMessage(messageId, value, at);
  }

  private addUserMessage(messageId: string, value: string, at: number) {
    this.settledByGuess = false; this.stopRequested = false;
    // One run works at a time: the run this turn's start reopened was closed, and a run the engine never
    // closed ends where the next message begins (T3 shows any open run as working).
    if (this.reopened) { this.reopened.run.status = this.reopened.status; this.reopened.run.completedAt = this.reopened.completedAt; this.reopened = null; }
    for (const run of this.runs) if (run.status === 'running' || run.status === 'waiting') { run.status = 'completed'; run.completedAt = at; }
    if (!this.title) this.title = value.split('\n')[0].trim().slice(0, 80);
    const run: Run = { id: 'run-' + messageId, ordinal: this.runs.length + 1, model: this.model, userMessageId: messageId, status: 'running', requestedAt: at, startedAt: at, completedAt: null };
    this.runs.push(run);
    this.messages.push({ id: messageId, runId: run.id, role: 'user', text: value, streaming: false, createdAt: at, updatedAt: at });
    this.items.push({
      ...this.base('item-' + messageId, 'user_message', 'completed', at, run.id),
      createdBy: 'user', creationSource: 'web', messageId, inputIntent: 'turn_start', text: value, attachments: [],
    });
    this.running = true;
  }

  private assistantText(key: string, messageId: string, value: string, at: number) {
    const live = this.live.get(key);
    if (live) { this.live.delete(key); this.retired.add(live.attemptId); }
    const id = live?.itemId ?? 'item-a-' + messageId;
    const existing = this.item(id);
    if (!value) { if (existing) this.items.splice(this.items.indexOf(existing), 1); return; }
    const msgId = 'msg-a-' + messageId;
    if (existing) {
      Object.assign(existing, { text: value, messageId: msgId });
      this.settleItem(existing, 'completed', at);
    } else if (!this.item('item-a-' + messageId)) {
      this.items.push({ ...this.base(id, 'assistant_message', 'completed', at), messageId: msgId, text: value, streaming: false });
    }
    const message = this.messages.find(entry => entry.id === msgId);
    if (message) Object.assign(message, { text: value, streaming: false, updatedAt: at });
    else this.messages.push({ id: msgId, runId: this.currentRun()?.id ?? null, role: 'assistant', text: value, streaming: false, createdAt: at, updatedAt: at });
  }

  private toolCall(data: Record<string, any>, at: number) {
    const rawId = text(data.callId); if (!rawId) return;
    const callId = String(data.turn ?? '') + ':' + rawId;
    if (this.calls.has(callId)) return;
    const name = text(data.name); const args = argumentsOf(data.arguments);
    const id = 'item-tool-' + callId.replace(/[^A-Za-z0-9_.:-]/g, '_');
    const path = text(args.file_path ?? args.path ?? args.filePath ?? args.notebook_path);
    const kind = toolKind(name);
    let item: Item;
    if (kind === 'command') item = { ...this.base(id, 'command_execution', 'running', at), input: text(args.command ?? args.cmd) || name };
    else if (kind === 'write' && path.trim()) {
      const newStr = text(args.new_string ?? args.new_str ?? args.content ?? args.file_text);
      const oldStr = text(args.old_string ?? args.old_str);
      item = { ...this.base(id, 'file_change', 'running', at), fileName: path.trim(), ...(newStr ? { newStr } : {}), ...(oldStr ? { oldStr } : {}) };
    } else if (kind === 'search') {
      const pattern = text(args.pattern ?? args.query ?? args.path);
      item = { ...this.base(id, 'file_search', 'running', at), ...(pattern ? { pattern } : {}) };
    } else if (kind === 'web') {
      const pattern = text(args.query ?? args.url);
      item = { ...this.base(id, 'web_search', 'running', at), ...(pattern ? { patterns: [pattern] } : {}) };
    } else item = { ...this.base(id, 'dynamic_tool', 'running', at), toolName: name.trim() || null, input: args };
    this.calls.set(callId, id);
    this.toolInputs.set(rawId, { name, args, itemId: id });
    this.items.push(item);
  }

  private toolResult(message: Record<string, any>, turn: unknown, at: number) {
    const id = this.calls.get(String(turn ?? '') + ':' + text(message.toolCallId));
    const item = id ? this.item(id) : undefined;
    if (!item) return;
    const output = contentText(message.content) || text(message.content);
    const failed = message.isError === true;
    if (item.type === 'command_execution') {
      const match = Array.from(output.matchAll(/\[(?:Command finished with exit code|shell exited: code)\s+(-?\d+)\]/gi)).at(-1);
      const exitCode = match ? Number(match[1]) : undefined;
      Object.assign(item, { output: output.slice(0, 20_000), ...(exitCode === undefined ? {} : { exitCode }), ...(failed || (exitCode ?? 0) !== 0 ? { outputIndicatesFailure: true } : {}) });
    } else if (item.type === 'dynamic_tool') item.output = output.slice(0, 20_000);
    this.settleItem(item, failed ? 'failed' : 'completed', at);
  }

  private endTurn(reason: string, at: number, error?: unknown) {
    this.reopened = null;
    const run = this.currentRun();
    const status: RunStatus = reason === 'aborted' || (this.stopRequested && reason !== 'error') ? 'interrupted' : reason === 'error' ? 'failed' : 'completed';
    this.stopRequested = false;
    if (run && (run.status === 'running' || run.status === 'waiting')) { run.status = status; run.completedAt = at; }
    for (const item of this.items) {
      if (item.status === 'running' || item.status === 'waiting' || item.status === 'pending')
        this.settleItem(item, item.type === 'assistant_message' ? 'completed' : status === 'completed' ? 'completed' : 'interrupted', at);
    }
    for (const live of this.live.values()) this.retired.add(live.attemptId);
    this.live.clear();
    for (const request of this.requests) if (request.status === 'pending') { request.status = 'cancelled'; request.resolvedAt = at; }
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
        // A turn without a message of its own continues the last run; a message, when it follows, starts its own.
        const run = this.currentRun();
        if (run && run.status !== 'running' && run.status !== 'waiting') {
          this.reopened = { run, status: run.status, completedAt: run.completedAt };
          run.status = 'running'; run.completedAt = null;
        }
        this.running = true; this.settledByGuess = false;
        break;
      }
      case 'turn/end': this.endTurn(text(record(data.reason).kind), at, record(data.reason).error); this.engineRunning = false; this.settledByGuess = false; break;
      case 'assistant/message': {
        const message = record(data.message);
        if (!text(message.id)) return false;
        this.assistantText(String(data.turn ?? '') + ':' + String(data.step ?? ''), text(message.id), contentText(message.content), at);
        break;
      }
      case 'tool/call': this.toolCall(data, at); break;
      // The permission request of a tool call: the card shows that call as T3 shows a Claude request.
      case 'approval/asked': {
        const item = this.item('item-approval-' + text(data.id));
        const tool = this.toolInputs.get(text(data.callId));
        if (!item || !tool) return false;
        const request = toolRequest(tool.name, tool.args);
        item.requestKind = request.requestKind; delete item.prompt;
        if (request.prompt) item.prompt = request.prompt;
        const entry = this.requests.find(candidate => candidate.id === text(data.id));
        if (entry) entry.kind = request.requestKind;
        break;
      }
      case 'tool/result': this.toolResult(record(data.message), data.turn, at); break;
      default: return false;
    }
    this.touch(at);
    return true;
  }

  /** Live text of the answer (`session.assistant-stream`), replaced by the durable `assistant/message`. */
  applyStreamFrame(frame: Record<string, any>, at = Date.now()): boolean {
    const attempt = text(frame.attemptId);
    if (!attempt || this.retired.has(attempt)) return false;
    if (frame.type === 'start') {
      const key = String(frame.turn ?? '') + ':' + String(frame.step ?? '');
      if ([...this.live.values()].some(live => live.attemptId === attempt)) return false;
      const previous = this.live.get(key);
      if (previous) this.retired.add(previous.attemptId);
      const itemId = previous?.itemId ?? 'item-live-' + attempt;
      this.live.set(key, { itemId, attemptId: attempt, raw: '', last: -1, gapped: false });
      const existing = this.item(itemId);
      if (existing) Object.assign(existing, { text: '', streaming: true });
      return !!existing;
    }
    const found = [...this.live.entries()].find(([, live]) => live.attemptId === attempt);
    if (!found) return false;
    const [, live] = found;
    if (frame.type === 'gap') { live.gapped = true; return false; }
    if (frame.type === 'end') {
      this.retired.add(attempt);
      const item = this.item(live.itemId);
      if (item && !live.gapped) { item.streaming = false; item.updatedAt = iso(at); return true; }
      return false;
    }
    const chunk = record(frame.chunk);
    if (live.gapped || frame.type !== 'chunk' || chunk.type !== 'text-delta' || typeof chunk.text !== 'string') return false;
    if (!Number.isSafeInteger(frame.index) || frame.index <= live.last) return false;
    live.last = frame.index; live.raw += chunk.text;
    const item = this.item(live.itemId);
    if (item) Object.assign(item, { text: live.raw, updatedAt: iso(at) });
    else this.items.push({ ...this.base(live.itemId, 'assistant_message', 'running', at), messageId: 'msg-live-' + attempt, text: live.raw, streaming: true });
    this.touch(at);
    return true;
  }

  setRunning(running: boolean, at = Date.now()): boolean {
    this.engineRunning = running;
    const run = this.currentRun();
    if (running && this.settledByGuess && run && run.status === 'interrupted') {
      // The history ended mid-turn because the turn is still going: it was not stopped.
      run.status = 'running'; run.completedAt = null; this.settledByGuess = false; this.running = true;
      return true;
    }
    // The engine says idle: no run stays open, whatever this flag held (the sidebar lists a thread with an open run as working).
    const open = !running && !!run && (run.status === 'running' || run.status === 'waiting') && this.outgoing.length === 0;
    if (this.running === running && !open) return false;
    this.running = running;
    if (open) this.endTurn('completed', at);
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
   * A permission request of the assistant (`approval.pending`), answered in the chat. The engine's own text
   * (`humanText`) is not English; the request is the tool call waiting for it, which `approval/asked` names.
   */
  approvalPending(params: { approvalId: string; humanText?: string; risk?: string }, at = Date.now()) {
    if (!text(params.approvalId) || this.requests.some(request => request.id === params.approvalId)) return false;
    const run = this.currentRun();
    // Until `approval/asked` names it: the latest call still running is the one that waits.
    const waiting = [...this.toolInputs.values()].reverse().find(tool => this.item(tool.itemId)?.status === 'running');
    const request = waiting ? toolRequest(waiting.name, waiting.args) : { requestKind: 'command' };
    this.requests.push({ id: params.approvalId, kind: request.requestKind, status: 'pending', createdAt: at, resolvedAt: null });
    this.items.push({
      ...this.base('item-approval-' + params.approvalId, 'approval_request', 'waiting', at),
      requestId: params.approvalId, requestKind: request.requestKind,
      ...(request.prompt ? { prompt: request.prompt } : {}),
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

  private modelSelection() { return { instanceId: CLAUDE_INSTANCE_ID, model: this.model, ...(this.options?.length ? { options: this.options } : {}) }; }
  private titleText() { return nonBlank(this.title, 'New thread'); }

  /** `OrchestrationV2ThreadProjection`, encoded. */
  projection() {
    const updatedAt = iso(this.updatedAt);
    return {
      thread: {
        createdBy: 'user', creationSource: 'web',
        id: this.id, projectId: this.projectId, title: this.titleText(), providerInstanceId: CLAUDE_INSTANCE_ID,
        modelSelection: this.modelSelection(), runtimeMode: runtimeModeOf(this.mode), interactionMode: 'default',
        branch: null, worktreePath: null, activeProviderThreadId: null,
        lineage: { parentThreadId: null, relationshipToParent: null, rootThreadId: this.id },
        forkedFrom: null, createdAt: iso(this.createdAt), updatedAt,
        archivedAt: this.archivedAt === null ? null : iso(this.archivedAt), settledOverride: null, settledAt: null, lastVisitedAt: null, deletedAt: null,
      },
      runs: this.runs.map(run => ({
        id: run.id, threadId: this.id, ordinal: run.ordinal, providerInstanceId: CLAUDE_INSTANCE_ID,
        modelSelection: { instanceId: CLAUDE_INSTANCE_ID, model: run.model }, providerThreadId: null, userMessageId: run.userMessageId,
        rootNodeId: null, activeAttemptId: null, status: run.status, requestedAt: iso(run.requestedAt),
        startedAt: run.startedAt === null ? null : iso(run.startedAt), completedAt: run.completedAt === null ? null : iso(run.completedAt),
        checkpointId: null, contextHandoffId: null,
      })),
      attempts: [], nodes: [], subagents: [], providerSessions: [], providerThreads: [], providerTurns: [],
      runtimeRequests: this.requests.map(request => ({
        id: request.id, nodeId: NODE_ID, providerTurnId: null, nativeRequestRef: null, kind: request.kind, status: request.status,
        responseCapability: { type: 'live', providerSessionId: 'hoh-' + this.id },
        createdAt: iso(request.createdAt), resolvedAt: request.resolvedAt === null ? null : iso(request.resolvedAt),
        ...(request.decision ? { decision: request.decision } : {}), ...(request.answers ? { answers: request.answers } : {}),
      })),
      messages: this.messages.map(message => ({
        createdBy: message.role === 'user' ? 'user' : 'agent', creationSource: message.role === 'user' ? 'web' : 'provider',
        id: message.id, threadId: this.id, runId: message.runId, nodeId: null, role: message.role, text: message.text,
        attachments: [], streaming: message.streaming, createdAt: iso(message.createdAt), updatedAt: iso(message.updatedAt),
      })),
      plans: [],
      turnItems: this.items.map(item => ({ ...item })),
      checkpointScopes: [], checkpoints: [], contextHandoffs: [], contextTransfers: [],
      visibleTurnItems: this.items.map((item, position) => ({ position, visibility: 'local', sourceThreadId: this.id, sourceItemId: item.id, item: { ...item } })),
      updatedAt,
    };
  }

  /**
   * What the home screen shows of this chat (HOH-41 В2), from what the host already knows: the state of the last run,
   * its real steps (tool calls), and the request that waits for the person. Nothing is estimated.
   */
  home(): HomeEntry | null {
    const run = this.currentRun();
    if (!run) return null;
    const steps = this.items.filter(item => item.runId === run.id && STEP_TYPES.has(item.type));
    const pending = this.requests.find(request => request.status === 'pending');
    const waitingItem = pending && this.items.find(item => item.requestId === pending.id);
    const open = run.status === 'running' || run.status === 'waiting';
    const question = waitingItem?.type === 'user_input_request' ? text(waitingItem.questions?.[0]?.question) : '';
    return {
      id: this.id, title: this.titleText(), projectId: this.projectId,
      state: open ? (pending ? 'waiting' : 'working') : run.status === 'completed' ? 'done' : 'ended',
      startedAt: run.startedAt ?? run.requestedAt, doneAt: run.completedAt,
      steps: { done: steps.filter(item => item.status !== 'running' && item.status !== 'waiting').length, total: steps.length },
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
      id: this.id, projectId: this.projectId, title: this.titleText(), providerInstanceId: CLAUDE_INSTANCE_ID,
      modelSelection: this.modelSelection(), runtimeMode: runtimeModeOf(this.mode), interactionMode: 'default',
      branch: null, worktreePath: null,
      lineage: { parentThreadId: null, relationshipToParent: null, rootThreadId: this.id },
      forkedFrom: null, activeProviderThreadId: null,
      latestRunId: run?.id ?? null, activeRunId: active,
      status: run ? (active ? (run.status === 'waiting' ? 'waiting' : 'running') : run.status) : 'idle',
      pendingRuntimeRequest: pending ? { id: pending.id, kind: pending.kind, createdAt: iso(pending.createdAt), blocking: true } : null,
      latestVisibleMessage: visible ? { id: visible.id, role: visible.role, text: visible.text.slice(0, 400), updatedAt: iso(visible.updatedAt) } : null,
      latestUserMessageAt: lastUser ? iso(lastUser.createdAt) : null,
      hasActionableProposedPlan: false,
      itemCount: this.items.length, visibleItemCount: this.items.length,
      createdAt: iso(this.createdAt), updatedAt: iso(this.updatedAt),
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
