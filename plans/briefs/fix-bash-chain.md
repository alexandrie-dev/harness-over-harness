# Круг исправлений по ревью rv-bash: hoh-bash-local, hoh-shell-env, hoh-tool-bash

Ревью (Codex, только чтение): `/mnt/toshiba/alexandrie/hoh/logs/rv-bash.full.md` — прочитай целиком. Исходники dsh:
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/shell/{bash-local,shell-env,tool-bash}`. Автор крейтов (s9) занят
другим пакетом; крейты на это время твои. Каждое исправление — с подтверждающим тестом из ревью (тест сначала падает).

1. **Порядок `on_started` → завершение** (`crates/hoh-bash-local/src/execution.rs:473`): стартовый хук всегда раньше
   settlement; стартовый хук не перезаписывает итоговые факты sandbox. Тест: уже завершённый провайдер, задержка в
   `on_started`, многопоточный runtime.
2. **Уведомление о восстановлении рабочего каталога** (`crates/hoh-tool-bash/src/execute.rs:356`): агент получает текст
   `The working directory "<old>" is unavailable. The working directory is now "<new>".`, как в TS (`tool-bash/index.ts:533`).
   Если для `inject` нужен настоящий `Agent` и его нет — сделай адаптер, который доставляет сообщение, и тест через bash tool.
3. **`DSH_INSTALL_DIR`** (`crates/hoh-shell-env/src/lib.rs:262`): корень установки, а не каталог бинарника. Если в
   `ProfileContext`/`hoh-loader` нет якоря установки — добавь параметр/провайдер в hoh-shell-env и верни точный тест
   `shell-env.spec.ts:71` (каталог установки ≠ каталог бинарника). hoh-loader не правь (его пишет l1) — опиши, что ему нужно.
4. **Дробный `maxOutputBytes`** (`crates/hoh-bash-local/src/lib.rs:176`): `0.5` принимается как в TS (`index.ts:128`), без
   паники; общее правило — конфигурация, которую TS принимает, не вызывает панику. Тест через pipeline.

Также поправь текст в PORT_NOTES hoh-tool-bash: признак продвижения не «всегда мёртв» — условие истинно, если в presenter
приходит `ToolExecutionSuccess` с `value` (ревью, `pipeline.ts:626`, `schema.ts:623`); перенеси это поведение, если тип
позволяет, иначе запиши точное условие.

## Условия
Меняешь только `crates/hoh-bash-local`, `crates/hoh-shell-env`, `crates/hoh-tool-bash`. Сборка:
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-sonnet`, `CARGO_BUILD_JOBS=2`, `--offline`; остановленные регулятором
(T/Tl) сборки не возобновлять. Проверки: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` по трём
крейтам. Ничего не устанавливать, не коммитить, не публиковать. Отчёт по-русски: «ГОТОВ fix bash: тесты N/0, clippy, fmt»,
по каждому пункту — что изменено и какой тест.
