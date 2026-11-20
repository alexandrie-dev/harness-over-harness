Первый показ я бы готовил в порядке **`session/fs-read` → `session/fs-write` → `session/bash-tool-turn` → `session/text-turn` → `sdk/text-turn`**. Первые три действительно вызывают по одному инструменту; headless text-turn дополнительно требует миграции V1, SDK text-turn — feedback-сервисы. Основания: `snapshots/session/{fs-read,fs-write,bash-tool-turn}/session.v4.jsonl:16`, `snapshots/session/text-turn/snapshot.yml:6`, `snapshots/sdk/text-turn/feedback-producer.mjs:9`.

Ниже `P/` — `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/`, `S/` — `snapshots/` того же клона; `M:n` — `docs/porting/MAP.tsv:n`, `E:n` — **`conformance/EXPECTED.tsv:n`**: таблица находится в корне репозитория.

Работа только чтением; ничего не изменялось, не собиралось и не проверялось исполнением.

**Сначала требуется закончить проверяющий тракт.** Conformance запускает штатный бинарник с профилем и патчами; модель воспроизводится плагином на уровне `StreamChunk`, инструменты исполняются реально. Сравниваются нормализованные журналы, промпты, схемы, файловое дерево, headless stdout/stderr/exit и протокольные результаты SDK/ACP. Это 192 session + 41 SDK + 9 ACP; браузерные сценарии исключены (`crates/hoh-conformance/README.md:10`, `:21`, `:53`, `:157`, `:582`).

Сейчас модельные сценарии упираются в `strictV4Catalog not implemented`, все SDK — в незаконченные сценарные assertions. Поэтому порт runtime сам по себе ещё не даёт `pass` (`crates/hoh-conformance/src/runner.rs:379`, `:425`). Положительный сквозной результат настоящего `hoh` не подтверждён (`crates/hoh-conformance/PORT_NOTES.md:191`, `:223`).

Ранжирование ниже — **консервативное статическое замыкание**, а не доказанный минимум исполняемого кода — **не проверено**. Учитываются активные плагины из EXPECTED, обязательные `dependencies`/необязательными не объявленные peers, библиотеки вызываемых инструментов, launcher, replay и conformance; `done` исключены. Бандлы считаются данными, SDK startup — отдельно от состава его профиля. Windows-ветви Linux-переноса исключены. Это соответствует принятому разделению профилей и платформ (`crates/hoh-conformance/README.md:57`, `:642`; `P/bundle/base/src/index.ts:1`; `P/bundle/sdk-app/src/index.ts:8`; `research/v1-engine-scope.codex.md:56`).

Исправление к EXPECTED: headless/SDK отключают `session-telemetry-otel` последним boot-патчем; ACP его сохраняет (`crates/hoh-conformance/README.md:110`; `P/boot/app-boot/src/profile-context.ts:52`).

Полный рейтинг 242 сценариев представлен через номера строк E; внутри одинакового N порядок равный.

| Места | N незавершённых crates | Строки E |
|---|---:|---|
| 1 | 55 | 208 |
| 2–4 | 84 | 8,10,55 |
| 5–7 | 85 | 9,104,137 |
| 8 | 86 | 105 |
| 9–10 | 89 | 159,211 |
| 11–14 | 90 | 11,52,115,164 |
| 15–33 | 91 | 116–129,131–135 |
| 34–124 | 92 | 13–14,16–18,23,25,32,34,39–43,46,51,53–54,57–61,63,65–79,83,87–90,100,102–103,107–113,130,138,145,149–151,155,157–158,160,162–163,166–171,173–189,191–192 |
| 125–174 | 93 | 2–7,12,15,22,24,28–31,36–38,44–45,47–50,56,62,64,80,82,85,91–92,101,106,114,136,139–144,146,148,153,156,161,165,172,190,193 |
| 175–189 | 94 | 19–21,26–27,84,93–97,99,147,152,154 |
| 190–194 | 95 | 33,35,81,86,98 |
| 195–196 | 99 | 213–214 |
| 197–214 | 100 | 194–195,207,209,216,224–226,233,235–243 |
| 215–229 | 101 | 197,203,205,215,218–223,227–229,231,234 |
| 230 | 102 | 232 |
| 231 | 103 | 217 |
| 232 | 107 | 196 |
| 233–242 | 108 | 198–202,204,206,210,212,230 |

