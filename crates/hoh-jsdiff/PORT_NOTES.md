# hoh-jsdiff ← jsdiff 9.0.0 (npm package `diff`) @ v9.0.0

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/diff/base.ts | src/diff/base.rs | Синхронный алгоритм Myers с `timeout` и `maxEditLength` перенесён по шагам |
| src/diff/line.ts | src/diff/line.rs | Перенесены `diffLines` и tokenizer без дополнительных режимов |
| src/patch/create.ts | src/patch/create.rs | Перенесены синхронные `structuredPatch`, `formatPatch`, `createTwoFilesPatch`, `createPatch` в рамках задания |
| test/patch/create.js | tests/patch_create.rs | Перенесено 25 случаев, не перенесено 30 |
| test/diff/line.js | tests/diff_line.rs | Перенесено 10 случаев полностью и 1 частично; не перенесено 13 (включая две группы с непосредственными проверками вне `it`) |
| Результаты jsdiff 9.0.0, подготовленные ведущим | tests/oracle.rs, tests/fixtures/jsdiff_oracle.jsonl, tests/fixtures/gen-jsdiff-oracle.mjs | Oracle и детерминированный генератор ведущего: 1500 пар текстов и 60 форматированных patch |
| LICENSE | LICENSE | BSD-3-Clause скопирована без изменений |

## Отличия от образца
- `undefined` при срабатывании лимита представлен как `None`; успешный результат — `Some`. Асинхронных callback-перегрузок нет.
- `formatPatch` принимает один `StructuredPatch` и поддерживает только форму заголовков по умолчанию (`INCLUDE_HEADERS`). Массивы patch, Git-метаданные и отсутствующие имена файлов не представляются заданным Rust API.
- `timeout_ms` измеряет прошедшее время через монотонный `std::time::Instant`: отсчёт начинается перед первым `extract_common`, продолжение разрешено при elapsed в целых миллисекундах ≤ timeout. В отличие от `Date.now()`, перевод системных часов на лимит не влияет; `max_edit_length` — детерминированный лимит.
- Предыдущие компоненты хранятся в общих `Rc`-цепочках. `Drop` отделяет уникальные звенья циклом и останавливается на общем префиксе, который освободит последний владелец. Освобождение выигравшего пути, удалённых путей и остатка `best_path` не рекурсивно; arena, удерживающей все созданные компоненты, нет.
- Регрессии освобождения работают на потоке со стеком 256 КиБ (модульные тесты `src/diff/base.rs`): внутренние `add_to_path`/`extract_common` строят известный выигравший путь из 400000 компонентов без поиска диагоналей, `build_values` проверяется по всем changes; отдельно проверяется освобождение общих цепочек из `best_path`, включая отсутствие утечки через `Weak`. Оба теста падают с переполнением стека, если отключить итеративное освобождение (проверено ведущим 2026-11-16), и идут 0,2 с. Теста через полный `diff_lines` на N=200000 нет: поиск квадратичен (около 20 миллиардов посещений диагоналей, 40 минут с оптимизацией), а при N=5000 прежний код стек не переполнял — такой тест ничего не доказывает. Его просило задание круга исправлений; ведущий снял это требование как собственную ошибку.
- Для полного прогона стресс-регрессии включена оптимизация тестового профиля через `CARGO_PROFILE_TEST_OPT_LEVEL=3`; обязательные `CARGO_NET_OFFLINE=true` и `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-m1` сохранены. Число повторений, стек и assertions не меняются.
- Проверено, что при временном удалении итеративного `Drop` новые регрессии завершаются переполнением стека (`SIGABRT`); после восстановления `Drop` обе проходят.
- Верхняя граница диагоналей пересчитывается на каждой итерации, как в образце. Пустой диапазон с началом 0 форматируется с началом -1; проверка отсутствующих заголовков сравнивает весь `StructuredPatch`.
- Дополнительные тесты в `tests/edge_cases.rs` проверяют пустые и идентичные тексты, пустую сторону, финальный перевод строки в обе стороны, CRLF, изменения по краям, объединение hunk, context 0, лимит, 4000 строк с разрозненными изменениями и не-ASCII текст.
- Oracle ведущего сравнивает все координаты и строки hunks, порядок, значения и счётчики changes, а также `None` при `max_edit_length`. Включены неоднозначные кратчайшие пути, пустые тексты и строки, LF/CRLF и смешанные окончания, отсутствие финального перевода строки, Unicode и разные context. Ещё 60 записей сравнивают весь форматированный patch с разными именами файлов (включая экранирование) и заголовками. Генератор читает TypeScript jsdiff, удаляет типы в отдельной scratch-копии и использует фиксированный seed; в этом раунде он не запускался. Oracle, fixtures и dev-dependencies не изменены.

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
- `ignores absence of newline on the last line of file wrt equality when ignoreNewlineAtEof`: перенесена первая проверка без опций — unchanged `a\n` (1), removed `b\nc` (2), added `b` (1). Остальные проверки этого случая требуют неперенесённых опций.

### test/diff/line.js — не перенесённые случаи
- `terminates early - async`: требует callback.
- `emits one change per line`: требует `oneChangePerToken`.
- Все девять случаев `#TrimmedLineDiff`: требуют `diffTrimmedLines`/`ignoreWhitespace`; две из них дополнительно требуют `newlineIsToken` или callback.
- Группа `#diffLinesNL`: требует `newlineIsToken`.
- Группа `Strip trailing CR`: требует `stripTrailingCr: true`.
- Неперенесённая часть `ignores absence of newline on the last line of file wrt equality when ignoreNewlineAtEof`: две проверки с `ignoreNewlineAtEof`. Первая проверка без опций перенесена и учтена как один частично перенесённый случай.

Итого по двум исходным файлам: 35 полностью перенесённых случаев, 1 частично перенесённый и 43 не перенесённых. Дополнительные регрессии и oracle в эти числа не входят.

## Не проверено
Нет платформенных ветвей. Точный момент срабатывания wall-clock `timeout_ms` не проверяется тестами, чтобы они не зависели от реального времени; детерминированный `max_edit_length` проверен.

## Вопросы ведущему
Нет.
