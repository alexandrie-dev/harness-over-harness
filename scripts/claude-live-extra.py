#!/usr/bin/python3 -I
"""Extra live rows of HoH with the real `claude` (CEO 21.11 ~03:38; verify-4, "Что добавить в живой
прогон"). Started by scripts/claude-live-extra.sh; see the header of that script.

A host drives the built `hoh --profile window` over stdio JSON-RPC as the window core does: it
answers `approval/request` (with the MAC of `hoh_protocol::approval::mac`) and `user-question/ask`.
`claude` is started through a tap (fixtures `claude-tap`) that passes everything through and
records, per start, its argv, folder, pids, stdout frames and stderr. Nothing of the human's
~/.claude or data folder is changed: every row has its own DSH_HOME in the run folder, and the host
answers "rejected" to every request to write there.
"""
import argparse, hashlib, hmac, json, os, queue, re, select, shlex, signal, subprocess, sys, threading, time, uuid

SECRET_VALUE = "sk-ant-live-extra-" + "0123456789abcdefghijkl"  # made up; only to see whether it is shown
KEY = bytes(i + 7 for i in range(32))
SECRET_RE = re.compile(r"sk-ant-[A-Za-z0-9_-]{8,}|sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}"
                       r"|(?i:bearer)\s+[A-Za-z0-9._~+/=-]{20,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}")
MASKED = []  # exact secrets of this run, never printed


def clean(value, limit=200):
    text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
    for secret in MASKED:
        text = text.replace(secret, "<секрет>")
    text = SECRET_RE.sub("<секрет>", text)
    text = " ".join(text.split())
    return text if len(text) <= limit else text[:limit] + "…"


def yes(value):
    return "да" if value else "нет"


# ---------------------------------------------------------------------------- processes

def alive(pid):
    if not pid:
        return False
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8") as file:
            return file.read().rsplit(")", 1)[1].split()[0] != "Z"
    except (OSError, IndexError):
        return False


def cmdlines():
    found = {}
    for entry in os.listdir("/proc"):
        if entry.isdigit() and int(entry) != os.getpid():
            try:
                with open(f"/proc/{entry}/cmdline", "rb") as file:
                    found[int(entry)] = file.read().replace(b"\0", b" ").decode("utf-8", "replace")
            except OSError:
                pass
    return found


def with_token(token):
    """Pids of our own processes whose command line carries the run's unique token."""
    return [pid for pid, line in cmdlines().items() if token in line]


def descendants(pid):
    parents = {}
    for entry in os.listdir("/proc"):
        if entry.isdigit():
            try:
                with open(f"/proc/{entry}/stat", encoding="utf-8") as file:
                    parents[int(entry)] = int(file.read().rsplit(")", 1)[1].split()[1])
            except (OSError, IndexError, ValueError):
                pass
    found, todo = [], [pid]
    while todo:
        current = todo.pop()
        for child, parent in parents.items():
            if parent == current and child not in found:
                found.append(child)
                todo.append(child)
    return found


def stop_pids(pids):
    """Stop processes this run started, by their saved pids."""
    for pid in pids:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass


# ---------------------------------------------------------------------------- the tap

TAP = r'''#!/usr/bin/python3 -I
# Tap of the HoH live-extra run: runs the real claude (tap.json next to it) with the same
# arguments, folder and stdin; records argv, folder and pids per start in <starts>/, and copies
# stdout and stderr to <start>.out and <start>.err while passing them on.
import json, os, signal, subprocess, sys, threading, time

here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, "tap.json"), encoding="utf-8") as file:
    config = json.load(file)
args = sys.argv[1:]
stem = os.path.join(config["starts"], "%d-%d" % (time.time_ns(), os.getpid()))
meta = {"argv": args, "cwd": os.getcwd(), "tap_pid": os.getpid(), "child_pid": None}


def save():
    with open(stem + ".part", "w", encoding="utf-8") as file:
        json.dump(meta, file)
    os.rename(stem + ".part", stem + ".json")


save()
child = subprocess.Popen([config["claude"], *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
meta["child_pid"] = child.pid
save()
for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    signal.signal(number, lambda received, _frame: child.send_signal(received))


def pump(source, target, path):
    with open(path, "ab") as copy:
        for line in iter(source.readline, b""):
            copy.write(line)
            copy.flush()
            try:
                target.write(line)
                target.flush()
            except (BrokenPipeError, OSError):
                source.close()  # the reader is gone: the child gets EPIPE on its next write
                return


threads = [threading.Thread(target=pump, args=(child.stdout, sys.stdout.buffer, stem + ".out")),
           threading.Thread(target=pump, args=(child.stderr, sys.stderr.buffer, stem + ".err"))]
for thread in threads:
    thread.start()
code = child.wait()
for thread in threads:
    thread.join(5)
sys.exit(code if code >= 0 else 128 - code)
'''