Пять самых дешёвых кандидатов и пять предпочтительных проб:

| Сценарий | Места; N; исходных строк | Точное учтённое множество¹ | Вызовы и дополнительные условия |
|---|---|---|---|
| `sdk/minimal-image-compaction` | 1; 55; 80 439 | C ∪ M | `read_image`; compaction, image offload, checkpoint observer. `S/sdk/minimal-image-compaction/session.v4.jsonl:13`, `:16`; `checkpoint-observer.mjs:3`; E:208 |
| `session/apply-patch-partial` | 2–4; 84; 91 895 | C ∪ H | Один `apply_patch`; readonly setup, частичный успех. `S/session/apply-patch-partial/session.v4.jsonl:16`; `snapshot.yml:9`; E:8 |
| `session/apply-patch-update` | 2–4; 84; 91 895 | C ∪ H | Два `read`, один `apply_patch`. `S/session/apply-patch-update/session.v4.jsonl:16`, `:17`, `:23`; E:10 |
| `session/fs-glob-sampling` | 2–4; 84; 91 895 | C ∪ H | Один `glob`, sampling и spill; фиксированные mtimes. `S/session/fs-glob-sampling/session.v4.jsonl:16`; `snapshot.yml:11`; E:55 |
| `session/apply-patch-rejected` | 5–7; 85; 92 065 | C ∪ H ∪ R | `apply_patch`, `read`, повторный `apply_patch`; observation policy включена runtime-патчем. `S/session/apply-patch-rejected/session.v4.jsonl:16`, `:21`, `:26`; `runtime.cordis.yml:1`; E:9 |
| `session/fs-read` | 34–124; 92; 99 393 | (C ∪ H ∪ D) − I | Один `read`. `S/session/fs-read/session.v4.jsonl:16`; E:57 |
| `session/fs-write` | 34–124; 92; 99 393 | (C ∪ H ∪ D) − I | Один `write`, сравнение итогового дерева. `S/session/fs-write/session.v4.jsonl:16`; `snapshot.yml:8`; E:60 |
| `session/bash-tool-turn` | 34–124; 92; 99 393 | (C ∪ H ∪ D) − I | Один `bash`. `S/session/bash-tool-turn/session.v4.jsonl:16`; E:17 |
| `session/text-turn` | 34–124; 92; 99 393 | (C ∪ H ∪ D) − I | Без инструментов, вход V1. `S/session/text-turn/session.v1.jsonl:5`; `snapshot.yml:6`; E:173 |
| `sdk/text-turn` | 230; 102; 108 834 | ((C ∪ H ∪ D) − I − `headless`) ∪ S | Без model tool calls, но с feedback producer. `S/sdk/text-turn/snapshot.yml:6`; `feedback-producer.mjs:14`; E:232 |

¹ Группы раскрыты ниже; `I={plugin-package-inventory-deepseek}`. Все имена имеют префикс `hoh-`, кроме `apps/hoh`. Числа — **полный исходный TS-объём MAP**, не остаток работы и не прогноз Rust-объёма.

Абсолютный лидер сложнее обычного текстового хода: его assertions требуют один summary, один offload и правильную последовательность уведомлений (`S/sdk/sdk.snapshot.ts:1480`). Поэтому минимальное N не равно минимальному сроку.

Объединение десяти множеств — **108 crates, 112 556 исходных строк**. Таблица одновременно раскрывает множества и очередь зависимостей. `Rvw=review`; остальные статусы написаны буквально; без пометки — `todo`. Завершённые зависимости опущены.

