# Первая версия: 118 строк описи — состояние на 20.11 15:05

Состав первой версии — `research/v1-engine-scope.codex.md` (решение основателя 18.11): 118 строк `MAP.tsv`, 109 крейтов, 114,8 тыс. строк TS. Остальные 87 строк отложены там же, по группам.

| Состояние | Строк | Крейтов | Строк TS | Крейты |
|---|---:|---:|---:|---|
| Принято, в HEAD | 103 | 94 | 98 366 | — |
| Принято конвейером, ждёт коммита с соседом | 2 | 2 | 432 | subagent-spawn-in-process, tool-subagent-control (← subagent) |
| Пишется / круг исправлений | 8 | 8 | 11 648 | subagent (x5), session-query (x9), agent-preset (x4), skill-office, office-kit, tool-office (x6), tool-subagent (x5), tool-web (x8) |
| Не начато | 5 | 5 | 4 363 | compaction-basic, compaction-tool-result-pruner, file-reference-local, jobs-local, tool-jobs |
| **Итого** | **118** | **109** | **114 809** | |

Замечания:
- Три из пяти «не начатых» (compaction-basic, compaction-tool-result-pruner — x7; file-reference-local — x8) в MAP помечены `writing` с 19.11 09:43, но крейтов нет: писателей тогда перевели на веху. Отметки были ложными; выдаю заново.
- **Этап 4 (ACP, LSP, SSH — 10 строк) в 118 не входит.** Все 10 (acp, acp-app, lsp, lsp-stdio, tool-lsp, ssh, fs-ssh, sandbox-ssh, ssh-helper-runtime, subprocess-ssh) — в отложенной группе «Integrations/developer features… outside first acceptance flow». Этап 5 (Windows) тоже отложен.
- `port-status.sh` (147 из 205) считает все 205 строк переноса, а не 118 первой версии.
- Ночью писатели вели и пакеты из отложенных 87 (goal, agent-team, terminal-bash, mcp-manager, otel, session-stats/reference/turn-outline, message-quotes, spill и др.) — по порядку этапов MAP. Отныне строки первой версии идут первыми; отложенные круги исправлений — после них.
- `hoh-web-search-yandex` — новый крейт HoH (не строка описи): поставщик поиска для первой версии по выбору основателя 18.11.
- Сервисы `hoh-employee`, `hoh-task` (новая работа HoH поверх 118) — отдельная записка `docs/employee-task-services.md`.
