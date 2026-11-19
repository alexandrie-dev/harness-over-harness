# hoh-web-fetch-http ← packages/web/web-fetch-http @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | конфигурация/defaults, исходная проверка чисел, Plugin и Inject |
| src/provider.ts | src/provider.rs | GET, redirects, byte/UTF-16 caps, deadline, причины ошибок, metadata |
| src/policy.ts | src/policy.rs | URL, origin, Content-Type, charset и строгая UTF-8 recovery |
| src/transport-failure.ts | src/transport_failure.rs | все классы, четыре cause hops, aggregate attempts, bounded detail |
| tests/fetch-http.spec.ts | tests/fetch_http.rs | 57 из 58 именованных групп написаны, все таблицы сохранены; исключение ниже |
| tests/network-error.spec.ts | tests/network_error.rs | все 16 групп, включая 13 строк таблицы, написаны |
| tests/proxy.spec.ts | tests/proxy.rs | все пять случаев написаны; dev dependency разрешён ведущим |
| собственные проверки | tests/boundaries.rs, tests/fetch_http.rs, tests/compression.rs, tests/decoder.rs | URL/whitespace/BOM/config, transport/size, compressed streams и in-memory decoder checks |
| locale/{de,en,es,fr,ja,pt,ru,zh}.json | locale/ | скопированы побайтно; diff -rq: exit 0 |
| README.md, package.json | эти заметки, Cargo.toml | прочитаны; отдельной лицензии/заимствованного фрагмента нет |

**Текущее состояние: исправление по redrun 125537.** Все dependency-решения получены.
Внешний прогон review 124814 дал 104 passed / 1 failed: новый malformed-chunk regression
доказал неполное распознавание формулировок Hyper. Обе ветки исправлены ниже.
Прежние остановленные сборки не возобновлялись. Чужие crates, Cargo.toml workspace и
Cargo.lock вручную не менялись.

## Отличия от образца

- WebFetchHttpPlugin реализует Plugin с исходным NAME=web-fetch-http; Deps с Service<Web>
  заменяет inject=['web']. Config сохраняет поля и defaults. Raw numeric config остаётся
  f64 до исходных проверок, чтобы отрицательное/дробное значение давало исходный текст,
  а не ранний generic integer error схемы. Порядок validation сохранён. Resolved counters
  HttpFetchLimits — u64 по PORTING §4, timeout_ms — f64. Положительная дробная capacity
  округляется вниз, как source slice/subarray; чрезвычайно большие capacities saturate
  до u64::MAX, недостижимого для in-memory response. Диагностика использует исходное
  число через js_number_string. Config::{validate,limits} и ConfigError — новые Rust API.
- HttpFetchProvider реализует общий WebFetchProvider. info() заменяет id/description;
  описание и порядок восьми локалей скопированы из source. title отсутствует.
  available() всегда Ok(true), default ready() общего trait — true. new возвращает
  Result из-за fallible reqwest builder. with_client_builder, with_transport и
  HttpFetchTransport — дополнительные Rust API; первый всегда ставит redirect none,
  referer false, 10 s connect / 300 s read. Клиент по умолчанию отключает автоматическую
  proxy-env политику reqwest. Cookie store/authorization не включаются.
- Глобальный undici dispatcher заменяется явным builder согласно DEPS.md и уже
  принятому PORT_NOTES hoh-http-proxy: composition передаёт route_client_builder через
  дополнительную метку HttpFetchClientBuilder; у каждого provider собственный pooled
  Client. Зафиксированный builder выполняется один раз при mount. Конфигурационных
  полей proxy в пакет не добавлено. Тесты используют общий matcher hoh-http-proxy,
  а не копию его policy; этот dev dependency разрешён ведущим отдельным сообщением.
- Все WebError — общий HarnessError, без wrapper. Deadline/AbortSignal/SharedError
  взяты из общего hoh-timeout/hoh-error. Предварительно отменённый сигнал даёт WEB_ABORTED
  без cause, request-phase cancel сохраняет reason, body-read cancel имеет AbortError
  как reader образца; собственный timeout восстанавливается из signal reason и сохраняет
  его shared identity. Никаких собственных голых CancellationToken в API нет.
- Reqwest response Drop отменяет незаконченный body на каждом error/redirect пути.
  Строки исходных ошибок, порядок бюджет → Location → URL validation → origin и число
  выполненных запросов перенесены. Адреса private/loopback/link-local/metadata разрешены;
  DNS filtering и address pinning в src отсутствуют. Комментарий policy.ts о public
  resolution устарел; README и реальные тесты прямо подтверждают разрешение этих адресов.
