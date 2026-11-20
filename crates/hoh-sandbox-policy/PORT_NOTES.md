# hoh-sandbox-policy ← packages/sandbox/sandbox-policy @ 39163dd3

## Перенос

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | config, `sandboxPolicy`, resolution, runtime grants, prompt context |
| `src/session-mode.ts` | `src/lib.rs` | modes, event payload, projection, `set_sandbox_mode` |
| `tests/policy.spec.ts` | `tests/policy.rs` | 26 / 0 исходных объявлений |
| `locale/*.json` | `locale/*.json` | 8 файлов без изменений |

## Rust-интерфейс

- Caller-owned effect передаётся явно: `register_writable_root(&Context, &Session, path)`.
- `set_sandbox_mode` и методы, читающие projection/header, возвращают `Result` вместо
  синхронного JavaScript throw.
- Неизменяемые дополнительные roots хранятся как `Arc<[String]>`; каждый `resolve`
  возвращает отделённый `Vec<String>` в `SandboxExecutionPolicy`.
- Оконное расширение относительно dsh: deployment-owned файлы или каталоги `protectedRoots` хранятся одним
  неизменяемым набором и переносится в каждый `SandboxExecutionPolicy`; одобрение режима его
  не удаляет. Один async effect-lock сериализует model-facing fs/bash при активной защите. Это
  закрывает credential paths от инструментов модели, не мешая самому движку.
- Для оконного профиля `protectedRoots` содержит весь канонический `DSH_HOME`. Listener
  `working-directory/access` до коммита отклоняет равенство и оба отношения ancestor/descendant
  между Session cwd и защищённым корнем как `WORKING_DIRECTORY_INVALID`; это расширение HoH,
  которого нет в dsh. Cwd служебного процесса не проверяется: desktop намеренно запускает engine
  из его data folder, а модель получает отдельный Session cwd.
- Канонизация protected root разрешает symlink существующего предка и достраивает отсутствующий
  suffix. Поэтому ещё не созданный путь под symlink-DSH_HOME остаётся закрытым.
- Для существующего protected root сервис при старте сохраняет `(dev, ino)`. Перед каждым bash
  совпадение проверяется под общим effect-lock: перенос корня/предка и подмена старого pathname
  новым объектом дают fail-closed отказ, поэтому bwrap не маскирует ложный mountpoint.
- Typert/Remote entry points отсутствуют по PORTING §8.

## Адаптация тестов

- Проверка byte-stability не меняет глобальный `TMPDIR` внутри параллельного Rust test
  process: production renderer вообще не читает environment, а тест дважды собирает полный
  prompt/context и проверяет отсутствие host-temp bytes.
- Kernel `Disposer` — consuming single-shot capability, поэтому повторный вызов одной JS
  disposer-функции не выражается. Токенная независимость одинаковых grants проверяется
  повторной регистрацией того же корня из одного context, отдельными disposer-ами и выгрузкой
  owning child context.

## Интеграционный остаток вне этого крейта

`hoh-tool-bash`, `hoh-tool-fs` и `hoh-fs-sandbox` сейчас объявляют собственные
`SHIM(hoh-sandbox-policy)` service-key/trait типы. Одинаковая строка `sandboxPolicy`
не объединяет разные Rust `TypeId`; после приёмки этого крейта их нужно перевести на
`hoh_sandbox_policy::{SandboxPolicy, SandboxPolicyRequest}`. Аналогично принятый
`hoh-permission-presets` содержит локальный shim `set_sandbox_mode`; его файлы в этом
ходе не менялись.

## resolve и замок (круг исправлений credguard)

- `resolve` читает проекцию `sandbox/mode` сессии только когда режим не задан явно (`index.ts:215`).
- `lock_protected_effect_within(signal, timeout_ms)` — ожидание замка с сигналом и сроком; см. PORT_NOTES
  `hoh-bash-sandbox`.
