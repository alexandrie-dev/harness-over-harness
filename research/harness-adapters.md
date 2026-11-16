# Как Rust-программе управлять чужими harness: разбор T3 Code, протоколов и ACP

Дата исследования: 2026-11-16. Только чтение источников; ничего не собиралось и не запускалось.

## 0. Источники и условные обозначения

Клоны (shallow) лежат в
`/tmp/claude-1000/-home-save-Alexandrie-HarnessOverHarness/a3276e7a-5aae-4a80-860b-e32609c6b177/scratchpad/refs/`.
Ссылка вида `t3code@1f698f01:path` означает файл в клоне на указанном коммите.

| Клон | Репозиторий | Коммит | Лицензия (файл LICENSE) |
|---|---|---|---|
| `t3code` | github.com/pingdotgg/t3code | `1f698f01` (2026-11-16) | MIT, © 2026 T3 Tools Inc. |
| `acp-spec` | github.com/agentclientprotocol/agent-client-protocol | `30f2eba` (2026-11-16) | Apache-2.0 |
| `acp-rust-sdk` | github.com/agentclientprotocol/rust-sdk | `f673a75` (2026-11-16) | Apache-2.0 |
| `codex` | github.com/openai/codex | `662a634` (2026-11-16) | Apache-2.0 |
| `claude-agent-sdk-python` | github.com/anthropics/claude-agent-sdk-python | `4720d36` (2026-11-13) | MIT + Commercial Terms (README) |
| `qwen-code` | github.com/QwenLM/qwen-code | `14d24a15` (2026-11-16) | Apache-2.0 |
| `pi` | github.com/earendil-works/pi | `747c217` (2026-11-16) | MIT |
| `deepseek-harness` | github.com/deepseek-ai/deepseek-harness | `39163dd3` (2026-11-13) | MIT |
| `vibe-kanban` | github.com/BloopAI/vibe-kanban | `3f095f0` (2026-11-15) | Apache-2.0 |

Клон `deepseek-harness` уже существовал в каталоге `refs` до моей работы (его сделал не я); я проверил
только `origin` и читал файлы.

Пометки: **[проверено]** — прочитано в первичном источнике (репозиторий или официальная документация);
**[вторично]** — только блоги и агрегаторы; **[не проверено]** — утверждение из памяти модели или не
найдено. Версии пакетов и имена моделей взяты из прочитанных файлов, а не из памяти.

## 1. T3 Code

### 1.1. Что это сегодня — и чем отличается от «быстрого поиска»

Предварительная картина («Node-сервер, Codex через app-server, Claude через Agent SDK, OpenCode»)
устарела по масштабу. На коммите `1f698f01` это монорепозиторий на 32 377 файлов с собственным
слоганом «The open-source control plane for coding agents» (`t3code@1f698f01:README.md`). Поддержаны
Claude Code, Codex, Cursor, OpenCode, Grok Build, Antigravity, Kiro, Pi, Muse Code, fx и «any ACP
agent» через ACP Registry. В документации T3 Code прямо описан запуск DeepSeek Harness:
«For DeepSeek Harness, use `dsh` with arguments `--profile` and `acp`»
(`t3code@1f698f01:docs/user/providers-acp.md`).

Следствие для нас: «взять T3 Code» означает взять не тонкую обёртку, а продукт с event sourcing,
мобильными клиентами, relay-сервисом и адаптерами по 9–15 тысяч строк каждый.

### 1.2. Структура репозитория

| Путь | Содержимое |
|---|---|
| `apps/server` | Node-сервер, npm-пакет `t3` (bin `t3`). Оркестрация, адаптеры, git, терминалы, auth, MCP |
| `apps/web` | Клиент: React, Vite, TanStack Router |
| `apps/desktop` | Electron 44.10.1, встраивает сервер |
| `apps/mobile` | Expo 58 / React Native 0.88.1 (iOS, Android) |
| `apps/marketing` | Сайт |
| `packages/contracts` | Схемы RPC и доменная модель (`rpc.ts` 2335 строк, `orchestrationV2.ts` 4616 строк) |
| `packages/provider-core` | Граница адаптера: `ProviderAdapter.ts` (827 строк), драйвер, auth flow, usage |
| `packages/provider-acp`, `provider-acp-registry` | Общий ACP-адаптер и работа с ACP Registry |
| `packages/provider-{pi,opencode,cursor,grok,kiro,muse,fx}` | Драйверы отдельных агентов |
| `packages/effect-acp`, `effect-codex-app-server` | Типизированные клиенты протоколов ACP и Codex app-server на Effect |
| `packages/client-runtime` | Общая логика соединения и состояния для web и mobile |
| `packages/source-control-*`, `secret-provider-*`, `ssh`, `tailscale` | Интеграции с forge, секретами, удалённым доступом |
| `native/resource-monitor` | Единственный Rust-код: мониторинг ресурсов на `sysinfo` 0.39 |
| `native/libghostty-vt` | C ABI терминального эмулятора для web (WASM) и Android |
| `infra/relay` | Облачный relay «T3 Connect» (Cloudflare Worker, Durable Objects) |

Адаптеры Claude и Codex ещё не вынесены в пакеты и лежат в
`apps/server/src/orchestration-v2/Adapters/` (`ClaudeAdapterV2.ts` — 14 530 строк,
`CodexAdapterV2.ts` — 8 930). Общий ACP-адаптер — 9 981 строка плюс `AcpSessionRuntime.ts` на 3 025.
Адаптер Pi — 4 225, OpenCode — 4 478. Это цена «честной» поддержки всех функций, а не минимального
запуска хода.

### 1.3. Разделение сервер/клиент и транспорт

Принцип: «Provider processes, terminals, Git, and project files belong to the server»
(`t3code@1f698f01:docs/internals/overview.md`). Клиент никогда не подставляет свою файловую систему
или учётные данные вместо серверных; desktop-приложение встраивает сервер, но его renderer
подчиняется той же границе.

- Транспорт: HTTP плюс WebSocket. RPC построен на `effect/rpc` (`RpcServer`), сериализация JSON
  (`RpcSerialization.layerJson`), `t3code@1f698f01:apps/server/src/ws.ts` строки 112, 3615–3650.
- Контракт: `packages/contracts/src/rpc.ts` — граница между независимо версионируемыми клиентами
  и сервером. Подписки отдают snapshot плюс курсор; клиент, смотрящий один тред, не платит за
  историю остальных.
- Командная модель: команда с `commandId` → оркестратор решает события → `EventSink` в одной
  транзакции SQLite пишет события, проекции, квитанцию команды и outbox-эффекты → `EffectWorker`
  выполняет побочные действия (вызовы провайдера) после фиксации намерения. Подтверждение команды
  означает «намерение записано», а не «провайдер закончил».

### 1.4. Интерфейс адаптера провайдера

Файл `t3code@1f698f01:packages/provider-core/src/server/ProviderAdapter.ts`. Адаптер — это сервис с
тремя методами; вся работа идёт через объект сессии.

```ts
// строки 811–827
export class ProviderAdapterV2 extends Context.Service<
  ProviderAdapterV2,
  {
    readonly instanceId: ProviderInstanceId;
    readonly driver: ProviderDriverKind;
    readonly getCapabilities: () => Effect.Effect<
      OrchestrationV2ProviderCapabilities,
      ProviderAdapterV2Error
    >;
    readonly planSelectionTransition: (
      input: ProviderSelectionTransitionInput,
    ) => Effect.Effect<ProviderSelectionTransitionPlan, ProviderAdapterV2Error>;
    readonly openSession: (
      input: ProviderAdapterV2OpenSessionInput,
    ) => Effect.Effect<ProviderAdapterV2SessionRuntime, ProviderAdapterV2Error, Scope.Scope>;
  }
>()("@t3tools/provider-core/server/ProviderAdapter/ProviderAdapterV2") {}
```

```ts
// строки 698–809, комментарии и часть необязательных методов опущены
export interface ProviderAdapterV2SessionRuntime {
  readonly instanceId: ProviderInstanceId;
  readonly driver: ProviderDriverKind;
  readonly providerSessionId: ProviderSessionId;
  readonly providerSession: OrchestrationV2ProviderSession;
  readonly canResumeAfterRestart?: boolean;
  readonly events: Stream.Stream<ProviderAdapterV2Event, ProviderAdapterV2Error>;
  readonly subscribeEvents?: Effect.Effect<ProviderAdapterV2EventSubscription>;
  readonly hasPendingBackgroundWork?: Effect.Effect<boolean>;
  readonly ensureThread: (input: ProviderAdapterV2EnsureThreadInput)
    => Effect.Effect<OrchestrationV2ProviderThread, ProviderAdapterV2Error>;
  readonly resumeThread: (input: { /* providerThread, threadId?, modelSelection?, runtimePolicy? */ })
    => Effect.Effect<OrchestrationV2ProviderThread, ProviderAdapterV2Error>;
  readonly injectHistory?: (input: /* … */) => Effect.Effect<boolean, ProviderAdapterV2Error>;
  readonly startTurn: (input: ProviderAdapterV2TurnInput) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly resumeTurn?: (input: ProviderAdapterV2ResumeTurnInput) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly compactThread?: (input: ProviderAdapterV2TurnInput) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly steerTurn: (input: ProviderAdapterV2SteerInput) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly applyModelSelection?: (input: /* … */) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly stopTask?: (input: /* providerThread, taskId */) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly interruptTurn: (input: ProviderAdapterV2InterruptInput) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly stopSubagent?: (input: /* … */) => Effect.Effect<void | ProviderAdapterV2SubagentOutcome, ProviderAdapterV2Error>;
  readonly unloadThread?: (input: /* … */) => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly respondToRuntimeRequest: (input: ProviderAdapterV2RuntimeRequestResponseInput)
    => Effect.Effect<void, ProviderAdapterV2Error>;
  readonly readThreadSnapshot: (input: ProviderAdapterV2ReadThreadSnapshotInput)
    => Effect.Effect<ProviderAdapterV2ThreadSnapshot, ProviderAdapterV2Error>;
  readonly mcpApps?: ProviderAdapterV2McpApps;
  readonly rollbackThread: (input: ProviderAdapterV2RollbackThreadInput)
    => Effect.Effect<ProviderAdapterV2ThreadSnapshot, ProviderAdapterV2Error>;
  readonly forkThread: (input: ProviderAdapterV2ForkThreadInput)
    => Effect.Effect<OrchestrationV2ProviderThread, ProviderAdapterV2Error>;
}
```

