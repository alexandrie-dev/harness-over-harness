# Круг исправлений hoh-agent по ревью rv-agent

Ревью (Codex, только чтение): `/mnt/toshiba/alexandrie/hoh/logs/rv-agent.full.md` — прочитай целиком. Исходник:
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/agent`. Автор крейта s6 пишет hoh-agent-loop; на время круга
`crates/hoh-agent` твой (s6 его не трогает). Пункт 3 ревью (hoh-agent-loop/src/factory.rs) — НЕ твой, его делает s6.

Исправь пункты 1, 2, 4, 5, 6, 7, 8 — каждый с подтверждающим тестом из ревью (тест падает на текущем коде):
1 (P1) выгрузка из initiator-границы не взаимоблокируется; убери обход `release_reentrant_initiator_runs()` из теста
  `tests/agent_initiator.rs:237`, тест проходит без него.
2 (P1) announce и detach атомарны: либо ошибка `not live`, либо упорядоченные created/disposed; многопоточный тест с барьером.
4 (P2) fused dispatcher перезаписывает `agent` своим; перенеси исходный тест «injects the fused subject…»; исправь PORT_NOTES.
5 (P2) инициатор виден во вложенной задаче (как AsyncLocalStorage); если для `tokio::spawn` без обёртки это невозможно — дай
  обёртку spawn, которая переносит инициатора, используй её во всех местах крейта и запиши правило в PORT_NOTES.
6 (P2) новая граница при выгружаемом предке — `agent initiator scope is disposed`.
7 (P2) схема modelSelection: отсутствующие `lastUsed`/`pending` отвергаются, как в TS model-selection.ts:76.
8 (P3) тексты предупреждений как в TS (`listener threw: Error: …`), тесты с точными ожиданиями TS.

## Условия
Меняешь только `crates/hoh-agent`. Сборка `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-sonnet`, `CARGO_BUILD_JOBS=2`,
`--offline`; остановленные регулятором (T/Tl) сборки не возобновлять. Проверки: `cargo test -p hoh-agent`, `cargo clippy -p
hoh-agent --all-targets -- -D warnings`, `cargo fmt -p hoh-agent --check`; плюс `cargo check -p hoh-agent-loop --tests` (не
сломать соседа). Ничего не устанавливать, не коммитить. Отчёт по-русски: «ГОТОВ fix hoh-agent: тесты N/0, clippy, fmt»,
по каждому пункту — правка и тест.
