# Круг исправлений hoh-tools (по ревью Codex)

Рабочий каталог `/home/save/Alexandrie/HarnessOverHarness`. Крейт `crates/hoh-tools` (ты или предыдущий писатель
перенёс его из `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/tools`). Ревью:
`/mnt/toshiba/alexandrie/hoh/logs/rv-tools.report.md` — прочитай полностью, там вход, ожидаемое и предложенный тест
для каждого пункта. Ведущий принял все 13 пунктов; исправь все, к каждому — регрессионный тест с ожиданием из источника.

Кратко: 1 канонический результат изменяем (замороженность в TS) — изменённый обёрткой результат не должен проходить
как канонический; 2 атомарная регистрация исполнителя в области; 3 порядок execute(A) раньше pre(B) в batch.rs:252;
4 записи по областям не держат область сильно (TS WeakMap), но живые области сохраняют «недоступно»;
5 проверка полной сериализации отложенного контекста (-0 в source); 6 паника условия активации → ToolActivationError
TOOL_ACTIVATION_FAILED (и при опросе future); 7 конвейер без сервиса tools создаётся, отказ — только когда нужен;
8 `{type:"tuple"}` — как в источнике (`unreachable variant in JsonSchemaType: "tuple"`), вернуть ожидание теста
источника; 9 простые ошибки остаются простыми (`ToolsError::Error`, без info); 10 конфиг принимает посторонние поля
как в источнике; 11 явный null у rawInput сохраняется; 12 экспортировать isPlainJsonRecord/isJsonSchemaRecord/
isPlainJsonArray (или запись об исключении с причиной — лучше экспорт); 13 тест составного disposer через Undos.

Также: сквозной clippy соседних крейтов ранее падал на hoh-tools — после правок `cargo clippy -p hoh-tools
-p hoh-tool-executor-native --all-targets -- -D warnings` должен быть чист. Добавь в hoh-tool-executor-native тест
describe для defer_loading None / Some(false) / Some(true) (ключ опущен / опущен / `"deferLoading":true`).

Правила сборки: `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s7 CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true`;
пока пишешь — `cargo check`; тесты, clippy, fmt — один раз в конце. Сборки, остановленные общим регулятором
(состояние T/Tl), НЕ возобновлять самому — ждать. Чужие крейты, кроме теста в hoh-tool-executor-native, не менять.
Ничего не устанавливать и не публиковать. Последнее сообщение — отчёт по-русски: пункт → правка → тест → результаты команд.
