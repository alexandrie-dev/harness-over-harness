// Only Node built-ins. Executes the authored Rust replay stub, never a model or
// the programs mentioned in its read-only recordings.
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { readFile, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export const SOURCE = '/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/snapshots/sdk';
export const STUB = process.env.HOH_WINDOW_STUB ?? '/mnt/toshiba/alexandrie/hoh/target-accept-b/debug/hoh-window-stub';
export const WORKSPACE = '/workspace/chat';
export async function recording(name) {
  return (await readFile(join(SOURCE, name, 'session.v4.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line));
}
export async function recordedPrompt(name) {
  const rows = await recording(name);
  const row = rows.find(e => e.type === 'agent/inbox/spliced' && e.data.inserted?.some(m => m.source?.kind === 'user'));
  return row.data.inserted.find(m => m.source?.kind === 'user').content.map(b => b.text ?? '').join('').replaceAll('{{cwd}}', WORKSPACE);
}
export class StubClient {
  static async open(scenario = 'text-turn', extra = [], options = {}) {
    const home = await mkdtemp(join(tmpdir(), 'hoh-chat-stub-'));
    const child = spawn(STUB, ['--scenario', scenario, ...extra], {
      env: { PATH: '/usr/bin:/bin', HOME: home, LANG: 'C.UTF-8', LC_ALL: 'C.UTF-8' },
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    const client = new StubClient(child, home, options);
    try {
      await client.request('initialize', { cwd: WORKSPACE, provider: 'deepseek-official', model: 'deepseek-v4-flash', reasoningEffort: 'high', maxTokens: 4096, approvalRequests: false, capabilities: { userQuestions: false } });
      return client;
    } catch (error) { await client.close(); throw error; }
  }
  constructor(child, home, options) {
    this.child = child; this.home = home; this.pending = new Map(); this.listeners = new Set();
    this.requests = []; this.notifications = []; this.nextId = 0; this.stderr = '';
    this.closed = false;
    child.stderr.on('data', data => { this.stderr += data.toString(); });
    const fail = error => {
      for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(error); }
      this.pending.clear();
    };
    child.on('error', fail);
    child.on('exit', (code, signal) => { this.closed = true; fail(new Error(`Replay stub exited (${code ?? signal}): ${this.stderr}`)); });
    this.lines = createInterface({ input: child.stdout });
    this.lines.on('line', line => {
      try {
        const frame = JSON.parse(line);
        if (this.cut) return;
        if (frame.method && frame.id !== undefined) {
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: frame.id, error: { code: -32601, message: 'No reverse requests in chat tests' } }) + '\n');
        } else if (frame.method) {
          this.notifications.push(frame);
          for (const listener of this.listeners) listener(frame);
        } else {
          const pending = this.pending.get(frame.id); if (!pending) return;
          this.pending.delete(frame.id); clearTimeout(pending.timer);
          if (frame.error) pending.reject(new Error(frame.error.message));
          else {
            pending.resolve(frame.result);
            if (options.crashAfterPromptReceipt && pending.method === 'session/prompt') {
              // Break at the wire receipt, before forwarding later buffered
              // frames. SIGKILL terminates our actual replay child, not a mock.
              this.cut = true; child.kill('SIGKILL');
            }
          }
        }
      } catch (error) { fail(error); }
    });
  }
  request(method, params) {
    const id = ++this.nextId; this.requests.push({ method, params });
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Replay request timed out: ${method}`)); }, 30000);
      this.pending.set(id, { resolve, reject, timer, method });
      this.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n', error => {
        if (error) { clearTimeout(timer); this.pending.delete(id); reject(error); }
      });
    });
  }
  subscribe(listener) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  async close() {
    if (!this.closed) {
      try { await this.request('shutdown', {}); } catch { /* process may already have exited */ }
      this.child.stdin.end();
      if (!this.closed) await Promise.race([
        new Promise(resolve => this.child.once('exit', resolve)),
        new Promise(resolve => setTimeout(() => { if (!this.closed) this.child.kill('SIGTERM'); resolve(); }, 2000).unref()),
      ]);
    }
    this.lines.close();
    await rm(this.home, { recursive: true, force: true });
  }
}

// Models the native host lifecycle around real replay processes. The stub has
// no disk writer: a checkpoint fixture supplies persisted history to the new
// process, which serves it through the real session/events RPC. No UI history
// or answer is injected by the host adapter.
export class RestartingStubHost {
  static async open() {
    const host = new RestartingStubHost();
    host.directory = await mkdtemp(join(tmpdir(), 'hoh-chat-restart-'));
    host.base = await recording('text-turn');
    host.history = [
      { type: 'user/message', data: { id: 'earlier-user', source: { kind: 'user' }, content: [{ type: 'text', text: 'Предыдущий вопрос' }] } },
      { type: 'assistant/message', data: { message: { id: 'earlier-answer', content: [{ type: 'text', text: 'Предыдущий ответ' }] } } },
    ];
    const path = join(host.directory, 'initial.jsonl');
    await host.write(path, [host.base[0], ...host.history, ...host.base.slice(1)]);
    host.attach(await StubClient.open('text-turn', ['--recording', path], { crashAfterPromptReceipt: true }));
    host.client.child.once('exit', () => host.status('restarting'));
    return host;
  }
  constructor() { this.listeners = new Set(); this.statusListeners = new Set(); this.requests = []; }
  write(path, rows) { return writeFile(path, rows.map(row => JSON.stringify(row)).join('\n') + '\n'); }
  attach(client) {
    this.client = client;
    this.off = client.subscribe(frame => { for (const listener of this.listeners) listener(frame); });
  }
  request(method, params) { this.requests.push({ method, params }); return this.client.request(method, params); }
  subscribe(listener) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  subscribeStatus(listener) { this.statusListeners.add(listener); return () => this.statusListeners.delete(listener); }
  status(state) { for (const listener of this.statusListeners) listener({ state, message: 'private /etc/secret agent JSON' }); }
  async restart({ unfinished = false } = {}) {
    assertReceipt(this.client);
    const checkpoint = this.client.notifications.filter(frame => frame.method === 'session.event').flatMap(frame => {
      const event = frame.params.event;
      // Input actions must not remain replay cursors in a history-only prefix.
      if (event.type === 'agent/inbox/spliced') return (event.data.inserted ?? [])
        .filter(message => message.source?.kind === 'user').map(data => ({ type: 'user/message', data }));
      return event.type === 'command/run' ? [] : [event];
    });
    this.off(); await this.client.close();
    const next = JSON.parse(JSON.stringify(this.base.slice(1)).replaceAll('{{message:', '{{restart-message:'));
    const path = join(this.directory, 'restarted.jsonl');
    // Extra planted checkpoint covers a persisted turn without a closing event.
    const tail = unfinished ? [
      { type: 'turn/start', data: { turn: 1 } },
      { type: 'tool/call', data: { turn: 1, name: 'read_file', callId: 'interrupted', arguments: { path: 'note.txt' } } },
      { type: 'assistant/message', data: { message: { id: 'interrupted-answer', content: [{ type: 'text', text: 'Сохранённая часть ответа' }] } } },
    ] : [];
    await this.write(path, [this.base[0], ...this.history, ...checkpoint, ...tail, ...next]);
    this.attach(await StubClient.open('text-turn', ['--recording', path]));
    this.status('ready');
  }
  async close() { this.off(); await this.client.close(); await rm(this.directory, { recursive: true, force: true }); }
}
function assertReceipt(client) {
  if (!client.closed || !client.cut || client.child.signalCode !== 'SIGKILL') throw new Error('Expected real stub exit after prompt receipt');
}
