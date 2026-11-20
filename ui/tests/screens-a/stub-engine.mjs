// Запускает hoh-window-stub (дочерний процесс, JSON-RPC строками по stdio) и даёт request(method, params).
// Бинарник собирает ведущий: HOH_WINDOW_STUB=/путь/к/hoh-window-stub. Без него проверки идут на встроенной подмене.
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

// `env` — переменные заглушки (HOH_STUB_CLAUDE_FILE: файл, по которому discover отвечает в одном из четырёх состояний);
// `capabilities` — что окно говорит в initialize (userQuestions: заглушка присылает вопрос подтверждения Claude).
export async function startStub(binary, { env = {}, capabilities = {} } = {}) {
  const child = spawn(binary, ['--scenario', 'text-turn', '--approvals', 'local'], { stdio: ['pipe', 'pipe', 'inherit'], env: { ...process.env, ...env } });
  const asked = [];
  const waiting = new Map();
  let next = 1;
  createInterface({ input: child.stdout }).on('line', line => {
    let frame;
    try { frame = JSON.parse(line); } catch { return; }
    if (frame.method === 'user-question/ask') {
      // Вопрос подтверждения: запоминаем и отвечаем «Разрешить», чтобы ход закончился.
      asked.push(frame.params);
      const answers = frame.params.questions.map(q => ({ id: q.id, selected: [q.options[0].label] }));
      child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: frame.id, result: { answers } }) + '\n');
      return;
    }
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
  await request('initialize', { cwd: '/tmp', provider: 'p', model: 'm', capabilities });
  return { request, asked, stop: () => child.kill() };
}