Политика выполнения, которую адаптер получает на каждый ход (строки 48–60):

```ts
export const ProviderAdapterV2RuntimePolicy = Schema.Struct({
  runtimeMode: RuntimeMode,
  interactionMode: ProviderInteractionMode,
  cwd: Schema.NullOr(Schema.String),
  approvalPolicy: Schema.optional(Schema.Unknown),
  sandboxPolicy: Schema.optional(Schema.Unknown),
  maxRuntimeMode: Schema.optional(RuntimeMode),
});
```

Правила, которые T3 Code считает обязательными для адаптера
(`t3code@1f698f01:docs/internals/adding-a-provider.md`, `providers.md`):

- «Orchestration and clients read capabilities, never the driver kind». Возможность, объявленная
  включённой и упавшая в runtime, считается ошибкой.
- Режимы разрешений предлагаются только те, которые провайдер соблюдает сам: «Do not imitate a
  missing mode by answering approvals in T3: T3's check is weaker than the agent's own enforcement».
- Идентификаторы вариантов approval и вопросов провайдера передаются без изменения: «a display
  label is not necessarily a valid reply».
- Driver и instance различаются: driver — интеграция, instance — конкретная конфигурация и учётная
  запись. Работа маршрутизируется по instance, окружение очищается от чужих учётных данных.
- Проверка статуса не должна открывать сессию: открытие может запустить MCP-серверы, hooks или
  браузер входа.
- Список «ловушек, на которых сломалось больше одного адаптера»: настройки, применённые до того,
  как агент их объявил; `default` как sentinel, а не модель; Stop, отправленный раньше, чем агент
  зарегистрировал prompt; фоновая работа и subagent, переживающие ход; незакрытые tool calls после
  сбоя; лимиты использования как отдельный класс ошибки; symlink-пути рабочего каталога.

### 1.5. Каноническая модель событий и тредов

Архитектурные инварианты (`t3code@1f698f01:docs/orchestration-v2/README.md`): «App ids are
primary. Provider ids are refs»; «Provider events are never rewritten to look like another provider
event»; «Child execution completion never closes the parent run»; «Missing provider capability is
represented explicitly and handled by policy». Модель спроектирована по результатам проб Codex
app-server: «Codex is treated as the richest protocol we currently have; weaker providers are
adapted into the same model with app-owned ids and explicit capability flags».

Мысленная схема из того же документа:

```text
AppThread
  Run 1
    root ExecutionNode
      tool ExecutionNode
      approval ExecutionNode
      subagent ExecutionNode
        ProviderThread
          child root ExecutionNode
  Run 2
    root ExecutionNode
```

В цитатах ниже имена и значения приведены как в исходнике; переносы строк уплотнены, пропуски
отмечены `/* … */`.

**События адаптера** (`ProviderAdapter.ts`, строки 83–207): объединение, в котором почти каждое
событие — upsert целой сущности, а не дельта.

```ts
export const ProviderAdapterV2Event = Schema.Union([
  Schema.Struct({ type: Schema.Literal("app_thread.created"), driver: ProviderDriverKind,
    appThread: OrchestrationV2AppThread, createOnly: Schema.optional(Schema.Boolean) }),
  Schema.Struct({ type: Schema.Literal("provider_session.updated"), driver: ProviderDriverKind,
    providerSession: OrchestrationV2ProviderSession }),
  Schema.Struct({ type: Schema.Literal("provider_thread.updated"), /* … */ providerThread: OrchestrationV2ProviderThread,
    pendingBackgroundWork: Schema.optional(Schema.Boolean), /* … */ }),
  Schema.Struct({ type: Schema.Literal("provider_turn.updated"), /* … */ providerTurn: OrchestrationV2ProviderTurn, /* … */ }),
  Schema.Struct({ type: Schema.Literal("node.updated"), driver: ProviderDriverKind, node: OrchestrationV2ExecutionNode }),
  Schema.Struct({ type: Schema.Literal("subagent.updated"), driver: ProviderDriverKind, subagent: OrchestrationV2Subagent }),
  Schema.Struct({ type: Schema.Literal("message.updated"), driver: ProviderDriverKind, message: OrchestrationV2ConversationMessage }),
  Schema.Struct({ type: Schema.Literal("turn_item.updated"), driver: ProviderDriverKind, turnItem: OrchestrationV2TurnItem, /* … */ }),
  Schema.Struct({ type: Schema.Literal("steer.updated"), /* threadId, runId, messageId */
    status: Schema.Literals(["pending", "completed", "cancelled"]) }),
  Schema.Struct({ type: Schema.Literal("background_work.updated"), /* providerTurnId */ pending: Schema.Boolean }),
  /** Unpersisted deltas go only to clients showing the row; final output arrives in turn_item.updated. */
  Schema.Struct({ type: Schema.Literal("command_output.delta"), /* threadId, itemId */ chunk: Schema.String }),
  Schema.Struct({ type: Schema.Literal("prompt_suggestion.updated"), /* threadId, runId */ text: Schema.String }),
  Schema.Struct({ type: Schema.Literal("runtime_request.updated"), /* … */ runtimeRequest: OrchestrationV2RuntimeRequest }),
  Schema.Struct({ type: Schema.Literal("plan.updated"), driver: ProviderDriverKind, plan: OrchestrationV2PlanArtifact }),
  Schema.Struct({ type: Schema.Literal("turn.terminal"), /* providerThreadId, providerTurnId, runOrdinal */
    status: Schema.Literals(["completed", "interrupted", "cancelled"]), failure: Schema.Null,
    threadDisposition: Schema.Literals(["reusable", "broken"]), /* … */ }),
  Schema.Struct({ type: Schema.Literal("turn.terminal"), /* … */ status: Schema.Literal("failed"),
    failure: OrchestrationV2ProviderFailure, retry: Schema.optional(OrchestrationV2ProviderRetry),
    threadDisposition: Schema.Literals(["reusable", "broken"]), /* … */ }),
]);
```

**Доменные типы** (`t3code@1f698f01:packages/contracts/src/orchestrationV2.ts`).

Ссылка на нативный идентификатор с оценкой надёжности (строки 155–165):

```ts
export const OrchestrationV2NativeRefStrength = Schema.Literals(["strong", "weak", "none"]);

export const OrchestrationV2ProviderRef = Schema.Struct({
  driver: ProviderDriverKind,
  nativeId: Schema.NullOr(TrimmedNonEmptyString),
  strength: OrchestrationV2NativeRefStrength,
  fingerprint: Schema.optional(TrimmedNonEmptyString),
  ordinal: Schema.optional(NonNegativeInt),
});
```

Статусы run (строки 638–649) и узел дерева выполнения (строки 795–833):

```ts
export const OrchestrationV2RunStatus = Schema.Literals([
  "preparing", "queued", "starting", "running", "waiting",
  "completed", "interrupted", "failed", "cancelled", "rolled_back",
]);

export const OrchestrationV2ExecutionNode = Schema.Struct({
  id: NodeId,
  threadId: ThreadId,
  runId: Schema.NullOr(RunId),
  parentNodeId: Schema.NullOr(NodeId),
  rootNodeId: NodeId,
  kind: Schema.Literals([
    "root_turn", "assistant_message", "reasoning", "plan", "todo_list", "tool_call",
    "approval_request", "user_input_request", "subagent", "hook", "system",
  ]),
  status: Schema.Literals([
    "idle", "pending", "running", "waiting", "completed",
    "interrupted", "failed", "cancelled", "rolled_back",
  ]),
  countsForRun: Schema.Boolean,
  providerThreadId: Schema.NullOr(ProviderThreadId),
  providerTurnId: Schema.NullOr(ProviderTurnId),
  nativeItemRef: Schema.NullOr(OrchestrationV2ProviderRef),
  runtimeRequestId: Schema.NullOr(RuntimeRequestId),
  checkpointScopeId: Schema.NullOr(CheckpointScopeId),
  startedAt: Schema.NullOr(Schema.DateTimeUtc),
  completedAt: Schema.NullOr(Schema.DateTimeUtc),
});
```

Запрос к пользователю, переживающий перезапуск (строки 1276–1300):

```ts
export const OrchestrationV2RuntimeRequest = Schema.Struct({
  id: RuntimeRequestId,
  nodeId: NodeId,
  providerTurnId: Schema.NullOr(ProviderTurnId),
  nativeRequestRef: Schema.NullOr(OrchestrationV2ProviderRef),
  kind: Schema.Union([
    ProviderRequestKind,
    Schema.Literals(["dynamic_tool_call", "user_input", "auth_refresh"]),
  ]),
  status: Schema.Literals(["pending", "resolved", "expired", "cancelled"]),
  responseCapability: Schema.Union([
    Schema.Struct({ type: Schema.Literal("live"), providerSessionId: ProviderSessionId,
      supportsFeedback: Schema.optional(Schema.Boolean) }),
    Schema.Struct({ type: Schema.Literal("message") }),
    Schema.Struct({ type: Schema.Literal("not_resumable"), reason: Schema.String }),
  ]),
  createdAt: Schema.DateTimeUtc,
  resolvedAt: Schema.NullOr(Schema.DateTimeUtc),
  decision: Schema.optional(ProviderApprovalDecision),
  feedback: Schema.optional(Schema.String),
  answers: Schema.optional(ProviderUserInputAnswers),
});
```

Элемент ленты `OrchestrationV2TurnItem` (строки 1704–1933) — объединение по полю `type` со
значениями: `notification`, `user_message`, `assistant_message`, `reasoning`, `proposed_plan`,
`todo_list`, `user_input_request`, `file_change`, `command_execution`, `file_search`, `web_search`,
`approval_request`, `checkpoint`, `run_interrupt_request`, `run_interrupt_result`, `system_notice`,
`error`, `compaction`, `handoff`, `fork`, `thread_created`, `secret_request`, `link_request`,
`subagent`, `dynamic_tool`. Общие поля (строки 1602–1629): `id`, `threadId`, `runId`, `nodeId`,
`providerThreadId`, `providerTurnId`, `nativeItemRef`, `parentItemId`, `ordinal`, `status`, `title`,
`startedAt`, `completedAt`, `updatedAt`.

