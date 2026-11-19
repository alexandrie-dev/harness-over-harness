// Запускает hoh-window-stub (дочерний процесс, JSON-RPC строками по stdio) и даёт request(method, params).
// Бинарник собирает ведущий: HOH_WINDOW_STUB=/путь/к/hoh-window-stub. Без него проверки идут на встроенной подмене.
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

export async function startStub(binary) {
  const child = spawn(binary, ['--scenario', 'text-turn'], { stdio: ['pipe', 'pipe', 'inherit'] });
  const waiting = new Map();
  let next = 1;
  createInterface({ input: child.stdout }).on('line', line => {
    let frame;
    try { frame = JSON.parse(line); } catch { return; }
    if (frame.id !== undefined && waiting.has(frame.id)) {
      const { resolve, reject } = waiting.get(frame.id);
      waiting.delete(frame.id);
      frame.error ? reject(new Error(frame.error.message)) : resolve(frame.result);
    }
  });
  const request = (method, params) => new Promise((resolve, reject) => {
    const id = next++;
    waiting.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
  await request('initialize', { cwd: '/tmp', provider: 'p', model: 'm', capabilities: {} });
  return { request, stop: () => child.kill() };
}
