# hoh-tool-testkit ← packages/test-support/tool-testkit

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | все три экспорта перенесены |
| `tests/model-call.spec.ts` | `tests/tool_testkit.rs` | 3 / 3 случаев образца |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки и `Cargo.toml` | публичный контракт и состав пакета учтены |

LICENSE в пакете образца отсутствует.

## Отличия от образца

1. Асинхронные TypeScript-функции, которые могут отклонить Promise, в Rust возвращают `Result`. Точные
   тексты и JS-классы двух ошибок `executeModelToolFixture` сохранены через
   `ToolsError::TypeError` и `ToolsError::Error`.
2. TypeScript-рендер делает только unchecked cast массива JSON к `ContentBlock[]`. Rust-тип не
   может представить необъектный блок, поэтому искажённое policy-значение даёт ошибку
   десериализации вместо позднего невалидного model content.

## Не перенесено

Нет. Все 3 случа тестового образца перенесены; Rust-тесты дополнительно проверяют два
остальных экспорта пакета.

## Не проверено

- Нет.
