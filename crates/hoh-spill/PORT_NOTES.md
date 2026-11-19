# hoh-spill ← packages/spill/spill @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён контракт `SpillStore` и имя сервиса `spillStore` |
| src/types.ts | src/types.rs | Пересены все пять публичных типов и конструктор локатора |
| tests/service.spec.ts | tests/service.rs | Перенесены все 3 `it`; добавлена регрессия точных JSON-имён обоих вариантов |
| README.md, README.zh.md | PORT_NOTES.md | Прочитаны; границы контракта учтены |
| package.json | Cargo.toml | Зависимости перенесены на workspace crates |

Отдельного `LICENSE` и каталога `locale` в пакете нет.

## Отличия от образца

- Абстрактный TypeScript-класс разделён на object-safe trait `SpillStoreApi` и типизированный ключ `SpillStore`. Конкретный backend предоставляет `Arc<dyn SpillStoreApi>` через `Context::provide`; это сохраняет один сервис на контекст и автоматическое снятие при disposal.
- TypeScript-функция `SpillLocator` названа `spill_locator` по правилам имён Rust; она по-прежнему только оборачивает строку без валидации.
- `SpillRef.bytes` имеет тип `u64`, как размер по PORTING.md §4.

## Не перенесено

Нет. Механизм TypeScript declaration merging заменён типизированным ключом ядра по PORTING.md §8.

## Не проверено

Нет платформенных веток. Обязанности конкретного backend по приватному хранению, санитаризации имени и точной записи проверяются в crate реализации, как и в образце.

## Вопросы ведущему

Нет.

## Проверки

Все команды выполнены offline с `CARGO_BUILD_JOBS=2` и
`CARGO_TARGET_DIR=/dev/shm/hoh/target-x6` (фактическая цель заданной ссылки `target-x6`).

- `timeout 180s cargo --offline test -p hoh-spill -- --test-threads=2`: exit 0,
  **4 passed; 0 failed**; unit и doc-tests — 0.
- `cargo --offline clippy -p hoh-spill --all-targets --no-deps -- -D warnings`: exit 0,
  без warnings.
- `cargo fmt -p hoh-spill -- --check`: exit 0.