- Дополнительный CharsetDecoder заменяет глобальный TextDecoder; encoding() даёт
  каноническое WHATWG имя в нижнем регистре, fatal сохранён. При настоящем byte truncation
  строгая UTF-8 recovery допускает только незаконченный последний scalar. Обычное
  декодирование заменяет malformed bytes и удаляет BOM своей кодировки. Строковые
  пределы считаются в UTF-16; одиночный surrogate на границе не представим в String,
  поэтому сохраняется целый предшествующий scalar (обязательное правило PORTING §4).
  ASCII case folding charset и JS whitespace используют общую таблицу hoh-util-values.
  В отличие от encoding_rs отвергаются replacement, x-user-defined и ISO-8859-16
  со всеми их labels. Отсутствие ISO-8859-16 у Node TextDecoder подтверждено локальной
  документацией `/usr/share/doc/nodejs/api/util.html`; Node 24 отдельно не запускался.
- Content-Encoding декодируется потоково через async-compression; все автоматические
  decoder reqwest выключены. Исходные Content-Length/Content-Type доступны до чтения;
  byte cap применяется к декодированным байтам. Цепочка кодировок раскрывается с конца;
  наличие неизвестной кодировки оставляет всю цепочку без декодирования. Gzip допускает
  несколько members и отсутствие trailer (аналог finishFlush Z_SYNC_FLUSH); deflate
  распознаёт zlib/raw по первому CMF, br и zstd имеют отдельные decoders. Новых
  публичных символов для этого не добавлено.
  В source clone нет node_modules/undici и его bundled docs; package.json требует
  Node >=24.9.0 и undici 8.11.2. **Предположение без Node-оракула:** fetch этой версии
  декодирует gzip/x-gzip, deflate, br, zstd, а Accept-Encoding — gzip, deflate для HTTP
  и br, gzip, deflate, zstd для HTTPS. Это явно не выдаётся за проверку Node 22 или 24.
  Политика malformed compression вне добавленных fixtures и тексты backend decoder
  errors не сверены с undici; Rust decoder может давать другие технические details.
- TransportFailure сохраняет code,message и исходный порядок. Дополнительные plain
  Error-типы TransportError/TransportValue представляют metadata Node и non-Error throws,
  поскольку Rust Error не имеет произвольных свойств. code хранится как Option<Value>:
  исходный случай code=-54 передаёт именно число, а classifier принимает только string.
  SharedError разворачивается без
  дополнительного cause hop. Aggregate раскрывается только на верхнем innermost уровне,
  показываются четыре попытки и счётчик остальных, detail ограничен 500 UTF-16 units.
  node_error_code используется для real io errors. Отдельный adapter переводит known
  reqwest/DNS/socket/TLS failures в Node-style summaries; оригинальная reqwest ошибка
  сохраняется причиной WebError. Системные API двух клиентов различны; ограничения ниже.
  Оба дополнительных типа — enum с thiserror. TransportError::Node хранит свойства
  Node Error; методы new/fetch_failed/aggregate/with_name/with_code/with_cause нужны
  Rust transport adapter и deterministic replay. TransportValue::Thrown хранит raw JSON,
  TransportValue::Undefined — явно брошенный undefined. Повторные response headers
  объединяются через `, `, значения читаются как Latin-1. User-Agent преобразуется
  в ByteString, очищается от HTTP whitespace по краям и проверяется до соединения.
  Внутренние client timeout различаются по фазам: connect → UND_ERR_CONNECT_TIMEOUT,
  headers → UND_ERR_HEADERS_TIMEOUT, body → UND_ERR_BODY_TIMEOUT. Последние два кода,
  как в исходном classifier, относятся к generic network failure. Системный ETIMEDOUT
  сохраняет свой код; read errno не получает ошибочный адрес connect в detail. Короткое
  HTTP-тело с объявленным большим Content-Length у Hyper выражается как
  `end of file before message length reached`; adapter переводит его в исходный
  `UND_ERR_SOCKET`, который source относит к `WEB_FETCH_CONNECTION_INTERRUPTED`.
  Общий признак reqwest `is_body()` не считается закрытием сокета: malformed chunk
  framing остаётся generic network failure. Hyper `Invalid chunk size line: missing size
  digit` (невалиден первый символ) и `Invalid chunk size line: Invalid Size` (невалиден
  последующий символ) переводятся в вид undici 8.11.2: `HPE_INVALID_CHUNK_SIZE` и
  `Response does not match the HTTP/1.1 protocol (Invalid character in chunk size)`.
