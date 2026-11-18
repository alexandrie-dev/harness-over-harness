# hoh-web ← packages/web/web @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | WebRuntime, конфигурация, два реестра, выбор, readiness, ограничение sources, сервис web |
| src/types.ts | src/types.rs | все типы запросов/ответов и providers, событие, WebError |
| tests/web.spec.ts | tests/web.rs | все 34 именованные группы; четыре строки it.each исполняются в цикле (37 развёрнутых исходных случаев) |
| собственные проверки | tests/web.rs | ещё 15 тестов: JSON-текст, схема/default, environment, ошибки и signal identity, fetch resolution, границы cap, lifecycle и concurrency |
| locale/{de,en,es,fr,ja,pt,ru,zh}.json | locale/ с теми же именами | восемь файлов без изменений; diff -rq не нашёл различий |
| README.md, package.json | PORT_NOTES.md, Cargo.toml | прочитаны; отдельного LICENSE и заимствованных фрагментов нет |

Перед реализацией перечитаны актуальные stage1-writer.md, PORTING.md, обязательные разделы
docs/kernel.md, публичные API и заметки зависимостей. Изменения ограничены crates/hoh-web.
Сетевых запросов в исходном сервисе нет: их выполняют подключаемые providers.

## Отличия от образца

- WebRuntime одновременно реализует Plugin с NAME=WebRuntime и предоставляет типизированный ключ
  Web (имя сервиса web). Дополнительные публичные символы Rust: ключ Web,
  WebProvidersChanged (исходное имя web/providers-changed), WebFuture (boxed Send future).
  Clone WebRuntime разделяет реестры и Cfg одного экземпляра, не создаёт новый сервис.
  Публичный модуль types соответствует точке входа ./types; корень реэкспортирует те же типы.
  Внешнее имя строки пакета @deepseek-ai/dsh-web не переименовано; регистрацию псевдонимов
  выполняет общий загрузчик, вне границ этого crate.
  У исходного класса нет export const name: registry.ts:324 берёт plugin.name, то есть
  имя класса WebRuntime. Оно сохранено для fiber diagnostics; logger.name внутри
  сервисного метода — web по Cordis service association, поэтому logger явно named("web").
- Provider-интерфейсы стали object-safe traits с info() -> WebProviderInfo вместо трёх
  свойств id/title/description. id стабилен на всё время регистрации; available() возвращает
  Result<bool, BoxError>, чтобы представить исходный throw. ready() по умолчанию даёт true.
  Методы search/fetch принимают owned request и Option<&hoh_timeout::AbortSignal> без
  подмены сигнала; возвращают Result<T, hoh_error::BoxError>. Ошибки availability,
  readiness и выполнения сохраняют исходную причину, тип и identity SharedError.
  Readiness поглощает только отказ выбора, а не отказ самого ready().
- Методы регистрации принимают caller &Context первым параметром (docs/kernel.md §7).
  Возвращается kernel Disposer вместо JS функции: dispose() сразу начинает синхронное
  снятие, а его future можно отбросить или дождаться. Drop handle не снимает регистрацию;
  выгрузка contributing fiber снимает её автоматически. Проверка дубликата и вставка
  неделимы под коротким mutex; IndexMap сохраняет порядок, shift_remove его не меняет.
  Provider callbacks и уведомления вызываются после освобождения mutex.
- Событие использует kernel Contained: root().dispatch(...).invoke(report) соответствует
  сырому events.dispatch образца, без receiver filter contributing context. Каждая ошибка
  слушателя получает два исходных warn, принадлежащих контексту сервиса. Обычный emit
  не обеспечивает эту семантику. Async listener tasks принадлежат их fiber средствами ядра.
- WebRuntimeConfig использует Cfg и Volatile<Option<String>>. Читается актуальный снимок
  при каждом выборе; окружение DSH_WEB_SEARCH_PROVIDER/DSH_WEB_FETCH_PROVIDER сохраняется
  один раз в new(). Явная пустая строка остаётся выбранным id. Дополнительный публичный
  with_environment получает два уже снятых значения окружения для embedding/чистых тестов;
  обычный Plugin читает настоящее окружение в исходном месте. Некорректные OS-строки
  декодируются с replacement через to_string_lossy, как UTF-8 окружение Node.
  Корневой schema default={} сохранён явно: его ставит фабрика object в
  vendor/schemastery/src/index.ts:912–913. Поэтому отсутствующая/null конфигурация
  при загрузке становится пустым объектом; оба provider id остаются unset.
- maxResults и statusCode представлены u64 по PORTING.md §4 (счётчик и HTTP-код).
  Дробные/отрицательные/NaN/Infinity maxResults TypeScript number не представимы этим API;
  источник сам их не валидирует и для них применил бы JS comparison/slice. Обычные bounds,
  включая 0, равенство длине, отсутствие и очень большой предел, проверены. Новых настроек
  и скрытого ограничения statusCode диапазоном 2xx/HTTP не вводится.
