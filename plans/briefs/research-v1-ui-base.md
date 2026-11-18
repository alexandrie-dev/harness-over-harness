# Research: what the HoH window is built from

Working directory `/home/save/Alexandrie/HarnessOverHarness`. You only read and you write nothing; your answer
is your final message. No network, no installs, no builds.

Read `plans/briefs/v1-requirements.md` first: it says what the founder wants from the first version. The
interface may now be TypeScript. Three candidate bases are on disk under `/mnt/toshiba/alexandrie/hoh/refs/`:

- `t3code` (MIT) — the app the founder likes;
- `deepseek-harness` (MIT) — its own window and client: `apps/web`, `apps/desktop`, `apps/desktop-host`,
  `packages/client/*`, `packages/api/*`, `packages/bundle/web-app`, and the client generator `typert`;
- writing a new interface (earlier research: `research/desktop-ui-rust.md`, Tauri 2 + Leptos or GPUI).

Establish, from the code, with paths and line numbers:

1. T3 Code: how the window is built (frameworks, state, how it talks to its server and to the harness
   processes), how many lines and how many direct and transitive npm dependencies the UI has
   (`package.json`, lockfile), and how the three things the founder named are made: the preview of results
   inside the window, schedules and timed tasks, and what gives the interface its look and speed. Which parts
   are tied to code work (git, worktrees, pull requests, terminals) and would be removed for ordinary users.
2. dsh's own window: the same questions; what it already has for sessions, agents, skills, commands, plans,
   jobs/schedules, approvals, attachments, previews; how it is bound to the engine (the Typert remote layer,
   `packages/api/*`, the SDK JSON-RPC protocol `packages/sdk/*`).
3. For each of: virtual employees (create, give a role and rules, give skills, run, schedule); the user's own
   buttons and commands; "a task is a plugin" (describe, be questioned, see an example, say yes); preview;
   schedule — what exists in T3 Code, what exists in dsh's window, what would be new.
4. Three options, each with what is copied, what is written, the size of the work in files and lines, and the
   risks: (A) take T3 Code's interface and connect it to our engine; (B) take dsh's window with our engine
   underneath (the engine is modelled on dsh, so the protocol already fits); (C) a new interface.
   Say which protocol between window and engine each option implies.
5. Your recommendation, and the first five work packages for it, each small enough for one writer.

Mark every statement you did not verify in the code as "not checked". At most 1500 words.