Классы сбоя и возможности (строки 1549–1556 и 397–413):

```ts
export const OrchestrationV2ProviderFailureClass = Schema.Literals([
  "usage_limit", "provider_error", "transport_error",
  "permission_error", "validation_error", "unknown",
]);

export const OrchestrationV2ProviderCapabilities = Schema.Struct({
  sessions: OrchestrationV2SessionCapabilities,
  threads: OrchestrationV2ThreadCapabilities,
  turns: OrchestrationV2TurnCapabilities,
  streaming: OrchestrationV2StreamingCapabilities,
  tools: OrchestrationV2ToolCapabilities,
  approvals: OrchestrationV2ApprovalCapabilities,
  planning: OrchestrationV2PlanningCapabilities,
  subagents: OrchestrationV2SubagentCapabilities,
  context: OrchestrationV2ContextCapabilities,
  checkpointing: OrchestrationV2CheckpointCapabilities,
  identity: OrchestrationV2IdentityCapabilities,
  runtimePolicy: OrchestrationV2RuntimePolicyCapabilities.pipe(/* default: "client-boundary" */),
});
```

Примеры флагов: `turns.supportsActiveSteering`, `turns.supportsSteeringByInterruptRestart`,
`turns.terminalStatusQuality: "strong" | "weak" | "none"`, `threads.canRollbackThread`,
`threads.canForkFromTurn`, `approvals.approvalCallbacksAreLiveOnly`,
`sessions.pendingRequestsSurviveRestart`, `runtimePolicy.enforcement: "native" | "client-boundary"`.

Доменные события, которые пишутся в журнал (строки 1979–2129): `thread.created`, `run.created`,
`run.updated`, `run-attempt.created|updated`, `node.updated`, `subagent.updated`,
`provider-session.detached`, `provider-thread.updated`, `provider-turn.updated`,
`runtime-request.updated`, `message.updated`, `turn-item.updated`, `plan.updated`,
`checkpoint-scope.created`, `checkpoint.captured`, `checkpoint.rollback-requested`,
`context-handoff.updated`, `context-transfer.created|updated` и несколько служебных.

### 1.6. Хранение

- SQLite через встроенный `node:sqlite` и `effect/sql`
  (`t3code@1f698f01:apps/server/src/persistence/Sqlite.ts`), WAL, миграции, блокировка владения
  базой между сервером и CLI.
- Событийный журнал хранит около суток истории для повторного подключения и идемпотентных
  повторов; `ProjectionMaintenance` удаляет более старые события, поэтому проекции нельзя
  перестроить из журнала. Истина — проекции (`docs/internals/overview.md`).
- Квитанции команд остаются: повтор команды с тем же `commandId` не повторяет побочные эффекты.
- Сырые кадры провайдера — только диагностический ротируемый лог с бюджетом 64 KiB на запись, без
  токен-дельт (`docs/internals/providers.md`, раздел Provider diagnostics).
- Вложения хранятся вне рабочего каталога проекта.

### 1.7. Git worktrees и контрольные точки

- Стратегия рабочего каталога треда (`orchestrationV2.ts`, строки 702–718): `root` (каталог
  проекта), `existing_worktree`, `worktree` с `baseRef`, необязательной веткой и `startFromOrigin`.
- Новые worktree создаются в папке `worktrees` домашнего каталога T3; путь настраивается на машину.
  Есть правила именования веток, рекурсивная инициализация submodule, автоочистка по неактивности
  или после «settle», на macOS — быстрый checkout через APFS copy-on-write
  (`docs/user/project-settings.md`).
- `t3.json` в проекте задаёт скрипты, в том числе с `runOnWorktreeCreate`.
- Контрольные точки: «use hidden Git refs to capture workspace state without adding commits to the
  user's branch». Откат обязан согласовать файлы и разговор провайдера: «A provider that cannot
  roll back its conversation must reject that operation before changing the filesystem»
  (`docs/internals/overview.md`). Для ACP-агентов откат файлов работает, но следующий ход
  начинается в новой сессии агента без памяти о разговоре (`docs/user/providers-acp.md`).
- Diff считается по контрольным точкам, а не по событиям провайдера: Codex-адаптер при
  инициализации отказывается от `turn/diff/updated`.

### 1.8. Approvals и права

Четыре режима на тред (`docs/user/permission-modes.md`): **Supervised**, **Auto-accept edits**,
**Auto** (автоматический ревьюер самого провайдера), **Full access**. По умолчанию для новых
тредов — Full access. Администратор может ограничить потолок через managed policy
(`maxRuntimeMode`). Различия между провайдерами документированы явно: OpenCode и Antigravity не
имеют Auto и «fall back to asking»; Pi получает режимы через блокирующий tool hook, и это «not an
operating-system sandbox»; ACP-агенты работают в собственном режиме, а T3 только отвечает на их
запросы согласно режиму треда. Запросы subagent показываются в родительском разговоре.

Асинхронные вопросы Codex приходят как уведомления и получают ответ новым сообщением
пользователя (`responseCapability: { type: "message" }`); такие запросы переживают завершение
хода, выход процесса и перезапуск сервера (`docs/internals/providers.md`, Protocol traps).

### 1.9. Аутентификация с каждым провайдером

T3 Code заявляет: «T3 Code does not resell tokens. It drives the agents you are already signed in
to». Учётные данные остаются в хранилище самого агента.

| Провайдер | Как входит пользователь | Изоляция нескольких аккаунтов |
|---|---|---|
| Claude | `claude auth login` на машине сервера; T3 только запускает бинарь | `CLAUDE_CONFIG_DIR` на instance; API-ключ или роутер — через переменные окружения instance (`ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`) |
| Codex | Либо управляемый «Connect with ChatGPT» (T3 сам ставит Codex, вход идёт на стороне OpenAI с согласием «sharing of your ChatGPT plan»), либо существующий `codex login` | `CODEX_HOME` плюс «shadow home» со своим `auth.json` |
| OpenCode | Собственный вход OpenCode; пароль сервера `OPENCODE_SERVER_PASSWORD` | Внешний сервер остаётся внешним |
| Pi | `pi`, затем `/login`, либо ключи в окружении instance | `PI_CODING_AGENT_DIR` |
| ACP-агенты | Методы входа объявляет агент; browser-метод открывается на устройстве пользователя, terminal-метод — во встроенном терминале на сервере | «ACP does not describe account isolation» |
| Antigravity | Отдельный профиль на instance, файловое хранилище ключей, окружение очищается от Google-учёток | Да, через профиль |

Источники: `t3code@1f698f01:docs/user/providers-{claude,codex,opencode,pi,acp}.md`,
`docs/internals/providers.md`. Связь «Connect with ChatGPT» с программой OpenAI «Sign in with
ChatGPT» выглядит вероятной, но в коде не проверялась **[не проверено]**.

### 1.10. Удалённый и мобильный доступ

Модель: клиент подключается к «environment» (серверу) по HTTP и WebSocket; способ достижения не
меняет модель исполнения (`docs/internals/remote.md`).

- Маршруты: прямой LAN, Tailscale, SSH (desktop сам поднимает туннель и при необходимости
  сервер), T3 Connect. Сохранённое окружение держит упорядоченный список маршрутов; клиент
  подключается по первому рабочему и запоминает адреса, которые сообщает сервер.
- Аутентификация окружения (`docs/internals/environment-auth.md`): pairing выдаёт набор scope;
  cookie, bearer и DPoP-токены — адаптеры одной модели сессий; короткоживущие WebSocket-тикеты,
  чтобы долгие токены не попадали в URL.
- T3 Connect (`docs/internals/t3-connect.md`): облачная идентичность через Clerk; relay выдаёт
  одноразовый bootstrap-credential, привязанный к DPoP-ключу клиента, и после этого трафик идёт
  через tunnel-hostname окружения, а relay не проксирует сессии.
- Фоновый сервис слушает все IPv4-интерфейсы, но закрывает соединения не из loopback, частных или
  tailnet-адресов.
- Мобильные приложения — полноценные клиенты того же сервера (iOS и Android, Expo).
- Внешние агенты могут управлять T3 через его MCP-endpoint `/mcp` с OAuth
  (`docs/orchestration-v2/orchestrator-mcp-server.md`): создавать треды, делегировать задачи другому
  провайдеру, ждать результата, планировать повторяющуюся работу.

### 1.11. Что привязано к Node и Electron

| Привязка | Где | Замена в Rust |
|---|---|---|
| Effect (Layer, Schema, Stream, `effect/rpc`, `effect/sql`) | весь сервер и contracts | tokio, serde, собственный JSON-RPC или axum + WebSocket |
| `@anthropic-ai/claude-agent-sdk` `^0.3.321` | `ClaudeAdapterV2.ts` (`query()`, `canUseTool`, `spawnClaudeCodeProcess`) | Прямой протокол `claude -p --input-format stream-json` (см. 2.2) |
| `@cursor/sdk` 1.3.1, `@muse-code/sdk` 1.4.6 | драйверы Cursor и Muse | Rust-аналогов нет; путь через ACP |
| `effect-codex-app-server` (сгенерированные TS-типы) | Codex | crate `codex-app-server-protocol` или JSON Schema из `codex app-server generate-json-schema` |
| `effect-acp` | ACP | crate `agent-client-protocol` |
| `node:sqlite` | persistence | rusqlite или sqlx |
| `node-pty` `^1.2.0-beta.15` | терминалы | `portable-pty` |
| Electron 44 | desktop | Tauri либо локальный web UI, который отдаёт сам бинарь |
| Expo / React Native | mobile | вне первой поставки |
| Clerk, Cloudflare Workers, Durable Objects | T3 Connect | вне первой поставки |
| `playwright-core`, `transcribe-cpp`, `ffi-rs` | preview, голос, Win32 | вне первой поставки |

### 1.12. Что переносить в Rust, а что нет

Переносить как проект (переписывая, код TypeScript напрямую не годится):

1. Границу адаптера с объявлением возможностей и правилом «оркестрация не знает имени
   провайдера».
