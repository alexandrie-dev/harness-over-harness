#!/usr/bin/python3
"""Local WebKitGTK/Tauri benchmark and startup smoke in an isolated home.

Stage built UI assets, point TAURI_CONFIG at the staged frontendDist and build
hoh-desktop with --features probe. Run this script under xvfb-run and a fresh
dbus-run-session; only the smoke mode uses a real HoH engine executable.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
LONG_CHAT = "perf-long"
HISTORY_TURNS = 2500
STREAM_CHUNKS = 360
CHUNK = "Строка **ответа** с `кодом`, формулой $x^2$ и деталями.\n\n"
MARKER = "HOH_PERF "

PROBE = """
localStorage.setItem('hoh:first-run:done', '1');
(async () => {
  const now = () => performance.now();
  const raf = () => new Promise(resolve => requestAnimationFrame(resolve));
  const wait = async (predicate, name) => {
    const end = now() + 20000;
    while (now() < end) {
      const value = predicate();
      if (value) return value;
      await raf();
    }
    throw new Error(name);
  };
  const query = selector => document.querySelector(selector);
  const row = id => query('[data-chat-id="' + id + '"]');
  const ready = () => query('[data-chat-screen][data-connection=ready]');
  const available = () => ready() && !query('[data-chat-input]').disabled;
  const openings = [];
  try {
    await wait(() => ready() && row('perf-long'), 'initial chat');
    for (let round = 0; round < 5; round++) {
      const start = now();
      row('perf-long').click();
      await wait(() => query('[data-entry-id=assistant-last]'), 'recent history');
      await raf();
      const visible = now() - start;
      await wait(available, 'action ready');
      openings.push({visible, ready: now() - start});
      row('window-main').click();
      await wait(() => available() && row('window-main').getAttribute('aria-current'), 'return');
    }
    const input = query('[data-chat-input]');
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(input, 'perf');
    input.dispatchEvent(new Event('input', {bubbles:true}));
    await wait(() => !query('[data-chat-send]').disabled, 'draft');
    const started = now();
    query('[data-chat-send]').click();
    await wait(() => query('[data-streaming=true]'), 'first stream frame');
    const first = now() - started;
    const gaps = [];
    let previous = await raf(), latest = previous;
    while (query('[data-chat-stop]') && now() - started < 10000) {
      latest = await raf();
      gaps.push(latest - previous);
      previous = latest;
    }
    await wait(() => query('[data-entry-id=assistant-perf-stream] [data-renderer=t3]'), 'final Markdown');
    gaps.sort((a,b) => a-b);
    const stream = {first, frames:gaps.length, p95:gaps[Math.floor(gaps.length*.95)], max:gaps.at(-1)};
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(input, 'stop');
    input.dispatchEvent(new Event('input', {bubbles:true}));
    await wait(() => !query('[data-chat-send]').disabled, 'second draft');
    query('[data-chat-send]').click();
    await wait(() => query('[data-streaming=true]'), 'stop stream');
    const beforeStop = now();
    query('[data-chat-stop]').click();
    await wait(() => query('[data-chat-send]') && !query('[data-streaming=true]'), 'stop confirmed');
    document.title = 'HOH_PERF ' + JSON.stringify({openings, stream, stop:now()-beforeStop});
  } catch (error) {
    document.title = 'HOH_PERF ' + JSON.stringify({error:String(error), detail:query('[role=alert]')?.textContent, openings});
  }
})();
"""


COMPARE = """
localStorage.setItem('hoh:first-run:done', '1');
(async () => {
  const q = selector => document.querySelector(selector);
  const frame = () => new Promise(requestAnimationFrame);
  const wait = async (predicate, label) => {
    const until = performance.now() + 16000;
    do {
      const result = predicate();
      if (result) return result;
      await frame();
    } while (performance.now() < until);
    throw new Error(label);
  };
  const stats = values => {
    const sorted = values.slice().sort((a, b) => a - b);
    const n = sorted.length;
    return {
      n,
      p50: Math.round((sorted[Math.floor((n - 1) / 2)] + sorted[Math.ceil((n - 1) / 2)]) / 2),
      p95: Math.round(sorted[Math.ceil(n * .95) - 1]),
      max: Math.round(sorted.at(-1)),
    };
  };
  const legacy = location.pathname.endsWith('legacy.html');
  const quick = globalThis.hohPerfQuick === true;
  const row = id => legacy ? q('[data-chat-id="' + id + '"]') :
    [...document.querySelectorAll('[data-sidebar-row]')].find(el => el.getAttribute('data-sidebar-row').endsWith(id));
  const atLong = () => legacy ? q('[data-entry-id=assistant-last]') :
    [...(quick ? q('.hoh-quick-view') ?? document.createElement('div') : document).querySelectorAll('[data-message-role=assistant]')]
      .some(el => el.textContent.includes('Ответ 2500.'));
  const atShort = () => legacy ? row('window-main')?.getAttribute('aria-current') === 'true' : location.pathname.endsWith('/window-main');
  const ready = () => q('[data-chat-screen][data-connection=ready]');
  const card = () => q('.hoh-card[data-thread-id="perf-long"] button.hoh-card__open');
  const samples = [];
  let readyMs;
  try {
    await wait(() => quick ? card() : row('perf-long') && row('window-main'), 'chat entries');
    await wait(() => legacy ? ready() : q('[data-chat-composer-form]'), 'composer');
    const boot = Math.round(performance.now());
    for (let index = 0; index < 25; index++) {
      const started = performance.now();
      (quick ? card() : row('perf-long')).click();
      await wait(atLong, 'last reply');
      await frame();
      samples.push(Math.round(performance.now() - started));
      if (index === 0 && legacy) {
        await wait(ready, 'full history');
        readyMs = Math.round(performance.now() - started);
      }
      if (quick) {
        q('.hoh-quick-view button[aria-label="Close"]').click();
        await wait(() => !q('.hoh-quick-view__chat'), 'quick view closed');
      } else {
        row('window-main').click();
        await wait(() => atShort() && !atLong(), 'short chat selected');
      }
      await frame();
    }
    document.title = 'HOH_PERF ' + JSON.stringify({
      legacy, quick, boot, cold: samples[0], readyMs, warm: stats(samples.slice(1)), samples,
    });
  } catch (error) {
    document.title = 'HOH_PERF ' + JSON.stringify({
      error: String(error), legacy, quick, path: location.pathname, samples, text: document.body.innerText.slice(0, 320),
    });
  }
})();
"""

SMOKE = """
localStorage.setItem('hoh:first-run:done', '1');
let pass = false;
const timer = setInterval(() => {
  if (!document.querySelector('[data-chat-screen][data-connection=ready]')) return;
  pass = true;
  clearInterval(timer);
  document.title = 'HOH_PERF {"ready":true}';
}, 25);
setTimeout(() => {
  if (pass) return;
  clearInterval(timer);
  document.title = 'HOH_PERF ' + JSON.stringify({
    error: 'native startup', detail: document.querySelector('[role=alert]')?.textContent,
  });
}, 20000);
"""


def send(message):
    print(json.dumps({"jsonrpc": "2.0", **message}, ensure_ascii=False), flush=True)


def engine():
    state = {"window-main": [], LONG_CHAT: []}
    now_ms = int(time.time() * 1000)

    def block(text):
        return [{"type": "text", "text": text}]

    def event(chat, event_type, data):
        history = state[chat]
        row = {"type": event_type, "seq": len(history), "time": now_ms, "data": data}
        history.append(row)
        return row

    for i in range(HISTORY_TURNS):
        turn = i + 1
        event(LONG_CHAT, "user/message", {"id": f"user-{i}", "source": {"kind": "user"}, "content": block(f"Вопрос {turn}")})
        event(LONG_CHAT, "turn/start", {"turn": turn})
        event(LONG_CHAT, "assistant/message", {"turn": turn, "step": 1, "message": {
            "id": "last" if turn == HISTORY_TURNS else f"msg-{turn}", "content": block(f"Ответ {turn}. " + CHUNK * 3)}})
        event(LONG_CHAT, "turn/end", {"turn": turn, "reason": {"kind": "completed"}})

    event("window-main", "user/message", {"id": "short-user", "source": {"kind": "user"}, "content": block("Короткий чат")})
    event("window-main", "assistant/message", {"turn": 1, "step": 1, "message": {"id": "short-reply", "content": block("Короткий ответ")}})
    event("window-main", "turn/end", {"turn": 1, "reason": {"kind": "completed"}})

    def notify(method, chat, **params):
        send({"method": method, "params": {"sessionId": chat, **params}})

    def live(chat, turn, count):
        attempt = f"perf-{turn}"
        notify("session.status", chat, status="running")
        notify("session.event", chat, event=event(chat, "turn/start", {"turn": turn}))
        notify("session.assistant-stream", chat, frame={"type": "start", "attemptId": attempt, "revision": 1, "turn": turn, "step": 1})
        for i in range(count):
            notify("session.assistant-stream", chat, frame={
                "type": "chunk", "attemptId": attempt, "revision": i + 2, "index": i, "time": 1,
                "chunk": {"type": "text-delta", "text": CHUNK}})
            time.sleep(.002)
        if turn == 1:
            notify("session.assistant-stream", chat, frame={
                "type": "end", "attemptId": attempt, "revision": count + 2, "index": count, "outcome": "committed"})
            notify("session.event", chat, event=event(chat, "assistant/message", {
                "turn": turn, "step": 1, "message": {"id": "perf-stream", "content": block(CHUNK * count)}}))
            notify("session.event", chat, event=event(chat, "turn/end", {"turn": turn, "reason": {"kind": "completed"}}))
            notify("session.status", chat, status="idle")

    fixture = ROOT / "protocol/fixtures"
    responses = {name: json.loads((fixture / f"{name.replace('/', '-')}.response.json").read_text())["result"]
                 for name in ("hoh/settings/get", "hoh/security/summary")}
    prompt = 0
    cwd = None
    for line in sys.stdin:
        request = json.loads(line)
        method = request.get("method")
        params = request.get("params", {})
        chat = params.get("sessionId", "window-main")
        message_id = request.get("id")
        if method == "session/prompt":
            prompt += 1
            send({"id": message_id, "result": {"messageId": f"prompt-{prompt}"}})
            live(chat, prompt, STREAM_CHUNKS if prompt == 1 else 10)
            continue
        if method == "initialize":
            cwd = params["cwd"]
            result = {"serverInfo": {"name": "perf", "version": "1"}, "hoh": {"protocolVersion": 1, "approvals": "host"}}
        elif method == "session/list":
            result = {"sessions": [{"id": key, "title": key, "employeeId": "default", "updatedAt": now_ms} for key in state]}
        elif method == "session/working-directory/get":
            result = {"cwd": cwd}
        elif method == "session/events":
            after = params.get("afterSeq", -1)
            limit = params["limit"]
            events = state[chat][-limit:] if params.get("tail") else state[chat][after+1:after+1+limit]
            result = {"events": events, "nextSeq": events[-1]["seq"] if events else after}
        elif method in ("session/resume", "session/cancel", "shutdown"):
            result = {}
        elif method in responses:
            result = responses[method]
        else:
            send({"id": message_id, "error": {"code": -32601, "message": "method unavailable"}})
            continue
        send({"id": message_id, "result": result})
        if method == "shutdown":
            return


def stage(dist, destination, smoke=False, compare=False, quick=False):
    shutil.copytree(dist, destination)
    entries = ("index.html", "legacy.html") if compare or quick else ("index.html",)
    for entry in entries:
        index = destination / entry
        if index.exists():
            index.write_text(index.read_text().replace("</head>", '<script src="/native-perf.js"></script></head>', 1))
    probe = SMOKE if smoke else COMPARE if compare or quick else PROBE
    if quick:
        probe = "globalThis.hohPerfQuick = true;\n" + probe
    (destination / "native-perf.js").write_text(probe)


def run(binary, engine_path=None, *, legacy=False):
    with tempfile.TemporaryDirectory(prefix="hoh-native-perf-") as home:
        tmp = Path(home)
        (tmp / "runtime").mkdir(mode=0o700)
        (tmp / "Documents").mkdir()
        env = {
            "PATH": "/usr/bin:/bin",
            "DISPLAY": os.environ["DISPLAY"],
            **{key: os.environ[key] for key in ("XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS") if key in os.environ},
            "HOME": home, "XDG_RUNTIME_DIR": str(tmp / "runtime"), "GDK_BACKEND": "x11",
            "HOH_NATIVE_PERF_PROBE": "1",
            **({"HOH_WINDOW_SHELL": "legacy"} if legacy else {}),
            **({"HOH_ENGINE_PATH": engine_path} if engine_path else {"HOH_TEST_ENGINE": str(Path(__file__).resolve())}),
        }
        process = subprocess.run([binary], env=env, capture_output=True, text=True, timeout=100)
        for line in process.stdout.splitlines():
            if line.startswith(MARKER):
                metrics = json.loads(line[len(MARKER):])
                print(json.dumps(metrics, ensure_ascii=False, indent=2))
                return process.returncode
        raise SystemExit(f"Native benchmark produced no metrics (exit={process.returncode}).\n{process.stderr}")


if __name__ == "__main__":
    if len(sys.argv) == 1:
        engine()
    elif sys.argv[1] == "stage":
        stage(Path(sys.argv[2]), Path(sys.argv[3]), smoke="--smoke" in sys.argv[4:],
              compare="--compare" in sys.argv[4:], quick="--quick" in sys.argv[4:])
    elif sys.argv[1] == "run":
        sys.exit(run(sys.argv[2], *[arg for arg in sys.argv[3:] if arg != "--legacy"],
                     legacy="--legacy" in sys.argv[3:]))
