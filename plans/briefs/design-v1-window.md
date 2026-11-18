# Design: the HoH window for the first version

Working directory `/home/save/Alexandrie/HarnessOverHarness`. Write exactly one file, `docs/window.md` (in
Russian, plain words, no invented terms); change nothing else, install nothing, run no builds. You may read
anything in the repository and under `/mnt/toshiba/alexandrie/hoh/refs/`, and search the web for current
facts about libraries (cite URL and date).

## What is decided (18 November 2026, by the founder)

Read `plans/vision-1118.md` (his own words) and `plans/briefs/v1-requirements.md`. Then the three research
reports written today: `research/v1-ui-base.codex.md` (T3 Code's and dsh's windows, what each already has),
`research/supply-chain-ts.md` (the protection scheme and its dependency ceiling), `research/v1-engine-scope.codex.md`
(what the engine will have and what is new). Earlier: `research/desktop-ui-rust.md`, `PLAN.md`.

The founder chose: the engine is ours, in Rust (a port of DeepSeek Harness, in progress); the window is **new**,
with few dependencies, because security is to be shown as a feature. He likes T3 Code's look and speed, its
preview of results inside the window and its schedules; the product is for ordinary people (a seller of IT
equipment who watches tenders, reads documents, tracks a GPU price), it must be easy to learn, and it must
have a convenient way to create virtual employees, the user's own buttons and commands, own roles and rules,
and tools the model writes on request ("a task is a plugin").

## What to design

1. **Stack.** Tauri 2 shell is kept. Decide the interface layer between two candidates and say why:
   (a) Rust compiled to WebAssembly (Leptos), no npm packages at all; (b) TypeScript with a small audited set
   of packages inside the ceiling of `research/supply-chain-ts.md`. For each: the exact list of direct
   dependencies you would allow and the count of transitive ones (from their published manifests — check, do
   not guess), what each buys, what we would write ourselves instead (router, state, virtualized list,
   Markdown rendering, charts, PDF and Office preview), and how the look and speed of T3 Code is reached
   (name what in T3 Code produces it: `research/v1-ui-base.codex.md` section 1, and look at
   `/mnt/toshiba/alexandrie/hoh/refs/t3code/apps/web/src/index.css` and the timeline component).
   Give one recommendation.
2. **Screens**, for an ordinary user, as a list with a text sketch of each (what is on it, what the buttons
   do): first launch; the single chat; employees (list, create, edit: name, role, rules, skills, memory,
   schedule); tasks ("New task": describe → the agent asks → shows an example → "yes" or a correction → the
   task becomes a reusable skill with its own button); results with preview (page, picture, chart, document);
   schedule; approvals of risky actions; settings (model, security). What an ordinary user never sees (git,
   terminals, diffs, worktrees).
3. **The founder's acceptance example, step by step through those screens**: a tool that analyses how to talk
   to a person from his public Telegram account, and a virtual employee "psychoanalyst" that uses it — every
   click and every screen, what the engine does at each step, where the user says "yes".
4. **The protocol between window and engine.** Start from dsh's SDK protocol (newline-delimited JSON-RPC 2.0,
   `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/sdk/protocol/src/types.ts`), which the engine
   keeps. List the methods and notifications the window needs that the SDK lacks (employees, tasks, schedules,
   reading a result for preview, custom buttons), each with its parameters and result, and say which engine
   package would serve it. Approvals of dangerous actions must not be confirmable by interface code alone
   (`research/supply-chain-ts.md`, the Tauri section): design that path.
5. **Security the user can see**: what the window shows about where data goes and what an employee is allowed
   to do, in the words of an ordinary user.
6. **Work packages**: the first eight, each small enough for one writer, with files and a check that proves
   it done; mark which can start before the engine can hold a session (against a recorded session of dsh
   replayed by a stub) and which need the engine.
7. Open questions for the founder: at most five, each with your recommended answer.

At most 3500 words. Mark everything you did not verify as "не проверено".
