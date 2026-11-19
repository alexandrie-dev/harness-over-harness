# hoh-util-ripgrep ← packages/util/ripgrep @ 39163dd3

## Файлы

| Образец | Перенос | Состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/path.rs`, `src/spawn.rs` | Полностью: process-local lazy resolution, sidecar/dependency/Electron paths, `--no-config`, incremental UTF-8 records, early stop и полное ожидание managed process range |
| `tests/path.spec.ts` | `tests/path.rs` | Все 5 исходных случаев; дополнительно fail-loud при отсутствии Rust-sidecar и Node-правило расширения для Windows dotfile |
| `tests/stream.spec.ts` | `tests/stream.rs` | Все 9 исходных случаев; дополнительно проверена замена невалидного UTF-8 на U+FFFD |
| `README.md`, `README.zh.md`, `package.json` | rustdoc и эти примечания | Прочитаны; библиотека не является плагином, service/event/config не объявляет |

Каталогов `locale/` и собственного `LICENSE` нет; заимствованного кода нет. Все
production exports используются: `spawnRipgrep` и реэкспорт `resolveRgPath` —
`tool-fs-search`, `spawnRipgrep`/`readRipgrepLines`/`RipgrepLinesResult` —
`workspace-files` (сам пакет API исключён из текущего этапа, но контракт сохранён).

## Отличия от образца

- В поставке Rust нет каталога `node_modules/@vscode/ripgrep`. Публичный
  `resolve_rg_path` считает HoH single-file runtime и принимает только существующий
  соседний sidecar: `<process.execPath>-rg`, на Windows `<name>-rg.exe`. Отсутствие
  sidecar возвращает обычную ошибку
  `packaged ripgrep executable was not found at …`; fallback на системный `PATH`
  намеренно не добавлен (его нет в образце). Чистый resolver и тесты сохраняют все
  ветви образца: ordinary dependency, packaged fallback и первая `.asar` перед
  `/` или `\` → `.asar.unpacked`. Версия/укладка бинарника остаётся вопросом ниже.
- `rgPathPromise` представлен `OnceLock<Result<String, SharedError>>`: и успешный
  путь, и первая ошибка process-wide кэшируются, как fulfilled/rejected Promise.
  `current_exe` и одна синхронная проверка существования соответствуют
  `process.execPath`/`existsSync`; не-UTF-8 путь ОС проходит через lossless-невозможную
  для `String` замену, как строковая граница Node.
- `spawn_ripgrep` принимает `&dyn SubprocessRuntime`, владеющий
  `SubprocessSpawnSpec`, и возвращает `Result<SubprocessHandle, BoxError>`.
  Синхронная ошибка provider проходит без замены. Все поля spec сохраняются,
  меняется только `argv`: `[executable, "--no-config", ...argv]`.
- Строковое объединение separator стало закрытым `RipgrepSeparator::{Newline,
  Nul}`. Callback возвращает `Result<bool, BoxError>`: `Ok(false)` соответствует
  false, `Err` — синхронному throw JavaScript с сохранением типа/identity ошибки.
  Это дополнительный Rust-тип, необходимый для представления исходного union.
- Node `Readable.destroy()` при отказе выражен уничтожением owned `AsyncRead`.
  Чтение и `done` конкурируют; после любого отказа вызываются terminate, ожидание
  `done` и `wait_for_exit`. После успеха `wait_for_exit` также обязателен. Записи
  буферизуются как bytes до ASCII-разделителя и затем декодируются целиком через
  `from_utf8_lossy`, поэтому разрезанный UTF-8 сохраняется, а неверный даёт U+FFFD.

## Не перенесено

Нет. Все 14 исходных `it` перенесены; npm build/type tests отсутствуют.

## Не проверено

- Реальный packaged `rg` не запускался: unit tests используют точный subprocess
  contract и in-memory streams. Поэтому формат/порядок вывода конкретной версии
  `ripgrep` этой crate не утверждается.
- Windows sidecar и Electron ASAR проверены над строками на Linux; Windows и macOS
  исполняемые файлы не запускались.

## Вопросы ведущему

- `docs/porting/DEPS.md`, решение 12, всё ещё не задаёт версию и механизм доставки
  `rg`. Crate ожидает существующий sidecar рядом с приложением и не может сама его
  создать. Нужны выбранная версия бинарника и шаг packaging, который кладёт его как
  `<exe>-rg` / `<name>-rg.exe`; код crate после этого менять не требуется.
