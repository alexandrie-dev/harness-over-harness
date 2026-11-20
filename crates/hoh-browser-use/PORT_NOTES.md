# hoh-browser-use ← packages/browser-use/browser-use @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | service `browserUse`, exclusive registration, getter и lifecycle-effect |
| `src/brand.ts` | `src/brand.rs` | `BrowserUseProviderName` и конструктор без валидации |
| `src/types.ts` | `src/types.rs` | публичный type-only entry представлен реэкспортом типа |
| `tests/registry.spec.ts` | `tests/registry.rs` | 2/2 исходных случая и 5 Rust lifecycle/API/concurrency regressions |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml` | rustdoc и этот файл | поведенческий контракт учтён |

## Сохранённое поведение

- Plugin без конфигурации публикует service `browserUse`; одновременно service допускает ровно одну
  именованную регистрацию.
- `provider_name()` возвращает branded имя с момента успешной регистрации до завершения её cleanup.
- Повторная регистрация, включая то же имя, отклоняется точным diagnostic с именем действующего provider:
  `browser use provider "<name>" is already registered`.
- Как в образце, duplicate diagnostic проверяется до создания effect и побеждает ошибку inactive caller;
  повторная проверка внутри effect сохраняет атомарность между конкурентными Rust-вызовами.
- Регистрация принадлежит `Context` вызывающего provider: явный disposer или выгрузка provider fiber
  освобождает слот. Undo устанавливается до публикации и сравнивает identity регистрации, поэтому ни ошибка
  effect setup, ни запоздалый cleanup не удаляют чужую регистрацию.
- `BrowserUseProviderName` не валидирует и не изменяет provider-owned строку.

## Отличия от образца

- JavaScript `ctx.browserUse` представлен типизированным service key `BrowserUse`; traceable Cordis proxy
  заменён явным первым параметром `ctx: &Context` у `register` по правилам kernel.
- `providerName` перенесён как `provider_name()` и возвращает clone branded-значения вместо заимствования
  из синхронного `Mutex`.
- Cordis dispose-функция допускает повторный вызов; `hoh_kernel::Disposer` является affine и потребляется
  `dispose()`. Kernel снимает effect ровно один раз, а identity-check cleanup сохраняет исходную идемпотентную
  семантику состояния.

## Не перенесено

- Повторный вызов одной и той же cleanup-функции из `registry.spec.ts:23` не имеет прямого выражения:
  `hoh_kernel::Disposer::dispose(self)` потребляет affine-значение, а `Disposer` не реализует `Clone`.
  Kernel выполняет Undo ровно один раз; identity-check дополнительно гарантирует, что cleanup старой
  регистрации не снимет новую.
- TypeScript default export не имеет отдельного Rust-механизма; ему соответствует публичный plugin type
  `BrowserUseRegistry`.
- README и README i18n metadata не копируются как runtime-ресурсы; их контракт отражён в rustdoc и заметках.
- Experimental browser runtimes/providers принадлежат отдельным пакетам.

## Не проверено

- Реальные browser providers не запускаются: этот пакет хранит только их branded имена и не содержит browser API.

## Вопросы ведущему

Нет. SHIM, новые внешние зависимости и правки чужих API не понадобились.

## Круг исправлений 021432

- Точный текст без дефиса проверяют оба полных `assert_eq!` в
  `rejects_a_second_provider_and_permits_registration_after_disposal`; старый текст ревью не проходит.
- `releases_a_provider_contribution_when_its_plugin_unloads` использует настоящий
  `TestBrowserUseProvider` с `Inject<Service<BrowserUse>>` и снимает регистрацию через dispose plugin fiber.
- `duplicate_diagnostic_precedes_an_inactive_callers_error` фиксирует исходный порядок: занятый слот
  диагностируется до попытки создать effect даже для уже неактивного caller.
- `exports_the_source_identity_and_brands_without_validation` проверяет service/plugin имена и branded API;
  runtime использует точную метку `browserUse.register()` из `src/index.ts:44`.
- Повторный вызов disposer учтён как Rust-адаптация выше; это не пропущенная доступная ветвь API.

## Проверка

Команды запускаются offline с `CARGO_BUILD_JOBS=2`, выделенным `CARGO_TARGET_DIR` и внешним `timeout`.

- `cargo test -p hoh-browser-use --offline`: 7 passed, 0 failed; doc-tests 0/0.
- `cargo clippy -p hoh-browser-use --all-targets --no-deps --offline -- -D warnings` — чисто.
- `cargo fmt -p hoh-browser-use -- --check` — чисто.