- WebFetchBody — закрытый serde enum html/text. Остальные структуры сохраняют исходные
  JSON-имена и порядок объявленных полей; None не пишет optional-поле. LocalizedText
  берётся из hoh-package-manifest и сохраняет порядок локалей. Это типизированный контракт
  построения новых значений, не lossless редактор произвольного JSON: serde игнорирует
  неизвестные ключи, Option при чтении null становится None, известные ключи пишутся в
  порядке полей. Сам сервис JSON-текст не разбирает и не пишет; raw JSON транспорта и
  сериализация текстов через общий JS codec принадлежат вызывающему протоколу/инструменту.
- WebError — пространство конструктора с NAME; new(message, code) возвращает общий
  hoh_error::HarnessError, а не отдельную обёртку. Код остаётся открытой строкой, как в
  исходнике; причина задаётся общим with_cause(). Код не закрыт enum встроенных ошибок.
- Тест live-config заменяет fixture из settings/tests прямым commit_volatile kernel:
  обе операции меняют выбранный provider без remount, проверены uid и identity значения
  сервиса. Настройки и загрузчик здесь не дублируются и не добавляются в зависимости.

Поиск root/subpath imports в переносимых src (без client/api/typert/host/experimental,
окон и тестов) нашёл пользователей в web-fetch-http, web-search-deepseek,
web-deepseek-api и tool-web. Они импортируют WebError, оба Provider, оба Request,
оба Result, WebSearchSource, WebFetchBody; регистрацию/выполнение вызывают через ctx.web.
Без прямого именованного импорта в переносимых src: WebRuntime (default и named),
WebRuntimeConfig, WebProviderInfo, WebProviderList. Последние два используются через
результат listProviders; WebRuntime загружается строкой пакета и используется в тестах.
Все эти экспорты перенесены.

## Не перенесено

TypertRemoteService/@Remote, generated ./typert и ./remote transport, declaration merging
как механизм — PORTING.md §8. Метод list_providers остаётся public и помечен точной строкой
`/// Remote: web.listProviders`. Текст и код ошибок сервиса не меняются.
Пропущенных исходных runtime-тестов нет: 34/34 группы, 37/37 развёрнутых случаев.
Два случая с пометкой HMR safety проверяют обычную выгрузку fiber и перенесены полностью.
Замена исполняемого JS-кода в них не происходит. README и npm metadata не копируются.

## Не проверено

Платформенных веток нет. Windows/macOS отдельно не запускались. Реальные provider API,
HTTP, настройки через settings/loader и клиентский Typert транспорт не запускались:
они принадлежат другим пакетам. Непредставимые JS-состояния и граница serde описаны выше.
Обычный Plugin/new проверен с явными provider id, без изменения process.env;
ветвление fallback/priority/snapshot проверено with_environment. Реальная смена
окружения процесса между mount и вызовом отдельно не выполнялась.

## Вопросы ведущему

Недостающих API зависимостей нет. Вопросов, блокирующих перенос, нет.
Ограничение number -> u64 следует обязательному PORTING.md §4; если требуется воспроизводить
некорректные внешние дробные bounds, нужен отдельно согласованный числовой wire API.

## Проверки

Все Cargo-команды выполнены с
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w1`.

- `cargo test -p hoh-web`: exit 0, финальное состояние после правки имени fiber/logger;
  `test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
  Unit и doc-tests отдельно: 0 passed, 0 failed. Из этих 49 Rust-тестов 34 соответствуют
  исходным группам (37 развёрнутых случаев), ещё 15 — собственные регрессии.
- `cargo clippy -p hoh-web --all-targets -- -D warnings`: exit 0, предупреждений нет;
  `Finished dev profile [unoptimized + debuginfo] target(s) in 4m 30s`.
- `cargo fmt -p hoh-web -- --check`: exit 0 после последних изменений Rust-кода.
- `diff -rq <source>/locale crates/hoh-web/locale`: exit 0, различий нет.
- `git diff --check -- crates/hoh-web`: exit 0; поиск todo!/unimplemented!/TODO/stub
  в crate не нашёл совпадений.

Начальный Cargo запуск уже загрузил прежний manifest, когда в нём serde_json был только
dev dependency; добавление schema default потребовало обычной зависимости. После обновления
manifest новый запуск прошёл. Ошибок сборки в чужих crates не было; их код не менялся.
Финальный повтор тестов занял 12m03s из-за I/O contention; само исполнение — 0.01s.
Все обязательные проверки завершены. Заглушек и незаконченной реализации в crate нет.
