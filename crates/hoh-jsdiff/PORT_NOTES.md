# hoh-jsdiff ← jsdiff 9.0.0 (npm package `diff`) @ v9.0.0

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/diff/base.ts | src/diff/base.rs | Синхронный алгоритм Myers с `timeout` и `maxEditLength` перенесён по шагам |
| src/diff/line.ts | src/diff/line.rs | Перенесены `diffLines` и tokenizer без дополнительных режимов |
| src/patch/create.ts | src/patch/create.rs | Перенесены синхронные `structuredPatch`, `formatPatch`, `createTwoFilesPatch`, `createPatch` в рамках задания |
| test/patch/create.js | tests/patch_create.rs | Перенесено 25 случаев, не перенесено 30 |
| test/diff/line.js | tests/diff_line.rs | Перенесено 10 случаев, не перенесено 14 (включая две группы с непосредственными проверками вне `it`) |
| LICENSE | LICENSE | BSD-3-Clause скопирована без изменений |

## Отличия от образца
- `undefined` при срабатывании лимита представлен как `None`; успешный результат — `Some`. Асинхронных callback-перегрузок нет.
- `formatPatch` принимает один `StructuredPatch` и поддерживает только форму заголовков по умолчанию (`INCLUDE_HEADERS`). Массивы patch, Git-метаданные и отсутствующие имена файлов не представляются заданным Rust API.
- `timeout_ms` сравнивает показания системных часов в миллисекундах, как `Date.now()`; `max_edit_length` — детерминированный лимит.
- Дополнительные тесты в `tests/edge_cases.rs` проверяют пустые и идентичные тексты, пустую сторону, финальный перевод строки в обе стороны, CRLF, изменения по краям, объединение hunk, context 0, лимит, 4000 строк с разрозненными изменениями и не-ASCII текст.

## Не перенесено
По заданию не перенесены callback-формы, `ignoreWhitespace`, `stripTrailingCr`, `newlineIsToken`, `ignoreNewlineAtEof`, `oneChangePerToken`, `diffTrimmedLines`, настройки заголовков, Git-поля и массивы в `formatPatch`, остальные виды diff, разбор, применение и reversing patch. Harness этими возможностями не пользуется.

### test/patch/create.js — перенесённые случаи
- `#createPatch`: `should handle files with the last line changed`; все три проверки `should output "no newline"...` (`new missing nl`, `old missing nl`, `both missing nl`); случай issue 531; `context missing nl`; `empty file`; `eof outside hunk`.
- `#createPatch`: `should generate a patch with default context size`, `with context size 0`, `with context size 2`.
- `headers handling`: `should output headers only for identical files`, `should omit headers if undefined`, `should safely handle empty inputs`, `should omit index with multiple file names`.
- `#createPatch`: `should respect maxEditLength`, `should not crash when encountering an enormous hunk`, `ignoreWhitespace: false`, `stripTrailingCr: false`.
- `#structuredPatch`: `should handle files with the last line changed`, `sets oldHeader and newHeader to undefined if they are not provided`, `terminates early`.
- `#formatPatch`: `should generate a patch string from an input diff`, `should include all headers with INCLUDE_HEADERS`, `should not mutate hunk objects`.

### test/patch/create.js — не перенесённые случаи
- `should diff large content blocks`: требует `diffWords`.
- `should handle INCLUDE_HEADERS the same as not specifying options regarding headers`, `should respect FILE_HEADERS_ONLY`, `should respect OMIT_HEADERS`: требуют аргумент `headerOptions`.
- `ignoreWhitespace: true`, `stripTrailingCr: true`: требуют соответствующие неперенесённые опции.
- `isn't allowed any more, since the patches produced were nonsense`: требует `newlineIsToken`.
- `takes an optional callback option`, `lets you provide a callback by passing a function as the options parameter`, `still supports early termination when in async mode` в `#createPatch`, а также обе callback-проверки в `#structuredPatch`: требуют асинхронный callback.
- `supports serializing an array of structured patch objects...`: требует массив в `formatPatch`.
- `should roughly be the inverse of parsePatch`, `should round-trip a Git rename patch through formatPatch and parsePatch`: требуют `parsePatch` (второй также Git-поля).
- `should include only file headers with FILE_HEADERS_ONLY`, `should omit all headers with OMIT_HEADERS`, обе проверки массива с `INCLUDE_HEADERS`/`FILE_HEADERS_ONLY`, ошибка массива с `OMIT_HEADERS`: требуют настройки заголовков и/или массивы.
- `should silently skip headers when filenames are undefined`: заданный API не допускает отсутствующие имена файлов.
- Все случаи Git-заголовков, режимов, rename/copy/create/delete и ошибок пустых Git-имён: требуют неперенесённые Git-поля.

### test/diff/line.js — перенесённые случаи
- `should diff lines`, `should treat identical lines as equal`, `should handle leading and trailing whitespace`, `should handle windows line endings`, `should handle empty lines`, `should handle empty input`.
- `Should prefer to do deletions before insertions, like Unix diff does`.
- `given options.maxEditLength: terminates early`.
- `given options.maxEditLength === 0`: обе проверки — нормальный результат для идентичных строк и раннее завершение при изменении.

### test/diff/line.js — не перенесённые случаи
- `terminates early - async`: требует callback.
- `emits one change per line`: требует `oneChangePerToken`.
- Все девять случаев `#TrimmedLineDiff`: требуют `diffTrimmedLines`/`ignoreWhitespace`; две из них дополнительно требуют `newlineIsToken` или callback.
- Группа `#diffLinesNL`: требует `newlineIsToken`.
- Группа `Strip trailing CR`: требует `stripTrailingCr: true`.
- `ignores absence of newline on the last line of file wrt equality when ignoreNewlineAtEof`: требует `ignoreNewlineAtEof`.

## Не проверено
Нет платформенных ветвей. Точный момент срабатывания wall-clock `timeout_ms` не проверяется тестами, чтобы они не зависели от реального времени; детерминированный `max_edit_length` проверен.

## Вопросы ведущему
Нет.
