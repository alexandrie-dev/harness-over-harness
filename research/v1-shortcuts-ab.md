# Варианты (a) и (b) для 65 нетронутых строк v1: готовые crates и части открытых движков

Задание: `plans/briefs/research-v1-shortcuts-ab.md`. Вариант (c) — в `research/v1-shortcuts-c.md`, здесь не повторяется.
Дата работы: 2026-11-18. Всё прочитано статически: ничего не собиралось и не устанавливалось. Что не проверено,
помечено «не проверено».

Обозначения путей: `P/` = `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/` (dsh, коммит `39163dd3`);
`C/` = `/mnt/toshiba/alexandrie/hoh/refs/codex/codex-rs/` (OpenAI Codex, коммит `662a634e88fc`, 2026-11-16);
`G/` = `/mnt/toshiba/alexandrie/hoh/refs/goose/` (Goose, коммит `f6aef1c4d715`, 2026-11-18, склонирован неглубоко
для этой работы). Данные crates.io сняты 2026-11-18 с `https://crates.io/api/v1/crates/<имя>`.

## 1. Главное

1. **Готовый crate или часть чужого движка закрывает не больше 3 % из 69 000 строк.** Оценка по размерам файлов dsh
   (≈ 1 900 строк по (a) и ≈ 200 по (b)); это не измерение. Причина одна: 65 строк — это не библиотечный код, а
   контракты и логика над ядром dsh («Service Definition» в шапках `P/shell/shell/src/index.ts`,
   `P/jobs/jobs/src/index.ts`, `P/interaction/user-approval/src/index.ts`) и тексты, которые видит модель. У
   Codex и Goose такого слоя нет, а их общие типы сообщений и событий другие.
2. **Сторонних библиотек npm во всех 65 строках восемь** (таблица 2): `chokidar`, `koffi`, `diff`, `@js-temporal/polyfill`,
   `p-limit`, `node-pty`, `yaml`, `turndown` (+ `@joplin/turndown-plugin-gfm`). Для каждой замена уже выбрана решениями
   ведущего в `docs/porting/DEPS.md` §5.5 (или не нужна). Новых «готовых crates» сверх этого список не даёт.
3. **Codex: crates.io не содержит официальных `codex-*`** (ответ 404 для `codex-core`, `codex-linux-sandbox`,
   `codex-execpolicy`, `codex-shell-command`, `codex-mcp`, `codex-rmcp-client`, `codex-apply-patch`; `codex-protocol`
   0.63.0 — чужой форк `namastexlabs/codex`). Брать пришлось бы копированием исходников или git-зависимостью. Из
   нужных нам по теме crates самостоятельны (без внутренних зависимостей) только `file-search`, `utils/pty`,
   `process-hardening`, `execpolicy` (1 внутренняя). `sandboxing` тянет 26 внутренних crates, `linux-sandbox` 29,
   `apply-patch` 44, `rmcp-client` 46, `mcp` 71 (замыкание по `Cargo.toml`, скрипт в черновиках, считал по
   `[workspace.dependencies]` с `path`).
4. **Goose: часть crates опубликована** (`goose-agent`, `goose-providers`, `goose-sdk`, `goose-context-management`,
   версия `0.1.0-alpha.14`, 2026-11-17, Apache-2.0, Rust 1.94.1), но это альфа, и они завязаны на собственные типы
   сообщений `goose-provider-types` (32 991 строка). Репозиторий теперь `aaif-goose/goose` (`G/Cargo.toml:12`
   лицензия Apache-2.0; версия workspace 1.60.0).
5. **Выбор (a)/(b) для строки означает отказ от доказательства сверкой с dsh**: журнал сессии, тексты инструментов и
   схемы в «пинах» сравниваются побайтно (`crates/hoh-conformance/README.md`), а у чужого кода они другие. Взамен
   нужны собственные эталонные тесты, т.е. вместо порта пишутся спецификации.
