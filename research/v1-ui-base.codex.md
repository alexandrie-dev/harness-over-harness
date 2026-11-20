**I recommend dsh’s window as the starting point, with T3’s visual treatment as a design reference.** This is an engineering judgment—**not checked** by implementation or performance measurement. The main reason is that dsh already exposes most required capabilities through its GUI protocol; adapting T3 would also require translating its coding-oriented application model.

Read-only inspection only: no network, installs, builds, or checks. Requirements came first from [v1-requirements.md](/home/save/Alexandrie/HarnessOverHarness/plans/briefs/v1-requirements.md:18).

Below, `T/` means `/mnt/toshiba/alexandrie/hoh/refs/t3code/`; `D/` means `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/`. References use `path:line`.

**1. What T3’s window contains**

T3 uses TypeScript, React, TanStack Router, Vite/Vite Plus, and Electron. The renderer starts at `T/apps/web/src/main.tsx:1`; build integration is at `apps/web/vite.config.ts:5`. Electron creates the sandboxed window with an isolated preload and enabled webviews at `apps/desktop/src/window/DesktopWindow.ts:476`.

State combines Effect atoms (`apps/web/src/rpc/atomRegistry.ts:1`) and Zustand viewing state (`apps/web/src/browser/browserSurfaceStore.ts:1`). The renderer uses typed Effect RPC over JSON WebSockets: `packages/client-runtime/src/rpc/protocol.ts:1`, `rpc/session.ts:245`. Electron launches the backend process at `apps/desktop/src/backend/DesktopBackendManager.ts:480`.

Harness processes sit behind server adapters, rather than being driven directly by React. Codex uses its app-server client and a spawned process (`apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts:85,1594`); Claude uses its Agent SDK/CLI (`Adapters/ClaudeAdapterV2.ts:61,255`).

The three requested qualities:

- **Preview:** file rendering dispatches Markdown, Mermaid diagrams, Office documents, PDF, HTML, and images (`apps/web/src/components/files/AttachmentFilePreview.tsx:303`). Browser previews use hosted Electron webviews (`apps/web/src/browser/ElectronBrowserHost.tsx:35`). Thus preview is several subsystems, not one reusable component. Arbitrary chart-format coverage: **not checked**.
- **Schedules:** an actual server service, persisted task records, and settings UI. Timing supports one-shot, intervals, fixed local times/weekdays, and webhook triggers (`apps/server/src/scheduledTasks/Schedule.ts:13`; `ScheduledTaskService.ts:409`; `apps/web/src/components/settings/ScheduledTasksSettings.tsx:17`). Scheduling depends on T3 thread/project launch semantics.
- **Look/speed:** Tailwind, Base UI, Lucide, Inter, configurable themes; virtualized LegendList timelines, memoized components, and deferred imports (`apps/web/package.json:17`; `src/index.css:1`; `src/main.tsx:4,28`; `components/chat/MessagesTimeline.tsx:49,343`). Actual latency, memory, and comparative speed: **not checked**.

Coding coupling reaches the main chat: terminals and PR panels (`components/ChatView.tsx:124,196,252`), worktree selection inside scheduling (`settings/ScheduledTasksSettings.tsx:80`), and project scripts that execute shell commands (`packages/contracts/src/t3ProjectFile.ts:25`). **Proposed removal—not checked:** expose employees/tasks/results instead; remove git, worktree, PR-review, and terminal controls while retaining engine execution underneath.

**2. Measured source and dependency size**

Counts are physical lines, including comments/blanks, in tracked source-shaped TS/TSX/JS/CSS/HTML files under `src`; tests/specs excluded. These are source inventories, not bundle sizes.

| Scope | Files | Lines |
|---|---:|---:|
| T3 `apps/web/src` | 1,407 | 320,414 |
| T3 `apps/desktop/src` | 170 | 46,218 |
| T3 `packages/client-runtime/src` | 252 | 55,934 |
| dsh `apps/web/src` | 4 | 67 |
| dsh `apps/desktop/src` + `desktop-host/src` | 96 | 12,195 |
| dsh `packages/client/*/src` | 1,456 | 218,507 |
| dsh `packages/api/*/src` | 109 | 24,531 |
| dsh `packages/typert/*/src` | 25 | 10,886 |
| dsh `packages/sdk/*/src` | 15 | 3,046 |

T3 web declares **78 dependencies: 72 external + six workspaces**, and **24 devDependencies** (`T/apps/web/package.json:16,96`). Following lockfile dependencies, optional dependencies, aliases, and workspace links yields **686 external resolved instances: 72 direct + 614 transitive-only**. Including development declarations throughout that closure: **1,141 instances**. Evidence: `T/pnpm-lock.yaml:1039,13087`.

dsh web declares **one dependency + 47 devDependencies**, comprising **15 external and 33 workspace entries** (`D/apps/web/package.json:33,82`). The equivalent all-declaration closure is **984 external instances: 15 direct + 969 transitive-only** (`D/pnpm-lock.yaml:1019,27978`). dsh puts bundled client requirements in devDependencies; its production-only web manifest therefore does **not** describe renderer dependency exposure.

Instances distinguish versions/peer contexts; optional platform packages are included. Neither count measures shipped or executed packages.

**3. What dsh already provides**

dsh also uses TypeScript/React/Vite inside Electron (`D/apps/web/package.json:59`; `apps/desktop/package.json:76`). Electron launches a separate Node host (`apps/desktop/src/host-process.ts:227`); that host boots the desktop profile and loopback web application (`apps/desktop-host/src/index.ts:45`).

