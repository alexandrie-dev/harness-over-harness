# hoh-workspace ← packages/workspace/workspace

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/registry.rs` | plugin/service, bootstrap, durable registry, archive/pin lifecycle |
| `src/entity.ts` | `src/entity.rs` | workspace snapshot и сериализованные durable mutations |
| `src/paths.ts` | `src/paths.rs` | fully-qualified path, realpath и default title |
| `src/spec.ts`, `src/types.ts` | `src/spec.rs`, `src/lib.rs` | domain v2 schema, ids, events и ошибки |
| `tests/workspace.spec.ts` | `tests/workspace.rs`, unit-тесты `registry.rs` | 79 `it` / 84 развёрнутых source cases, включая failure/restart/cancellation regressions |
| `locale/*.json` | `locale/*.json` | побайтно скопированы |
| README/build metadata | rustdoc, этот файл, Cargo manifest | прочитаны; npm/tsconfig metadata не копируется |

Зависимости только path/workspace. `feruca` уже закреплён в workspace и `Cargo.lock` как
общий аналог `String.localeCompare`; новых внешних пакетов нет. Чужие крейты не менялись.

## Отличия от образца

- JS Promise-tail registry перенесён как ticket-sequencer: ticket резервируется синхронно
  при первом poll до `tokio::spawn`, detached-задача ждёт точный номер, а RAII turn всегда
  передаёт очередь дальше. Поэтому сохраняются и source-order первого poll, и завершение уже
  начатой durable mutation после отмены вызывающего Rust future. Такая очередь есть отдельно
  у registry и у каждой entity publication, чтобы позднее пробуждение старой записи не затёрло
  новую.
- `AggregateError` представлен `WorkspaceAggregateError` с отдельным `message` и двумя
  исходными `BoxError`. Ошибка attach с `cause` представлена
  `WorkspaceAttachValidationError::source()`.
- `Map` insertion order хранится через `IndexMap`; source `localeCompare` tie-break использует
  CLDR-root collator `feruca`. `Date.prototype.toISOString()` для bootstrap реализован на всей
  допустимой JS Date шкале, включая extended years.
- Live session header проектируется только в нужные workspace поля. Это сохраняет уже
  провалидированный Session header с JSON-числом вида `1000.0`, которое полный serde decode
  структуры с `i64` иначе отверг бы.
- Все filesystem-вызовы публичных async-операций используют `tokio::fs`. На Windows снимается
  extended-length prefix, добавляемый `canonicalize`; UNC принимает любую пару начальных
  Win32-разделителей (`\\server\\share`, `//server/share`, `\/server/share`,
  `/\server/share`) ровно как `win32.parse`. Три и более ведущих разделителя и UNC без
  share-компонента отвергаются, поскольку parse возвращает одиночный root.
- Текст ошибок в warning очистки pending marker рендерится как JavaScript `String(error)`:
  обычная ошибка — `Error: message`, `HarnessError` — `Name: message`.
- Kernel parallel admission возвращает один `KernelError::Listeners`; при `stopActivity`
  каждая вложенная причина логируется отдельно в исходном тексте `Error: ...`.

## Не перенесено / не проверено

- Нестандартные implementation-defined формы `Date.parse` не эмулируются. Durable timestamps
  пакета и тесты используют ISO: canonical UTC, RFC 3339 с offset и date-only поддержаны;
  неразбираемое значение, как `NaN` в JS comparator, переходит к следующим tie-breaks.
- Реальный Windows filesystem в Linux-прогоне недоступен; чистые Win32 path cases проверены,
  а снятие `\\?\` prefix покрыто кодом под `cfg(windows)`.
- Vitest spies заменены наблюдаемым medium state, logger buffer и счётчиками test backend.
- Source создаёт `new WorkspaceRegistry(new Context())` и проверяет доступ до `Service.init`.
  Rust `WorkspaceRegistry` не имеет публичного незапущенного конструктора: экземпляр появляется
  только после успешного `start`/plugin apply, поэтому такое состояние непредставимо типами.

## Круг ревью 122213

- Исправлен реальный race порядка: прежний код резервировал FIFO mutex уже внутри двух
  независимо запланированных задач. Новые multi-thread regressions по 200 итераций проверяют
  `detach → attach` на entity и `archive → pin` на registry; старый код мог выполнить пары
  в обратном порядке.
- Warning после успешного delete и сбоя очистки marker теперь содержит точное
  `Error: selected bootstrap marker failure`; тест также проверяет сохранённый pending state
  и отсутствие лишних `sessionPersistence.list`.
- Исправлен Win32 title для повторно записанного drive root: как `win32.parse(...).root`,
  `C:\\\\` даёт `C:\`.
- Добавлены проверки plugin pending без `sessionPersistence`, lazy live header без повторного
  list, cached `list()`, pin persistence failure, cwd-less live/archive default guard, live
  arrival во время подготовки, исчезнувшей symlink-цели, ELOOP/ENOTDIR, сброса missing
  exemption и точных attach/ENOENT diagnostics. EACCES проверяется детерминированно unit-тестом
  `only_not_found_preserves_a_missing_path_candidate`, а не `chmod 000`, который root обходит.
- Race `realpath → stat` проверяется через общий чистый classifier: только `NotFound` сохраняет
  missing candidate; остальные kinds invalid. Тот же helper вызывается после обоих filesystem
  этапов. Private JS-map corruption заменён unit-тестом private ordered-cache helper с точным
  diagnostic, без добавления публичного test seam в Rust API.
- Исправлен счёт source: 79 именованных `it` и 84 развёрнутых case, а не 76.

## Круг ревью 125937

- Исправлена проверка mixed-separator UNC: `\/server/share` и `/\server/share` теперь
  fully-qualified, как `win32.parse`; три ведущих разделителя и UNC без share-компонента
  остаются невалидными. Регрессия `accepts_unc_roots_with_mixed_separators_and_normalizes_drive_root_titles`
  падает на прежнем классификаторе.
- Поведение дополнительно сверено напрямую с Node `path.win32.parse`: root смешанных форм
  содержит полный UNC share, а не одиночный разделитель.
- Недетерминированная под root проверка EACCES через `chmod 000` удалена из integration-теста:
  `PermissionDenied` покрывает чистый classifier unit-тест, а integration-тест теперь честно
  проверяет только реально полученные ELOOP и ENOTDIR.

## Вопросы ведущему

Правок чужого API и временных dependency shims не требуется.

## Проверки

Команды выполнялись offline с `CARGO_BUILD_JOBS=2` и
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x2`; тесты — под `timeout`.

- `cargo test --offline -p hoh-workspace --no-fail-fast` — 89 passed, 0 failed; doc 0/0.
- `cargo clippy --offline -p hoh-workspace --all-targets --no-deps -- -D warnings` — exit 0.
- `cargo fmt -p hoh-workspace -- --check` — exit 0.
- `diff -ru <source>/locale crates/hoh-workspace/locale` — exit 0.