def json_lines(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as file:
            for line in file:
                try:
                    value = json.loads(line)
                except ValueError:
                    continue
                if isinstance(value, dict):
                    yield value
    except OSError:
        return


class Env:
    """One row's world: data folder, work folder, tap, markers, and the engine's environment."""

    def __init__(self, ctx, name):
        self.ctx = ctx
        self.name = name
        self.root = os.path.join(ctx.run, name)
        self.dsh = os.path.join(self.root, "dsh-home")
        self.work = os.path.join(self.root, "work")
        self.starts_dir = os.path.join(self.root, "starts")
        self.marks = os.path.join(self.root, "markers")
        self.home = os.path.join(self.root, "home") if ctx.home == "isolated" else os.environ.get("HOME", "")
        for folder in (self.work, self.starts_dir, self.marks):
            os.makedirs(folder)
        os.makedirs(self.dsh, mode=0o700)
        os.chmod(self.dsh, 0o700)
        if ctx.home == "isolated":
            os.makedirs(os.path.join(self.home, ".claude"))
        self.write(os.path.join(self.work, "notes.txt"), "notes\n")
        self.write(os.path.join(self.work, "first.txt"), "first line\n")
        self.tap = os.path.join(self.root, "claude-tap")
        self.write(self.tap, TAP)
        os.chmod(self.tap, 0o755)
        self.write(os.path.join(self.root, "tap.json"), json.dumps({"claude": ctx.claude, "starts": self.starts_dir}))
        profile = os.path.join(self.dsh, "profiles", "window")
        os.makedirs(profile)
        self.write(os.path.join(profile, "package.json"), json.dumps(
            {"name": "dsh-profile-window", "private": True, "dependencies": {}, "dsh": {"profile": {"bundles": ["@hoh/window"]}}}))
        self.write(os.path.join(profile, "cordis.patch.yml"),
                   "- id: model-host\n  config:\n    baseUrl: http://127.0.0.1:9/v1\n    model: window-mock\n"
                   f"- id: harness-claude\n  config:\n    executable: {self.tap}\n")
        self.choices = os.path.join(profile, "external-harness.json")

    @staticmethod
    def write(path, text):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as file:
            file.write(text)

    def environ(self):
        env = dict(os.environ)
        env.update({"DSH_HOME": self.dsh, "DSH_TELEMETRY_DISABLED": "1", "HOME": self.home})
        env.setdefault("TMPDIR", "/dev/shm/hoh")
        return env

    def starts(self):
        found = []
        for name in sorted(os.listdir(self.starts_dir)):
            if name.endswith(".json"):
                path = os.path.join(self.starts_dir, name)
                try:
                    with open(path, encoding="utf-8") as file:
                        start = json.load(file)
                except (OSError, ValueError):
                    continue
                start["out"], start["err"] = path[:-5] + ".out", path[:-5] + ".err"
                found.append(start)
        return found

    def sessions(self):
        return [s for s in self.starts() if "--session-id" in s["argv"] or "--resume" in s["argv"]]

    def pids(self):
        return [pid for s in self.starts() for pid in (s.get("tap_pid"), s.get("child_pid")) if pid]

    def last_init(self):
        found = None
        for start in self.starts():
            for frame in json_lines(start["out"]):
                if frame.get("type") == "system" and frame.get("subtype") == "init":
                    found = frame
        return found

    def cleanup(self, extra=()):
        pids = set(self.pids())
        for pid in list(pids):
            pids.update(descendants(pid))
        stop_pids(sorted(pids | set(extra)))


def value_of(argv, switch):
    return argv[argv.index(switch) + 1] if switch in argv[:-1] else None


def mode_of(argv):
    return {"default": "confirm", "bypassPermissions": "full"}.get(value_of(argv, "--permission-mode") or "", "?")


# ---------------------------------------------------------------------------- the host

class Failure(Exception):
    pass


class Policy:
    """What the host answers: the window's own privileged calls are allowed, Claude's actions get `claude`."""

    def __init__(self):
        self.claude = "rejected"
        self.pick = None  # the label picked for every question

    def approval(self, params):
        return "allowed-once" if "/" in params.get("toolName", "") else self.claude


class Host:
    def __init__(self, env, policy):
        self.env, self.policy = env, policy
        self.frames, self.events, self.approvals, self.questions = [], [], [], []
        self.idle, self.running = {}, {}
        self.next_id = 0
        self.responses = {}
        self.queue = queue.Queue()
        self.proc = None

    # -- process
    def start(self):
        err = open(os.path.join(self.env.root, "engine-stderr.log"), "ab")
        self.env.ctx.hosts.append(self)
        self.proc = subprocess.Popen([self.env.ctx.hoh, "--profile", "window"], env=self.env.environ(), cwd=self.env.work,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)
        threading.Thread(target=self._read, args=(self.proc,), daemon=True).start()
        self.request("initialize", {
            "cwd": self.env.work, "provider": "openai-compatible", "model": "window-mock",
            "hoh": {"protocolVersion": 1, "approvalKey": KEY.hex()},
            "capabilities": {"approvals": "host", "userQuestions": True}})

    def _read(self, proc):
        for line in proc.stdout:
            try:
                self.queue.put(json.loads(line))
            except ValueError:
                pass

    def send(self, frame):
        self.proc.stdin.write((json.dumps(frame, ensure_ascii=False) + "\n").encode())
        self.proc.stdin.flush()

    def exited(self):
        return self.proc.poll() is not None

    def shutdown(self, wait=20):
        try:
            self.request("shutdown", {}, timeout=wait)
        except (Failure, OSError):
            pass
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(wait)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()

    def kill(self):
        self.proc.kill()
        self.proc.wait()

    # -- frames
    def pump(self, timeout=0.1):
        try:
            frame = self.queue.get(timeout=timeout)
        except queue.Empty:
            return False
        self.frames.append(frame)
        method = frame.get("method")
        if method and "id" in frame:
            self._reverse(frame)
        elif method == "session.event":
            params = frame.get("params") or {}
            event = params.get("event") or {}
            self.events.append((params.get("sessionId"), event.get("type"), event.get("data") or {}))
        elif method == "session.status":
            params = frame.get("params") or {}
            sid, status = params.get("sessionId"), params.get("status")
            if status == "idle":
                self.idle[sid] = self.idle.get(sid, 0) + 1
            elif status == "running":
                self.running[sid] = self.running.get(sid, 0) + 1
        elif "id" in frame:
            self.responses[frame["id"]] = frame
        return True

    def _reverse(self, frame):
        params = frame.get("params") or {}
        if frame["method"] == "approval/request":
            outcome = self.policy.approval(params)
            self.approvals.append(dict(params, _outcome=outcome))
            record = {"approvalId": params.get("approvalId"), "outcome": outcome, "actionHash": params.get("actionHash"),
                      "risk": params.get("risk"), "humanText": params.get("humanText"), "expiresAt": params.get("expiresAt")}
            mac = hmac.new(KEY, json.dumps(record, separators=(",", ":"), ensure_ascii=False).encode(), hashlib.sha256).hexdigest()
            self.send({"jsonrpc": "2.0", "id": frame["id"], "result": {"approvalId": params.get("approvalId"), "outcome": outcome, "mac": mac}})
        elif frame["method"] == "user-question/ask":
            self.questions.append(params)
            answers = [{"id": q["id"], "selected": [self.policy.pick or ((q.get("options") or [{}])[0].get("label", ""))]}
                       for q in params.get("questions", [])]
            self.send({"jsonrpc": "2.0", "id": frame["id"], "result": {"answers": answers}})
        else:
            self.send({"jsonrpc": "2.0", "id": frame["id"], "error": {"code": -32601, "message": "not served by the host"}})

    def wait(self, condition, timeout, what):
        until = time.monotonic() + timeout
        while not condition():
            if time.monotonic() > until:
                raise Failure(f"не дождались: {what} ({timeout} с)")
            if self.exited() and self.queue.empty():
                raise Failure(f"движок завершился, не дождались: {what}")
            self.pump()

    def request(self, method, params, timeout=60):
        self.next_id += 1
        rid = f"host-{self.next_id}"
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        self.wait(lambda: rid in self.responses, timeout, method)
        frame = self.responses.pop(rid)
        if "error" in frame:
            raise Failure(f"{method}: {clean((frame['error'] or {}).get('message', 'ошибка'))}")
        return frame.get("result")

    # -- window calls
    def select(self):
        self.request("hoh/executor/select", {"executor": "claude", "model": "default"})

    def set_access(self, mode):
        self.request("hoh/executor/access/set", {"executor": "claude", "mode": mode})

    def access(self):
        return self.request("hoh/executor/access/get", {"executor": "claude"})

    def prompt_async(self, session, text):
        self.next_id += 1
        rid = f"host-{self.next_id}"
        mark = (len(self.events), len(self.approvals), len(self.questions), self.idle.get(session, 0), len(self.env.sessions()))
        self.send({"jsonrpc": "2.0", "id": rid, "method": "session/prompt",
                   "params": {"sessionId": session, "contentBlocks": [{"type": "text", "text": text}]}})
        return {"rid": rid, "session": session, "mark": mark}

    def finish(self, turn, timeout):
        sid = turn["session"]
        self.wait(lambda: turn["rid"] in self.responses and self.idle.get(sid, 0) > turn["mark"][3], timeout, "конец хода")
        response = self.responses.pop(turn["rid"])
        events, approvals, questions, _, starts = turn["mark"]
        mine = [(t, d) for s, t, d in self.events[events:] if s == sid]
        texts = []
        for kind, data in mine:
            if kind == "assistant/message":
                for block in (data.get("message") or {}).get("content") or []:
                    if isinstance(block, dict) and block.get("type") == "text":
                        texts.append(block.get("text") or "")
        reason = next((d.get("reason") for t, d in reversed(mine) if t == "turn/end"), None) or {}
        error = response.get("error")
        return {"text": "\n".join(texts), "kind": reason.get("kind") if isinstance(reason, dict) else None,
                "error": clean((reason.get("error") or {}).get("message", "")) if isinstance(reason, dict) else "",
                "code": (reason.get("error") or {}).get("code") if isinstance(reason, dict) else None,
                "approvals": self.approvals[approvals:], "questions": self.questions[questions:],
                "new_starts": self.env.sessions()[starts:], "rejected": clean(error) if error else None}

    def turn(self, session, text, timeout=None):
        return self.finish(self.prompt_async(session, text), timeout or self.env.ctx.timeout)

    def journal(self, session):
        return json.dumps(self.request("session/events", {"sessionId": session, "limit": 1000}), ensure_ascii=False)


# ---------------------------------------------------------------------------- prompts

def p_cmd(command):
    return f"Run this shell command, then reply with its output only: `{command}`"


P_OK = "Reply with the single word ok."


def p_remember(word):
    return f"Remember the word {word}. Reply with ok."


P_RECALL = "What word did I ask you to remember? Reply with that word only."


class Ctx:
    def __init__(self, args):
        self.hoh, self.claude, self.timeout = args.hoh, args.claude, args.timeout
        self.home = args.home
        self.after = [float(x) for x in args.check_after.split(",")]
        self.run = args.dir
        self.rows = []
        self.hosts = []

    def row(self, status, name, detail):
        self.rows.append((status, name, clean(detail, 700)))


def fresh(ctx, name, mode="confirm", claude="rejected"):
    """An engine on new data with Claude selected and `mode` chosen by the human."""
    env = Env(ctx, name)
    policy = Policy()
    policy.claude = claude
    host = Host(env, policy)
    host.start()
    host.select()
    if mode == "full":
        host.set_access("full")
    return env, host


def alive_text(pid):
    return yes(alive(pid))


def guarded(ctx, name, body):
    """Run a row; an exception is a FAIL with its words."""
    try:
        body()
    except Failure as error:
        ctx.row("FAIL", name, f"нет факта: {error}")
    except Exception as error:  # a broken row must not hide the others
        ctx.row("FAIL", name, f"нет факта: {type(error).__name__}: {error}")
    for host in ctx.hosts:
        if host.proc is not None and host.proc.poll() is None:
            host.kill()  # an engine this run started, by its handle


# ---------------------------------------------------------------------------- rows

def row_stop(ctx, mode):
    name = f"live.stop.{mode}"

    def body():
        env, host = fresh(ctx, name, mode, claude="allowed-once")
        token = str(3000 + uuid.uuid4().int % 6000)
        try:
            turn = host.prompt_async("chat-stop", p_cmd(f"touch long-started; sleep {token}; touch long-done"))
            host.wait(lambda: os.path.exists(os.path.join(env.work, "long-started")), ctx.timeout, "начало длинной команды")
            before = env.sessions()
            started = time.monotonic()
            host.request("session/cancel", {"sessionId": "chat-stop"})
            done = host.finish(turn, ctx.timeout)
            took = time.monotonic() - started
            until = time.monotonic() + 10
            while with_token(f"sleep {token}") and time.monotonic() < until:
                time.sleep(0.2)
            left = with_token(f"sleep {token}")
            stop_pids(left)
            again = host.turn("chat-stop", P_OK)
            claude_pids = [s.get("child_pid") for s in before]
            same = len(env.sessions()) == len(before)
            ok = (done["kind"] not in (None, "completed", "error") and not left and
                  not os.path.exists(os.path.join(env.work, "long-done")) and again["kind"] == "completed")
            ctx.row("PASS" if ok else "FAIL", name,
                    f"ход прерван за {took:.1f} с (причина {done['kind']}); команда sleep осталась: {yes(left)}; "
                    f"процесс claude после «Стоп»: {'жив' if any(alive(p) for p in claude_pids) else 'нет'}, "
                    f"{'тот же' if same else 'запущен заново'}; следующий ход: {again['kind']}, ответ есть: {yes(again['text'].strip())}")
        finally:
            host.kill() if not host.exited() else None
            env.cleanup(with_token(f"sleep {token}"))

    guarded(ctx, name, body)


def restart(host):
    env = host.env
    host.shutdown()
    again = Host(env, host.policy)
    again.start()
    return again


def row_resume(ctx, mode):
    name = f"live.resume.{mode}"

    def body():
        env, host = fresh(ctx, name, mode)
        try:
            word = "HOHW" + uuid.uuid4().hex[:8]
            host.turn("chat-resume", p_remember(word))
            first = env.sessions()
            host = restart(host)
            host.select()
            host.request("session/resume", {"sessionId": "chat-resume"})
            answer = host.turn("chat-resume", P_RECALL)
            starts = env.sessions()
            resumed = value_of(starts[-1]["argv"], "--resume") if len(starts) > len(first) else None
            native = value_of(first[0]["argv"], "--session-id")
            ok = word in answer["text"] and resumed is not None and resumed == native
            ctx.row("PASS" if ok else "FAIL", name,
                    f"после перезапуска движка на тех же данных claude запущен с --resume {'тем же id' if resumed == native else resumed or 'нет'}; "
                    f"ответ помнит слово: {yes(word in answer['text'])}; ход: {answer['kind']}")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


def row_approve(ctx, mode):
    name = f"live.approve.{mode}"

    def body():
        env, host = fresh(ctx, name, mode, claude="allowed-once")
        try:
            done = host.turn("chat-approve", p_cmd("echo approved > approved.txt"))
            made = os.path.exists(os.path.join(env.work, "approved.txt"))
            journal = host.journal("chat-approve")
            logged = "allowed-once" in journal and "approval" in journal
            asked = [(a.get("toolName"), a["_outcome"]) for a in done["approvals"]]
            if mode == "confirm":
                ok = bool(asked) and made and logged
                detail = (f"запросов к хосту {len(asked)} ({clean(asked, 120)}); ответ «разрешить»; файл-маркер создан: {yes(made)}; "
                          f"решение в журнале сессии: {yes(logged)}")
            else:
                ok = not asked and made
                detail = f"запросов к хосту {len(asked)} (в full ждём 0); файл-маркер создан: {yes(made)}"
            ctx.row("PASS" if ok else "FAIL", name, detail)
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


def row_question(ctx, mode):
    name = f"live.question.{mode}"

    def body():
        env, host = fresh(ctx, name, mode)
        try:
            a, b = "OPT-RED-" + uuid.uuid4().hex[:6], "OPT-BLUE-" + uuid.uuid4().hex[:6]
            host.policy.pick = b
            done = host.turn("chat-question", f"Use the AskUserQuestion tool to ask me which token I choose, with options {a} and {b}. "
                                              "Then reply with the chosen token only.")
            reached = bool(done["questions"])
            returned = b in done["text"]
            if mode == "confirm":
                ok = reached and returned and done["kind"] == "completed"
                ctx.row("PASS" if ok else "FAIL", name,
                        f"вопрос дошёл до хоста: {yes(reached)} (вопросов {len(done['questions'])}); выбран второй вариант; "
                        f"маркер выбора в ответе Claude: {yes(returned)}; ход: {done['kind']}")
            else:
                ctx.row("PASS" if done["kind"] == "completed" else "FAIL", name,
                        f"в full вопрос дошёл до хоста: {yes(reached)}; маркер выбора в ответе: {yes(returned)}; ход: {done['kind']}")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


def row_v1(ctx):
    name = "live.v1.mode-switch"

    def body():
        env, host = fresh(ctx, name, "confirm")
        seen = []
        try:
            def run(chat, label):
                done = host.turn(chat, P_OK)
                argv = env.sessions()[-1]["argv"]
                got = host.access()
                seen.append((label, mode_of(argv), got["mode"], got["effective"]["mode"], len(done["new_starts"])))

            def change(mode):
                host.set_access(mode)
                got = host.access()
                seen.append((f"set {mode}", "—", got["mode"], got["effective"]["mode"], None))

            run("chat-a", "A confirm")
            change("full")
            run("chat-a", "A после full")
            run("chat-b", "B в full")
            change("confirm")
            run("chat-a", "A обратно")
            run("chat-b", "B обратно")
            want = [("confirm", "confirm"), ("—", "full"), ("full", "full"), ("full", "full"), ("—", "confirm"),
                    ("confirm", "confirm"), ("confirm", "confirm")]
            # the chosen mode is the human's, `effective` is what the running process (else the next start) uses
            ok = [(m, c) for _, m, c, _, _ in seen] == want and all(e == m for _, m, _, e, _ in seen if m != "—")
            ctx.row("PASS" if ok else "FAIL", name,
                    "; ".join(f"{label}: процесс {m}, выбор {c}, effective {e}" + (f", запусков {n}" if n is not None else "")
                              for label, m, c, e, n in seen) + f" (ждали выбор {', '.join(c for _, c in want)}; effective процесса = его флаги)")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


def form_of(start):
    """How the CLI reported a lost conversation: stderr before initialize, or a result frame with errors."""
    err = ""
    try:
        with open(start["err"], encoding="utf-8", errors="replace") as file:
            err = clean(file.read(), 160)
    except OSError:
        pass
    frames = list(json_lines(start["out"]))
    result = next((f for f in frames if f.get("type") == "result" and (f.get("errors") or f.get("is_error"))), None)
    init_seen = any(f.get("subtype") == "init" for f in frames)
    parts = []
    if err:
        parts.append(f"stderr{' до system/init' if not init_seen else ''}: «{err}»")
    if result:
        parts.append(f"кадр result (subtype {result.get('subtype')}, is_error {result.get('is_error')}) errors: «{clean(result.get('errors') or '', 160)}»")
    return "; ".join(parts)


def row_v2(ctx):
    def body():
        env, host = fresh(ctx, "live.v2", "confirm", claude="allowed-once")
        try:
            word = "HOHF" + uuid.uuid4().hex[:8]
            host.turn("chat-folder", p_remember(word))
            second = os.path.join(env.root, "folder2")
            os.makedirs(second)
            second = os.path.realpath(second)
            # The contract of i7: a conversation with Claude keeps its folder; a change is refused.
            native = value_of(env.sessions()[0]["argv"], "--session-id")
            first_folder = os.path.realpath(env.sessions()[0]["cwd"])
            try:
                host.request("session/working-directory/set", {"sessionId": "chat-folder", "path": second})
                refusal = None
            except Failure as error:
                refusal = str(error)
            fixed = refusal is not None and "EXECUTOR_FOLDER_FIXED:" in refusal
            where = host.turn("chat-folder", p_cmd("pwd"))
            start = env.sessions()[-1]
            reported = os.path.realpath(where["text"].strip().splitlines()[-1]) if where["text"].strip() else ""
            stayed = reported == first_folder and os.path.realpath(start["cwd"]) == first_folder
            before = len(env.sessions())
            host = restart(host)
            host.select()
            try:
                host.request("session/resume", {"sessionId": "chat-folder"})
                resumed = "session/resume принят"
            except Failure as error:
                resumed = f"session/resume отклонён: {error}"
            answer = host.turn("chat-folder", P_RECALL)
            remembered = word in answer["text"]
            starts = env.sessions()[before:]
            again = value_of(starts[-1]["argv"], "--resume") if starts else None
            kept = bool(starts) and os.path.realpath(starts[-1]["cwd"]) == first_folder
            ok = fixed and stayed and remembered and again == native and kept
            ctx.row("PASS" if ok else "FAIL", "live.v2.cwd",
                    f"смена папки: {'отклонена с EXECUTOR_FOLDER_FIXED' if fixed else 'принята' if refusal is None else 'отклонена другим текстом: ' + clean(refusal, 120)}; "
                    f"следующий ход в прежней папке: {yes(stayed)} (pwd: {clean(where['text'], 60) or 'нет'}); "
                    f"после перезапуска движка: --resume {'тот же id' if again == native else clean(again, 40) or 'нет'}, "
                    f"папка процесса прежняя: {yes(kept)}, слово помнит: {yes(remembered)}")
            form = "; ".join(f for f in (form_of(s) for s in starts) if f)
            where_now = ", ".join("новая" if os.path.realpath(x["cwd"]) == second else "прежняя" for x in starts) or "нет"
            detail = (f"{resumed}; новых запусков claude {len(starts)} (папка: {where_now}); слово помнит: {yes(remembered)}; ход: {answer['kind']}"
                      + (f" ({answer['code']}: {answer['error']})" if answer["error"] else ""))
            if remembered:
                ctx.row("PASS", "live.v2.lost-form", detail + "; разговор не потерян, форма потери не наблюдалась")
            else:
                ctx.row("PASS" if form or answer["error"] else "FAIL", "live.v2.lost-form",
                        detail + f"; форма сообщения CLI: {form or 'не поймана'}")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, "live.v2", body)


