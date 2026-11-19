# hoh-tmux-context ← packages/context/tmux-context @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | плагин, durable-проекция `tmuxContext`, prepended pre-step listener, tmux-запрос, throttling и snapshot-context перенесены |
| `tests/tmux-context.spec.ts` | `tests/tmux_context.rs` | все 18 объявлений и все 4 строки `it.each`, плюс 6 regression-сценариев ревью: 27 Rust-тестов |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `package.json`, конфиги сборки | `PORT_NOTES.md`, `Cargo.toml` | прочитаны; отдельной лицензии в пакете нет |

Сохранены точная shell-команда с проверкой tty, восемь полей tmux, отказ без инъекции при nonzero/malformed/empty pane,
изоляция ошибок resolve/execute/result, durable stable state без заголовка turn, suppress неизменившегося location,
проверка interval до запуска shell и source metadata `tmux-context`/`snapshot`.

## Отличия от образца

1. TypeScript function-plugin представлен типизированными `TmuxContext`, `Deps`, `Config` и константами `NAME`/`INJECT`.
2. `refreshIntervalMs` хранится как `Option<f64>`, но custom-deserializer и JSON schema принимают только отсутствующее
   поле либо число: явный `null`, как и у исходного `z.number().optional()`, отклоняется. Runtime-проверка воспроизводит
   zod-отказ для дробных, отрицательных, бесконечных и небезопасных чисел до преобразования в `u64`.
3. Общий JavaScript Promise регистрации представлен `tokio::sync::watch`: listener устанавливается до ожидания,
   а перекрывающиеся вызовы получают один и тот же `ProjectionHandle` либо одну `SharedError`. Doc-hidden
   `TmuxContext::apply_with_registration_gate` — только conformance seam: тесты детерминированно держат регистрацию
   в полёте и проверяют ready/cancelled/disposed/failed тем же production listener-путём.
4. Rust `ShellExecutor::resolve` синхронен и не возвращает `Result`. Для сохранения source-containment ошибочной
   реализации panic на resolve, при создании futures execute/result и при их polling/await перехватывается и даёт тот
   же warning/no-location исход. `futures-util` уже был workspace/Cargo.lock-зависимостью; новых внешних пакетов нет.
5. Произвольный JavaScript non-`Error` rejection на Rust-границе представлен `BoxError::from(String)` из `result()`;
   точный текст сохраняется в warning.
6. При dispose во время ожидающей регистрации Rust-шаг отклоняется текстом `inactive context`; исходный
   `ctx.fiber.assertActive()` также отклоняет шаг, но формулировка его ошибки отличается. В обоих случаях shell,
   событие контекста и дальнейший request не создаются.

## Не перенесено

Нет исключённых production-экспортов или сценариев тестов. TypeScript declaration merging заменён kernel service API.

## Не проверено

- Запуск внутри настоящего tmux не выполнялся: shell boundary проверен fake executor с точной командой и ответами.
- Платформенных веток нет; команда предполагает Unix `ps`, tty и tmux, как и исходный пакет.

## Вопросы ведущему

- Нет. Новых внешних зависимостей и правок чужих API нет.

## Проверка

- `cargo test -p hoh-tmux-context --offline`: 27 passed, 0 failed.
- `cargo clippy -p hoh-tmux-context --all-targets --no-deps --offline -- -D warnings`: чисто.
- `cargo fmt -p hoh-tmux-context -- --check`: чисто.

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, offline и под внешним `timeout`.