Cordis composes client plugins and slots. Zustand/Immer snapshot stores hold interaction state (`packages/client/store/src/index.ts:1`); session objects and projections come through API controllers. Styling uses CSS modules, theme tokens, font stacks, radii, and motion curves (`packages/client/ui-theme/src/styles/base.css:6`). Frame-batched notifications and a virtualized turn navigator provide concrete performance mechanisms (`packages/api/session-controller/src/client/sessions/notifier.ts:39`; `packages/client/ui-chat/src/client/chat/TurnNavigator.tsx:110`). Measured speed: **not checked**.

Existing surfaces include:

- Sessions/agent control: `packages/api/session-controller/src/index.ts:135`.
- Preset roster, selection, composition viewer, Creator entry: `packages/client/ui-agent-preset/src/client/AgentPresetSection.tsx:60`.
- Skill discovery: `ui-skill/src/client/index.ts:115`; slash commands: `ui-commands/src/client/index.ts:51`.
- Plan cards/review: `ui-plan/src/client/index.ts:1`; questions: `ui-user-questions/src/client/index.ts:1`.
- Background-job observation/kill: `ui-jobs/src/client/index.ts:33`.
- Approval requests: `ui-approval/src/client/index.ts:107`; attachments: `ui-attachment/src/client/index.ts:20`.
- Schedule catalog/create/edit/run/history: `ui-schedule/src/client/index.ts:125`; durable absolute/interval/cron rules: `packages/schedule/schedule/src/types.ts:32,46,100`.
- Document previews: text, Markdown, HTML, images, PDF, Office, spreadsheets (`ui-sidebar-documentpreview/src/client/index.ts:124`). HTML uses sandboxed frames (`html/HtmlBody.tsx:68,102`); browser previews have iframe and Electron implementations.

Its GUI protocol is **Typert Remote**, with generated host/client artifacts (`packages/typert/generator/src/emitter.ts:33`), API controllers, unary Connection requests, and Gateway WebSocket streams (`packages/api/gateway/src/index.ts:1`). The SDK is separate: newline-delimited JSON-RPC 2.0, prompt/query/resume/wait and approval callbacks (`packages/sdk/protocol/src/transport.ts:1`; `types.ts:209`). SDK compatibility alone does not establish GUI compatibility. HoH’s complete Typert compatibility: **not checked**.

**4. Fit to founder workflows**

| Workflow | T3 | dsh | New work—**not checked** |
|---|---|---|---|
| Virtual employee: role/rules/skills/run/schedule | Provider skills, threads, schedules | Presets, skills, Creator, sessions, schedules | Employee editor, explicit skill assignment, employee-linked runs |
| Own buttons/commands | Script editor/menu; shell-backed (`ProjectScriptsControl.tsx:18`) | Command registry and slash UI | No-code button authoring and placement |
| “Task is a plugin” | Full guided flow **not checked** | “Create plugin” opens Creator (`ui-agent-preset/src/client/CreatePluginMenuItem.tsx:39`) | Enforced questions → example → approval → reusable release |
| Preview | Extensive file/browser surfaces | Extensive document/browser surfaces | Consistent task-result presentation |
| Schedule | Real task service | Real task manager and engine rules | Employee/task-centric language and targeting |

dsh already has a “Show coding view” preference (`ui-settings-general/src/client/locales.ts:127`). **Proposed— not checked:** remove developer-facing trajectory/diffs/terminals and experimental worktrees from the ordinary-user composition.

**5. Options and recommendation**

All implementation ranges below are estimates—**not checked**; copied inventories are upper bounds, not required minimums.

| Option | Copy | Write/change; protocol | Risks |
|---|---|---|---|
| A: T3 | Up to 1,829 files/422,566 lines above | **Not checked:** 60–100 files/12–25k lines; T3 Effect-RPC compatibility adapter, or replace client-runtime with HoH APIs | Deep thread/project/git coupling; preview depends on desktop/server facilities |
| B: dsh | Up to 1,556 client/app files/230,769 lines; API/Typert separately | **Not checked:** 35–65 files/7–14k lines; retain Typert GUI contract above HoH | Broad generated API surface; plugin runtime coupling; Linux packaging needs investigation |
| C: new | Selected assets/components only | **Not checked:** 80–140 files/18–35k lines; versioned HoH API with explicit streaming/approval/schedule extensions | Rebuild interaction polish; GPUI document embedding or Tauri/Leptos integration uncertainty |

Earlier Rust alternatives are documented at `research/desktop-ui-rust.md:26`; their implementation suitability remains **not checked** here.

For B, first five single-writer packages, each **proposed/not checked**:

1. **Protocol inventory and adapter slice:** session open/prompt/stream/cancel/approval; 5–8 files, 800–1,500 lines.
2. **Ordinary-user shell:** employees/tasks/results navigation, coding surfaces removed; 5–8 files, 700–1,200 lines.
3. **Employee editor:** role, rules, skill selection, saved preset; 6–10 files, 1–2k lines.
4. **Task creation wizard:** questions, example preview, explicit approval, reusable command; 6–10 files, 1–2k lines.
5. **Release trust boundary:** reviewed dependency snapshot, denied install scripts, signed artifacts/updates; 5–8 files, 600–1,200 lines.

Supply-chain protection is a required work package: dsh currently references an external spreadsheet tarball (`ui-sidebar-documentpreview/package.json:90`). Whether either repository already satisfies HoH’s complete security scheme is **not checked**.