- Tokio loopback fixtures имеют собственные JoinSet и teardown. Тестовое paused time
  удерживается от auto-advance во время реального socket I/O; deadline продвигается
  вручную после наблюдения запроса/заголовков. Сеть вне 127.0.0.1 не используется;
  network-error tables идут через replay HttpFetchTransport до любого DNS.

Поиск именованных imports в переносимых src (без client/api/typert/host/experimental)
не нашёл внешних потребителей экспортов этого пакета: он загружается строкой плагина.
Все root и ./src/* exports сохранены через публичные модули provider, policy,
transport_failure и Plugin; name/inject/apply представлены NAME/Deps/Plugin::apply.

## Не перенесено

- `has no default export (namespace plugin export shape)` — единственный исключённый
  source case: проверка формы JavaScript namespace, PORTING §8/10. Rust модуль не имеет
  механизма default export. Остальные 78 из 79 именованных групп написаны: 108 из 109
  развёрнутых случаев. Это учёт написанного переноса, **не утверждение о прохождении**.
- Dynamic import undici/браузерный startup механизм — PORTING §8; reqwest с rustls.

## Не проверено

Библиотека и все test binaries скомпилированы. Внешний прогон ведущего до исправления
дал 103 passed / 1 failed; единственным отказом был короткий body. После исправления
локальный полный прогон дал 43 passed, а 61 listener-test не смог начать сценарий:
sandbox запрещает bind 127.0.0.1:0 (`EPERM`). Поэтому итоговый socket-прогон должен
повторить ведущий вне sandbox. Тесты не пропущены и не ослаблены; in-memory checks
дополняют loopback.
Платформенных веток в src нет; Windows/macOS,
реальная DNS, TLS/сертификаты и HTTP/2 не запускались. Моки проверяют все исходные
transport cases, но реальные rustls failures не дают точно те же OpenSSL code/details;
UnknownIssuer, в частности, не различает self-signed leaf и incomplete chain в
доступном reqwest Error API. Реальные negotiated TLS roots и wire defaults отличаются
согласно ранее принятому DEPS.md. Не заявляется их побайтная эквивалентность Node.
Полный corpus WHATWG URL edge cases вне tests не сверялся.

## Вопросы ведущему

Все три исходных запроса разрешены ведущим: encoding_rs 0.8.44, async-compression
0.4.42 и hoh-http-proxy dev dependency добавлены только в manifest этого crate.
Для HTTP deflate дополнительно включена feature zlib одобренного async-compression.
Ведущий также заполнил offline cache zstd 0.13.5 / zstd-safe 7.4.1. Все первоначальные
dependency вопросы закрыты. Внешний прогон исправления 123741 дал 104 passed / 0 failed.
Для нового listener-regression `a_malformed_chunk_size_remains_a_generic_network_failure`
нужен такой же прогон вне sandbox: bind loopback в текущей среде возвращает EPERM.
Listener fixtures не заменялись.

## Архив проверок до очистки target-w1

Заново прочитаны brief, PORTING.md и актуальные правила/заметки kernel. По рабочему дереву
подтверждены четыре исходных файла (601 строка), все exports, 104 написанных Rust tests на Linux,
восемь locale-файлов. Исходный consumer scan
повторён. encoding_rs/async-compression теперь присутствуют в workspace; зависимости
добавлены в manifest этого crate согласно явному разрешению ведущего.

Команды с `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w1`:

- `cargo fmt -p hoh-web-fetch-http -- --check`: повторно exit 0 после последних правок.
- `cargo test -p hoh-web-fetch-http`: сборка завершилась за 62m 41s, exit 101 на tests.
  unit: 0 passed / 0 failed; boundaries: **6 passed / 0 failed**; compression:
  **0 passed / 8 failed**, каждый отказ — bind в tests/support/mod.rs:78:
  `Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }`.
  Cargo после этого не запускал fetch_http/network_error/proxy. Это отказ среды,
  а не подтверждение прохождения HTTP-поведения. Listener tests сохранены.
- `cargo test -p hoh-web-fetch-http --test network_error`: **16 passed / 0 failed**, exit 0.
- `cargo test -p hoh-web-fetch-http --test decoder`: первоначальные **4 passed / 0 failed**, exit 0;
  после добавления двух regressions для body timeout и Linux raw read errno запущен повторно.
  Проверены реальные gzip/x-gzip, zlib/raw deflate, br, zstd и stacked decoders,
  original Content-Length, decoded cap/exact EOF, gzip members/отсутствующий trailer,
  malformed body. Эти случаи дополняют исходные loopback fixtures.
- `cargo clippy -p hoh-web-fetch-http --all-targets -- -D warnings`: первый прогон exit 0,
  13m 03s; после исправления классификации client body/header timeout и read errno
  требуется повторный прогон на последнем состоянии.
- `diff -rq <source>/locale crates/hoh-web-fetch-http/locale`: exit 0, различий нет.
- `git diff --check -- crates/hoh-web-fetch-http`: exit 0.

Прежние незаконченные проверки не объявляются успешными. Все dependency вопросы закрыты;
shared manifests и другие crates не редактировались.

## Исправление по внешнему прогону 123741

Внешний прогон ведущего: 103 passed / 1 failed, clippy и fmt чисты. Reqwest/Hyper при
раннем EOF тела с большим объявленным Content-Length сообщает
`end of file before message length reached`; прежний adapter не узнавал эту формулировку
и ошибочно выдавал `WEB_FETCH_NETWORK_ERROR`. В source
`src/transport-failure.ts:50-52` код `UND_ERR_SOCKET` входит в класс
`WEB_FETCH_CONNECTION_INTERRUPTED`. Adapter теперь переводит эту Hyper-ошибку в
`UND_ERR_SOCKET`; существующий падавший на старом коде loopback-regression дополнительно
проверяет полный model-visible message.

После исправления с `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2`:

- `cargo test -p hoh-web-fetch-http --no-fail-fast`: все targets собраны; 43 passed,
  61 listener-tests отказали до сценария на bind с `EPERM`, code failures не наблюдались;
- `cargo clippy -p hoh-web-fetch-http --all-targets --no-deps -- -D warnings`: exit 0;
- `cargo fmt -p hoh-web-fetch-http -- --check`: exit 0.

Повторный внешний loopback-прогон дал 104 passed / 0 failed; лог ведущего
`pl-124814-hoh-web-fetch-http.lead-test.txt`.

## Исправление по review 124814

Убран широкий fallback `error.is_body()`: он ошибочно превращал любой body protocol
error в `UND_ERR_SOCKET`. Добавлен loopback-regression с
`Transfer-Encoding: chunked` и невалидным размером `zz`; на старом коде он получал
`WEB_FETCH_CONNECTION_INTERRUPTED`. Теперь ожидаются `WEB_FETCH_NETWORK_ERROR` и полный
Node-совместимый текст. Форму ошибки подтверждают установленные вместе с dsh Node
24.22.3 и undici 8.11.2: `lib/dispatcher/client-h1.js:439-449` и
`lib/core/errors.js` (`HTTPParserError`). TS classifier оставляет неизвестный
`HPE_INVALID_CHUNK_SIZE` в generic class и показывает его в suffix.

Примечание ревью о backend-текстах malformed compression не требует кода: различие уже
явно записано выше и класс закреплён тестом. `cargo check -p hoh-web-fetch-http --tests`
после исправления: exit 0. Финальные проверки с `CARGO_BUILD_JOBS=2`:

- `cargo test -p hoh-web-fetch-http --no-fail-fast`: все targets собраны; 43 passed,
  62 listener-tests, включая новый regression, отказали до сценария на bind с `EPERM`;
- `cargo clippy -p hoh-web-fetch-http --all-targets --no-deps -- -D warnings`: exit 0;
- `cargo fmt -p hoh-web-fetch-http -- --check`: exit 0.

Полный набор теперь содержит 105 тестов. Новый socket-regression должен повторить ведущий
вне sandbox; предыдущий внешний прогон того же набора до его добавления был 104/0.

## Исправление по redrun 125537

Внешний прогон нового regression дал 104 passed / 1 failed. Для fixture `zz` Hyper 1.12
останавливается на первом символе и пишет `Invalid chunk size line: missing size digit`;
adapter знал только вторую ветку `Invalid Size`. Первая формулировка добавлена в то же
отображение `HPE_INVALID_CHUNK_SIZE`. Ожидание теста не изменено: его полный текст следует
undici 8.11.2 `client-h1.js:439-449` и `HTTPParserError` `errors.js:309-315`.
Тот же тест падал на старом коде с фактическим generic detail из Hyper, что зафиксировано
в `pl-125537-hoh-web-fetch-http.lead-test.txt` (104/1).

После исправления с `CARGO_BUILD_JOBS=2`:

- `cargo check -p hoh-web-fetch-http --tests`: exit 0;
- `cargo test -p hoh-web-fetch-http --no-fail-fast`: все targets собраны; 43 passed,
  62 listener-tests отказали до сценария на sandbox bind с `EPERM`;
- `cargo clippy -p hoh-web-fetch-http --all-targets --no-deps -- -D warnings`: exit 0;
- `cargo fmt -p hoh-web-fetch-http -- --check`: exit 0.

Полный внешний прогон исправленного socket-regression остаётся следующим pipeline gate;
в наборе 105 тестов.
