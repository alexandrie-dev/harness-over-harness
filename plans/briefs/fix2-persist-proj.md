# Второй круг исправлений: hoh-session-persistence и hoh-session-projection

Проверка закрытия (Codex, только чтение): `/mnt/toshiba/alexandrie/hoh/logs/rc-persist.full.md` — прочитай целиком.
Исходник dsh: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/{session-persistence,session-projection}`.
Первый круг сделал s12 (записи в PORT_NOTES обоих крейтов). Каждое исправление — с тестом, который падает на текущем коде.

1. **Порядок handoff в `register()`** (`crates/hoh-session-projection/src/registry.rs:303`): в TS handoff выполняется только
   после завершения текущего синхронного стека, поэтому `register(); owner.dispose()` даёт отказ. Сейчас задача из
   `ctx.spawn` на многопоточном Tokio может зафиксировать успех раньше `dispose()`. Сделай порядок детерминированным
   (например, решение принимается не раньше первого опроса/следующего шага, а не в отдельной задаче) и добавь многопоточный
   тест с управляемой синхронизацией на оба порядка (`register; dispose` → отказ; передача до выгрузки → успех).
2. **Возвращаемый заголовок** (`crates/hoh-session-persistence/src/storage_contract.rs:115`): возвращай снимок после
   слияния `extra`, как TS:120. Тест: `created_at=-1`, `extra.createdAt=7` → у результата createdAt == 7.
3. **`__proto__` в `restore()`** (`crates/hoh-session-projection/src/registry.rs:740`): как в `checkpoint()` — ключ не
   попадает в обновлённый checkpoint; TS даёт `{}`. Тест из ревью (host-only счётчик, версия 1, состояние 0, пустой лог).

## Условия
Меняешь только `crates/hoh-session-persistence`, `crates/hoh-session-projection`. Сборка
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-sonnet-persist`, `CARGO_BUILD_JOBS=2`, `--offline`; остановленные
регулятором (T/Tl) сборки не возобновлять. Проверки: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check` по двум крейтам. Ничего не устанавливать, не коммитить. Отчёт по-русски: «ГОТОВ fix2 persist/proj:
тесты N/0, clippy, fmt», по каждому пункту — правка и тест.