2. Модель сущностей: тред приложения, run, попытка, дерево узлов, нативные ссылки с `strength`,
   запрос к пользователю с `responseCapability`.
3. Команды с идемпотентными квитанциями, запись намерения до побочного эффекта, подписки
   «snapshot + cursor».
4. Тестирование воспроизведением: записанные транскрипты реального провайдера прогоняются через
   настоящий оркестратор; подменяется только транспорт. Набор общих сценариев: `simple`,
   `multi_turn`, `queued_turn`, `turn_interrupt`, `message_steering`, `provider_thread_resume`,
   `tool_call_*`.
5. Различие driver / instance и очистку окружения.
6. Контрольные точки в скрытых git refs и стратегии worktree.
7. Собственный MCP-сервер, внедряемый в каждого агента. Для нас это канал, через который
   «задача-плагин» отдаёт инструменты и память любому harness.
8. Список ловушек из 1.4 как чек-лист приёмки адаптера.

Не переносить в первую версию: мобильные клиенты, relay, интеграции с forge и pull request,
triage, preview-браузер, голос, автообновление чужих CLI, перенос контекста при смене провайдера
в середине треда, поддержку нескольких аккаунтов.

Лицензия MIT позволяет заимствовать и проект, и фрагменты с сохранением уведомления.

Существенная альтернатива для Rust-части: crate `executors` из Vibe Kanban (Apache-2.0) уже
содержит на Rust клиент протокола Claude stream-json, клиент Codex app-server и ACP-обвязку для
v1 и v2 (см. раздел 4). Проект объявил о закрытии, код остаётся доступным.

## 2. Управляющие поверхности harness на ноябрь 2026

### 2.0. Сводная таблица

| Harness | Лучшая поверхность | Запуск | Формат | Зрелость | Лицензия | Локальный endpoint |
|---|---|---|---|---|---|---|
| Codex | app-server | `codex app-server` (stdio) | JSON-RPC 2.0 без поля `jsonrpc`, JSONL | В CLI помечен `[experimental]`; схема привязана к версии | Apache-2.0 | Только Responses API |
| Claude Code | stream-json + control protocol | `claude -p --input-format stream-json --output-format stream-json --verbose` | JSONL | Флаги документированы; control-сообщения описаны только кодом SDK | Проприетарный бинарь; SDK под Commercial Terms | Anthropic Messages; не-Claude модели не поддерживаются вендором |
| OpenCode | HTTP API сервера или ACP | `opencode serve` / `opencode acp` | REST + поток событий / JSON-RPC stdio | API 2.x назван «Experimental HttpApi surface» | **[не проверено]** | OpenAI- и Anthropic-совместимые, автообнаружение vLLM |
| Qwen Code | ACP | `qwen --acp` | JSON-RPC stdio | ACP v1 стабилен; `qwen serve` экспериментален | Apache-2.0 | OpenAI- и Anthropic-совместимые |
| Gemini CLI | ACP | `gemini --acp` | JSON-RPC stdio | Продукт вытесняется Antigravity CLI | **[не проверено]** | Не найдено |
| Goose | ACP | `goose acp` / `goose serve` | JSON-RPC stdio / HTTP + WebSocket | ACP v1 | Apache-2.0 **[вторично]** | OpenAI-, Anthropic-, Ollama-совместимые |
| pi | RPC mode | `pi --mode rpc` | JSONL-команды и события | Документирован, версия зависит от `pi --help` | MIT | `openai-completions`, `openai-responses`, `anthropic-messages` |
| DeepSeek Harness | ACP | `dsh acp` | JSON-RPC stdio | «developer preview», обещаны ломающие изменения | MIT | Через pi-ai: те же три формата |

### 2.1. Codex

**app-server** **[проверено]**: `https://learn.chatgpt.com/docs/app-server` (адрес, на который
перенаправляет `developers.openai.com/codex/app-server`) и `codex@662a634`.

- Запуск: `codex app-server` (stdio по умолчанию), `--listen unix://` или `unix://PATH`,
  `--listen ws://IP:PORT` («experimental and unsupported»), `--listen off`
  (`codex@662a634:codex-rs/app-server/src/main.rs` строки 43–50).
- Кадры: JSONL на stdio; JSON-RPC 2.0, «the `"jsonrpc":"2.0"` header omitted on the wire».
- Рукопожатие: запрос `initialize` с `clientInfo`, затем уведомление `initialized`. Возможности
  клиента: `experimentalApi`, `optOutNotificationMethods` и другие.
- Методы: `thread/start`, `thread/resume`, `thread/fork`, `thread/read`, `thread/list`,
  `thread/archive`, `thread/compact/start`, `turn/start`, `turn/steer`, `turn/interrupt`,
  `review/start`, `model/list`, `command/exec`.
- Уведомления: `thread/started`, `thread/status/changed`, `turn/started`, `turn/completed`,
  `item/started`, `item/completed`, `item/agentMessage/delta`.
- Типы `ThreadItem`: `userMessage`, `agentMessage`, `plan`, `reasoning`, `commandExecution`,
  `fileChange`, `mcpToolCall`, `dynamicToolCall`, `collabToolCall`, `webSearch`, `imageView`,
  `enteredReviewMode`, `exitedReviewMode`, `contextCompaction`, `functionCallOutput`.
- Approvals — запросы сервера к клиенту: `item/commandExecution/requestApproval`,
  `item/fileChange/requestApproval`, `item/permissions/requestApproval`, `tool/requestUserInput`.
  Решения: `accept`, `acceptForSession`, `decline`, `cancel`,
  `{ "acceptWithExecpolicyAmendment": … }`. Закрытие запроса подтверждается
  `serverRequest/resolved`.
- Аутентификация: `account/read`, `account/login/start` с типами `apiKey`, `chatgpt`,
  `chatgptDeviceCode`, `chatgptAuthTokens` (экспериментальный), `account/logout`,
  `account/rateLimits/read`.
- Собственные инструменты хоста: `dynamicTools` в `thread/start`, запрос `item/tool/call`
  (экспериментально).
- Схема: `codex app-server generate-ts --out DIR`, `codex app-server generate-json-schema --out DIR`;
  результат соответствует конкретной версии Codex.
- Нестабильность видна по фактам: документация сообщает, что `thread/rollback` удалён; T3 Code
  использует `thread/revert`; DeepSeek Harness закрепляет «Codex app-server 0.153.4»
  (`deepseek-harness@39163dd3:packages/subagent/subagent-codex/src/wire.ts`), Vibe Kanban —
  тег `rust-v0.132.0`. Вывод: версию нужно закреплять.
- Rust: crate `codex-app-server-protocol` есть в рабочем пространстве Codex и подключается как
  git-зависимость по тегу (так делает Vibe Kanban). Публикация на crates.io **[не проверено]**.

**`codex exec --json`** **[проверено]**: неинтерактивный запуск, JSONL-события `thread.started`,
`turn.started`, `item.started`, `item.updated`, `item.completed`, `turn.completed`, `turn.failed`,
`error` (`codex@662a634:sdk/typescript/src/events.ts`; флаг `--json` с псевдонимом
`--experimental-json` в `codex-rs/exec/src/cli.rs`). TypeScript SDK `@openai/codex-sdk` — обёртка
над этим режимом. Нет интерактивных approvals и steering; подходит для пакетных задач.
Документация рекомендует SDK для автоматизации и CI, app-server — для глубокой интеграции.

**Режим MCP-сервера**: в `codex@662a634:codex-rs/cli/src/main.rs` подкоманды `mcp-server` нет;
`codex mcp` управляет внешними MCP-серверами (`list`, `get`, `add`, `remove`, `login`, `logout`).
Crate `mcp-server` в рабочем пространстве отсутствует. Когда режим удалён и есть ли он в
установленных у пользователей версиях — **[не проверено]**. Рассчитывать на него не следует.

**Условия использования** **[проверено]**, та же страница app-server:
«If you've built a local or open-source application using Codex app-server authentication, you can
continue using it»; «App-server authentication has never been permitted for commercial or hosted
services»; рекомендуется перейти на Sign in with ChatGPT (`https://developers.openai.com/siwc`,
лист ожидания партнёров `https://openai.com/form/sign-in-with-chatgpt-interest/`). Корпоративным
интеграциям предлагается связаться с OpenAI для включения в список известных клиентов и
заполнять `clientInfo.name`. Для коммерческого продукта вход через ChatGPT-подписку в Codex
без участия в этой программе — юридический риск. Вход по API-ключу этим ограничением не затронут.

**Локальная модель** **[проверено]**: `wire_api = "chat"` удалён —
`codex@662a634:codex-rs/model-provider-info/src/lib.rs` строка 110: «`wire_api = "chat"` is no
longer supported. How to fix: set `wire_api = "responses"`». Встроенные провайдеры Ollama и
LM Studio создаются с `WireApi::Responses`. Следовательно, локальный сервер обязан отдавать
`/v1/responses`. vLLM такой endpoint имеет (см. 2.9). Качество работы Qwen под промптами и
инструментами Codex **[не проверено]**.

Платформы: npm-пакеты `@openai/codex-linux-arm64`, `-darwin-arm64`, `-win32-x64`, `-win32-arm64`
(`codex@662a634:codex-cli/bin/codex.js`), то есть aarch64 Linux для DGX Spark покрыт.

### 2.2. Claude Code

**Headless-режим** **[проверено]**: `https://code.claude.com/docs/en/headless`,
`https://code.claude.com/docs/en/cli-reference`.

```bash
claude -p --input-format stream-json --output-format stream-json --verbose \
  [--include-partial-messages] [--replay-user-messages] \
  [--permission-prompt-tool stdio] [--permission-mode default|acceptEdits|plan|auto|dontAsk|bypassPermissions] \
  [--session-id <uuid> | --resume <id> [--fork-session]] \
  [--mcp-config <file-or-json> --strict-mcp-config] [--setting-sources user,project,local]
```

- Выход: JSONL. Первое событие — `system/init` (модель, инструменты, MCP-серверы, плагины и массив
  `capabilities`, например `interrupt_receipt_v1`, для определения возможностей без сравнения
  версий). Далее `assistant`, `user`, `stream_event` (при `--include-partial-messages`),
  `system/api_retry`, в конце `result`. Сообщения subagent несут `parent_tool_use_id`.