6. **MCP-клиент (`rmcp` 3.11.0, 2026-11-17, Apache-2.0, Rust 1.88, 51,4 млн загрузок) ни одной из 65 строк не
   нужен**: MCP-пакеты dsh (`mcp-client`, `mcp-manager`, `mcp-resources`) в отложенных 87 строках
   (`research/v1-engine-scope.codex.md`). `rmcp` понадобится, если основатель вернёт MCP. SDK окна (`sdk-protocol`,
   `sdk-jsonrpc-server`) — собственный JSON-RPC 2.0 по stdio, не MCP.

**Рекомендация: для 65 строк остаётся (d), порт вручную; (a) применять точечно** там, где crate уже принят
решениями (таблица 2) и где он реально снимает ручной код (`zstd`, `notify`, `portable-pty`, процессная таблица).
(b) не брать ни для одной строки.

## 2. Сторонние библиотеки в 65 строках (из `package.json` пакетов)

Источник: `dependencies` каждого `P/<путь>/package.json`, без `@deepseek-ai/*` и `zod` (`zod` 4.6.6 стоит почти
везде; в Rust его заменяют `serde`+`schemars`, решение ядра).

| Строка(и) | npm-библиотека | Место | Rust-замена | Состояние |
|---|---|---|---|---|
| fs-local, skill-filesystem | `chokidar` 5.0.0 | `P/fs/fs-local/src/index.ts:12`, `P/skill/skill-filesystem/src/index.ts:16` | `notify` 8.2.2 (CC0-1.0, 2026-11-08, Rust 1.88) | решение 6 в `DEPS.md` §5.5; `awaitWriteFinish` и `atomic` пишутся вручную |
| fs-local, session-persistence-jsonl, subprocess-local | `koffi` 3.1.1 | вызовы Win32/libc | `rustix` 1.1.6 / Windows отложен | решение 5 §5.5 |
| tool-fs | `diff` ^9.0.0 | журнал `diffs` | `hoh-jsdiff` (готов) | решение 4 §5.5 |
| schedule | `@js-temporal/polyfill` 0.5.1 | `P/schedule/schedule/src/domain.ts:6` | `jiff` 1.0.0 + `hoh-util-time` (готов); cron и RRULE берутся из `hoh-util-time` (`domain.ts:8-20`) | нового crate не нужно |
| session-persistence-jsonl | `p-limit` 7.3.3 | `P/session/session-persistence-jsonl/src/index.ts:22` | `tokio::sync::Semaphore` | свой код ~20 строк |
| subprocess-local | `node-pty` 1.2.0-beta.15 | `P/subprocess/subprocess-local/src/terminal.ts:1` | `portable-pty` 0.9.0 (MIT, 2025-02-11) | решение 10 §5.5 |
| skill-filesystem | `yaml` ^2.4.2 | frontmatter | `hoh-yaml` (решение 2) | не начат |
| tool-web | `turndown` ^7.2.4, `@joplin/turndown-plugin-gfm` | `P/web/tool-web/src/fetch.ts:9-10` | `html5ever` + свой порт (решение 14) | порт самого turndown не входит в 69 000 строк |

## 3. Чего стоят (a) и (b) по группам строк

«Экономия» — строки исходника dsh, которые не пришлось бы портить вручную; **оценка**, не измерение. Группы
покрывают все 65 строк без пропусков (сумма 69 000).

