# hoh-sdk-app ← packages/bundle/sdk-app @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | `SdkApp`, `Config`, команда SDK, публикация `sdkAppStartup`, привязка EOF |
| `tests/startup.spec.ts` | `tests/startup.rs` | 3/3 случая; сверх них: временный glue без конкурирующего stdin-reader, два случая с `profile: null`, текст отказа для `profile: 5` |
| `tests/sdk-app.spec.ts` | `tests/sdk_app.rs` | 1/1 случай; все значения patch, порядок startup/server и inject закреплены |
| `cordis.patch.yml` | `cordis.patch.yml`, `CORDIS_PATCH` | скопирован без изменений |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md` | этот учёт | прочитан полностью; пользовательское поведение команды и lifecycle покрыто |

Перенесено **4 из 4** исходных случаев, не перенесено 0. Дополнительно 5 Rust-проверок:
временный стык stdio, справка и готовность при `profile: null` (2), текст отказа для `profile: 5`,
схема/имена плагина (в `tests/sdk_app.rs`).

## Отличия от образца

- `name` представлен `SdkApp::NAME`, сервис — типизированными `SdkAppStartupKey` и
  `SdkAppStartup`. `Config.profile` — `String` с serde-default `sdk`, а не необязательное поле
  TypeScript; явный `null` (`profile:`/`profile: ~`) тоже даёт `sdk`, как у schemastery (`SHIM(hoh-kernel)`,
  схема — `Option<String>`); эффективное значение совпадает. `CORDIS_PATCH` соответствует package export.
  Следствие `SHIM(hoh-kernel)`: схема `string | null`, поэтому для `profile: 5` ядро пишет
  `expected string | null but got 5 (at profile)`, а образец — `expected string but got 5`. Отказ тот же,
  отличается формулировка; закреплено тестом `refuses_a_profile_of_the_wrong_type_with_the_shim_wording`.
  Условие снятия: ядро подставляет умолчание вместо `null` до проверки схемы (нужно и `hoh-user-approval`) —
  тогда убрать `profile_or_default`/`schemars(with)` и ждать `expected string but got 5`.
- Action commander синхронный, а `Context::provide` асинхронный. Поэтому action сначала
  регистрирует EOF-effect и отмечает принятие, затем `apply` публикует сервис. Флаг обязателен:
  `parse_cmdline` возвращает `Ok` также после `--help`, где сервис публиковать нельзя.
- Process-global stdout/stdin не подменяются. Публичный runtime-only `SdkAppRuntimeHooks`
  передаёт `AppStdin` и `Output`; это дополнительный Rust API для host и тестов.
- Проверка npm `package.json.dependencies` преобразована в проверку строки
  `@deepseek-ai/dsh-sdk-jsonrpc-server`, её положения и `inject: [sdkAppStartup, loader]` в
  настоящем patch. Неиспользуемая Cargo-зависимость на соседний crate не добавлена: регистрацию
  обоих плагинов выполняет владелец композиции части 1.

## Не перенесено

- npm metadata/exports/build и механизм установки зависимостей: в Rust композиция собирается
  реестром `apps/hoh`, а patch экспортируется напрямую из crate.

## Не проверено

- Платформенных веток нет. Настоящий общий stdin проверяется сквозной частью 1/2b; unit-тест
  использует не потребляющий байты управляемый EOF observer.

## Вопросы ведущему

- `GLUE-SHIM(2b)` (снят в `apps/hoh` для профиля `window`: `SdkAppRuntimeKey` публикует `window_glue::provide_stdio`;
  для остальных профилей остаётся запасной вариант): пока host не публикует `SdkAppRuntimeKey`, плагин использует не завершающийся
  `PendingStdin`, чтобы не запускать второй reader и не красть JSON-RPC-кадры. Hook-backed путь
  полностью повторяет source и протестирован. Текущий `hoh-sdk-jsonrpc-server` на EOF только
  помечает transport failed; при стыковке l1/2b должен передать сюда единый не потребляющий EOF
  signal либо сам запросить bounded exit и удалить этот shim.