- Сигналы: SIGTERM — код выхода 143, незавершённый ход остаётся без результата; чтобы завершить
  ход, нужен SIGINT или `interrupt()` из SDK.
- `--bare` пропускает hooks, skills, плагины, MCP, память и CLAUDE.md. Важно: «In bare mode, Claude
  Code never reads OAuth credentials or the system keychain», и «`--bare` is the recommended mode
  for scripted and SDK calls, and will become the default for `-p` in a future release». Когда
  умолчание сменится, запуск `-p` с подпиской потребует явного отказа от bare-режима; способ
  отказа **[не проверено]**.

**Control protocol** **[проверено по коду SDK]**:
`claude-agent-sdk-python@4720d36:src/claude_agent_sdk/types.py` строки 2882–2990 и
`_internal/transport/subprocess_cli.py` строки 523, 582, 742. SDK запускает CLI с
`--output-format stream-json --verbose --input-format stream-json` и, когда задан обработчик
разрешений, с `--permission-prompt-tool stdio`. Поверх потока идут сообщения
`{"type":"control_request","request_id":…,"request":{"subtype":…}}` и
`{"type":"control_response",…}`. Подтипы: `initialize`, `interrupt`, `can_use_tool`,
`set_permission_mode`, `hook_callback`, `mcp_message`, `rewind_files`, `mcp_reconnect`,
`mcp_toggle`, `stop_task`. Отдельной спецификации этого протокола в документации я не нашёл;
эталоном служит код SDK. Vibe Kanban реализует его на Rust
(`vibe-kanban@3f095f0:crates/executors/src/executors/claude/{protocol.rs,client.rs}`), T3 Code и
DeepSeek Harness используют TypeScript SDK.

**Agent SDK**: пакеты только для TypeScript и Python. README Python SDK: использование
регулируется Commercial Terms of Service, в том числе когда на SDK построен продукт для своих
клиентов.

**Ограничения на вход по подписке** **[проверено]**:
`https://code.claude.com/docs/en/legal-and-compliance`.

- «Anthropic does not permit third-party developers to offer Claude.ai login into their own
  applications, or to route requests through Free, Pro, Max, or Ultra plan credentials on behalf of
  their users. Moreover, developers may not collect, store, or intermediate Claude.ai credentials or
  session tokens — sign-in to a Claude account must complete through Anthropic's own flow.»
- Разработчикам, включая использующих Agent SDK, следует применять API-ключи.
- Разрешено: «preinstalling or running Claude Code in your products or services» при согласии с
  Commercial Terms и двух условиях — бинарь не модифицирован и ни один встроенный способ входа не
  отключён; продукт не оплачивает и не перепродаёт использование, каждый конечный пользователь
  входит своими учётными данными. Прямо сказано, что ограничение не мешает пользователю войти в
  немодифицированный Claude Code своей подпиской, в том числе когда платформа его размещает.
- Нельзя использовать имя и логотип Claude Code в названии продукта или функции.

Практический вывод: запускать у пользователя установленный им `claude` и не трогать его вход —
в рамках правил; встраивать «Войти через Claude» в свой интерфейс, хранить токены или гонять
запросы через подписку от имени пользователя — нет.

**Локальная модель** **[проверено]**: `ANTHROPIC_BASE_URL` плюс учётные данные шлюза направляют
Claude Code на endpoint формата Anthropic Messages. При этом
`https://code.claude.com/docs/en/llm-gateway`: Anthropic «doesn't support routing Claude Code to
non-Claude models through any gateway». Технически vLLM и Ollama отдают `/v1/messages`, но это
неподдерживаемая конфигурация.

### 2.3. OpenCode

- **1.x** **[проверено]**, `https://opencode.ai/docs/server`: `opencode serve [--port] [--hostname]
  [--cors]`, по умолчанию `127.0.0.1:4096`; HTTP basic auth через `OPENCODE_SERVER_PASSWORD`;
  спецификация OpenAPI на `/doc`; события — SSE `GET /event`; `POST /session/:id/message`,
  `/prompt_async`, `/abort`, `/fork`, `/revert`, `/permissions/:permissionID`.
- **2.x** **[проверено]**, `https://opencode.ai/docs/api/`, `https://opencode.ai/v2/docs/cli.md`:
  один общий фоновый сервер на пользователя; API под `/api/…` назван «Experimental HttpApi
  surface»; OpenAPI 3.1 в `/docs/openapi.json` (170 операций); `POST /api/session/{id}/prompt`,
  `/interrupt`, `/fork`, `/revert/stage|commit`, `/permission/{requestID}/reply`, формы вопросов
  `/form/{formID}/reply`; поток `GET /api/event` описан как непостоянный (медленный потребитель
  теряет события). Интеграции, написанные под API 1.x, требуют переделки **[вторично]**.
- **ACP** **[проверено]**, `https://opencode.ai/docs/cli/acp/`: `opencode acp`, JSON-RPC по stdio,
  версия протокола 1, процесс поднимает частный сервер и не использует общий фоновый.
  Поддержаны new, load, resume, fork, list, close, delete, отмена, варианты конфигурации (model,
  effort, mode), slash-команды, запросы разрешений и вопросы-формы. Не поддержаны: MCP over ACP,
  MCP по SSE, OAuth для HTTP MCP; `resume` и `fork` не воспроизводят сообщения.
- T3 Code требует OpenCode не ниже 1.14.19 или 2.0.18, сам определяет ветку и держит два адаптера.
  На 1.x запускает по серверу на тред, потому что регистрации MCP там привязаны к каталогу
  (`t3code@1f698f01:docs/internals/providers.md`).
- Локальные модели **[проверено]**, `https://opencode.ai/docs/providers/`: автообнаружение Ollama
  (`127.0.0.1:11434`), LM Studio (`127.0.0.1:1234/v1`), vLLM (`127.0.0.1:8000/v1`); собственный
  провайдер через пакет `@opencode/ai/providers/openai-compatible` или
  `…/anthropic-compatible` с `settings.baseURL`. Оговорка: «discovery cannot infer tool support, so
  discovered vLLM models start with tools disabled» — инструменты нужно включать в `capabilities`
  модели вручную.
- Лицензия и текущий адрес репозитория **[не проверено]**.

### 2.4. Qwen Code

**[проверено]** по `qwen-code@14d24a15`.

- **ACP**: `qwen --acp` (`docs/users/integration-zed.md`); агент есть в ACP Registry.
- **Headless**: `qwen -p "<prompt>" --output-format text|json|stream-json`,
  `--include-partial-messages`, `--input-format stream-json`, `--continue`, `--resume <id>`,
  `--approval-mode plan|default|auto-edit|auto|yolo`, `--yolo`. Про двунаправленный ввод сказано:
  «Stream-json input mode is currently under construction and is intended for SDK integration»
  (`docs/users/features/headless.md`). Формат сообщений родственен Claude Code (`assistant`,
  `result`, `stream_event`).
- **SDK**: `@qwen-code/sdk` (TypeScript, «minimum experimental», с `canUseTool`), Python и Java.
- **Демон**: `qwen serve` — HTTP плюс SSE, по умолчанию `127.0.0.1:4170`, встроенный Web Shell,
  bearer-токен, статус «Experimental» (`docs/users/qwen-serve.md`,
  `docs/developers/qwen-serve-protocol.md`). Внутри демон сам управляет дочерними ACP-процессами.
- **Аутентификация**: Qwen OAuth с подпиской chat.qwen.ai либо API-ключи.
- **Локальная модель**: `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `OPENAI_MODEL` или `modelProviders` в
  `settings.json` с `baseUrl` и `envKey`; поддержан также протокол `anthropic` с
  `ANTHROPIC_BASE_URL` (`docs/users/configuration/{auth,model-providers}.md`).
- `--yolo` не включает sandbox; headless-запуск с ним выполняет инструменты с правами процесса.

### 2.5. Gemini CLI и Antigravity

- `gemini --acp` — ACP по stdio; headless-режим отдаёт JSONL
  (`https://geminicli.com/docs/cli/acp-mode/`) **[проверено]**.
- Та же страница: «Gemini CLI was replaced by Antigravity CLI on June 18th, 2026» (для бесплатного
  уровня и Google One) и «Gemini CLI has not been optimized for Gemini 4 Argon». ACP Registry
  содержит отдельные записи «Gemini CLI» и «Google Antigravity».
- T3 Code поддерживает Antigravity как вариант общего ACP-адаптера с изоляцией профиля.
- Поддержки произвольного OpenAI-совместимого endpoint в Gemini CLI я не нашёл **[не проверено]**.
  Для заказчика с локальным Qwen эта ветка не нужна: Qwen Code — её независимо развивающийся
  наследник.

### 2.6. Goose

- Документация переехала на `goose-docs.ai`, репозиторий — `github.com/aaif-goose/goose`, копирайт
  AAIF (Agentic AI Foundation) **[проверено]**. Язык реализации Rust и лицензия Apache-2.0
  **[вторично]**.
- `goose acp` — ACP по stdio; `goose serve` — ACP по HTTP и WebSocket на
  `http://127.0.0.1:3284/acp`, секрет `GOOSE_SERVER__SECRET_KEY` в заголовке `X-Secret-Key`
  (`https://goose-docs.ai/docs/gdk/acp/`) **[проверено]**.
- Goose сам является «harness над harness»: провайдеры `claude-acp`, `codex-acp`, `amp-acp`,
  `pi-acp` запускают чужих агентов через ACP-адаптеры (`@agentclientprotocol/claude-agent-acp`,
  `@agentclientprotocol/codex-acp`), расширения Goose передаются им как MCP-серверы; resume и fork
  сессий для таких провайдеров «not supported yet»
  (`https://goose-docs.ai/docs/guides/acp-providers`) **[проверено]**.
- Локальная модель: `OPENAI_HOST`, `OPENAI_BASE_PATH`, `OLLAMA_HOST`, собственные провайдеры в
  `~/.config/goose/custom_providers/` с движками OpenAI, Anthropic, Ollama. Предупреждение
  документации: goose «relies heavily on tool calling capabilities»
  (`https://goose-docs.ai/docs/getting-started/providers`) **[проверено]**.

### 2.7. pi