| № | Группа (строки, исх. строк) | (a) готовый crate | (b) часть Codex / Goose | Экономия a / b | Что теряется; какие сценарии перестают быть доказательством | Цена встраивания |
|---|---|---|---|---|---|---|
| 1 | compaction, compaction-basic, compaction-image-offload, compaction-tool-result-pruner, token-meter, llm-retry (4 737) | Для повтора `backon` 1.6.0 (Apache-2.0, 2025-10-18) или `tokio-retry` 0.3.2 (MIT, 2026-06-09): считают только паузу, а `llm-retry` пишет каждый повтор в журнал до ожидания (`P/llm/llm-retry/src/index.ts:2-3`). Для токенов `tiktoken-rs` 0.12.1 (MIT, 2026-09-24) | Goose `goose-context-management` (1 191 строка, Apache-2.0, alpha.14): сжатие удалением 0/10/20/50/100 % и сводка из шаблонов `minijinja` (`G/crates/goose-context-management/src/summarize.rs:14`, `prompts/`). У Codex сжатие вшито в `core/src/compact*.rs` внутри `codex-core` (532 199 строк), отдельного crate нет | 0 / 0 | `token-meter` — фиксированная оценка «4 символа на токен» (`P/llm/token-meter/src/estimate.ts:13`); `tiktoken-rs` даст другие числа, а они определяют момент сжатия. Тексты и порядок сводки у dsh другие. Теряются: `session/compaction-output-reserve`, `compaction-recovery`, `compaction-summary-headroom`, `model-switch-compaction`, `model-switch-overflow-recovery`, `multimodal-spill-ends`, `multimodal-spill-middle`, `empty-response-retry(-current)`, `sdk/manual-compact`, `sdk/minimal-image-compaction`, `acp/image-compaction` (названия каталогов; содержимое не открывалось) | Тип сообщения Goose (`goose-provider-types`) ≠ типы dsh: нужен слой преобразования в обе стороны |
| 2 | file-reference, file-reference-local, time-context, agent-instructions, working-directory (3 416) | нет | Codex: `core/src/agents_md.rs`, `agents_md_manager.rs` внутри `codex-core`; Goose: `goose/src/hints` (не читал глубже) | 0 / 0 | Правила выбора, бюджет и тексты `AGENTS.md` у dsh свои (`P/context/agent-instructions/src/{files,render,state}.ts`, 1 570 строк). Теряются `session/agent-instructions`, `-budget`, `-budget-chains`, `-group-failure`, `working-directory`, `-admission`, `-consumers`, `provider-cwd`, `sdk/desktop-context` | — |
| 3 | agent, agent-loop, tool-executor-native, tools (7 772) | `jsonschema` 0.61.2 (MIT, 2026-11-12) уже отклонён в `DEPS.md` §4; `P/core/tools/src/json-schema.ts` — свой урезанный набор с собственными текстами отказов (`json-schema.ts:213-358`) | Goose `goose-agent` (5 443 строки, Apache-2.0, alpha.14): цикл как машина состояний над сохранённым разговором, шаги `Operation`, инструменты на `rmcp` (`G/crates/goose-agent/README.md`) | 0 / 0 | Другая архитектура цикла и других типов. Модельные запросы, повторы, журнал событий и пины схем инструментов перестают совпадать: **все 242 сценария** | Переписать ядро под типы Goose или обёртка с преобразованием; вместе с ядром (`hoh-kernel`, уже готов) это двойная работа |
| 4 | commands, permission-presets, tool-ask-user, user-approval, user-questions (3 016) | нет | Goose: `goose-agent` `tool_approval.rs`, `slash_command.rs`, `action_required_manager.rs`; Codex: `utils/approval-presets` (внутренний) | 0 / 0 | Словарь режимов и тексты запросов dsh свои. Теряются `sdk/approval`, `sdk/permission-switch`, `sdk/command-identity`, `sdk/plan-review`, `acp/escalation-approved`, `acp/escalation-rejected`, `acp/fs-escalation-approved`, `session/headless-auto-review`, `session/subagent-child-question-rejection` | — |
| 5 | fs, fs-local, tool-fs, fs-sandbox (4 742) | `notify` (решение 6) для слежения; `similar` 3.2.1 (Apache-2.0) не берём: журнал `diffs` побайтный, порт jsdiff готов; для картинок `image` уже есть | Codex `file-system` (1 336 строк, 15 внутренних crates); `apply-patch` готов в `hoh-fs-patch` (разбор и применение перенесены вместе с тестами). Goose `developer/edit.rs` (466 строк): замена строки, другие имена и схема | 350 / 0 | Имена инструментов, схемы, сообщения об ошибках и диффы dsh (`P/fs/tool-fs/src/{read,write,edit,apply-patch-output,read-render}.ts`) у чужого кода другие. Теряются `session/fs-*`, `apply-patch-*`, `workspace-edit`, `read-image*`, `acp/fs-same-mode` | — |
| 6 | shell, shell-env, bash-local, bash-sandbox, tool-bash, jobs, jobs-local, tool-jobs, subprocess-local (9 679) | `portable-pty` (решение 10), `rustix` 1.1.6 и/или `sysinfo` 0.40.4 (MIT, 2026-11-16, Rust 1.95) / `procfs` 0.18.0 (2025-08-30) вместо `process-inspector.ts` (560 строк, обход `/proc`); для групп процессов `process-wrap` 10.0.1 (Apache-2.0 OR MIT, 2026-09-23) вместо `command-group` (2023-11-18). Из-за идентичности «pid + время запуска» (`process-inspector.ts:10-12`) готовые обёртки не гарантируют того же | Codex `utils/pty` (0 внутренних зависимостей, обёртка над `portable-pty` + tokio); Goose `developer/shell.rs` (1 316), `shell_process.rs` (186) — без реестра заданий и systemd-области | 800 / 200 | Реестр заданий, кольцевой вывод, запуск в systemd-области (`linux-scope.ts`, 726 строк), тексты для модели. Теряются `session/bash-*`, `background-*`, `foreground-confinement-timeout`, `terminal-panel-tools`, `pty-tools-sandbox-backend`, `sdk/bash-tool`, `sdk/background-wait-steer`, `sdk/persistent-tools`, `sdk/ptc-cancel`, `acp/cancel`, `acp/cancel-tool-calls` | Свои обёртки над `portable-pty` пишем при любом варианте; `codex-utils-pty` экономит не более 200 строк склейки, но приносит 10 011 строк чужого кода и tokio-обёртки |
| 7 | sandbox, sandbox-local, sandbox-policy (1 773) | `landlock` 0.4.7 (MIT OR Apache-2.0, 2026-07-27), `seccompiler` 0.5.0 (2025-03-07, Apache/BSD-3). `birdcage` 0.8.1 — GPL-3.0, `bwrap` 1.3.0 — 2023, не подходят. dsh и так вызывает внешний `bwrap` и запускающий Landlock (`P/sandbox/sandbox-local/src/index.ts:2-3,179`; `node-addon-system` готов) | Codex `linux-sandbox` 14 668 + `sandboxing` 12 083 строки, замыкание 26–29 crates (Apache-2.0); в Goose песочницы нет (поиск `landlock|bwrap|seatbelt|sandbox-exec` по `goose`, `goose-agent`, `goose-mcp`, `goose-cli` ничего не нашёл) | 0 / 0 | У dsh свой словарь режимов (read-only / workspace-write / danger-full-access), цепочка «bwrap, затем landlock» и признаки отказов по тексту stderr (`index.ts:226-256`). Теряются `session/partial-landlock-child-failure`, `missing-sandbox-runner`, `background-confinement-failure`, `session-sandbox-root`, `workflow-confinement`, `fs-policy-reject` | Привнос сетевого прокси, uds, otel (`codex-network-proxy` 36 023 строки) ради песочницы |
| 8 | schedule, tool-schedule (4 065) | Для cron `croner` 4.0.3 (MIT, 2026-11-04), `cron` 0.17.0 (2026-06-18), `tokio-cron-scheduler` 0.15.1 (2025-10-28, Goose его использует: `G/crates/goose/src/scheduler/full.rs:11`); RRULE `rrule` 0.14.0 (2025-04-20: выпуска в 2026 нет, `DEPS.md` §3). Расчёт времени уже в `hoh-util-time` | Goose `scheduler` (2 023 строки, расписания рецептов, `chrono`) | 0 / 0 | `schedule` — разбор и проверка записей, отбор срабатываний, доставка и журнал (`P/schedule/schedule/src/domain.ts` 1 320 строк, `index.ts` 804): crate закрывает только вычисление следующего момента, а оно уже есть. **В 242 сценариях инструменты `schedule_*` не вызываются** (вызовы есть лишь в `snapshots/web/subagent-fork-schedule`, `web/tool-details`, вне набора); доказательство — только портируемые тесты `schedule` (7 346 строк) | — |
| 9 | sdk-protocol, sdk-jsonrpc-server (1 620) | JSON-RPC 2.0: `jsonrpsee` 0.26.1 (MIT, 2026-09-30) ориентирован на HTTP/WS; `jsonrpc-core` 18.0.0 (2021), `json-rpc-types` 1.3.4 (BSL-1.0, 2023) устарели или вне набора лицензий. `agent-client-protocol` 5.4.0 (Apache-2.0, 2026-11-18, Rust 1.88) — слой ACP; пригодность его stdio-транспорта к нашему кадрированию **не проверено** | Goose `goose-sdk` (ACP); Codex `app-server-protocol` (не читал) | 150 / 0 | Кадры, коды ошибок и порядок событий dsh сверяются побайтно (`sdk` 41 сценарий). Кадрирование «строка = сообщение» проще написать самим, чем подгонять | — |
| 10 | session-query, session-checkpoint-policy, session-format-catalog, session-format-v3-to-v4, session-persistence, session-persistence-jsonl, session-projection, session-projection-cache, session-title (12 598) | `zstd` 0.14.4 (BSD-3-Clause, 2026-11-12; решение 8) вместо `zstd.ts` + `zstd-private-decoder.ts` (334 строки); блокировка между процессами `fd-lock` 4.0.4 (2025-03-10) или `fs4` 1.1.0 (2026-04-28) вместо Windows-ветки (отложена); `serde_json` уже есть | Codex `rollout` (19 524 строки, 30 внутренних crates) и `thread-store` (40 235): свой формат `rollout`, SQLite; Goose `session/` (10 911): SQLite через `sqlx` (`G/crates/goose/src/session/session_manager.rs:24`) | 400 / 0 | Формат журнала V4 (поколения, чанки, заголовок `version: 4`) побайтно сверяется в каждом сценарии. Чужой формат несовместим. Теряются **все 242**, а также `sdk/session-resume`, `persistence-provider-unload`, `packed-chunks`, `in-history-series-start`, `subagent-archive-lineage`, `session-query-spill`, `session-reference-spill`, `sdk/session-title-*` | Миграция v0→v4 уже частично сделана (hoh-session-format-*); чужой формат её обесценил бы |
| 11 | subagent, subagent-spawn-in-process, tool-subagent, tool-subagent-control (6 454) | нет | Goose `goose-agent/src/subagent.rs`, `platform_extensions/summon.rs` (2 996 строк); Codex `agent/` внутри `codex-core` | 0 / 0 | Передача, дерево, очередь сообщений, глубина, фильтры инструментов у dsh свои. Теряются `session/subagent-*` (≈ 18), `product-subagent-*`, `team-*`, `sdk/subagent-*`, `acp/` в части дочерних | — |
| 12 | skill-filesystem, tool-skill, tool-workspace-dependencies, agent-preset-registry, agent-preset, workspace, tool-todo, tool-present, tool-web, web-deepseek-api, office-kit, tool-office (9 128) | `notify` для слежения в `skill-filesystem`; `htmd` 0.6.0 (Apache-2.0, 2026-11-18) вместо порта turndown (побайтно не равен, `DEPS.md` §5.4 п.14); `html-to-markdown-rs` 3.22.2 (MIT) требует Rust 1.99 — вне условия ≤ 1.97; `html2md` 0.2.17 — GPL-3.0+; `html2text` 0.17.4 — другой формат вывода. LibreOffice аналога в Rust нет (решение 11) | Goose `skills` (2 075), `slash_commands`, `todo.rs` (172), `computercontroller/{docx,pdf,xlsx}_tool.rs` (читают файлы Office, но не преобразуют и не рендерят, как LibreOffice); Codex `skills` (2 840; зависит от `codex-protocol`) | 200 / 0 | Формат `SKILL.md`, корни поиска и тексты dsh свои; `web_fetch` печатает Markdown от turndown, подмена изменит вывод. Теряются `session/skill-load`, `skill-root-isolation`, `office-*` (6), `workspace-dependencies`, `todo-write`, `repeat-tool-reminder`, `web-fetch`, `deepseek-fetch`, `web-fetch-transport-failure`, `web-search-*` (5) | — |
| | **Итого** | | | ≈ 1 900 / ≈ 200 | | |

