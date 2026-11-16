# hoh-kernel-macros ← Rust API docs/kernel.md §2.8

## Файлы

`src/lib.rs`: `#[derive(Inject)]`, `#[plugin]`. Одиннадцать compile-fail фикстур и одна pass-фикстура (полный, минимальный и переставленный набор параметров) в `tests/ui` и `tests/pass`.

## Отличия от спецификации

Trybuild запускается обычным `#[test]`: проверяется компилятор, не асинхронное поведение. В исходном разрешённом manifest macros crate нет зависимости tokio; добавлена только явно разрешённая dev-зависимость `hoh-kernel`.

## Не перенесено

Нет. Ошибки полей Inject, имени параметра, типа Cfg и отсутствия async проверены; обязательные проверки режима события, повторного next/dispose и закрытого конструктора Service также проверены. Проверки `reflect.spec` «access check» и «access error» перенесены в UI.

## Не проверено

Схемы с переименованным в Cargo именем зависимости hoh-kernel: макросы используют контрактное имя `::hoh_kernel`.

## Вопросы ведущему

Нет.

## Проверки

`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`:

- `cargo test -p hoh-kernel-macros`: 1 passed; 0 failed, включая 11 compile-fail и 1 pass fixture.
- `cargo clippy -p hoh-kernel-macros --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-kernel-macros -- --check`: exit 0.
