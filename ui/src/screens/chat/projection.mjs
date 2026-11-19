// Framework-independent projection of the existing V4 session events. No I/O.
const record = value => value && typeof value === 'object' && !Array.isArray(value) ? value : {};
const text = value => typeof value === 'string' ? value : '';
export function contentText(blocks) {
  return Array.isArray(blocks) ? blocks.filter(b => b?.type === 'text').map(b => text(b.text)).join('') : '';
}
function normalPath(value) {
  const source = text(value).replaceAll('\\', '/');
  const parts = [];
  for (const piece of source.split('/')) {
    if (!piece || piece === '.') continue;
    if (piece === '..') { if (!parts.length) return null; parts.pop(); }
    else parts.push(piece);
  }
  return (source.startsWith('//') ? '//' : source.startsWith('/') ? '/' : '') + parts.join('/');
}
export function humanFile(value, folder) {
  const path = normalPath(value);
  const root = normalPath(folder);
  if (!path || /[\x00-\x1f\x7f{}<>"]/.test(path)) return 'файл';
  const absolute = path.startsWith('/') || /^[A-Za-z]:\//.test(path);
  if (absolute && (!root || !path.startsWith(root.replace(/\/$/, '') + '/'))) return 'файл';
  return path.split('/').at(-1) || 'файл';
}
function fileAddress(value) {
  if (!/^file:\/{2}/i.test(value)) return value;
  try {
    const url = new URL(value);
    const path = decodeURIComponent(url.pathname);
    if (url.host && url.host !== 'localhost') return '//' + url.host + path;
    return /^\/[A-Za-z]:\//.test(path) ? path.slice(1) : path;
  } catch { return ''; }
}
function redactPaths(value, folder) {
  const file = path => humanFile(fileAddress(path), folder);
  let result = value;
  for (const [open, close] of [['"', '"'], ["'", "'"], ['`', '`'], ['«', '»'], ['(', ')'], ['[', ']'], ['<', '>']]) {
    const escaped = char => char.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const pattern = new RegExp(escaped(open) + '((?:file:\\/{2}|[A-Za-z]:[\\\\/]|\\\\\\\\|\\/|\\.\\.[\\\\/])[^\\r\\n' + escaped(close) + ']+)' + escaped(close), 'gi');
    result = result.replace(pattern, (_, path) => open + file(path) + close);
  }
  // No prefix test: a path is private even beside punctuation or a letter.
  // Consume ordinary web URLs whole so their path portions stay URL text.
  // A data URI's MIME header is not a path; its payload is still filtered.
  return result.replace(
    /data:[-\w!#$%&'*+.^`|~]+\/[-\w!#$%&'*+.^`|~]+(?=[;,])|[A-Za-z][A-Za-z\d+.-]*:\/{2}[^\s<>"'`()\[\]{};,]*|(?:[A-Za-z]:[\\/]|\\\\|\/|\.\.[\\/])[^\s<>"'`()\[\]{};,]*/gimu,
    address => /^data:/i.test(address) || /^[A-Za-z][A-Za-z\d+.-]*:\/{2}/.test(address) && !/^file:\/{2}/i.test(address) ? address : file(address),
  );
}
export function visibleText(value, folder) {
  const source = text(value).replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '');
  // A paired closing HTML tag is markup text, not an absolute path. Protect
  // only that complete tag, never its surrounding text or attribute values.
  // In particular, </etc/passwd> is not a closing tag and is always redacted.
  const opened = new Map(); const pieces = []; let cursor = 0;
  for (const match of source.matchAll(/<(\/?)([A-Za-z][\w:-]*)(?=[\s/>])/g)) {
    const name = match[2].toLowerCase();
    if (!match[1]) { opened.set(name, (opened.get(name) ?? 0) + 1); continue; }
    const closing = source.slice(match.index + match[0].length).match(/^\s*>/);
    if (!closing || !opened.get(name)) continue;
    const end = match.index + match[0].length + closing[0].length;
    pieces.push(redactPaths(source.slice(cursor, match.index), folder), source.slice(match.index, end));
    cursor = end; opened.set(name, opened.get(name) - 1);
  }
  pieces.push(redactPaths(source.slice(cursor), folder));
  return pieces.join('');
}
// Only host-generated surfaces use this filter. Assistant prose (including
// code and terminology) deliberately passes through visibleText alone.
const technical = /(?:инструмент|токен|сесси|агент)\p{L}*|\b(?:tools?|tokens?|sessions?|agents?|bash|shell|exec|read|write|read_file|file_read|read_image|write_file|file_write|str_replace_editor|edit_file|run_command|run_code|json|rpc|stdout|stderr)\b/iu;
export function windowText(value, folder, fallback = 'Содержимое результата не показывается.', toolName = '') {
  const safe = visibleText(value, folder);
  // JSON embedded in a sentence, fenced output, arrays and broken envelopes
  // are as technical as a complete JSON response. Keep the human fallback.
  if (technical.test(safe) || (toolName && safe.toLowerCase().includes(toolName.toLowerCase())) ||
      /\{|\[\s*(?:\]|\[|"|-?\d|true\b|false\b|null\b)/.test(safe)) return fallback;
  return safe;
}
function argumentsOf(value) {
  try { return record(typeof value === 'string' ? JSON.parse(value) : value); }
  catch { return {}; }
}
function operationOf(name, args) {
  if (['read', 'read_file', 'file_read', 'read_image'].includes(name) || (name === 'str_replace_editor' && args.command === 'view')) return 'read';
  if (['write', 'write_file', 'file_write', 'str_replace_editor', 'edit_file'].includes(name)) return 'write';
  if (['bash', 'shell', 'run_command', 'exec', 'run_code'].includes(name)) return 'command';
  return 'action';
}
const verbs = {
  read: ['Читаю', 'Прочитал', 'Не удалось прочитать', 'Чтение остановлено'],
  write: ['Записываю', 'Записал', 'Не удалось записать', 'Запись остановлена'],
  command: ['Выполняю команду…', 'Команда выполнена без ошибок', 'Не удалось выполнить команду', 'Команда остановлена'],
  action: ['Работаю…', 'Готово', 'Не удалось завершить действие', 'Действие остановлено'],
};
export function numberInWords(number) {
  const small = ['ноль', 'один', 'два', 'три', 'четыре', 'пять', 'шесть', 'семь', 'восемь', 'девять', 'десять', 'одиннадцать', 'двенадцать', 'тринадцать', 'четырнадцать', 'пятнадцать', 'шестнадцать', 'семнадцать', 'восемнадцать', 'девятнадцать'];
  if (!Number.isInteger(number) || number < 0 || number > 255) return 'неизвестен';
  const words = [];
  if (number >= 100) { words.push(number >= 200 ? 'двести' : 'сто'); number %= 100; }
  if (number >= 20) { words.push(['', '', 'двадцать', 'тридцать', 'сорок', 'пятьдесят', 'шестьдесят', 'семьдесят', 'восемьдесят', 'девяносто'][Math.floor(number / 10)]); number %= 10; }
  if (number || !words.length) words.push(small[number]);
  return words.join(' ');
}
function labelFor(step, status, code) {
  if (step.operation === 'command' && status === 'done' && code === undefined) return 'Команда завершена';
  if (status === 'error' && step.operation === 'command' && code !== undefined && code !== 0)
    return 'Команда завершилась с ошибкой: код ' + numberInWords(code);
  const index = { running: 0, done: 1, error: 2, stopped: 3 }[status];
  return verbs[step.operation][index] + (['read', 'write'].includes(step.operation) && status !== 'stopped' ? ' ' + step.file : '');
}
export function assistantChunks(event) {
  const result = [];
  const trace = event.data?.stream ?? event.stream;
  for (const entry of Array.isArray(trace) ? trace : []) {
    if (entry.type === 'text-chunks' && Array.isArray(entry.texts)) result.push(...entry.texts.filter(v => typeof v === 'string'));
    if (entry.type === 'chunk' && entry.chunk?.type === 'text-delta' && typeof entry.chunk.text === 'string') result.push(entry.chunk.text);
  }
  const finalText = contentText(event.data?.message?.content);
  // Invalid/incomplete trace cannot replace the authoritative final message.
  return result.length && result.join('') === finalText ? result : finalText ? [finalText] : [];
}
export class ChatProjection {
  constructor(folder = '') {
    this.folder = folder;
    this.entries = [];
    this.running = false;
    this.seen = new Set();
    this.users = new Map();
    this.assistants = new Map();
    this.calls = new Map();
    this.arguments = new Map();
    this.counter = 0;
    this.live = new Map();
    this.retired = new Set();
  }
  replace(index, entry) {
    const entries = this.entries.slice(); entries[index] = entry; this.entries = entries;
  }
  append(entry) { const index = this.entries.length; this.entries = [...this.entries, entry]; return index; }
  optimistic(value) {
    const id = 'local-' + ++this.counter;
    this.append({ id, kind: 'user', text: visibleText(value, this.folder), pending: true, at: Date.now() });
    return id;
  }
  receipt(localId, messageId) {
    const index = this.entries.findIndex(e => e.id === localId);
    if (index >= 0 && typeof messageId === 'string') {
      this.users.set(messageId, index);
      this.replace(index, { ...this.entries[index], pending: false });
    }
  }
  failed(localId) {
    const index = this.entries.findIndex(entry => entry.id === localId);
    if (index >= 0) this.replace(index, { ...this.entries[index], pending: false, failed: true });
  }
  accept(event) {
    if (!event || !Number.isSafeInteger(event.seq) || event.seq < 0 || this.seen.has(event.seq)) return false;
    this.seen.add(event.seq); return true;
  }
  event(event) { if (this.accept(event)) this.apply(event); }
  assistant(event, value, streaming = false) {
    const message = record(event.data?.message);
    if (!text(message.id) || !value) return;
    const id = 'assistant-' + message.id;
    let index = this.assistants.get(id);
    const entry = { id, kind: 'assistant', text: visibleText(value, this.folder), streaming };
    if (index === undefined) { index = this.append(entry); this.assistants.set(id, index); }
    else this.replace(index, entry);
  }
  /**
   * Live text of the answer, as the engine publishes it while the model is still writing
   * (`session.assistant-stream`: start | chunk{text-delta} | end). It is process-local: the durable
   * `assistant/message` that follows replaces it in place. Whatever arrived before a stop stays.
   */
  liveFrame(frame) {
    const frameRecord = record(frame); const type = frameRecord.type;
    const key = String(frameRecord.turn ?? '') + ':' + String(frameRecord.step ?? '');
    const attempt = frameRecord.attemptId; const revision = frameRecord.revision;
    // Contract (crates/hoh-agent/src/runtime_types.rs, AssistantStreamFrame): revision is monotone
    // only WITHIN an attempt and need not grow on every frame; a replacement attempt may restart it
    // at 1. So a new attemptId is accepted at any revision, frames of one attempt are ordered by
    // `index` (and never go back in revision), and an attempt that was replaced or settled is
    // remembered and all its frames are dropped.
    if (typeof attempt !== 'string' || !Number.isSafeInteger(revision) || this.retired.has(attempt)) return;
    if (type === 'start') {
      const known = [...this.live.values()].find(live => live.attemptId === attempt);
      if (known) return;
      const previous = this.live.get(key);
      // a retried attempt of the same step starts the text over; the replaced one never comes back
      if (previous) this.retired.add(previous.attemptId);
      this.live.set(key, { attemptId: attempt, raw: '', last: -1, index: previous?.index, revision });
      if (previous?.index !== undefined) this.replace(previous.index, { ...this.entries[previous.index], text: '', streaming: true });
      return;
    }
    const found = [...this.live.entries()].find(([, live]) => live.attemptId === attempt);
    if (!found) return;
    const [liveKey, live] = found;
    if (revision < live.revision) return;
    live.revision = revision;
    if (type === 'end') {
      this.retired.add(attempt);
      if (live.index !== undefined) this.replace(live.index, { ...this.entries[live.index], streaming: false });
      return;
    }
    const chunk = record(frameRecord.chunk);
    if (type !== 'chunk' || chunk.type !== 'text-delta' || typeof chunk.text !== 'string') return;
    if (!Number.isSafeInteger(frameRecord.index) || frameRecord.index <= live.last) return;
    live.last = frameRecord.index; live.raw += chunk.text;
    const entry = { id: 'live-' + live.attemptId, kind: 'assistant', text: visibleText(live.raw, this.folder), streaming: true };
    if (!entry.text) return;
    if (live.index === undefined) live.index = this.append(entry);
    else this.replace(live.index, entry);
    this.live.set(liveKey, live);
  }
  hasLive(event) {
    const live = this.live.get(String(event.data?.turn ?? '') + ':' + String(event.data?.step ?? ''));
    return live?.index !== undefined;
  }
  /** The durable answer takes the place of what was shown live. */
  settleLive(event, value) {
    const key = String(event.data?.turn ?? '') + ':' + String(event.data?.step ?? '');
    const live = this.live.get(key); const message = record(event.data?.message);
    if (live?.index === undefined || !text(message.id)) return false;
    this.live.delete(key); this.retired.add(live.attemptId);
    if (!value) return true;
    const id = 'assistant-' + message.id;
    this.replace(live.index, { id, kind: 'assistant', text: visibleText(value, this.folder), streaming: false });
    this.assistants.set(id, live.index);
    return true;
  }
  apply(event) {
    const data = record(event.data);
    // A prompt can be acknowledged before its turn starts. Persisted inbox
    // entries retain that user message even if the engine exits at receipt.
    if (event.type === 'agent/inbox/spliced') {
      for (const message of Array.isArray(data.inserted) ? data.inserted : [])
        this.apply({ type: 'user/message', data: message });
    }
    if (event.type === 'turn/start') this.running = true;
    if (event.type === 'turn/end') {
      if (data.reason?.kind === 'aborted') this.stop();
      else this.setRunning(false);
    }
    if (event.type === 'user/message' && data.source?.kind === 'user') {
      const id = text(data.id); const value = visibleText(contentText(data.content), this.folder);
      if (!id || !value || this.users.has(id)) return;
      const pending = this.entries.findIndex(e => e.kind === 'user' && e.pending && e.text === value);
      if (pending >= 0) { this.users.set(id, pending); this.replace(pending, { ...this.entries[pending], pending: false }); }
      else this.users.set(id, this.append({ id: 'user-' + id, kind: 'user', text: value }));
    }
    if (event.type === 'assistant/message') {
      const value = contentText(data.message?.content);
      if (!this.settleLive(event, value)) this.assistant(event, value);
    }
    if (event.type === 'tool/call') {
      const rawId = text(data.callId); if (!rawId) return;
      const callId = String(data.turn ?? '') + ':' + rawId; if (this.calls.has(callId)) return;
      const args = argumentsOf(data.arguments); const operation = operationOf(data.name, args);
      const step = { id: 'step-' + callId, kind: 'step', operation, status: 'running',
        file: windowText(humanFile(args.file_path ?? args.path ?? args.filePath, this.folder), this.folder, 'файл', text(data.name)), detail: '' };
      step.label = labelFor(step, 'running');
      this.calls.set(callId, this.append(step)); this.arguments.set(callId, { name: text(data.name), preview: text(args.file_text ?? args.content ?? args.new_str).slice(0, 1200) });
    }
    if (event.type === 'tool/result') this.result(record(data.message), data.turn);
  }
  result(message, turn) {
    const callId = String(turn ?? '') + ':' + text(message.toolCallId); const index = this.calls.get(callId);
    if (index === undefined) return;
    const before = this.entries[index]; const args = this.arguments.get(callId) ?? {};
    let output = contentText(message.content);
    const parsed = argumentsOf(output);
    const structuredCode = parsed.exitCode ?? parsed.exit_code ?? parsed.code;
    const match = Array.from(output.matchAll(/\[(?:Command finished with exit code|shell exited: code)\s+(-?\d+)\]/gi)).at(-1);
    const exitCode = typeof structuredCode === 'number' ? structuredCode : match ? Number(match[1]) : undefined;
    if (typeof parsed.stdout === 'string') output = parsed.stdout;
    else if (parsed.stdout && typeof parsed.stdout.text === 'string') output = parsed.stdout.text;
    else if (Object.keys(parsed).length) output = ''; // Never render a JSON envelope as step details.
    output = output.split(/\n?\[(?:Command finished|shell exited)/i)[0];
    let detail = output;
    if (before.operation === 'read' && /with line numbers/.test(output))
      detail = output.split('\n').filter(line => /^\s*\d+\s/.test(line)).map(line => line.replace(/^\s*\d+ {2}/, '')).join('\n');
    if (before.operation === 'write') detail = text(args.preview);
    const status = message.isError === true || (before.operation === 'command' && exitCode !== undefined && exitCode !== 0) ? 'error' : 'done';
    if (status === 'error' && before.operation !== 'command') detail = 'Действие не удалось завершить. Попробуйте ещё раз.';
    detail = windowText(detail, this.folder, undefined, args.name).slice(0, 1200);
    this.replace(index, { ...before, status, exitCode, detail, label: labelFor(before, status, exitCode) });
  }
  setRunning(running) {
    this.running = running;
    if (!running) this.entries = this.entries.map(entry => entry.kind === 'step' && entry.status === 'running'
      ? { ...entry, status: 'error', label: labelFor(entry, 'error'), detail: 'Действие не удалось завершить.' }
      : entry.streaming ? { ...entry, streaming: false } : entry);
  }
  stop() {
    this.running = false;
    this.entries = this.entries.map(entry => entry.kind === 'step' && entry.status === 'running'
      ? { ...entry, status: 'stopped', label: labelFor(entry, 'stopped') }
      : entry.streaming ? { ...entry, streaming: false } : entry);
  }
}
