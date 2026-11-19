# hoh-agent-tool-presentation ← packages/core/agent-tool-presentation @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | `Config`, обязательный `mode`, inject `tools` и выбор дочернего native/PTC plugin перенесены. |
| `tests/agent-tool-presentation.spec.ts` | `tests/agent_tool_presentation.rs` | 4/8 исходных случаев перенесены; добавлена точная проверка native-ветки. |
| `locale/*.json` | `locale/*.json` | Все 8 файлов скопированы без изменений. |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки | Прочитаны; собственного `LICENSE` нет. |

## Отличия от образца

1. Закрытый TypeScript union `mode: 'native' | 'ptc'` представлен публичным Rust enum
   `PresentationMode`; это типизированная пара исходного union. `Deps` публичен только потому, что является
   associated type публичного `Plugin`.
2. `ctx.plugin(module)` представлен статическим `Context::plugin::<P>()`. Native-ветка использует настоящий
   `hoh-tool-executor-native`; PTC-ветка пока монтирует приватный dependency-only child с точным именем
   `ptc-tool-executor` и inject `tools`, `systemPrompt`, `ptcRuntime`. Значение shim-сервиса `ptcRuntime`
   намеренно необитаемо: без redesign-крейта child не может ложно активироваться с пустой реализацией.

## Не перенесено

`packages/core/tool-executor-ptc` и `packages/ptc-runtime/ptc-runtime` имеют в `MAP.tsv` статус `redesign` и
не имеют Rust crates. Поэтому не перенесены четыре сквозных случая, поведение которых принадлежит этим пакетам:

- `gives its own agent PTC mode and leaves the rest native`;
- `rejects the selected scope after its provider unloads`;
- `applies once the runtime arrives`;
- `rejects an unsupported runtime without inheriting native execution` (`it.each` считается одним случаем).

Оставшиеся 4/8 случаев перенесены без ослабления. Дополнительный Rust-тест проверяет, что `native` выбирает
именно принятый `NativeToolExecutorPlugin`.

## Не проверено

Активная PTC-ветка, PTC SDK/`run_code`, отказ неподдерживаемого языка и поведение после снятия PTC provider —
до появления redesign-крейтов. Ветвей платформ в исходном пакете нет.

## Вопросы ведущему

1. **SHIM(hoh-tool-executor-ptc):** `src/lib.rs::ptc_shim` сохраняет точные имя и зависимости PTC child и
   корректное состояние Pending до появления `ptcRuntime`, поэтому наследованный native executor остаётся
   активным. После появления `hoh-tool-executor-ptc` заменить shim прямым `Context::plugin` и перенести четыре
   перечисленных сквозных теста; копировать реализацию redesign-пакета в этот compatibility crate нельзя.