**[проверено]** по `pi@747c217:packages/coding-agent/docs/`.

- `pi --mode rpc [--no-session]`: команды JSONL в stdin, ответы и события в stdout; команды несут
  необязательный `id`. Набор: `prompt`, `steer`, `follow_up`, `abort`, `clear_queue`,
  `new_session`, `get_state`, `get_messages`, `set_model`, `get_available_models`,
  `set_thinking_level`, `compact`, `bash`, `switch_session`, `fork`, `clone`, `get_tree`,
  `get_commands` и другие (`rpc.md`, `rpc-commands.md`).
- Жизненный цикл: успешный ответ на `prompt` означает только принятие; `agent_end` завершает один
  низкоуровневый прогон, а знать, что pi не продолжит сам, можно только по `agent_settled`.
- Кадры: строго LF; документация предупреждает не использовать разделители строк Unicode
  (`U+2028`, `U+2029`) и читать stdout непрерывно.
- Есть `--mode json` (поток событий одного запуска), in-process SDK для Node и готовый `RpcClient`.
- Модели: `models.json` с `baseUrl` и `api` из `openai-completions`, `openai-responses`,
  `anthropic-messages`, `google-generative-ai`; отдельные руководства `llama-cpp.md` и
  `nvidia-rtx-llama-cpp.md`.
- Встроенной системы разрешений нет: T3 Code реализует режимы через блокирующий tool hook pi и
  отмечает, что это не sandbox. ACP доступен только через сторонний адаптер `pi-acp`.
- Пакет переименован в `@earendil-works/pi-coding-agent`; требуется Node 22.19+
  (`t3code@1f698f01:docs/user/providers-pi.md`).

### 2.8. DeepSeek Harness

**[проверено]** по `deepseek-harness@39163dd3`.

- TypeScript на Node `>=24.9.0`, каркас Cordis, версия `0.2.2-rc.3`, MIT. README: «developer
  preview… THERE WILL BE COMPATIBILITY-BREAKING CHANGES».
- Поверхности (`docs/cli-help.md`): `dsh acp` («Serve automation clients over Agent Client Protocol
  stdio»), `dsh headless [--json] [--session-id …] [--permission-mode …] "<task>"` (NDJSON-события
  прогона), `dsh sdk` и `dsh sdk-minimal` («stdio JSON-RPC» для собственного SDK), `dsh web`
  (`127.0.0.1:3080`). Каждая поверхность — это профиль, то есть набор плагинов.
- ACP-сервер намеренно урезан: нет удаления, fork, воспроизведения транскрипта и дополнительных
  каталогов (`packages/acp/acp/README.md`).
- LLM-слой построен на pi-ai (`packages/llm/llm-pi-ai`) и поддерживает `openai-completions`,
  протоколы Responses и `anthropic-messages`; в каталоге конфигурации есть параметр
  `vllmPriority` (`docs/config-catalog.md`).
- Сам DeepSeek Harness запускает чужие harness как subagent-плагины
  (`packages/subagent/`): `subagent-claude-code` через `@anthropic-ai/claude-agent-sdk`,
  `subagent-codex` через Codex app-server, `subagent-acp` для любого ACP-агента. Плагины Claude и
  Codex несут закреплённый бинарь и «never falls back to the host `claude` executable». Это
  готовый образец идеи «harness запускает другие внутри себя», но только для делегирования одной
  задачи без интерактивных запросов.

### 2.9. Локальные endpoint: что кому нужно

vLLM **[проверено]**, `https://docs.vllm.ai/en/latest/serving/online_serving/`: отдаёт
`/v1/chat/completions`, `/v1/responses`, `/v1/messages` и `/v1/messages/count_tokens`. Оговорка для
Messages: запрос с `max_tokens=0` отклоняется с 400. Ollama **[вторично]**: `/v1/messages` с
версии 0.14.0, подмножество API, без `count_tokens` и `tool_choice`; поддержка Responses API
**[не проверено]**.

| Harness | Chat Completions | Responses | Anthropic Messages | Замечание |
|---|---|---|---|---|
| Qwen Code | да | да (`wireApi: "responses"`) | да | Родной для моделей Qwen |
| OpenCode 2.x | да | — | да | Инструменты для найденных vLLM-моделей выключены по умолчанию |
| pi | да | да | да | Нет собственных approvals |
| DeepSeek Harness | да | да | да | Через pi-ai |
| Goose | да | — | да | Сильно зависит от tool calling |
| Codex | нет | да | нет | Chat удалён |
| Claude Code | нет | нет | да | Не-Claude модели вендором не поддерживаются |
| Gemini CLI | не найдено | не найдено | не найдено | |

Прочерк означает «в прочитанных источниках не встретилось», а не подтверждённое отсутствие.

### 2.10. Кого ещё стоит поддержать

Через общий ACP-адаптер без отдельного кода: Cursor, GitHub Copilot CLI, Kiro CLI, Kimi Code,
Mistral Vibe, Factory Droid, Junie, Cline, Devin, Amp (через адаптер), а также GLM Agent, MiniMax
Code, Qoder CLI — все есть в ACP Registry (`acp-spec@30f2eba:docs/get-started/registry.mdx`).
Отдельные нативные адаптеры для них не оправданы.

## 3. Agent Client Protocol

### 3.1. Состояние

**[проверено]** по `acp-spec@30f2eba`.

- Управление вышло из Zed в организацию `agentclientprotocol`; есть GOVERNANCE.md, процесс RFD,
  реестр агентов.
- «The current stable ACP protocol version is `1`». Версия 2 — черновик: «v2 as a whole is still
  labeled draft, so gate v2 support behind explicit version negotiation and feature flags until it
  stabilizes» (`docs/protocol/v2/migration.mdx`). Crate схемы — `agent-client-protocol-schema`
  1.18.0 от 2026-11-14.
- Транспорт: JSON-RPC 2.0 по stdio; транспорт HTTP и WebSocket — в черновике RFD, реализован в
  отдельном crate.
- v1: `initialize`, `authenticate`, `session/new`, `session/load`, `session/prompt` (ответ
  завершает ход и несёт stop reason), `session/cancel`, `session/update`,
  `session/request_permission`, `session/set_mode`, клиентские `fs/*` и `terminal/*`.
- v2 меняет главное: ответ на `session/prompt` только подтверждает приём, а ход завершается
  уведомлением `state_update` (`running`, `idle`, `requires_action`); обновления становятся
  upsert по идентификатору; удалены клиентские `fs/*`, `terminal/*` и режимы сессии (режимы стали
  config options); появился обязательный базовый набор методов сессии (`session/resume`,
  `session/list`, `session/close`).
- Завершённые RFD: session list, resume, close, delete, config options, usage, message id,
  elicitation, auth methods, logout, session compaction, client system prompt, request
  cancellation, реестр. Preview: subagents. Черновики: session fork, proxy chains, MCP over ACP,
  custom LLM endpoint, session status, plan operations, agent skills и набор v2.

### 3.2. Кто говорит на ACP

Нативно (`docs/get-started/agents.mdx`, `registry.mdx`): Qwen Code, OpenCode, Goose, Gemini CLI,
Cursor, GitHub Copilot CLI, Kiro CLI, Kimi CLI, Mistral Vibe, Cline, Junie, OpenHands, DeepSeek
Harness (в списке нет, но `dsh acp` существует) и десятки других. Через адаптеры: Claude Agent
(`@agentclientprotocol/claude-agent-acp`, внутри — Claude Agent SDK), Codex
(`@agentclientprotocol/codex-acp`), Pi (`pi-acp`), Amp (`amp-acp`). Адаптеры распространяются
через npm, то есть добавляют зависимость от Node и собственный цикл выпуска.

Клиентов тоже много: Zed, JetBrains, neovim, T3 Code, Goose и длинный список настольных
оркестраторов, из них на Rust или Tauri заявлены Jockey, OctoShell, SpurCode, Daruda, Martty,
Athas, necoder (`docs/get-started/clients.mdx`; описания авторские, не проверялись).

### 3.3. Rust-crates

`acp-rust-sdk@f673a75`, все версии 5.3.0, Apache-2.0, Rust 1.88, edition 2024:

| Crate | Назначение |
|---|---|
| `agent-client-protocol` | Роли Client, Agent, Proxy, Conductor; построители соединений; feature `process` (запуск дочернего процесса на `async-process`, `rustix`, `windows-sys`), `stdio`, набор `unstable_*` |
| `agent-client-protocol-schema` 1.18.0 | Типы сообщений v1 и v2 |
| `agent-client-protocol-http` | Транспорт HTTP/SSE и WebSocket |
| `agent-client-protocol-rmcp` | Связка с MCP SDK `rmcp` |
| `agent-client-protocol-conductor` | Оркестрация цепочек прокси между клиентом и агентом |
| `agent-client-protocol-polyfill` | Прокси совместимости, в том числе MCP-over-ACP → HTTP |
| `agent-client-protocol-yopo` | Пример клиента одного запроса |

API явно разделён по версиям: `Client.v1()` и `Client.v2()`, типы в модулях `v1` и `v2`.
Vibe Kanban использует более старые версии crate (0.8 и 3.4.0) и держит две обвязки —
`harness.rs` и `harness_v2.rs`.

Цепочки прокси интересны для идеи «задача — плагин»: прокси вставляется между нами и агентом и
добавляет MCP-серверы или контекст, не меняя агента. Это черновик, опираться на него в первой
версии нельзя.

### 3.4. Чего ACP не покрывает по сравнению с нативными протоколами