| Волна | Crates: группа; строки; статус; источник |
|---|---|
| 0 | attachment-local(C) 1559 Rvw [M:16]; authorization(S) 541 [M:128]; base(H) 9 [M:124]; credentials-local(S) 930 `writing:w1` [M:129]; deepseek-llm-api-extensions(C) 203 Rvw [M:52]; loader(C) 8050 `writing:l1` [M:2–4,7–9]; office-kit(S) 1399 [M:37]; sdk-minimal(M) 9 [M:18]; session(C) 3485 `writing:w8` [M:30]; session-format-v1-to-v2(C) 1256 Rvw [M:73]; skill(D) 902 Rvw [M:87]; storage-json(H) 844 Rvw [M:158]; subprocess-local(C) 5104 [M:94]; util-ripgrep(H) 109 [M:166]; web(H) 507 Rvw [M:168] |
| 1 | otel(H) 475 [M:165]; sandbox(C) 759 [M:61]; sdk-app(M,S) 62 [M:17]; session-format-v2-to-v3(C) 815 `writing:c4` [M:74]; session-log-deepseek(C) 396 [M:76]; session-persistence(C) 636 [M:77]; session-projection(C) 1031 [M:79]; settings(H) 704 `writing:w5` [M:84]; skill-office(S) 141 `writing:w5` [M:88]; spill(H) 138 [M:155]; web-fetch-http(H) 601 `writing:w1` [M:170] |
| 2 | fs(C) 640 [M:39]; sandbox-local(C) 686 [M:62]; session-format-v3-to-v4(C) 1702 [M:75]; session-projection-cache(H) 579 [M:80]; shell(H) 377 [M:85]; spill-local(H) 727 [M:156]; workspace(M,S) 1448 [M:123] |
| 3 | agent(C) 1791 [M:26]; bash-local(H) 404 [M:149]; fs-local(C) 1401 [M:40]; fs-observation-policy(R) 170 [M:132]; llm-deepseek(C) 3737 [M:54]; skill-filesystem(D) 1064 [M:153] |
| 4 | agent-default-model(H) 98 [M:27]; commands(C) 734 [M:45]; goal(D,M) 1309 [M:43]; jobs(C) 714 [M:50]; llm-deepseek-api-key(C) 185 [M:55]; llm-retry(C) 319 [M:56]; plugin-package-inventory-deepseek(H,M,S) 233 [M:57]; sandbox-policy(C) 328 [M:63]; session-title(C) 1027 [M:81]; terminal(M) 694 [M:95]; user-approval(C) 430 [M:48]; user-questions(C) 964 [M:49]; web-search-deepseek(H) 696 [M:171]; working-directory(C) 290 [M:83] |
| 5 | bash-sandbox(H) 206 [M:150]; command-feedback(H) 197 [M:130]; command-goal(D) 200 [M:135]; compaction(C) 504 [M:19]; fs-sandbox(H) 257 [M:133]; jobs-local(C) 1050 [M:51]; permission-presets(H) 556 [M:46]; terminal-bash(M) 1369 [M:96]; tools(C) 3797 [M:33]; web-deepseek-api(H) 1052 [M:169] |
| 6 | agent-instructions(D) 2114 [M:127]; agent-loop(C) 2149 [M:28]; command-compact(H) 109 [M:126]; compaction-image-offload(C) 197 [M:21]; mcp-resources(C) 223 [M:59]; message-feedback(S) 490 [M:131]; plan-mode(H) 580 [M:141]; repeat-tool-reminder(H) 234 [M:138]; session-checkpoint-policy(C) 83 [M:69]; shell-env(H) 255 [M:151]; tool-bash-persistent(M) 488 [M:86]; tool-call-timeout-policy(H) 81 [M:139]; tool-executor-native(C) 35 [M:32]; tool-fs(C) 2444 [M:42]; tool-fs-search(H) 1546 [M:134]; tool-goal(D) 482 [M:137]; tool-jobs(D) 698 [M:140]; tool-skill(D) 452 [M:154]; tool-todo(H) 271 [M:103]; tool-web(H) 1054 [M:167]; tool-working-directory(C) 43 [M:82] |
| 7 | session-format-catalog(C) 154 [M:71]; token-meter(C) 1625 [M:58]; tool-bash(H) 871 [M:152] |
| 8 | compaction-tool-result-pruner(H) 303 [M:22]; conformance(C) 4819 Rvw [M:101]; llm-replay(C) 1212 [M:99]; session-persistence-jsonl(C) 5145 [M:78]; session-query(H) 2241 [M:68]; spill-policy(H) 346 [M:157] |
| 9 | compaction-basic(C) 1789 [M:20]; session-query-sqlite(H) 1781 [M:144]; subagent(C) 5057 [M:92] |
| 10 | agent-team(H) 1109 [M:163]; goal-round-driver(D) 510 [M:136]; headless(H) 992 [M:125]; schedule(M,S) 3569 [M:64]; sdk-protocol(M,S) 571 [M:66]; subagent-fork-in-process(H) 83 [M:159]; subagent-spawn-in-process(H) 59 [M:160]; tool-subagent(H) 965 [M:161]; tool-subagent-control(H) 373 [M:162] |
| 11 | sdk-jsonrpc-server(M,S) 1049 [M:67]; tool-agent-team(H) 235 [M:164] |
| 12 | apps/hoh(C) 1070 [M:13] |