def row_v3(ctx):
    name = "live.v3.lost-resume"

    def body():
        env = Env(ctx, name)
        argv = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages",
                "--permission-mode", "default", "--setting-sources", "user,project,local", "--permission-prompt-tool", "stdio",
                "--disallowedTools", "WebFetch,WebSearch", "--resume", str(uuid.uuid4())]
        proc = subprocess.Popen([ctx.claude, *argv], cwd=env.work, env=env.environ(), stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        lines = queue.Queue()
        for stream, label in ((proc.stdout, "out"), (proc.stderr, "err")):
            threading.Thread(target=lambda s=stream, l=label: [lines.put((time.monotonic(), l, x)) for x in iter(s.readline, b"")], daemon=True).start()
        try:
            proc.stdin.write((json.dumps({"type": "user", "parent_tool_use_id": None,
                                          "message": {"role": "user", "content": [{"type": "text", "text": P_OK}]}}) + "\n").encode())
            proc.stdin.flush()
        except OSError:
            pass
        out, err, init_first_err = [], [], None
        until = time.monotonic() + min(ctx.timeout, 90)
        while time.monotonic() < until and (proc.poll() is None or not lines.empty()):
            try:
                _, label, line = lines.get(timeout=0.2)
            except queue.Empty:
                continue
            text = line.decode("utf-8", "replace")
            if label == "err":
                if init_first_err is None:
                    init_first_err = not any('"init"' in x for x in out)
                err.append(text)
            else:
                out.append(text)
                if '"type":"result"' in text.replace(" ", ""):
                    break
        code = proc.poll()
        try:
            proc.stdin.close()
        except OSError:
            pass
        if proc.poll() is None:
            proc.kill()
        proc.wait()
        frames = []
        for text in out:
            try:
                frames.append(json.loads(text))
            except ValueError:
                pass
        result = next((f for f in frames if f.get("type") == "result"), None)
        parts = []
        if err:
            parts.append(f"stderr{' до system/init' if init_first_err else ' после system/init'}: «{clean(' '.join(err), 160)}»")
        if result:
            parts.append(f"кадр result (subtype {result.get('subtype')}, is_error {result.get('is_error')}) errors: «{clean(result.get('errors') or result.get('result') or '', 160)}»")
        ctx.row("PASS" if parts else "FAIL", name,
                f"несуществующий id в --resume: {'; '.join(parts) or 'форма не поймана'}; код выхода {code if code is not None else 'процесс жив'}")

    guarded(ctx, name, body)


def watch_after(ctx, env, host, how, name):
    """Mid-turn hard stop of the engine in full access, then whether claude and the marks go on."""
    marks = f"marks-{uuid.uuid4().hex[:8]}.txt"
    loop = f"for i in $(seq 1 60); do echo $i >> {marks}; sleep 2; done"
    path = os.path.join(env.work, marks)

    def lines():
        try:
            with open(path, encoding="utf-8") as file:
                return len(file.read().split())
        except OSError:
            return 0

    try:
        host.prompt_async("chat-kill", p_cmd(loop))
        host.wait(lambda: lines() >= 2, ctx.timeout, "метки в файле")
        pids = [p for s in env.sessions() for p in (s.get("tap_pid"), s.get("child_pid")) if p]
        pids += [d for p in pids for d in descendants(p)]
        n0 = lines()
        if how == "kill":
            host.kill()
            by = "KILL движка"
            gone = "—"
        else:
            host.send({"jsonrpc": "2.0", "id": "host-shutdown", "method": "shutdown", "params": {}})
            time.sleep(0.5)
            gone = yes(host.exited())
            if not host.exited():
                host.kill()
            by = "shutdown, ожидание 500 мс, затем KILL"
        t0, facts, ends = time.monotonic(), [], None
        for wait in ctx.after:
            time.sleep(max(0.0, t0 + wait - time.monotonic()))
            a = any(alive(p) for p in pids if p in env.pids())
            facts.append((wait, a, lines()))
            ends = a
        ok = not ends
        ctx.row("PASS" if ok else "FAIL", name,
                f"{by}{'' if how == 'kill' else f' (движок вышел сам за 0,5 с: {gone})'} на метке {n0}: "
                + "; ".join(f"через {w:g} с claude жив: {yes(a)}, меток {n}" for w, a, n in facts)
                + ("" if ok else "; claude пережил остановку движка"))
    finally:
        env.cleanup(with_token(marks))


def row_v5(ctx):
    for how, name in (("kill", "live.v5.kill-full"), ("shutdown", "live.v5.shutdown500-full")):
        def body(how=how, name=name):
            env, host = fresh(ctx, name, "full")
            watch_after(ctx, env, host, how, name)

        guarded(ctx, name, body)


MCP_SERVER = '''# Stub MCP server of the HoH live run: only writes markers (started, listed, called).
import json, os, sys
marks = sys.argv[1]
def mark(name):
    open(os.path.join(marks, name), "a").close()
mark("mcp-started")
for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    method, ident = message.get("method"), message.get("id")
    if ident is None:
        continue
    if method == "initialize":
        result = {"protocolVersion": (message.get("params") or {}).get("protocolVersion", "2025-06-18"),
                  "capabilities": {"tools": {}}, "serverInfo": {"name": "hoh-live", "version": "1"}}
    elif method == "tools/list":
        mark("mcp-listed")
        result = {"tools": []}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}), flush=True)
'''


def mcp_row(ctx, name, enable):
    def body():
        env, host = fresh(ctx, name, "confirm")
        try:
            server = os.path.join(env.root, "mcp_server.py")
            Env.write(server, MCP_SERVER)
            Env.write(os.path.join(env.work, ".mcp.json"), json.dumps(
                {"mcpServers": {"hoh-live": {"type": "stdio", "command": sys.executable, "args": ["-I", server, env.marks]}}}))
            if enable:
                Env.write(os.path.join(env.home, ".claude", "settings.json"), json.dumps({"enableAllProjectMcpServers": True}))
            done = host.turn("chat-mcp", P_OK)
            sessions = env.sessions()
            sources = value_of(sessions[-1]["argv"], "--setting-sources") if sessions else None
            init = env.last_init()
            started = os.path.exists(os.path.join(env.marks, "mcp-started"))
            if init is None:
                ctx.row("FAIL", name, "нет факта: system/init не пойман")
            else:
                servers = [s for s in init.get("mcp_servers") or [] if isinstance(s, dict)]
                shown = ", ".join(f"{clean(s.get('name'), 40)} (status {clean(s.get('status'), 20)}, source {clean(s.get('source', 'поля нет'), 20)})" for s in servers)
                mine = any(s.get("name") == "hoh-live" for s in servers)
                ctx.row("PASS", name,
                        f".mcp.json в папке при --setting-sources {sources if sources is not None else '(флага нет)'}{' и enableAllProjectMcpServers в личных настройках изолированного дома' if enable else ''}: "
                        f"mcp_servers [{shown or 'пусто'}]; сервер проекта hoh-live объявлен: {yes(mine)}; запущен (метка): {yes(started)}; ход: {done['kind']}")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


def row_v6(ctx):
    mcp_row(ctx, "live.v6.mcp-project", False)
    if ctx.home == "isolated":
        mcp_row(ctx, "live.v6.mcp-project-enabled", True)
    else:
        ctx.row("INFO", "live.v6.mcp-project-enabled", "пропущено: включение серверов проекта меняет личные настройки, это только на изолированном доме")


def seen_by_host(approvals):
    return " | ".join(clean(f"{a.get('toolName')}: {a.get('humanText')} {a.get('details') or ''}", 220) for a in approvals)


def row_v7(ctx):
    name = "live.v7.protected-write"

    def body():
        env, host = fresh(ctx, name, "confirm", claude="rejected")
        try:
            sha = lambda p: hashlib.sha256(open(p, "rb").read()).hexdigest() if os.path.exists(p) else None
            target = env.choices
            before = sha(target)
            done = host.turn("chat-protect", f"Write the text hoh-test into the file {target}, replacing its content.")
            asked = [a for a in done["approvals"] if os.path.basename(target) in json.dumps(a, ensure_ascii=False)]
            ok = bool(asked) and sha(target) == before
            ctx.row("PASS" if ok else "FAIL", name,
                    f"запись в external-harness.json: хост получил {len(done['approvals'])} запрос(ов), упоминают файл: {yes(asked)}; "
                    f"текст вопроса: «{seen_by_host(done['approvals']) or 'нет'}»; ответ: отказ; файл не изменён: {yes(sha(target) == before)}")
            if ctx.home == "isolated":
                settings = os.path.join(env.home, ".claude", "settings.json")
                Env.write(settings, "{}\n")
                before = sha(settings)
                done = host.turn("chat-protect", f"Write the text hoh-test into the file {settings}, replacing its content.")
                asked = [a for a in done["approvals"] if "settings.json" in json.dumps(a, ensure_ascii=False)]
                ctx.row("PASS" if asked and sha(settings) == before else "FAIL", "live.v7.settings",
                        f"запись в settings.json изолированного дома: упоминают файл: {yes(asked)}; "
                        f"текст вопроса: «{seen_by_host(done['approvals']) or 'нет'}»; ответ: отказ; файл не изменён: {yes(sha(settings) == before)}")
            else:
                ctx.row("INFO", "live.v7.settings", "пропущено: только на изолированном доме (настоящий ~/.claude не трогаем)")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


def row_v8(ctx):
    name = "live.v8.read-outside"

    def body():
        env, host = fresh(ctx, name, "confirm", claude="rejected")
        try:
            word = "OUTSIDE-" + uuid.uuid4().hex[:8]
            outside = os.path.join(env.root, "outside", "note.txt")
            Env.write(outside, word + "\n")
            done = host.turn("chat-outside", f"Read the file {os.path.realpath(outside)} and reply with its first line.")
            asked = [a for a in done["approvals"] if "note.txt" in json.dumps(a, ensure_ascii=False)]
            read = word in done["text"]
            ctx.row("PASS" if done["kind"] else "FAIL", name,
                    f"файл вне рабочей папки: CLI спросил хоста: {yes(asked)} (запросов {len(done['approvals'])}); "
                    f"хост увидел: «{seen_by_host(done['approvals']) or 'ничего'}»; ответ хоста: отказ; "
                    f"содержимое файла попало в ответ Claude: {yes(read)}; ход: {done['kind']}")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


def row_v9(ctx):
    name = "live.v9.secret-url"

    def body():
        env, host = fresh(ctx, name, "confirm", claude="rejected")
        try:
            MASKED.append(SECRET_VALUE)
            command = f'curl -s "http://127.0.0.1:9/v1/ping?api_key={SECRET_VALUE}&x=1"'
            done = host.turn("chat-secret", p_cmd(command))
            bash = [a for a in done["approvals"] if a.get("toolName") == "bash"]
            if not bash:
                ctx.row("FAIL", name, f"нет факта: запроса на команду bash к хосту не было (запросов {len(done['approvals'])}, ход {done['kind']})")
                return
            shown = " ".join(str(bash[0].get(k) or "") for k in ("humanText", "details"))
            whole = json.dumps(bash[0], ensure_ascii=False)
            ctx.row("PASS", name,
                    f"исходная команда: curl … ?api_key=<значение>&x=1; окно защиты получило: «{clean(shown, 260)}»; "
                    f"значение секретного параметра в тексте окна: {yes(SECRET_VALUE in shown)}, во всём запросе: {yes(SECRET_VALUE in whole)}; "
                    f"имя параметра api_key видно: {yes('api_key' in shown)}; текст совпадает с исходной командой: {yes(command in shown)}; ответ: отказ")
            host.shutdown()
        finally:
            env.cleanup()

    guarded(ctx, name, body)


GROUPS = {
    "stop": lambda c: [row_stop(c, m) for m in ("confirm", "full")],
    "resume": lambda c: [row_resume(c, m) for m in ("confirm", "full")],
    "approve": lambda c: [row_approve(c, m) for m in ("confirm", "full")],
    "question": lambda c: [row_question(c, m) for m in ("confirm", "full")],
    "v1": row_v1, "v2": row_v2, "v3": row_v3, "v5": row_v5, "v6": row_v6, "v7": row_v7, "v8": row_v8, "v9": row_v9,
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--hoh", required=True)
    parser.add_argument("--claude", required=True)
    parser.add_argument("--dir", required=True)
    parser.add_argument("--timeout", type=int, default=240)
    parser.add_argument("--home", choices=("real", "isolated"), default="real")
    parser.add_argument("--only", default="")
    parser.add_argument("--check-after", default="5,30")
    args = parser.parse_args()
    ctx = Ctx(args)
    names = [n for n in args.only.split(",") if n] or list(GROUPS)
    for name in names:
        if name not in GROUPS:
            print(f"неизвестная группа строк: {name} (есть: {', '.join(GROUPS)})", file=sys.stderr)
            return 2
    os.makedirs(ctx.run, exist_ok=True)
    for name in names:
        GROUPS[name](ctx)
    lines = [f"{status} {name}: {detail}" for status, name, detail in ctx.rows]
    failed = [name for status, name, _ in ctx.rows if status == "FAIL"]
    lines.append(f"ИТОГ: {'не прошло: ' + ', '.join(failed) if failed else 'нет FAIL'} ({ctx.run})")
    text = "\n".join(lines) + "\n"
    with open(os.path.join(ctx.run, "report.txt"), "w", encoding="utf-8") as file:
        file.write(text)
    sys.stdout.write(text)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