| Возможность | Нативно | ACP |
|---|---|---|
| Откат разговора к ходу | Codex (T3 Code вызывает `thread/revert`), Pi (файлы сессий) | Нет. T3 Code: «ACP agents cannot rewind their own conversation» |
| Откат файлов к сообщению | Claude (`rewind_files`) | Нет; делается на стороне клиента через git |
| Fork от произвольного хода | Codex `thread/fork`, Pi `fork` | Черновик RFD `session-fork` |
| Steering во время хода | Codex `turn/steer`, Pi `steer` | В v1 один prompt на сессию за раз; в v2 жизненный цикл переделан, но это черновик |
| Родословная subagent | Codex `collabToolCall`, Claude `parent_tool_use_id` | Preview. T3 Code: «ACP has no portable native subagent-lineage contract» |
| Изоляция аккаунтов | `CODEX_HOME`, `CLAUDE_CONFIG_DIR` | «ACP does not describe account isolation» |
| Настройка LLM endpoint | env и конфиги каждого агента | Черновик `providers/list`, `providers/set` |
| Управление терминалами агента | Codex `command/exec`, фоновые терминалы | В v2 вывод терминала только для показа |
| Запросы, переживающие перезапуск | Codex async questions | Не описано |
| Динамические инструменты хоста | Codex `dynamicTools`, Claude in-process MCP | Только через MCP-серверы сессии; MCP over ACP — черновик |
| Автоматический ревьюер approvals | Codex, Claude `auto` | Зависит от агента, выражается config option |
| Лимиты подписки с временем сброса | Codex `account/rateLimits/*` | `usage_update` без общей модели лимитов |

Отдельная практическая проблема, которую T3 Code называет первой в списке ловушек: агенты
объявляют config options поздно и могут молча принять запись, ничего не изменив.

### 3.5. Может ли ACP быть единственным интерфейсом адаптера

Как **первый и основной транспорт** — да. Один адаптер даёт Qwen Code, OpenCode, Goose, DeepSeek
Harness, Gemini CLI и весь реестр; этого достаточно для заказчика с локальным Qwen. T3 Code
пришёл к тому же правилу: ACP-агент начинается как запись реестра «with no per-agent handling»,
а отдельный драйвер появляется, только когда нужно поведение вне спецификации.

Как **внутренняя модель** — нет. Внутренний интерфейс должен быть шире ACP (run, попытки,
запросы с `responseCapability`, откат, fork, steering, фоновые задачи) и управляться флагами
возможностей; ACP-адаптер объявляет часть из них выключенными. Иначе нативные адаптеры Codex и
Claude потом придётся втискивать в урезанную модель.

Условия: поддерживать v1 сейчас и v2 за флагом с согласованием версии на каждом соединении;
для Claude и Codex выбирать между ACP-адаптером на Node (быстро, но беднее и с лишней
зависимостью) и нативным протоколом (дороже, полнее).

## 4. Сопоставимые продукты

| Продукт | На чём построен | Как управляет агентами | Чего нет в T3 Code | Статус |
|---|---|---|---|---|
| Zed, agent panel | Rust (GPUI) | ACP-клиент, установка из ACP Registry, `agent_servers` для своих команд | Редактор кода; журнал ACP-сообщений `dev: open acp logs` | Активен **[проверено]** `zed.dev/docs/ai/external-agents` |
| Goose | Rust **[вторично]** | Собственный агент плюс чужие через ACP-адаптеры; сам отдаёт ACP по stdio, HTTP, WebSocket | Агент и оркестратор в одном бинаре; расширения пробрасываются чужим агентам как MCP | Активен, под AAIF |
| Vibe Kanban | Rust-сервер на axum 0.8, React 18 + Vite, Tauri 2 (`crates/tauri-app`) | Trait `StandardCodingAgentExecutor`: `spawn`, `spawn_follow_up`, `spawn_review`, `normalize_logs`; Claude по stream-json, Codex по app-server, остальные по ACP | Канбан-планирование задач как основной интерфейс | README: «Vibe Kanban is sunsetting» **[проверено]** |
| Claude Squad | Go **[вторично]**, tmux, git worktrees | Запускает интерактивный TUI любой программы в tmux: `cs -p "<команда>"` | Не знает протоколов; работает с чем угодно, что живёт в терминале | AGPL-3.0, нужны tmux и gh **[проверено]** README |
| Conductor | Закрытый, macOS; стек **[не проверено]** | Worktree на задачу, Claude Code, Codex, Cursor **[вторично]** | — | Коммерческий |
| Crystal | Electron, MIT **[вторично]** | Параллельные сессии Claude Code и Codex | — | Объявлен устаревшим в феврале 2026, преемник — закрытый Nimbalyst **[вторично]** |
| DeepSeek Harness | TypeScript, Cordis | Subagent-плагины для Claude Code, Codex, ACP | Профили как наборы плагинов; закреплённые бинари чужих harness | Developer preview |
| OpenCode 2.x | TypeScript | Собственный агент; общий фоновый сервис | Один сервер на пользователя для TUI, web, desktop | Активен |
| Qwen Code `qwen serve` | TypeScript | Демон над собственными ACP-процессами | Web Shell и каналы из коробки | Экспериментален |

Наблюдения.

- В Rust написаны Zed, Goose и Vibe Kanban. Для нашего случая прямой донор кода — Vibe Kanban:
  его crate `executors` решает ровно задачу «Rust запускает Claude, Codex и ACP-агентов и
  нормализует их поток». Зависимости: `tokio`, `command-group` 5.0, `os_pipe`,
  `agent-client-protocol`, `codex-app-server-protocol` (git-тег), `reqwest`, `eventsource-stream`,
  `sqlx` 0.8.6, `ts-rs` и `schemars` для типов клиента
  (`vibe-kanban@3f095f0:crates/executors/Cargo.toml`). Версии протоколов там отстают от текущих.
- На docs.rs есть crates `gate4agent` и `unified-agent-api-*`, заявленные как обёртки над CLI
  агентов; содержимое не изучалось **[не проверено]**.
- Ни один из продуктов не делает того, что заявлено у нас как «каждая задача — плагин»:
  перенастройки harness под класс задач с накоплением источников, инструментов и памяти. Ближе
  всего по механике профили DeepSeek Harness и внедряемый MCP-сервер T3 Code.
- T3 Code и все перечисленные, кроме Claude Squad, рассчитаны на разработчика: worktree, diff,
  pull request. Низкий порог входа для неподготовленного пользователя никто из них целью не
  ставит.

## 5. Кроссплатформенное управление процессами из Rust

### 5.1. stdio JSON-RPC против PTY

Все структурированные поверхности из раздела 2 работают без терминала: Codex app-server, ACP,
Claude stream-json, pi RPC — строки JSON по stdio; OpenCode, `qwen serve`, `goose serve` — HTTP с
потоком событий. PTY для управления агентом не нужен и вреден: он смешивает stdout и stderr,
добавляет escape-последовательности и зависит от размера окна.

PTY нужен в трёх случаях: пользовательский терминал в интерфейсе; вход, который агент объявляет
как terminal-метод (в ACP это тип `terminal`, например `opencode acp --login`) или делает только
в своём TUI (`pi` → `/login`, `claude auth login`); управление агентом без протокола — путь Claude
Squad, которого стоит избегать.

### 5.2. Правила для stdio

- Отдельные задачи на чтение stdout, чтение stderr и запись stdin. Читать stdout непрерывно:
  клиент, переставший читать, останавливает агента (документация pi).
- Делить записи только по LF, предшествующий CR отбрасывать; не использовать читатели строк,
  которые режут по `U+2028` и `U+2029`. Лимит длины строки ставить большим: в кадрах бывают
  вложения в base64.
- stdout зарезервирован под протокол; stderr — хвост для диагностики (T3 Code держит `stderr tail`
  и показывает его при сбое).
- Неизвестные методы и поля не считать ошибкой: и ACP v2, и Codex расширяются между версиями. Для
  сырого журнала сохранять кадр до разбора (`serde_json::value::RawValue`).
- Запросы идут в обе стороны: Codex и ACP шлют клиенту запросы (approvals), на которые нужно
  ответить по `id`. Нужен единый мультиплексор запросов и ответов, а не «запрос — ответ».

### 5.3. Отмена и сигналы

Лестница остановки, от мягкого к жёсткому:

1. Протокольная отмена: `turn/interrupt` (Codex), `control_request` с `interrupt` (Claude),
   `session/cancel` (ACP), `abort` (pi), `POST …/interrupt` (OpenCode 2.x).
2. Ожидание терминального события с таймаутом. У Codex запрос `turn/interrupt` возвращается
   раньше, а итоговое состояние приходит в `turn/completed`. T3 Code выделяет для этого отдельную
   ошибку `ProviderAdapterInterruptTimeoutError` и показывает «Force stop».
3. Закрытие stdin: `opencode acp` завершается, когда клиент закрывает stdin; `dsh acp` работает
   «until the client disconnects»; Claude Code при конце ввода отменяет ожидающий запрос
   разрешения.
4. Unix: SIGINT (Claude Code завершает ход), затем SIGTERM всей группе процессов (у Claude Code
   код 143 и незавершённый ход), затем SIGKILL. Windows: `CTRL_BREAK_EVENT` процессу в новой
   группе, затем завершение Job Object **[не проверено]**.
5. После жёсткой остановки считать нативный тред возможно повреждённым — в модели T3 Code это
   `threadDisposition: "broken"`.

Отмена, отправленная раньше, чем агент зарегистрировал prompt, может быть проигнорирована:
нужно дождаться собственного сигнала агента о начале хода.

### 5.4. Дерево процессов и платформы

- Unix: запускать агента лидером новой группы процессов или сессии и сигналить группе; агент
  порождает shell-команды и MCP-серверы, и убийство одного PID оставит сирот. На Linux
  дополнительно `PR_SET_PDEATHSIG`, чтобы дети не пережили наш процесс **[не проверено]**.
- Windows: Job Object с завершением при закрытии дескриптора; флаг новой группы процессов для
  мягкого сигнала. CLI, установленные через npm, — это `.cmd`-обёртки: их нельзя запускать как
  обычный исполняемый файл, а экранирование аргументов для `cmd.exe` — известный источник
  уязвимостей **[не проверено]**. T3 Code держит для этого отдельный модуль
  `apps/server/src/process/windowsVerbatimSpawn.ts`.
- ConPTY доступен с Windows 10 1809 **[не проверено]**; crate `portable-pty` скрывает различие.
  Codex использует `portable-pty` 0.9.0 и собственный модуль `utils/pty` с каталогом `win/`
  (`codex@662a634:codex-rs/Cargo.toml` строка 456).
- Ограничение ресурсов: T3 Code помещает агентов в systemd-slice с весами CPU и I/O, `TasksMax` и
  лимитами памяти, оставляя серверу около 4–6 ГБ
  (`t3code@1f698f01:apps/server/src/process/agentScope.ts`). На DGX Spark память общая для CPU и
  GPU, поэтому процессы агентов конкурируют с моделью напрямую; ограничение через cgroup v2
  нужно с первой версии.
