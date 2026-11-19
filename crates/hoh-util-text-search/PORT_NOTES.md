# hoh-util-text-search ← packages/util/text-search

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `SearchKeys`, `localized_values`, построение, поиск и `update` |
| `fuzzysort` 4.0.2 `single` | `src/fuzzysort.rs`, `src/fuzzysort/remap.rs` | внутренний MIT-порт сопоставления и нормализованной оценки |
| `tests/text-search.spec.ts` | `tests/text_search.rs` | все 22 исходных `it` |
| `README.md`, `README.zh.md`, `package.json` | эти заметки и `Cargo.toml` | контракт и зависимости учтены |

Locale-файлов у исходного пакета нет.

## Поведение

- Ступени оценки сохранены: prefix `4`, substring fuzzy-ключа `3`, subsequence
  `2 + min(fuzzysortScore, 0.99)`, substring-ключ `1`. Каждый непустой термин обязателен;
  равные суммы сохраняют порядок построения.
- Нормализация повторяет `NFKD → удалить только U+0300–U+036F → NFKC → stroked letters →
  lowercase`. `icu_normalizer` 2.4.0 выполняет формы Unicode, а `icu_properties` 2.4.0 — точные
  `L`/`N`/`Lu`/`Ll` и Script_Extensions для Han, Hiragana, Katakana и Hangul. Обе версии уже были
  в `Cargo.lock`; новых внешних пакетов не добавлено.
- Внутренний `fuzzysort.single` переносит UTF-16 индексы, ASCII word beginnings, не более 200
  backtrack-шагов, special-case двух символов, multipart-поиск после пробела, формулу raw score,
  `normalizeScore` и штатные remappings версии 4.0.2. Это решение предписано строкой
  `fuzzysort 4.0.2` в `docs/porting/DEPS.md`; сторонний fuzzy matcher изменил бы порядок выдачи.
- Подготовки строк разделяются через `Arc` по исходной строке и повторно используются `update`;
  старый индекс остаётся валиден. Функция ключей вызывается ровно один раз на элемент только при
  построении нового индекса.

## Отличия Rust API

- `TextSearchIndex<T>` владеет `Vec<T>`, а `search` возвращает `Vec<&T>`. Поэтому идентичность
  самого JS-массива для пустого запроса не выражается; вместо копирования элементов возвращаются
  ссылки на те же элементы `items()`, в исходном порядке.
- `SearchKeys` владеет `Vec<Option<String>>`: подготовленные формы всё равно должны пережить
  callback построения, а владение исключает висячие ссылки. Отсутствующие массивы TypeScript
  соответствуют пустым векторам `Default`.
- Экспорт `localizedValues` назван по Rust-конвенции `localized_values` и возвращает заимствованные
  `&str` в порядке `IndexMap` манифеста.

## Не перенесено

Нет: перенесены единственный source-файл и все 22 тестовых объявления. Из `fuzzysort` намеренно
перенесён только реально используемый `single` с подготовкой и оценкой, как предписано `DEPS.md`;
его глобальный cache, `go`, highlight и snapshot этому пакету не нужны.

## Ограничения

- ICU Node 22 и ICU4X могут различаться только для символов, добавленных после Unicode-версии одной
  из баз. Алгоритм, категории и формы нормализации совпадают; тесты покрывают width, composed и
  decomposed accents, stroked letters, kana marks и supplementary Han.

## Вопросы ведущему

Нет.

## Проверка

Все cargo-команды запускаются с `CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x7`; тесты — только под внешним `timeout`.

- `cargo test -p hoh-util-text-search --offline`: 23 passed, 0 failed (22 исходных integration +
  1 внутренняя regression для remappings); unit/doc без дополнительных ошибок.
- `cargo clippy -p hoh-util-text-search --all-targets --no-deps --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-util-text-search -- --check`: exit 0.
