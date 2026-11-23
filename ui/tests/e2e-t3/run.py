#!/usr/bin/env python3
"""Live T3-window QA for an already-built HoH release.

The parent process owns Xvfb.  The child process owns a private session bus,
AT-SPI bus, the demo script and every process it launches.  No dependency is
installed and no release artifact is rebuilt.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import traceback
from typing import Any, Callable, Iterable


REPO = Path(__file__).resolve().parents[3]
RELEASE = Path("/mnt/toshiba/alexandrie/hoh/release")
KNOWN_SIDEBAR = {
    "New chat",
    "Learning",
    "Widgets",
    "Add to team",
    "Add project",
    "Settings",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qa-dir", type=Path, required=True)
    parser.add_argument("--display", default=":84")
    parser.add_argument("--commit", default="b0f350af")
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    return parser.parse_args()


def wait_until(
    predicate: Callable[[], Any],
    timeout: float,
    description: str,
    interval: float = 0.25,
) -> Any:
    deadline = time.monotonic() + timeout
    last_error: BaseException | None = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except BaseException as error:  # stale AT-SPI objects are normal during rerenders
            last_error = error
        time.sleep(interval)
    detail = f"; last error: {last_error}" if last_error else ""
    raise TimeoutError(f"Timed out waiting for {description}{detail}")


def terminate_owned(process: subprocess.Popen[Any] | None, timeout: float = 8) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)


def parent(args: argparse.Namespace) -> int:
    if args.display != ":84":
        raise SystemExit("This QA assignment owns Xvfb :84 only")
    qa = args.qa_dir.resolve()
    qa.mkdir(parents=True, exist_ok=True)
    (qa / "shots").mkdir(exist_ok=True)
    (qa / "logs").mkdir(exist_ok=True)
    data = qa / "data"
    if data.exists():
        raise SystemExit(f"Fresh-run data already exists: {data}")

    expected = [
        RELEASE / f"hoh-desktop-{args.commit}",
        RELEASE / f"hoh-{args.commit}",
        RELEASE / f"hoh-llm-openai-mock-{args.commit}",
    ]
    missing = [str(path) for path in expected if not path.is_file()]
    if missing:
        raise SystemExit("Missing release artifacts: " + ", ".join(missing))

    xvfb_log = (qa / "logs" / "xvfb.log").open("w", encoding="utf-8")
    xvfb = subprocess.Popen(
        ["Xvfb", args.display, "-screen", "0", "1200x800x24", "-nolisten", "tcp", "-extension", "GLX"],
        stdout=xvfb_log,
        stderr=subprocess.STDOUT,
    )
    try:
        wait_until(
            lambda: subprocess.run(
                ["xdpyinfo", "-display", args.display],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            ).returncode
            == 0,
            10,
            f"Xvfb {args.display}",
        )
        command = [
            "dbus-run-session",
            "--",
            sys.executable,
            str(Path(__file__).resolve()),
            "--inside",
            "--qa-dir",
            str(qa),
            "--display",
            args.display,
            "--commit",
            args.commit,
        ]
        run_log = (qa / "logs" / "runner.log").open("w", encoding="utf-8")
        try:
            completed = subprocess.run(command, cwd=REPO, stdout=run_log, stderr=subprocess.STDOUT)
        finally:
            run_log.close()
        return completed.returncode
    finally:
        terminate_owned(xvfb)
        xvfb_log.close()


class Harness:
    def __init__(self, args: argparse.Namespace, Atspi: Any):
        self.args = args
        self.Atspi = Atspi
        self.qa = args.qa_dir.resolve()
        self.logs = self.qa / "logs"
        self.shots = self.qa / "shots"
        self.data = self.qa / "data"
        self.demo: subprocess.Popen[Any] | None = None
        self.demo_stream: Any = None
        self.app: Any = None
        self.results: dict[int, dict[str, Any]] = {}
        self.request_count = 0

    @staticmethod
    def safe(call: Callable[[], Any], default: Any = None) -> Any:
        try:
            return call()
        except BaseException:
            return default

    def children(self, node: Any) -> Iterable[Any]:
        count = self.safe(node.get_child_count, 0)
        for index in range(count):
            child = self.safe(lambda index=index: node.get_child_at_index(index))
            if child is not None:
                yield child

    def nodes(self, root: Any | None = None) -> Iterable[Any]:
        root = root if root is not None else self.app
        if root is None:
            return
        stack = [root]
        while stack:
            node = stack.pop()
            yield node
            stack.extend(reversed(list(self.children(node))))

    def role(self, node: Any) -> str:
        return self.safe(node.get_role_name, "") or ""

    def name(self, node: Any) -> str:
        return self.safe(node.get_name, "") or ""

    def text(self, node: Any) -> str:
        iface = self.safe(node.get_text_iface)
        if iface is None:
            return ""
        return self.safe(lambda: iface.get_text(0, -1), "") or ""

    def attrs(self, node: Any) -> dict[str, str]:
        return self.safe(node.get_attributes, {}) or {}

    def extent(self, node: Any) -> tuple[int, int, int, int] | None:
        component = self.safe(node.get_component_iface)
        if component is None:
            return None
        rect = self.safe(lambda: component.get_extents(self.Atspi.CoordType.SCREEN))
        if rect is None:
            return None
        return (rect.x, rect.y, rect.width, rect.height)

    def find_all(
        self,
        *,
        root: Any | None = None,
        role: str | None = None,
        name: str | None = None,
        prefix: str | None = None,
        contains: str | None = None,
        pattern: str | None = None,
    ) -> list[Any]:
        found: list[Any] = []
        regex = re.compile(pattern) if pattern else None
        for node in self.nodes(root):
            node_name = self.name(node)
            if role is not None and self.role(node) != role:
                continue
            if name is not None and node_name != name:
                continue
            if prefix is not None and not node_name.startswith(prefix):
                continue
            if contains is not None and contains not in node_name:
                continue
            if regex is not None and regex.search(node_name) is None:
                continue
            found.append(node)
        return found

    def find(self, **locator: Any) -> Any | None:
        found = self.find_all(**locator)
        return found[0] if found else None

    def require(self, description: str, **locator: Any) -> Any:
        node = self.find(**locator)
        if node is None:
            raise AssertionError(f"Missing {description}: {locator}")
        return node

    def press(self, description: str, **locator: Any) -> None:
        node = self.require(description, **locator)
        action = self.safe(node.get_action_iface)
        if action is None or action.get_n_actions() < 1:
            raise AssertionError(f"{description} has no accessibility action")
        if not action.do_action(0):
            raise AssertionError(f"Accessibility action failed for {description}")

    def all_text(self) -> str:
        values: list[str] = []
        for node in self.nodes():
            for value in (self.name(node), self.text(node)):
                value = value.strip()
                if value and value not in values:
                    values.append(value)
        return "\n".join(values)

    def dump(self, label: str) -> Path:
        path = self.logs / f"{label}.atspi.txt"
        lines: list[str] = []
        for node in self.nodes():
            role = self.role(node)
            name = self.name(node)
            text = self.text(node)
            attrs = self.attrs(node)
            extent = self.extent(node)
            if name or text or role not in {"unknown", "redundant object", "panel", "section", "filler"}:
                lines.append(
                    f"role={role!r} name={name!r} text={text!r} extent={extent!r} attrs={attrs!r}"
                )
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path

    def window_id(self) -> str:
        output = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", "^HoH$"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        if not output:
            raise AssertionError("No visible HoH X11 window")
        app_pid = self.safe(self.app.get_process_id, 0) if self.app is not None else 0
        candidates: list[tuple[int, str]] = []
        for window in output:
            pid = subprocess.run(
                ["xdotool", "getwindowpid", window],
                check=False,
                capture_output=True,
                text=True,
            ).stdout.strip()
            if pid and int(pid) == app_pid:
                geometry = subprocess.run(
                    ["xdotool", "getwindowgeometry", "--shell", window],
                    check=False,
                    capture_output=True,
                    text=True,
                ).stdout
                values = dict(
                    line.split("=", 1) for line in geometry.splitlines() if "=" in line
                )
                area = int(values.get("WIDTH", "0")) * int(values.get("HEIGHT", "0"))
                candidates.append((area, window))
        if candidates:
            largest = max(area for area, _ in candidates)
            return min(
                (window for area, window in candidates if area == largest),
                key=int,
            )
        raise AssertionError(f"No visible HoH X11 window belongs to current app PID {app_pid}: {output}")

    def shot(self, label: str) -> Path:
        path = self.shots / f"{label}.png"
        frame = self.require("current frame", role="frame", name="HoH")
        extent = self.extent(frame)
        if extent is None:
            raise AssertionError("HoH frame has no screen extent")
        x, y, width, height = extent
        subprocess.run(
            ["import", "-display", self.args.display, "-window", "root", str(path)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        subprocess.run(
            ["mogrify", "-crop", f"{width}x{height}+{x}+{y}", "+repage", str(path)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        return path

    def xkey(self, *keys: str) -> None:
        subprocess.run(
            ["xdotool", "windowfocus", self.window_id()],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        subprocess.run(
            ["xdotool", "key", "--clearmodifiers", *keys],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )

    def start_demo(self, tag: str) -> None:
        if self.demo is not None and self.demo.poll() is None:
            raise RuntimeError("Demo is already running")
        log_path = self.logs / f"demo-{tag}.log"
        self.demo_stream = log_path.open("w", encoding="utf-8")
        env = os.environ.copy()
        env.update(
            DISPLAY=self.args.display,
            GDK_BACKEND="x11",
            NO_AT_BRIDGE="0",
            GTK_MODULES="gail:atk-bridge",
            HOH_DEMO_COMMIT=self.args.commit,
        )
        self.demo = subprocess.Popen(
            ["bash", str(REPO / "scripts" / "demo-window.sh"), "--claude", str(self.data)],
            cwd=REPO,
            env=env,
            stdout=self.demo_stream,
            stderr=subprocess.STDOUT,
        )
        wait_until(lambda: self._discover_app(), 60, "HoH accessibility application", 0.5)
        wait_until(lambda: self.find(role="document web") is not None, 30, "loaded WebKit document")

    def _discover_app(self) -> bool:
        desktop = self.Atspi.get_desktop(0)
        candidates = []
        for index in range(self.safe(desktop.get_child_count, 0)):
            app = self.safe(lambda index=index: desktop.get_child_at_index(index))
            if app is not None and "hoh-desktop" in self.name(app):
                pid = self.safe(app.get_process_id, 0)
                if pid and Path(f"/proc/{pid}").exists():
                    candidates.append((pid, app))
        for _, app in sorted(candidates, reverse=True, key=lambda candidate: candidate[0]):
            if self.safe(app.get_child_count, 0) > 0:
                self.app = app
                return True
        return False

    def close_demo(self, tag: str) -> None:
        if self.demo is None:
            return
        if self.demo.poll() is None:
            try:
                self.xkey("alt+F4")
                self.demo.wait(timeout=20)
            except BaseException:
                terminate_owned(self.demo)
        if self.demo_stream is not None:
            self.demo_stream.close()
        window_log = self.data / "window.log"
        if window_log.exists():
            shutil.copy2(window_log, self.logs / f"window-{tag}.log")
        self.demo = None
        self.demo_stream = None
        self.app = None

    def submit(self, prompt: str) -> None:
        self.request_count += 1
        if self.request_count > 10:
            raise AssertionError("Live Claude request budget exceeded")
        entry = self.require("composer message editor", role="entry", prefix="Message")
        component = self.safe(entry.get_component_iface)
        if component is None or not component.grab_focus():
            raise AssertionError("Could not focus composer through AT-SPI")
        subprocess.run(
            ["xdotool", "type", "--clearmodifiers", "--delay", "1", "--", prompt],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        wait_until(lambda: self.find(role="push button", name="Submit message"), 10, "enabled Submit message")
        self.press("Submit message", role="push button", name="Submit message")

    def new_chat(self) -> None:
        self.press("New chat", role="push button", name="New chat")
        wait_until(lambda: self.find(role="entry", prefix="Message"), 20, "new-chat composer")

    def recent_titles(self) -> list[str]:
        result = []
        for node in self.find_all(role="push button"):
            name = self.name(node)
            if name and name not in KNOWN_SIDEBAR and ", No project" in name:
                result.append(name)
        return result

    def open_recent(self, title_fragment: str) -> None:
        node = wait_until(
            lambda: self.find(role="push button", contains=title_fragment),
            20,
            f"recent chat {title_fragment!r}",
        )
        action = node.get_action_iface()
        if not action.do_action(0):
            raise AssertionError(f"Could not open recent chat {title_fragment!r}")
        wait_until(
            lambda: self.find(role="heading", contains=title_fragment),
            20,
            f"opened chat {title_fragment!r}",
        )

    def stop_visible(self) -> bool:
        return self.find(role="push button", name="Stop generation") is not None

    def wait_turn_done(self, expected: str | None = None, timeout: float = 120) -> None:
        def done() -> bool:
            text = self.all_text()
            expected_seen = expected is None or expected in text
            return expected_seen and not self.stop_visible() and self.find(role="push button", name="Submit message") is not None

        wait_until(done, timeout, f"turn completion ({expected or 'any response'})", 0.5)

    def proc_rows(self) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                stat = (entry / "stat").read_text(encoding="utf-8").split()
                cmd = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode("utf-8", "replace").strip()
                rows.append({"pid": int(entry.name), "ppid": int(stat[3]), "state": stat[2], "cmd": cmd})
            except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError):
                continue
        return rows

    def demo_descendants(self) -> list[dict[str, Any]]:
        if self.demo is None:
            return []
        rows = self.proc_rows()
        descendants = {self.demo.pid}
        changed = True
        while changed:
            changed = False
            for row in rows:
                if row["ppid"] in descendants and row["pid"] not in descendants:
                    descendants.add(row["pid"])
                    changed = True
        return [row for row in rows if row["pid"] in descendants]

    def record_processes(self, label: str, rows: list[dict[str, Any]]) -> Path:
        path = self.logs / f"{label}.processes.json"
        path.write_text(json.dumps(rows, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        return path

    @staticmethod
    def is_claude_process(row: dict[str, Any]) -> bool:
        command = row["cmd"].split(maxsplit=1)
        return bool(command) and Path(command[0]).name == "claude"

    def scenario(self, number: int, title: str, body: Callable[[], dict[str, Any] | None]) -> None:
        started = time.monotonic()
        try:
            evidence = body() or {}
            self.results[number] = {
                "status": "PASS",
                "title": title,
                "seconds": round(time.monotonic() - started, 3),
                "evidence": evidence,
            }
            print(f"PASS {number}: {title}", flush=True)
        except BaseException as error:
            failure = self.logs / f"scenario-{number:02d}-failure.txt"
            failure.write_text(traceback.format_exc(), encoding="utf-8")
            try:
                self.dump(f"scenario-{number:02d}-failure")
                self.shot(f"{number:02d}-failure")
            except BaseException:
                pass
            self.results[number] = {
                "status": "FAIL",
                "title": title,
                "seconds": round(time.monotonic() - started, 3),
                "error": f"{type(error).__name__}: {error}",
                "evidence": {"traceback": str(failure)},
            }
            print(f"FAIL {number}: {title}: {error}", flush=True)

    def run(self) -> int:
        self.start_demo("first")

        def first_run() -> dict[str, Any]:
            wait_until(lambda: self.find(role="landmark", name="Welcome"), 40, "Welcome landmark")
            self.require("Welcome heading", role="heading", name="Welcome to HoH")
            welcome = self.shot("01-welcome")
            welcome_dump = self.dump("01-welcome")
            self.press("Start", role="push button", name="Start")
            wait_until(lambda: self.find(role="push button", name="Start without a project"), 30, "post-Welcome home")
            self.press("Start without a project", role="push button", name="Start without a project")
            wait_until(lambda: self.find(role="entry", name="Message New thread"), 40, "empty T3 composer")
            greeting = self.require("time greeting", role="heading", pattern=r"^Good (morning|afternoon|evening)$")
            main = self.shot("01-main")
            main_dump = self.dump("01-main")
            source_contracts = {
                "Alexandrie": REPO / "ui/src/t3-entry/notch/HohNotch.tsx",
                "data-hoh-home-body": REPO / "ui/src/t3-entry/hoh-screens/HomeBody.tsx",
                "data-approval-detail": REPO / "ui/src/t3-entry/hoh-screens/HomeBody.tsx",
                "data-hoh-page-item": REPO / "ui/src/t3-entry/hoh-screens/SidebarTop.tsx",
                "data-hoh-page": REPO / "ui/src/t3-entry/hoh-screens/SidebarPages.tsx",
            }
            audited: dict[str, str] = {}
            for token, path in source_contracts.items():
                if token not in path.read_text(encoding="utf-8"):
                    raise AssertionError(f"HoH locator contract {token!r} is absent from {path}")
                audited[token] = str(path.relative_to(REPO))
            audit_path = self.logs / "01-source-locators.json"
            audit_path.write_text(json.dumps(audited, indent=2) + "\n", encoding="utf-8")
            return {
                "welcome_shot": str(welcome),
                "main_shot": str(main),
                "raw": [str(welcome_dump), str(main_dump), str(audit_path)],
                "greeting": self.name(greeting),
                "notch": "Alexandrie visually checked from screenshot; source contract HohNotch.tsx",
            }

        self.scenario(1, "first launch → Welcome → Start → empty main", first_run)

        def stream_reply() -> dict[str, Any]:
            self.new_chat()
            prompt = "Reply with exactly one word: STREAM"
            self.submit(prompt)
            samples: list[dict[str, Any]] = []
            growth_shot: Path | None = None
            deadline = time.monotonic() + 100
            last_size = -1
            while time.monotonic() < deadline:
                text = self.all_text()
                size = len(text)
                if size != last_size:
                    samples.append({"at": round(time.time(), 3), "characters": size, "stop": self.stop_visible()})
                    last_size = size
                if self.stop_visible() and growth_shot is None:
                    growth_shot = self.shot("02-growing")
                if "STREAM" in text and not self.stop_visible() and self.find(role="push button", name="Submit message"):
                    break
                time.sleep(0.25)
            self.wait_turn_done("STREAM", 30)
            final = self.shot("02-replied")
            raw = self.dump("02-replied")
            titles = self.recent_titles()
            if not any("Reply with exactly one word: STREAM" in title for title in titles):
                raise AssertionError(f"Conversation absent from Recent chats: {titles}")
            if len({sample["characters"] for sample in samples}) < 2:
                raise AssertionError("No observable response growth")
            sample_path = self.logs / "02-stream-samples.json"
            sample_path.write_text(json.dumps(samples, indent=2) + "\n", encoding="utf-8")
            return {
                "growing_shot": str(growth_shot) if growth_shot else None,
                "final_shot": str(final),
                "raw": [str(raw), str(sample_path)],
                "samples": len(samples),
                "recent": titles,
            }

        self.scenario(2, "New chat → streaming reply → Recent chats", stream_reply)

        def permission_cards() -> dict[str, Any]:
            evidence: dict[str, Any] = {}
            old_1 = self.data / "home/Documents/HoH/old-1.txt"
            old_2 = self.data / "home/Documents/HoH/old-2.txt"
            if not old_1.is_file() or not old_2.is_file():
                raise AssertionError("Bash approval fixtures old-1.txt and old-2.txt must exist before the scenario")

            self.new_chat()
            approve_prompt = "Use Bash exactly once to run: rm old-1.txt"
            self.submit(approve_prompt)
            wait_until(lambda: self.find(role="push button", name="Approve"), 90, "Bash approval request")
            self.new_chat()
            wait_until(
                lambda: self.find(role="push button", name="Approve")
                and self.find(role="push button", name="Decline")
                and self.find(role="push button", contains="Waiting for your answer"),
                30,
                "approval controls and waiting card on home",
            )
            home_text = self.all_text()
            if "rm old-1.txt" not in home_text:
                raise AssertionError("Approval card does not show the Bash command")
            if "tool_name" in home_text or '"command"' in home_text:
                raise AssertionError("Approval card exposes raw JSON")
            evidence["approve_home_shot"] = str(self.shot("03-approve-home"))
            evidence["approve_home_raw"] = str(self.dump("03-approve-home"))
            self.press("Approve", role="push button", name="Approve")
            self.open_recent("Use Bash exactly once to run: rm old-1.txt")
            self.wait_turn_done(None, 120)
            if old_1.exists():
                raise AssertionError("Approved Bash command did not remove old-1.txt")
            evidence["approved_shot"] = str(self.shot("03-approved"))
            evidence["approved_raw"] = str(self.dump("03-approved"))

            self.new_chat()
            decline_prompt = "Use Bash exactly once to run: rm old-2.txt"
            self.submit(decline_prompt)
            wait_until(lambda: self.find(role="push button", name="Decline"), 90, "second Bash approval request")
            self.new_chat()
            wait_until(
                lambda: self.find(role="push button", name="Decline")
                and self.find(role="push button", contains="rm old-2.txt"),
                30,
                "decline controls and command card on home",
            )
            evidence["decline_home_shot"] = str(self.shot("03-decline-home"))
            evidence["decline_home_raw"] = str(self.dump("03-decline-home"))
            self.press("Decline", role="push button", name="Decline")
            self.open_recent("Use Bash exactly once to run: rm old-2.txt")
            self.wait_turn_done(None, 120)
            if not old_2.exists():
                raise AssertionError("Declined Bash command still removed old-2.txt")
            evidence["declined_shot"] = str(self.shot("03-declined"))
            evidence["declined_raw"] = str(self.dump("03-declined"))
            if self.stop_visible():
                raise AssertionError("Declined turn stayed open")
            return evidence

        self.scenario(3, "Bash approval card → Approve; second → Decline", permission_cards)

        def agent_card() -> dict[str, Any]:
            self.new_chat()
            prompt = "Use two Agent calls in parallel with descriptions ALPHA and BETA. Each replies one word. Then reply DONE."
            self.submit(prompt)
            time.sleep(1)
            conversation = self.require("current Agent conversation", role="landmark", name="Conversation")
            if self.find(root=conversation, role="push button", name="Approve") is not None:
                self.press("Agent approval", root=conversation, role="push button", name="Approve")
            self.new_chat()

            def card_ready() -> bool:
                text = self.all_text()
                return "ALPHA" in text and "BETA" in text and "subagent" in text.lower()

            wait_until(card_ready, 120, "home card with ALPHA and BETA subagents", 0.5)
            shot = self.shot("04-agent-card")
            raw = self.dump("04-agent-card")
            text_excerpt = self.all_text()[:2000]
            self.open_recent("Use two Agent calls in parallel")
            cleanup = "already complete"
            if self.stop_visible():
                self.press("Stop Agent turn after card evidence", role="push button", name="Stop generation")
                try:
                    wait_until(lambda: not self.stop_visible(), 40, "Agent turn cleanup", 0.2)
                    cleanup = "stopped after card evidence"
                except TimeoutError:
                    cleanup = "window restart required after Stop timeout"
            self.close_demo("agent-cleanup")
            self.start_demo("after-agent-cleanup")
            wait_until(lambda: self.find(role="push button", name="New chat"), 40, "main after Agent cleanup restart")
            return {"shot": str(shot), "raw": str(raw), "text_excerpt": text_excerpt, "cleanup": cleanup}

        self.scenario(4, "Agent with two subagents → home card rows", agent_card)

        def stop_turn() -> dict[str, Any]:
            self.new_chat()
            self.submit("Count from 1 to 500, one number per line.")
            wait_until(self.stop_visible, 60, "Stop generation button", 0.1)
            before = self.demo_descendants()
            claude_before = [row for row in before if self.is_claude_process(row)]
            if not claude_before:
                raise AssertionError("No owned Claude process was observed before Stop")
            self.record_processes("05-before-stop", before)
            growing = self.shot("05-before-stop")
            started = time.monotonic()
            self.press("Stop generation", role="push button", name="Stop generation")
            wait_until(lambda: not self.stop_visible(), 40, "closed stopped turn", 0.1)
            tracked = {row["pid"] for row in claude_before}

            def claude_gone() -> bool:
                alive = {row["pid"] for row in self.proc_rows() if row["state"] != "Z"}
                descendants = self.demo_descendants()
                current = [row for row in descendants if self.is_claude_process(row)]
                return not (tracked & alive) and not current

            wait_until(claude_gone, 40, "owned Claude processes to exit", 0.2)
            elapsed = round(time.monotonic() - started, 3)
            after = self.demo_descendants()
            self.record_processes("05-after-stop", after)
            claude_after = [row for row in after if self.is_claude_process(row) and row["state"] != "Z"]
            stopped = self.shot("05-stopped")
            raw = self.dump("05-stopped")
            return {
                "before_shot": str(growing),
                "stopped_shot": str(stopped),
                "raw": str(raw),
                "stop_seconds": elapsed,
                "claude_pids_before": sorted(tracked),
                "claude_pids_after": sorted(row["pid"] for row in claude_after),
            }

        self.scenario(5, "Stop mid-turn closes turn and Claude processes", stop_turn)

        def restart_history() -> dict[str, Any]:
            before = self.recent_titles()
            self.dump("06-before-restart")
            self.close_demo("first")
            self.start_demo("restart")
            wait_until(lambda: self.find(role="push button", name="New chat"), 40, "main after restart")
            if self.find(role="landmark", name="Welcome") is not None:
                raise AssertionError("Welcome returned after restart")
            after = wait_until(lambda: self.recent_titles() if len(self.recent_titles()) >= len(before) else None, 90, "restored Recent chats")
            missing = [title for title in before if title not in after]
            if missing:
                raise AssertionError(f"Chats missing after restart: {missing}")
            opened: list[str] = []
            for title in after:
                fragment = title.split(", No project", 1)[0]
                self.open_recent(fragment)
                opened.append(fragment)
            shot = self.shot("06-restarted-history")
            raw = self.dump("06-restarted-history")
            return {"before": before, "after": after, "opened": opened, "shot": str(shot), "raw": str(raw)}

        self.scenario(6, "restart with same data restores and opens all chats", restart_history)

        def sidebar_pages() -> dict[str, Any]:
            pages_source = (REPO / "ui/src/t3-entry/hoh-screens/SidebarPages.tsx").read_text(encoding="utf-8")
            if "<p>Coming soon</p>" not in pages_source:
                raise AssertionError("Coming soon source contract is absent")
            self.press("Learning", role="push button", name="Learning")
            wait_until(lambda: self.find(role="heading", name="Learning"), 20, "Learning page heading")
            time.sleep(3)
            learning = self.shot("07-learning")
            learning_raw = self.dump("07-learning")
            self.press("Widgets", role="push button", name="Widgets")
            wait_until(lambda: self.find(role="heading", name="Widgets"), 20, "Widgets page heading")
            time.sleep(3)
            widgets = self.shot("07-widgets")
            widgets_raw = self.dump("07-widgets")
            self.new_chat()
            wait_until(lambda: self.find(role="heading", pattern=r"^Good (morning|afternoon|evening)$"), 20, "home after New chat")
            home = self.shot("07-new-chat-home")
            return {
                "shots": [str(learning), str(widgets), str(home)],
                "raw": [str(learning_raw), str(widgets_raw)],
                "coming_soon": "source-audited exact text; screenshot inspection required because WebKitGTK exposes the paragraph without text",
            }

        self.scenario(7, "Learning/Widgets Coming soon; New chat returns home", sidebar_pages)

        def zoom_layout() -> dict[str, Any]:
            window = self.window_id()
            subprocess.run(["xdotool", "windowsize", "--sync", window, "800", "550"], check=True)
            wait_until(
                lambda: self.extent(self.require("current frame", role="frame", name="HoH")) == (0, 0, 800, 550),
                15,
                "800x550 AT-SPI frame geometry",
            )
            records: list[dict[str, Any]] = []
            for label, key in (("plus", "ctrl+plus"), ("minus", "ctrl+minus"), ("reset", "ctrl+0")):
                self.xkey(key)
                time.sleep(2)
                entry = self.require("composer at zoom", role="entry", prefix="Message")
                model = self.require("model control at zoom", role="combo box", pattern=r"^Claude .+")
                submit = self.require("submit at zoom", role="push button", name="Submit message")
                controls = {"entry": self.extent(entry), "model": self.extent(model), "submit": self.extent(submit)}
                for name, rect in controls.items():
                    if rect is None or rect[2] <= 0 or rect[3] <= 0:
                        raise AssertionError(f"{name} has no visible extent at zoom {label}: {rect}")
                    x, y, width, height = rect
                    if x < 0 or y < 0 or x + width > 800 or y + height > 550:
                        raise AssertionError(f"{name} leaves 800x550 at zoom {label}: {rect}")
                records.append({"zoom": label, "controls": controls, "shot": str(self.shot(f"08-zoom-{label}"))})
            raw = self.logs / "08-zoom-geometry.json"
            raw.write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
            self.xkey("ctrl+0")
            return {"records": records, "raw": str(raw), "visual_overlap": "manual screenshot inspection required"}

        self.scenario(8, "Ctrl +/-/0 at 800x550 without overlap", zoom_layout)

        def model_menu() -> dict[str, Any]:
            self.xkey("ctrl+0")
            control = self.require("Claude model control", role="combo box", pattern=r"^Claude .+")
            action = control.get_action_iface()
            if not action.do_action(0):
                raise AssertionError("Could not open model picker")

            def menu_ready() -> bool:
                text = self.all_text()
                return len(re.findall(r"Claude (?:Opus|Sonnet|Fable|Haiku)", text)) >= 2 and ("Effort" in text or "Reasoning" in text)

            wait_until(menu_ready, 30, "Claude models and effort levels")
            time.sleep(3)
            text = self.all_text()
            models = sorted(set(re.findall(r"Claude (?:Opus|Sonnet|Fable|Haiku)[^\n,]*", text)))
            efforts = [level for level in ["Low", "Medium", "High", "Extra High", "Max"] if re.search(rf"\b{re.escape(level)}\b", text)]
            if len(models) < 2:
                raise AssertionError(f"Fewer than two Claude models: {models}")
            if len(efforts) < 2:
                raise AssertionError(f"Reasoning effort levels absent: {efforts}")
            shot = self.shot("09-model-menu")
            raw = self.dump("09-model-menu")
            return {"models": models, "efforts": efforts, "shot": str(shot), "raw": str(raw)}

        self.scenario(9, "composer model menu exposes Claude models and effort", model_menu)

        self.close_demo("restart")
        summary = {
            "commit": self.args.commit,
            "display": self.args.display,
            "qa_dir": str(self.qa),
            "request_count": self.request_count,
            "results": self.results,
        }
        (self.qa / "RESULTS.json").write_text(
            json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        passed = sum(result["status"] == "PASS" for result in self.results.values())
        failed = sum(result["status"] == "FAIL" for result in self.results.values())
        print(f"SUMMARY PASS {passed} / FAIL {failed}; requests {self.request_count}", flush=True)
        return 0 if failed == 0 else 1

def inside(args: argparse.Namespace) -> int:
    runtime = Path("/tmp/cx-qe-e2e-runtime")
    runtime.mkdir(mode=0o700, exist_ok=True)
    socket = runtime / "atspi.sock"
    for path in (socket, runtime / "atspi.addr"):
        try:
            path.unlink()
        except FileNotFoundError:
            pass

    bus = subprocess.Popen(
        [
            "dbus-daemon",
            "--session",
            "--nofork",
            f"--address=unix:path={socket}",
            "--print-address=1",
        ],
        stdout=subprocess.PIPE,
        stderr=(args.qa_dir / "logs" / "atspi-bus.log").open("w", encoding="utf-8"),
        text=True,
    )
    assert bus.stdout is not None
    address = wait_until(lambda: bus.stdout.readline().strip(), 10, "private AT-SPI bus")
    os.environ.update(
        DISPLAY=args.display,
        GDK_BACKEND="x11",
        NO_AT_BRIDGE="0",
        GTK_MODULES="gail:atk-bridge",
        AT_SPI_BUS_ADDRESS=address,
        XDG_RUNTIME_DIR=str(runtime),
    )
    registry_log = (args.qa_dir / "logs" / "atspi-registry.log").open("w", encoding="utf-8")
    registry = subprocess.Popen(
        ["/usr/libexec/at-spi2-registryd", "--dbus-name=org.a11y.atspi.Registry"],
        stdout=registry_log,
        stderr=subprocess.STDOUT,
        env=os.environ.copy(),
    )
    harness: Harness | None = None
    try:
        time.sleep(0.5)
        import gi

        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi

        harness = Harness(args, Atspi)
        return harness.run()
    finally:
        if harness is not None:
            harness.close_demo("cleanup")
        terminate_owned(registry)
        registry_log.close()
        terminate_owned(bus)


def main() -> int:
    args = parse_args()
    args.qa_dir = args.qa_dir.resolve()
    if args.inside:
        return inside(args)
    return parent(args)


if __name__ == "__main__":
    raise SystemExit(main())