- Symlink-пути рабочего каталога (например `/tmp` на macOS) ломают агентов, сравнивающих
  канонические пути.

### 5.5. Crates

| Задача | Crate | Где подтверждено использование |
|---|---|---|
| Асинхронные процессы и каналы | `tokio` (`tokio::process`), `tokio-util` | Codex, Vibe Kanban |
| Группы процессов и Job Object | `command-group` 5.0 | Vibe Kanban; преемник `process-wrap` **[не проверено]** |
| Низкоуровневые вызовы | `rustix`, `libc`, `windows-sys` 0.61 | Codex, ACP Rust SDK |
| PTY и ConPTY | `portable-pty` 0.9.0 | Codex |
| ACP | `agent-client-protocol` 5.3.0 | ACP Rust SDK |
| Codex app-server | `codex-app-server-protocol` (git-тег) | Vibe Kanban |
| MCP | `rmcp` (Codex закрепляет 3.5.1 по git-ревизии) | Codex |
| HTTP и SSE | `reqwest`, `eventsource-stream` | Vibe Kanban |
| WebSocket | `async-tungstenite` | ACP Rust SDK |
| Поиск бинарей | `which` 8 | Codex |
| Разбор командной строки | `shlex`, на Windows `winsplit` | Codex, Vibe Kanban |
| Типы для web-клиента | `ts-rs`, `schemars` | Vibe Kanban |
| SQLite | `sqlx` 0.8.6 | Vibe Kanban |

ACP Rust SDK запускает дочерний процесс через `async-process`, а не `tokio::process`; при
использовании его feature `process` это нужно учитывать либо запускать процесс самим и
передавать SDK готовые потоки.

## 6. Рекомендация

Допущения: первая поставка — одна машина DGX Spark (Linux aarch64) в локальной сети, локальная
модель Qwen за OpenAI-совместимым сервером, пользователи без навыков работы с CLI, облачные
подписки не обязательны. Если допущения неверны, порядок шагов меняется.

### 6.1. Интерфейс адаптера, словами

Адаптер делится на три части с разным временем жизни.

1. **Описание установки (driver).** Умеет ответить без запуска сессии: найден ли бинарь, какая
   версия, совместима ли она, в каком состоянии вход (включая значение «неизвестно»), какие
   модели доступны. Проверка не должна запускать MCP-серверы, hooks и браузер.
2. **Экземпляр (instance).** Одна конфигурация: путь к бинарю, аргументы, переменные окружения,
   каталог состояния агента, endpoint модели, учётные данные. Окружение дочернего процесса
   собирается с нуля из этой конфигурации, а не наследуется. Экземпляр объявляет набор
   возможностей в форме, близкой к `OrchestrationV2ProviderCapabilities`.
3. **Сессия.** Живой процесс или соединение. Операции: открыть; убедиться, что нативный тред
   существует, или возобновить его по сохранённой ссылке; начать ход; дослать сообщение в идущий
   ход, если возможность объявлена; прервать ход с таймаутом и принудительной остановкой; ответить
   на запрос (решение по approval в нативных идентификаторах вариантов либо ответы на вопросы);
   прочитать снимок треда; закрыть. Необязательные операции за флагами: откат, fork, сжатие
   контекста, смена модели и режима на лету, остановка фоновой задачи.

Сессия отдаёт один поток нормализованных событий и отдельно — кольцевой журнал сырых кадров для
диагностики и записи тестовых транскриптов.

Что адаптер получает от ядра на каждый ход: рабочий каталог, режим разрешений из единого словаря
(спрашивать всё; разрешить правки; автоматический ревьюер; полный доступ) и потолок режима,
выбранную модель, вложения, а также «пакет задачи» — список MCP-серверов, дополнение к системному
промпту и каталоги skills. Пакет задачи и есть точка, где «каждая задача — плагин» встречается с
harness: адаптер переводит его в то, что агент понимает (`mcpServers` в `session/new` у ACP,
`--mcp-config` и `--append-system-prompt` у Claude Code, конфигурация у Codex). Режим, которого
агент сам не соблюдает, не предлагается.

Реализации: общий ACP-адаптер (v1, v2 за флагом) — первым; нативные адаптеры — только там, где
ACP теряет нужную функцию.

### 6.2. Нормализованная модель событий

Сущности с идентификаторами приложения; нативные идентификаторы хранятся как ссылки с оценкой
надёжности (сильная, слабая, отсутствует).

- **Тред** — разговор, который видит пользователь; привязан к рабочему каталогу и экземпляру.
- **Run** — один видимый пользователю ход: `queued`, `starting`, `running`, `waiting`,
  `completed`, `interrupted`, `failed`, `cancelled`. Завершает run только терминальное событие
  корневого хода; завершение дочерней работы его не закрывает.
- **Элемент ленты** — типизированная запись внутри run: сообщение пользователя, сообщение
  ассистента, рассуждение, план или список дел, выполнение команды, изменение файлов, поиск,
  вызов внешнего инструмента (MCP), запрос разрешения, вопрос пользователю, subagent, сжатие
  контекста, уведомление, ошибка. У каждого — статус, порядковый номер, родитель, нативная ссылка.
- **Запрос** — ожидающее решение пользователя с явным способом ответа: живой обратный вызов,
  ответ новым сообщением, невосстановимо после перезапуска.
- **Сессия и нативный тред провайдера** — состояние процесса и ручка разговора у агента.

Правила потока.

- События — upsert целой сущности по идентификатору («элемент обновлён»), а не дельты. Клиент
  может подключиться в любой момент по снимку и курсору.
- Дельты текста и вывода команд — отдельный непостоянный канал для тех, кто смотрит на строку
  прямо сейчас; итог приходит в обновлении элемента.
- Терминальное событие хода несёт статус, класс сбоя (лимит использования с временем сброса,
  ошибка провайдера, транспорт, права, валидация, неизвестно) и признак, пригоден ли нативный
  тред к повторному использованию.
- События провайдера не переписываются под другого провайдера; чего агент не сообщает, то
  отсутствует и отражено в возможностях.
- Команды ядру идемпотентны по идентификатору команды; намерение записывается до побочного
  эффекта.

Для первой версии достаточно подмножества: тред, run, элементы девяти-десяти типов, запросы,
терминальное событие. Дерево узлов, попытки, родословную subagent и перенос контекста между
провайдерами можно добавить позже, если идентификаторы и правило upsert заложены сразу.

### 6.3. Какие harness поддержать первыми для заказчика с локальным Qwen

1. **Qwen Code через ACP** (`qwen --acp`). Родной для моделей Qwen, Apache-2.0, OpenAI-совместимый
   endpoint настраивается переменными окружения, ACP стабильной версии. Это эталонный агент
   первой поставки.
2. **OpenCode через ACP** (`opencode acp`), позже при необходимости — через HTTP API. Второй
   независимый агент на том же адаптере проверяет, что адаптер действительно общий. Требует
   ручного включения инструментов для моделей vLLM. HTTP API 2.x пока экспериментален, поэтому не
   с него начинать.
3. **Третий — по результату замера на устройстве**, кандидаты:
   - **pi через нативный RPC** — самый простой нативный адаптер (JSONL), MIT, развитая настройка
     локальных моделей, тот же LLM-слой, что внутри DeepSeek Harness. Минус: разрешения придётся
     реализовать самим через hook.
   - **DeepSeek Harness через ACP** (`dsh acp`) — если важна близость к его философии плагинов.
     Минус: developer preview и урезанный ACP.
   - **Codex через app-server** — первый нативный адаптер с точки зрения продукта в целом: самый
     богатый протокол и готовые Rust-типы. Для этого заказчика пригоден только при наличии
     `/v1/responses`, и качество Qwen под Codex нужно измерить.

Claude Code и Gemini CLI для этого заказчика в первую очередь не нужны: первый не поддерживается
вендором с не-Claude моделями, второй вытесняется Antigravity.

Порядок работ: общий ACP-адаптер и модель событий → Qwen Code → OpenCode → записанные транскрипты
и общие сценарии приёмки → замер третьего кандидата → первый нативный адаптер.

## 7. Что не проверено и открытые вопросы

Не проверено: лицензия и текущий репозиторий OpenCode; лицензия и язык Goose (только вторичные
источники); стек Conductor; судьба Crystal; публикация `codex-app-server-protocol` на crates.io;
поддержка Responses API в Ollama; поддержка сторонних endpoint в Gemini CLI; содержимое crates
`gate4agent` и `unified-agent-api-*`; способ отказа от будущего bare-умолчания `claude -p`; связь
«Connect with ChatGPT» в T3 Code с программой Sign in with ChatGPT; качество работы Qwen под
любым harness, кроме общих соображений; детали Windows (ConPTY, запуск `.cmd`).

Вопросы, от которых зависит проект.

1. Какой сервер моделей стоит на DGX Spark (vLLM, Ollama, llama.cpp, NIM) и какие API он отдаёт?
   От этого зависит, возможны ли Codex (Responses) и Claude Code (Messages).
2. Какая модель Qwen, какой парсер tool calls и какое окно контекста? Все перечисленные harness
   опираются на вызов инструментов.
3. Сколько одновременных сессий выдержит общая память устройства и нужны ли лимиты cgroup с
   первого дня?
4. Поставлять ли закреплённые бинари harness вместе с продуктом (как DeepSeek Harness) или
   использовать установленные у пользователя (как T3 Code)? Для неподготовленных пользователей
   первое проще, но требует Node на aarch64 и собственного обновления.
5. Есть ли у устройства доступ в интернет? ACP Registry, `npx` и вход по OAuth его требуют.
6. Нужны ли вообще облачные агенты по подписке? Если да, то для Codex вход через ChatGPT в
   коммерческом продукте требует участия в программе Sign in with ChatGPT, а для Claude допустим
   только немодифицированный бинарь с собственным входом пользователя и без нашего интерфейса
   входа.
7. Какой режим разрешений по умолчанию для пользователей без опыта и нужен ли sandbox? Режим
   «полный доступ» без sandbox исполняет команды с правами процесса.
8. Когда переходить на ACP v2: он черновой, а версия 1 остаётся стабильной.