## 4. Части открытых движков: лицензия, отделимость

| Движок, часть | Размер (строки .rs, включая тесты) | Лицензия | Отделимость | Вывод |
|---|---|---|---|---|
| Codex `utils/pty` | 10 011 | Apache-2.0 (`C/Cargo.toml:182`); NOTICE у репозитория упоминает Ratatui (`codex/NOTICE`); NOTICE для этого crate **не проверено** | 0 внутренних зависимостей; зависит от `portable-pty` | Годится как справочник; экономит не более 200 строк |
| Codex `file-search` | 1 818 | то же | 0 внутренних; использует `nucleo` (крейт `nucleo-matcher` 0.3.1 лицензирован MPL-2.0 — вне нашего набора) | Строки `fs-search` в отложенных; к 65 не относится |
| Codex `execpolicy` | 3 304 | то же | 1 внутренняя (`utils/absolute-path`) | Правила разрешения команд на их языке; словарь dsh другой |
| Codex `sandboxing`, `linux-sandbox` | 12 083; 14 668 | то же | 26 и 29 внутренних crates (прокси сети, uds, otel, windows-sandbox) | Не отделимы без большого переноса |
| Codex `apply-patch` | 5 555 | то же | 44 внутренних crates (`exec-server` 73 337 строк) | Не нужен: `hoh-fs-patch` готов |
| Codex `shell-command` | 10 544 | то же | 15 внутренних; `tree-sitter-bash`, `tree-sitter-powershell` | Разбор команд для оценки безопасности; в dsh такого пакета нет среди 65 |
| Codex `rollout`, `thread-store`, `state` | 19 524; 40 235; 30 787 | то же | 30 внутренних, свой формат | Несовместим с журналом V4 |
| Codex `rmcp-client`, `mcp` | 44 653; 33 947 | то же | 46 и 71 внутренних | MCP; не нужен для 65 строк |
| Goose `goose-agent` | 5 443 | Apache-2.0 | на crates.io alpha.14; тянет `goose-provider-types` (32 991) и `goose-sdk-types` (3 317) | Другие типы разговора и событий |
| Goose `goose-context-management` | 1 191 | Apache-2.0 | alpha.14; `minijinja`, `rmcp` | Другой алгоритм и тексты |
| Goose `goose-providers` | — | Apache-2.0 | alpha.14 | Провайдеры моделей: у нас `hoh-llm` (принят, 4 245 строк); к 65 не относится |
| Goose `goose` (основной crate): `session` 10 911, `scheduler` 2 023, `skills` 2 075, `agents/platform_extensions/developer` 3 831, `context_mgmt` 1 259 | — | Apache-2.0 | не опубликован отдельно; единый большой crate (конфигурация, провайдеры, telemetry) | Не отделимо |