Волны учитывают optional-связь, когда оба пакета уже выбраны. Например, subagent использует session-query, а compaction-basic — pruner; это уточняет общий порядок исходной wave-таблицы (`P/subagent/subagent/package.json:71`, `:96`; `research/v1-engine-scope.codex.md:37`).

Самая длинная незавершённая цепочка этого графа:

`session → session-persistence → workspace → agent → commands → compaction → compaction-image-offload → session-format-catalog → session-query → subagent → schedule → sdk-jsonrpc-server → apps/hoh`

**13 узлов, 21 935 строк**, включая финальную компоновку бинарника. Последовательные основания: `P/session/session-persistence/package.json:34`; `P/workspace/workspace/package.json:42`; `P/core/agent/package.json:47`; `P/interaction/commands/package.json:57`; `P/compaction/compaction/package.json:42`; `P/compaction/compaction-image-offload/package.json:41`; `P/session/session-format-catalog/package.json:44`; `P/session-query/session-query/package.json:65`; `P/subagent/subagent/package.json:71`; `P/schedule/schedule/package.json:60`; `P/sdk/server/package.json:46`; `P/bundle/sdk-app/cordis.patch.yml:17`. Это цепочка зависимостей, не оценка календарного срока — **не проверено**.

Есть неоценённая в crates работа: стандартные профили содержат пакеты с MAP-решениями `skip/later/redesign`: Typert/API, plugin-manager, account/pi-ai, PTC/workflow; headless — ещё hooks. Их нельзя молча считать реализованными. Нужны совместимые поверхности либо доказанное исключение без изменения сравниваемых данных (`E:57`, `E:232`; M:216,218,226,337–338,385–388,397–405). Локальные observer, spill-locator и feedback producer также требуют Rust-эквивалентов (`S/sdk/minimal-image-compaction/checkpoint-observer.mjs:9`; `S/session/apply-patch-update/cordis.snapshot.yml:83`; `S/sdk/text-turn/feedback-producer.mjs:9`).

Сокращать следует **реализацию ветвей, сохраняя наблюдаемую поверхность**:

| Можно отложить | Что сохранить |
|---|---|
| Сетевой DeepSeek-адаптер и SDK fallback | Replay уже предоставляет выбранный маршрут; fallback вызывается только при отсутствии адаптера. Общие helper-типы web пока импортируются — их придётся выделить. `S/session/text-turn/cordis.snapshot.yml:4`; `P/sdk/server/src/server.ts:233`; `P/web/web-deepseek-api/src/index.ts:27` |
| SQLite-поиск | Exact reads/API session-query; профиль имеет `openAt: never`. `P/bundle/base/cordis.patch.yml:149` |
| Zstd-сжатие | JSONL V4, flush/checkpoint и строгую валидацию: сценарные патчи задают `compression: none`. `S/session/text-turn/cordis.snapshot.yml:14`; `P/session/session-format-v3-to-v4/src/index.ts:6` |
| Выполнение невызванных subagent/workflow/web/goal-инструментов | Их точные схемы, prompt contributions и root identity. В эталоне перечислены даже невызванные инструменты. `S/session/text-turn/tool-schemas.expected.json:243`, `:328`, `:499`, `:518`; `S/session/fs-read/session.v4.jsonl:11` |
| Полный compaction для обычных коротких ходов | Регистрацию и отсутствие лишних событий; для minimal-image-compaction сокращение недопустимо. `S/sdk/sdk.snapshot.ts:1493` |
| Windows, дополнительные провайдеры | Linux-путь и выбранный каталог replay. `research/v1-engine-scope.codex.md:61`; `S/session/text-turn/cordis.snapshot.yml:35` |

Эквивалентность сокращений до прогона — **не проверено**. Ослаблять нормализацию, подставлять tool results или удалять проверки нельзя (`crates/hoh-conformance/README.md:580`).

