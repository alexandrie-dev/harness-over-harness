# hoh-spill-local ← packages/spill/spill-local @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | конфигурация, `LocalSpillStore`, сервис и lifecycle startup-cleanup |
| `src/store.ts` | `src/store.rs` | private root, UTF-16-safe segment encoding, session hash, exclusive 0600 write |
| `src/cleanup.ts` | `src/cleanup.rs` | trusted-root discovery, identity dedup, age sweep и pruning |
| `tests/spill-local.spec.ts` | `tests/spill_local.rs` | все 41 случая перенесены |
| `tests/loader-composition.spec.ts` | `tests/spill_local.rs` | 1 случай перенесён через Rust config deserialize и тот же cleanup path |
| `locale/*.json` | `locale/*.json` | все 8 файлов скопированы без изменений |

## Отличия от образца

- Исходный default export `LocalSpillStore` сам реализует `Plugin` и object-safe trait
  `SpillStoreApi`; backend предоставляет себя через типизированный service key `hoh_spill::SpillStore`.
- Экспорт `./src/*` сохранён публичными модулями `cleanup` и `store`; корень
  реэкспортирует только те функции, которые экспортирует исходный `src/index.ts`.
- Обычный non-strict schemastery object сохраняет неизвестные поля конфигурации:
  Rust хранит их в insertion-ordered `Config::extra`. Явный
  `cleanupPeriodDays: null`, как в schemastery, получает default `30`.
- Однократно создаваемый process-local default root хранится в `OnceLock`: значение после первой
  инициализации неизменно и повторяет ленивый `defaultRoot ??=` образца.
- Node `lstat`/`realpath`/mode/uid/dev/ino перенесены на безопасные `tokio::fs`, `MetadataExt` и
  `rustix`; системные вызовы и `unsafe` не используются. Имена `readdir` сортируются по байтам до обхода.
- Системные ошибки в предупреждениях и отказах `mkdtemp`/`mkdir`/`open`/`write`
  форматируются в видимой Node-форме через `hoh-error`, сохраняя исходный
  `io::Error` как причину; `mkdtemp` использует отображаемый Node-шаблон `XXXXXX`.
- `SaveTextOptions` реализует redacted `Debug`: путь и идентификаторы остаются видимыми,
  содержимое спилла заменено его длиной и не попадает в диагностический вывод.
- `cleanupPeriodDays` принимает целочисленные JSON/YAML float-значения вроде `30.0`, как
  JavaScript `z.number().step(1)`; отрицательные и дробные значения по-прежнему отклоняются.
- Loader-specific YAML transport не входит в этап порта. Его исходный тест перенесён на десериализацию
  той же camelCase-конфигурации и вызов того же cleanup path; поведение `cleanupPeriodDays: 30` сохранено.
- Rust `str` не может содержать одиночный UTF-16 surrogate. Для всех валидных Unicode-строк
  `encode_segment` обходит UTF-16 code units и совпадает с образцом; одиночные surrogate не представимы API.
- Явный `root: null` отклоняется схемой Rust до `apply`; schemastery пропускает
  `null`, после чего исходный `path.resolve(null)` отклоняет загрузку. Итог в
  обоих случаях — отказ загрузки, но стадия и диагностический текст различаются.

## Не перенесено

Нет runtime-ветвей. Cordis Loader/Include как транспорт конфигурации не переносится по PORTING.md §8.

## Не проверено

- Windows-ветки доверия к каталогам и case-insensitive filesystem identity; Linux/POSIX проверены.
- Редкие межпроцессные гонки исчезновения каталогов между `mkdir/open`, `lstat/readdir` и `unlink/rmdir`.

## Вопросы ведущему

Нет.

Все 42 исходных случая представлены отдельными Rust-сценариями или точными
assertions внутри них. Lifecycle-регрессия детерминированно доказывает, что
activation не ждёт startup sweep, сервис в этот момент пишет, disposal ждёт ту
же задачу и старый файл удалён до завершения. Девять дополнительных регрессий
проверяют unknown config keys, nullable-default и integral-float
`cleanupPeriodDays`, отрицательный pre-epoch `mtime`, прямой warning sink, завершение
direct sweep, Node-формы `mkdtemp`/`ENAMETOOLONG` и redacted `SaveTextOptions::Debug`.