Версия Rust у Goose 1.95/1.94.1 (`G/Cargo.toml:10`) в пределах нашего «не выше 1.97». Codex использует edition 2024
(`C/Cargo.toml:181`). Лицензии Apache-2.0 в нашем наборе разрешены (`DEPS.md`, шапка); атрибуция Apache-2.0 потребует
записи в `NOTICE`.

## 5. Какие crates на crates.io нашлись по задачам (снято 2026-11-18)

Версия, дата выпуска, лицензия, минимальный Rust. «В наборе» — удовлетворяет условиям `DEPS.md` (лицензия, 2026 год или обоснование, Rust ≤ 1.97).

| Задача | Crate | Версия, выпуск | Лицензия | Rust | В наборе |
|---|---|---|---|---|---|
| MCP | `rmcp` | 3.11.0, 2026-11-17 | Apache-2.0 | 1.88 | да (нужен позже, не для 65) |
| ACP | `agent-client-protocol` | 5.4.0, 2026-11-18 | Apache-2.0 | 1.88 | да (проверить кадрирование — не проверено) |
| Landlock | `landlock` | 0.4.7, 2026-07-27 | MIT OR Apache-2.0 | 1.71 | да |
| seccomp | `seccompiler` | 0.5.0, 2025-03-07 | Apache-2.0 OR BSD-3-Clause | — | нет: выпуск вне 2026 |
| Песочница | `birdcage` 0.8.1 (2024-04-19); `extrasafe` 0.5.1 (2024-04-16) | GPL-3.0-or-later; MIT | — | нет |
| PTY | `portable-pty` 0.9.0 (2025-02-11); `pty-process` 0.5.4 (2026-10-19) | MIT | — | первый принят решением 10 |
| Группы процессов | `process-wrap` 10.0.1 (2026-09-23); `command-group` 5.0.1 (2023-11-18) | Apache-2.0 OR MIT | 1.87; 1.68 | первый да |
| Таблица процессов | `sysinfo` 0.40.4 (2026-11-16); `procfs` 0.18.0 (2025-08-30) | MIT; MIT OR Apache-2.0 | 1.95; 1.70 | первый да |
| Эмулятор терминала | `vt100` 0.18.1 (2026-10-19); `alacritty_terminal` 0.26.0 (2026-04-06) | MIT; Apache-2.0 | 1.85 | да (выбор — в задании `terminal-bash`) |
| Разбор команды | `shlex` 2.0.1 (2026-05-17); `shell-words` 1.1.1 (2025-12-10); `which` 8.0.6 (2026-08-26) | MIT/Apache | — | да |
| Cron | `croner` 4.0.3 (2026-11-04); `cron` 0.17.0 (2026-06-18); `tokio-cron-scheduler` 0.15.1 (2025-10-28); `saffron` 0.1.0 (2021, нестандартная лицензия) | MIT и др. | — | первые два да |
| Повтор | `backon` 1.6.0 (2025-10-18); `tokio-retry` 0.3.2 (2026-06-09); `tokio-retry2` 0.9.1 (2026-01-11); `backoff` 0.4.0 (2021) | Apache-2.0; MIT | — | `tokio-retry*` да |
| Токены | `tiktoken-rs` 0.12.1 (2026-09-24); `tokenizers` 0.23.2 (2026-09-03) | MIT; Apache-2.0 | 1.85 | да, но не совпадает с оценкой dsh |
| Diff | `similar` 3.2.1 (2026-10-24) | Apache-2.0 | 1.85 | да; не побайтно с jsdiff |
| Блокировка файла | `fd-lock` 4.0.4 (2025-03-10); `fs4` 1.1.0 (2026-04-28) | MIT OR Apache-2.0 | 1.75 для `fs4` | `fs4` да |
| Файлы по маске | `ignore` 0.4.34, `globset` 0.4.21 (2026-10-27) | Unlicense OR MIT | 1.88 | да |
| Слежение | `notify` 8.2.2 (2026-11-08) | CC0-1.0 | 1.88 | да (решение 6) |
| HTML в Markdown | `htmd` 0.6.0 (2026-11-18, Apache-2.0); `html-to-markdown-rs` 3.22.2 (MIT, Rust 1.99); `html2text` 0.17.4 (MIT); `html2md` 0.2.17 (GPL-3.0+) | — | — | `htmd`, `html2text` да; побайтно не равны turndown |
| Front matter | `gray_matter` 0.3.2 (2025-07-10) | MIT | — | нет: 2025; нужен `hoh-yaml` |
| JSON-RPC / LSP | `jsonrpsee` 0.26.1 (MIT, 2026-09-30); `async-lsp` 0.2.4 (2026-04-24); `lsp-server` 0.10.0 (2026-07-16); `tower-lsp` 0.20.0 (2023) | — | 1.87 и др. | не подходят для кадрирования dsh |
| Шаблоны | `minijinja` 3.1.0 (2026-11-07) | Apache-2.0 | 1.70 | да (Goose использует для сводки) |
| Готовые агентные каркасы | `rig-core` 0.46.0 (MIT, 2026-10-31, Rust 1.97.1); `genai` 0.7.5 (2026-11-14); `adk-rust` 2.3.0 (2026-10-09); `swarms-rs` 0.3.0; `claude-agent-sdk` 0.1.1 (2025-09-30, обёртка над CLI Claude) | — | — | Каркасы целиком, не части; к строкам не привязаны, **не проверено** |