Для примера основателя обязательны вопросы, fetch открытых источников, запись программы, sandbox, сохранение задачи/сотрудника и просмотр результата. Расписание явно необязательно (`docs/window.md:92–104`). В baseline v1 следующую полноту можно оставить последней; части этих crates всё же нужны раньше для эталонных профилей.

| Поздняя полнота v1 | Почему не обязательна описанному примеру |
|---|---|
| office-kit, tool-office, skill-office; tool-workspace-dependencies [M:37–38,88–89] | Пример выдаёт страницу и фотографии; Office/runtime payload отдельно не требуются. `docs/window.md:103`; `research/v1-engine-scope.codex.md:19` |
| terminal, terminal-bash, persistent bash [M:95–96,86] | Для программы достаточно обычного shell/sandbox; нужны minimal-профилю, не самому примеру. `research/v1-engine-scope.codex.md:74`; `P/bundle/sdk-minimal/cordis.patch.yml:144` |
| subagent spawn/fork/control и команды команды [M:92,159–164] | Сотрудник и отдельная builder-session не требуют дерева делегирования — проектное сокращение, **не проверено**. `docs/window.md:92`, `:120`; `research/v1-engine-scope.codex.md:16` |
| tool-todo, goal/plan/compact-команды [M:103,126,135–141] | Полнота dsh-инструментария сверх описанной последовательности. `research/v1-engine-scope.codex.md:75–76` |
| DeepSeek account/search, feedback/телеметрия, расширенный history/indexing | Для локальной модели и собственного fetch/search не обязательны; SDK text-turn требует feedback раньше. `research/v1-engine-scope.codex.md:73`, `:77`; `S/sdk/text-turn/feedback-producer.mjs:14` |
| schedule/tool-schedule [M:64–65] | Можно отложить только необязательный шаг примера; это продуктовая функция, не исключительно совместимость. `docs/window.md:101` |

Очередь писателей после критических волн:

| Порядок | Следующая работа |
|---|---|
| 1 | Закрыть format/V4 и conformance gates; затем волны 0–12 выше. Сначала реализовывать срез для read/write/bash, сохраняя весь сравниваемый каталог. `crates/hoh-conformance/src/runner.rs:379`; `README.md:589` |
| 2а — пример | `tool-ask-user`, `tool-present`, `agent-preset-registry`, `file-reference`, `time-context`; затем `agent-preset`, `file-reference-local`; необязательный `tool-schedule`. Источники: M:23–25,36,47,60,65,142; зависимости: `P/preset/agent-preset/package.json:35`, `P/context/file-reference-local/package.json:37` |
| 2б — собственный HoH | Адаптер локального OpenAI-compatible/Qwen → employee/task/versioning → кнопки/artifact API → Tauri approvals/preview → настоящий пример. Qwen-совместимость **не проверено**. `research/v1-engine-scope.codex.md:12`, `:84–87`; `docs/window.md:112–137`, `:176` |
| 3а — остальное, независимые основания | Office/runtime tools [M:38,89]; testkit/mock/smoke [M:97–98,100,102]; telemetry/title-llm [M:145,148]; browser/computer/context/presentation/changes/editor [M:172–174,176–179]; mcp-client [M:180]; templates [M:182–183]; export/quotes/stats/outline/storage-sqlite/text-search [M:184–189]; lsp [M:192]; ssh [M:197] |
| 3б — их потребители | telemetry-otel, title-first-prompt-llm, session-reference, mcp-manager, acp, lsp-stdio/tool-lsp, fs-/sandbox-/subprocess-ssh. Источники: M:146–147,175,181,190,193–196,199; зависимости: `P/mcp/mcp-manager/package.json:52`, `P/acp/acp/package.json:39`, `P/lsp/lsp-stdio/package.json:34` |
| 3в — завершающие оболочки | acp-app; ssh-helper-runtime. M:191,198; `P/bundle/acp-app/package.json:41`; `P/ssh/ssh-helper-runtime/package.json:34` |
| 4 — платформенная полнота | win32-process/pwsh-local → sandbox-windows-acl/pwsh-sandbox → pwsh tools; реальные платформенные проверки отдельно. M:200–205; `P/sandbox/sandbox-windows-acl/package.json:44`; `P/shell/pwsh-sandbox/package.json:34` |

Инфраструктура `llm-mock-server` не сокращает путь к снимочным сценариям: они используют `llm-replay` (`crates/hoh-conformance/README.md:17`, `:157`).