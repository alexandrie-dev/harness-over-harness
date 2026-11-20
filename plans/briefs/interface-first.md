# Writing ahead of dependencies (applies to assignments that point here)

The founder asked for more parallel work; the CPU is the bottleneck (load 30–46 on 12 threads). So:

1. **Write against fixed interfaces.** Your package's dependencies may not be accepted yet (status `writing:` or
   `review` in `docs/porting/MAP.tsv`). Use their current Rust API in `crates/<dep>/src` as it is. If what you need
   is missing there, do **not** edit the dependency: write against the source's types and signatures (from dsh) in a
   private `shim` module of your crate, marked `// SHIM(<dep>): <what>; replace when <dep> provides it`, and list each
   shim in PORT_NOTES under «Вопросы ведущему». Integration (removing shims) happens after the dependency is accepted.
2. **Light builds.** `CARGO_BUILD_JOBS=2`. While writing, use `cargo check -p <your crate> --tests` only. Run
   `cargo test`, `clippy` and `fmt --check` **once**, at the end, when the code is complete. If your crate does not
   compile because a dependency in progress changed under you, wait a few minutes and retry; never edit it.
3. Everything else as in `plans/briefs/stage1-writer.md` and `docs/porting/PORTING.md`.

## Новый крейт создаётся сразу целиком (19.11 06:00)
Рабочее пространство подхватывает каждый каталог `crates/*`. Каталог без `Cargo.toml` или без `src/lib.rs` ломает
загрузку workspace у всех писателей и приёмок. Создавая крейт, первым же действием пиши `Cargo.toml` (как у соседних
крейтов: `version/edition/rust-version/publish.workspace = true`, `[lints] workspace = true`) и `src/lib.rs` (хотя бы
строку комментария); только потом остальные файлы.

## Остановленные сборки (с 18.11 вечер)
Сборки проекта ограничивает общий регулятор `scripts/cargo-governor.sh`: лишние cargo он ставит на паузу (состояние `T`/`Tl`).
Это не сбой. Не возобновляй их сам (`kill -CONT` и подобное запрещено) — дождись, регулятор продолжит их, когда освободится место.

## Сдача по одному пакету (CEO 18.11 23:45)
Если в задании несколько пакетов, сообщай о готовности каждого сразу, как только его тесты, clippy и fmt чисты:
одной строкой в своём ходе — «ГОТОВ <крейт>: тесты N/0, clippy, fmt». Ведущий сразу отдаёт его на ревью, а ты продолжаешь
следующий пакет. Круг исправлений по ревью может получить другой исполнитель — не трогай файлы пакета, который ты уже сдал,
пока ведущий не попросит.
