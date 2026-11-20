# hoh-subagent-spawn-in-process ← packages/subagent/subagent-spawn-in-process @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | config, provider capabilities, fresh continuable preparation и lifecycle-регистрация |
| `tests/subagent-spawn-in-process.spec.ts` | `tests/spawn_in_process.rs` | 24/24 случаев; production AgentLoop + scripted model + настоящий SubagentRuntime |
| `tests/harness.ts` | helpers в `tests/spawn_in_process.rs` | локальный offline harness без bash/persistence/network |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml` | crate docs и этот файл | поведенческие границы и выбор между spawn/fork учтены |

## Отличия от образца

- TypeScript provider object представлен `SpawnInProcessProvider`, а loader extension —
  типизированным `SubagentSpawnInProcess: Plugin`. Provider сделан публичным для прямой
  проверки и программной композиции; в образце его класс package-private.
- `Promise.resolve({})` представлен готовым boxed future с
  `ContinuableCreateSpec::default()`; отсутствие `seed` остаётся `None`.
- `ProviderRegistration` принадлежит kernel-effect и снимается вместе с fiber. Это
  типизированный эквивалент effect-owned регистрации Cordis.
- Тест приватного Cordis lifecycle-hook, который уничтожает parent ровно внутри
  unpublished child setup, проверяется через доступную Rust-границу: уже уничтоженный
  owner отвергает start без `subagent/start`/`subagent/end` и без ребёнка. Сам exact
  transaction guard (`owner disposed during setup`) принадлежит принятому
  `hoh-agent-loop::factory`; этот пакет не дублирует и не раскрывает его приватный hook.
- Для уже уничтоженного parent текущий `hoh-subagent::start_activation` сначала вызывает
  `workingDirectory.ensure`, и потому может вернуть его teaching error раньше исходного
  `subagent creation requires the exact live parent agent`. Тест сохраняет внешний контракт
  отказа и отсутствие новых Agent/Session; фактический kernel-текст сейчас ровно `inactive context`.
  Менять порядок в чужом принятом crate запрещено.
- Поле `AgentOptions.subagentDepth` отсутствует в Rust owning API; его авторитетный persisted
  эквивалент — `SessionHeader.delegationDepth`, который проверяется тестом как parent depth + 1.
- Ошибка глубины представлена общим `HarnessError` с `name = "SubagentError"`, а не отдельным
  JavaScript-классом `SubagentDepthError`; Rust API не содержит полей `attemptedDepth`/`maxDepth`.
  Тест строго проверяет доступные machine contract `name`, `code`, `message`, включая оба числа.

## Усиления после ревью

- Pre-aborted signal теперь проверяется тестом по pointer identity исходного `SharedError`, а не
  по подстроке `aborted`.
- Оба structured-output сценария сохраняют scope ребёнка и проверяют полный жизненный цикл
  `STRUCTURED_OUTPUT_TOOL`: инструмент виден в child scope во время запуска и отсутствует там после
  cleanup; unload case действительно запускается с `output_schema`.
- Backend-unload и оба disposed-parent сценария считают Agent/Session и слушают `session/created`
  плюс `agent/created`; поэтому временно созданный и сразу удалённый ребёнок не остаётся незамеченным.
  Публичного аналога Cordis `fiber.getEffects()` в `hoh-kernel::Fiber` нет, поэтому вместо внутреннего
  счётчика проверяются все доступные publication boundaries.
- `SubagentActivation` — закрытая Rust-структура без полей `steer`/`resume`; их отсутствие гарантирует
  тип, а не runtime property test. Синхронное отсутствие ребёнка сразу после JS `start()` также не
  имеет прямого аналога: Rust `start_activation(...).await` возвращает только после публикации.

## Не перенесено

- `tests/spawn-in-process.e2e.ts` намеренно не запускается и не переносится как Rust-тест:
  это key-gated smoke с настоящим `DEEPSEEK_API_KEY`, сетью, реальным bash и файловой
  системой. Исключение прямо задано `PORTING.md` §10. Его production-путь покрыт offline
  интеграцией scripted adapter → AgentLoop → SubagentRuntime → spawn provider.
- TypeScript-only проверка отсутствия `default` export заменена проверкой Rust plugin
  type, `NAME`, `INJECT` и default config; механизма default export в Rust нет.

## Не проверено

- Реальный DeepSeek transport, сетевой API и e2e-запись файла не проверялись по правилу
  key-gated e2e. Production-код этого crate не содержит сети, процессов или файлового I/O.

## Вопросы ведущему

- Если текст отказа для уже disposing/disposed parent должен строго предшествовать cwd
  resolution, в `hoh-subagent::SubagentRuntime::start_activation` проверку exact-live parent
  нужно перенести перед `resolve_cwd`. Чужой crate не менялся.
- Для production provider contract SHIM и новые внешние зависимости не понадобились.

## Проверка

- `cargo test -p hoh-subagent-spawn-in-process --offline`: 24 passed, 0 failed.
- `cargo clippy -p hoh-subagent-spawn-in-process --all-targets --no-deps --offline -- -D warnings`.
- `cargo fmt -p hoh-subagent-spawn-in-process -- --check`.