## 6. Что потеряно при выборе (a) или (b): общий вид

- Равенство с dsh доказывается сверкой записанного поведения (`docs/porting/PORTING.md` §1, §10). Для строки, заменённой
  чужим кодом, **теряются**: (1) её портируемые тесты (всего у 65 строк 142 809 строк тестов по `MAP.tsv`, у
  групп 1, 3, 10, 11 — 49 000 и более); (2) побайтная сверка журнала там, где строка пишет в журнал или влияет на текст
  для модели (группы 1–5, 10–12: **все 242 сценария**); (3) пины заголовка запроса — схемы и описания инструментов.
- **Чем заменить доказательства:** собственные эталонные тесты по спецификациям `PORT_NOTES.md`, снятые вручную
  примеры вывода dsh на вход тех же команд (диффы, Markdown-страницы, сводки) и «золотые» файлы. Это работа,
  сопоставимая с портом тестов, поэтому выгода от пропуска порта сомнительна.
- Движки Codex и Goose не воспроизводят запись V4: ни `rollout`, ни SQLite-сессии Goose не совпадают с форматом dsh;
  замена движка целиком (вне задания) обнулила бы смысл `hoh-conformance`.

## 7. Не вошло в (a)/(b), но снижает объём (d)

Гипотеза, **не проверено**: в `subprocess-local` (5 104 строки) часть кода обслуживает мост Node с Linux/Windows:
`spawn-runner.ts` (488), `runner-launch.ts` (315), `linux-execve.ts`, `runner-protocol.ts`, `control-spawn.ts`,
`windows-inspector.ts` (346) и `windows-job.ts` (Windows отложен решением 5). Они нужны Node, чтобы вызвать `execve` и держать
управляющий канал; в Rust это делается `pre_exec`. Если внешне видимое поведение (коды отказов запуска, тексты
в `bash-startup-timeout`) можно воспроизвести без них, порт сократится на 1 000–1 500 строк. Проверка — разбором
`P/subprocess/subprocess-local/tests` и сценариев `session/bash-startup-timeout`, `bash-tool-turn`.

## 8. Не проверено

- Не открывал содержимое сценариев `snapshots/*`: имена каталогов даны по названиям; отнесение сценария к группе — по
  имени и по составу плагинов в `conformance/EXPECTED.tsv` (почти все 242 включают почти все пакеты, поэтому
  вхождение плагина само по себе ничего не доказывает; поименованы только сценарии, чьё название или вызов инструмента
  (по поиску `"name":"<инструмент>"` в `session*.jsonl`) относится к строке).
- Число строк в `C/` и `G/` считал по `*.rs` вместе с тестами; без тестов цифры меньше.
- Применимость `agent-client-protocol` и `tokio-process-tools` 0.11.3, `process-wrap` к сценариям отмены и завершения
  процессов — не проверено; `sysinfo` — не проверено, даёт ли он время запуска процесса на всех целевых системах.
- NOTICE обоих движков в части сторонних компонентов; совместимость Apache-2.0 с конкретными файлами не выверена.
- Эффект замены на `tokio-retry*`/`backon` на текст и число повторов `llm-retry`; расчёт числа токенов `tiktoken-rs`.
- Глубокое чтение Goose `hints`, `skills`, `slash_commands` и Codex `app-server-protocol`, `skills`: сопоставление с
  dsh сделано по составу модулей и README, не по построчной сверке.
- `hoh-node-addon-system` (готов): что именно из Landlock он покрывает — не перечитывал.
- Время и объём: все цифры экономии в таблице 3 — оценка по размерам файлов dsh, не по пробному переносу.
