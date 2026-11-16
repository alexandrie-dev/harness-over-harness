# hoh-http-proxy ← packages/util/http-proxy @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/policy.ts` | `src/policy.rs` | Полностью: переменные и их приоритет, диагностика, fallback с различением отсутствия и отказа, NO_PROXY, loopback, маршрут URL. |
| `src/install.ts` | `src/install.rs` | reqwest 0.13.5; состояние, клиент и окружение принадлежат экземпляру, см. отличия. |
| `src/index.ts` | `src/lib.rs` | Все пять функций и оба типа экспортированы в snake_case; модули `policy` и `install` публичны. |
| `tests/policy.spec.ts` | `tests/policy.rs` | 56 исходных случаев, включая развёрнутые `it.each`; ещё 2 проверки точных текстов и граничных значений. Все 58 проходят. |
| `tests/install.spec.ts` | `tests/install.rs` | Все 24 исходных случая представлены. `removes_an_environment_name…` и `restores_the_dispatcher…` проверяют неизменность исходного снимка/внешней установки при layering и Drop: глобального состояния для восстановления нет. Identity транспорта проверяется двумя запросами через общий keep-alive сокет (один accept); in-flight lifetime — отдельным дополнительным случаем. В трёх случаях проверен overlay, но не запуск Node, см. ниже. |
| `tests/install-http1.spec.ts` | `tests/install-http1.rs` | Все 7 случаев проверяют options/http1_only установки, dispatcher и созданных ими routes, включая direct-слой поверх proxy. Из сохранённого record строится общий ClientBuilder; его Debug проверяется на наличие `http1_only: true` для Some(false) и отсутствие для None/Some(true). |
| `tests/matcher-parity.spec.ts` | `tests/matcher-parity.rs` | Все 9 случаев. Прямой путь использует loopback origin и DNS override вместо внешнего DNS и ожидаемого отказа. |
| Дополнительные проверки fix-round | `tests/transport.rs`, unit test в `src/install.rs` | Настройки fetch-транспорта, Debug трёх публичных типов, не-Unicode snapshot без изменения окружения процесса. Все 9 transport-тестов проходят: oversized URL, несовместимые символы authority, отказ для вручную собранной policy, граница длины и нормализация URL. |
| `README.md` | эти примечания и rustdoc | Прочитан полностью; публичный контракт и ограничения учтены. |

Чувствительность проверки pin подтверждена командой `cargo test -p hoh-http-proxy --test install-http1`:
при временном удалении `builder.http1_only()` — `3 passed; 4 failed`, все четыре отказа на новой
проверке Debug ClientBuilder; после восстановления вызова — `7 passed; 0 failed` в полном прогоне.

В образце нет `locale/`, собственного `LICENSE` и указаний на заимствованный код в этом пакете.
Новых внутренних зависимостей нет. Добавлены только разрешённые workspace-зависимости `indexmap`,
`serde`, `url`, `http` (проверка URI тем же парсером, что у reqwest), `thiserror` (ошибка построения);
для тестов `tokio` с `net`. `reqwest` уже был в манифесте. Все зависимости берутся из workspace;
корневые Cargo.toml и Cargo.lock вручную не менялись.

## Отличия от образца

1. По решению DEPS.md §5.5.1 и PORTING.md §6 отсутствуют глобальный dispatcher, глобальный `active`
   и изменение `process.env`. `install_proxy_from_environment(&env, report, options)` синхронно
   возвращает `Result<ProxyInstallation, ProxyBuildError>`. Построение клиента не выполняет запросов.
   Потребитель **обязан получить `installation.client().clone()` и отправлять запросы именно через
   этот `reqwest::Client`**: `client.get(url).send().await?`. Свободная функция `reqwest::get` и новый
   `Client::new()` не получают эту политику. Для собственного клиента вызывается
   `create_route_dispatcher(&env, options)?`; возвращаемый тип — `RouteDispatcher`, содержащий клиент
   и его исходные policy/options. Клиент доступен через `.client()`, методы клиента также доступны
   через Deref; `.route_for(&url)` описывает маршрут именно этого клиента.
2. `proxy_route_for(Some(&installation), &url)` возвращает `ProxyRoute::Proxied { proxy, dispatcher }`
   либо `ProxyRoute::Direct { options }`; `None` означает отсутствие установки. `dispatcher` —
   RouteDispatcher с дешёвой копией того же pooled Client. Options и http1_only доступны у установки,
   dispatcher и обоих вариантов route. Клиент строится в одном месте из того же RouteConfiguration,
   который хранит dispatcher; отдельной копии настроек для проверки нет. Такой route можно удерживать после удаления
   установки; уже начатые запросы завершаются. Вместо async disposer используется владение/Drop:
   пулы освобождаются после последнего владельца клиента и завершения запросов. Для вложенного
   окна вызывается `outer.layer(&env, report, options)?`; в окно передаётся inner, после него снова
   outer. Автоматического переключения всех потребителей нет.
3. Окружение публикуется как упорядоченная карта `ProxyEnvironment = IndexMap<String, Option<String>>`.
   Для каждого дочернего процесса сначала применяются `installation.environment()`, затем
   `proxy_environment_for_child(Some(&installation))`: `Some(v)` → `command.env(name, v)`,
   `None` → `command.env_remove(name)`. Это требуется и для direct-окна поверх proxy, чтобы вернуть
   исходные значения пользователя. `cleared_proxy_env()` применяется тем же способом к replay.
   Все шесть публикуемых имён снимаются одним исходным снимком; `ALL_PROXY` не публикуется.
   Снимок берётся из настоящего окружения после диагностики, только при первой proxy-установке;
   `ProxyInstallation::with_inherited_environment` позволяет передать тот же снимок явно.
   Пропущенные в явном снимке имена считаются отсутствующими, дополнительные не используются.
   Вложенная установка сохраняет исходный снимок самой внешней proxy-установки.
   Чтение настоящего окружения использует `var_os` + `to_string_lossy`: не-Unicode значение остаётся
   заданным именем, сохраняется для ребёнка с U+FFFD и не заменяется разрешённым fallback.
4. Один `reqwest::Proxy::custom` вызывает `proxy_for_url` для каждого URL. Встроенное чтение proxy-env
   reqwest отключено через `no_proxy()`, включая direct-клиент и отвергнутую схему. `allowH2: false`
   (`DispatcherOptions { allow_h2: Some(false) }`) вызывает `http1_only()` для всего клиента;
   `None` и `Some(true)` оставляют стандартное согласование HTTP/1.1 и HTTP/2. В первоначальном
   direct-случае всё равно возвращается direct Client, поскольку глобального клиента Rust нет.
   Общие настройки транспорта соответствуют решению fix-брифа: connect_timeout 10 s,
   read_timeout 300 s (одна настройка для ожидания заголовков и чтения тела), максимум 20 переходов,
   referer(false). Общего total timeout нет. Потребитель с `redirect: 'manual'` в образце должен
   вызвать `route_client_builder(policy, options)?`, затем `.redirect(reqwest::redirect::Policy::none())`
   перед `.build()`, используя возвращённый таким образом собственный клиент.
5. `EnvLookup::get` возвращает `Option<String>` вместо объекта `{ value }`. Trait можно реализовать
   замыканием, `IndexMap<String, String>` или `HashMap<String, String>`. Как в образце, свёртка
   имён на Windows — обязанность входного снимка, а не резолвера. Тестовый lookup её выполняет.
   Типизированные `ProxySource` и `ProxyDiagnosticKind` заменяют строковые объединения, serde сохраняет
   исходные имена, порядок полей и отсутствие необязательных proxy-полей.
6. Новые публичные символы, необходимые для явного владения: `ProxyInstallation` с методами
   `with_inherited_environment`, `layer`, `client`, `policy`, `environment`, `options`, `http1_only`; `ProxyEnvironment`;
   `ProxySource`, `ProxyDiagnosticKind`; `route_client_builder`. Последний возвращает
   `Result<ClientBuilder, ProxyBuildError>`, чтобы потребитель мог добавить DNS override, не создавая второй
   matcher. Все имена функций образца сохранены с преобразованием camelCase → snake_case.
   Добавлен `RouteDispatcher` с `client`, `options`, `http1_only`, `route_for`, `diagnostics` и Deref к Client;
   `DispatcherOptions` и `ProxyRoute` также имеют `http1_only`, у route есть `options`.
   `ProxyBuildError` имеет варианты `InvalidProxyUrl { origin }` и `Client(reqwest::Error)`;
   для InvalidProxyUrl введён новый текст Rust, у которого нет аналога в образце:
   `{origin}: proxy URL cannot be used to build the HTTP client`.
   Он называет поле и сообщает о жёстком отказе построения клиента, не раскрывая URL.
   Текст policy-диагностики `{origin} is not a valid URL; connecting directly` сохранён до байта.
7. Тестовые серверы слушают только `127.0.0.1:0`; порт выбирает ОС. Прямые запросы к тестовым доменам
   используют `ClientBuilder::resolve` с loopback-адресом fixture. Внешних DNS/HTTP-запросов нет.
   Тест lifetime ждёт получения запроса сервером до удаления установки и затем разрешает ответ.
8. Debug для ProxyPolicy, ProxyInstallation и ProxyRoute скрывает пароль URL как `***`, включая
   унаследованные и публикуемые proxy-env значения. Сериализация, реальные поля и значения ребёнка
   не изменяются. Debug вложенного RouteDispatcher использует тот же redacted ProxyPolicy.
   `ProxyEnvironment` — обычная упорядоченная карта: Debug `installation.environment()` или child overlay выводит исходные proxy URL вместе с паролями; перед логированием потребитель сам скрывает пароли.
9. Trust roots отличаются от Node: reqwest/rustls использует системное хранилище через
   rustls-platform-verifier; Node поставляет свои корни. Корпоративный CA, установленный в ОС,
   доверяется без отдельной настройки. `NODE_EXTRA_CA_CERTS`, рекомендуемый README образца, не читается.
   На Unix verifier через rustls-native-certs читает `SSL_CERT_FILE` и `SSL_CERT_DIR` настоящего процесса;
   это не переменные EnvLookup и не чтение `.env` этой библиотекой.
   Проверка исходников уточняет ограничение: на Unix без Android/Apple/wasm отсутствие хотя бы одного
   пригодного корня вызывает ошибку `No CA certificates were loaded from the system` при `build()`
   (`rustls-platform-verifier-0.7.1/src/verification/others.rs:85–107`,
   `reqwest-0.13.5/src/async_impl/client.rs:758–760`), даже у direct policy. Образ HoH для такой системы
   должен содержать системные сертификаты или указать пригодные SSL_CERT_FILE/SSL_CERT_DIR.
   Это утверждение не распространяется без проверки на нативные verifier Windows/macOS/Android.
10. Node fetch добавляет `accept: */*`, `accept-language: *`, `sec-fetch-mode: cors`, `user-agent: node`,
    `accept-encoding: gzip, deflate` для http и `br, gzip, deflate` для https, а также распаковывает ответ.
    Уточнение к fix-брифу: `accept: */*` reqwest уже задаёт сам
    (`reqwest-0.13.5/src/async_impl/client.rs:285`), поэтому дополнительно менять его не требуется.
    Остальные перечисленные заголовки при текущих features reqwest не добавляет. Потребитель,
    которому нужен тот же вид запроса на проводе, задаёт недостающие заголовки сам. Решение
    о распаковке принимается с первым потребителем тел (DEPS.md §5.5.1), в этом crate её нет.
11. Guard из §3 fix-брифа выполняется до fallback и до построения любого клиента/route.
    Установщик и `create_route_dispatcher` разрешают окружение через тот же алгоритм policy,
    добавляя к HTTP(S)-кандидату проверку реальной transport-совместимости: `reqwest::Proxy::all`
    запускает тот же IntoProxy, затем `http::Uri` разбирает сериализованный Url, после чего URI
    перестраивается с удалённым userinfo и путём `/` — как в `hyper-util-0.1.21`
    `src/client/proxy/matcher.rs:333–389`. Неподдерживаемые схемы/SOCKS/PAC обрабатываются исходной
    policy до этой дополнительной проверки; порядок диагностик ALL → HTTP → HTTPS сохранён.
    Отказ превращает слот в rejected, не absent: схема остаётся direct, не заимствуя fallback,
    и получает точный текст `{origin} is not a valid URL; connecting directly` с исходным регистром
    имени переменной. `install_proxy_from_environment` передаёт его report; потребитель отдельного
    dispatcher читает `dispatcher.diagnostics()` и передаёт каждое `.message` в свой журнал/report.
    Чистый `resolve_proxy_policy` и `is_supported_proxy_url` сохраняют исходное transport-free поведение.
    Вручную собранная policy при `route_client_builder` не может обойти guard: несовместимое поле
    возвращает `ProxyBuildError::InvalidProxyUrl` с origin `http_proxy` или `https_proxy`.
    В `Proxy::custom` захватываются только заранее проверенные неизменяемые Url, а не строки,
    которые reqwest мог бы затем не преобразовать и незаметно проигнорировать.

    Подтверждённые тестами входы, принятые url::Url, но отвергаемые transport:

    - сериализованная URL длиннее 65 534 байт (`http-1.5.0/src/uri/mod.rs:145,296`), включая
      `http://proxy.test/` с 65 535 символами пути; ровно 65 534 байта принимаются;
    - ASCII U+0022 (двойная кавычка), U+0060 (обратная кавычка), U+007B и U+007D (фигурные скобки)
      в hostname, например `http://proxy{bad}.test:3128/`: WHATWG URL их допускает без STD3,
      а таблица URI_CHARS и Authority::parse отвергают (`http-1.5.0/src/uri/mod.rs:154–168`,
      `src/uri/authority.rs:495–516`). Проверены все четыре символа для установки и отдельного route.

    Других подтверждённых отказов после успешного разбора HTTP(S) Url и Uri не найдено; проверка
    authority/rebuild всё равно выполняется. Отсутствие authority, неподдерживаемые схемы,
    неверные IPv6/порты и остаточные проценты hostname уже отсекаются исходным URL-парсером/политикой;
    percent-encoded буквы hostname нормализуются до этого. Нормализация
    регистра/пробелов/порта, IDN, IPv6 и percent-encoded credentials проверены как принимаемые.
    Учет длины идёт по Url после нормализации, как у reqwest, а не по сырому значению переменной.

По чтению `src` переносимых пакетов `proxyEnvironmentForChild` используется `subprocess`,
`clearedProxyEnv` — `loader-smoke` и `session-snapshot`; `installProxyFromEnvironment` используется
launcher в `apps/cli`. `createRouteDispatcher` используется только исключённым `experimental/liveshare`.
`proxyRouteFor`, `DispatcherOptions`, `ProxyRoute` внешних импортов в переносимых пакетах не имеют.
Публичные символы `policy.ts` также не имеют внешних импортов в исходном дереве; все перенесены.

## Не перенесено

Глобальные Symbol/dispatcher/process.env и disposer как механизм заменены явным владением согласно
решению ведущего; их наблюдаемые policy/env-операции представлены картами и слоями, описанными выше.
Динамический import undici отсутствует (PORTING.md §8); вместо него reqwest.

Полностью исключённых исходных тестовых случаев — 0: представлены 96 из 96 (56 + 24 + 7 + 9).
Три случая `withholds NODE_USE_ENV_PROXY ...` в `install.spec.ts` перенесены частично: сохранение
исходных значений и отсутствие флага проверяются, но `spawnSync(process.execPath, ...)` не выполняется.
Это проверка запуска стороннего Node runtime, которого нет в переносе; запуск Node запрещён брифом.
Реальное HTTP/2 согласование заменяет прежний mock только в рамках настройки reqwest, без TLS/h2 fixture.

## Не проверено

Listener-тесты выполняются только вне sandbox. В прогоне ведущего от **2026-11-16** прошёл весь
набор, включая 8 listener-тестов `install.rs` и 9 случаев `matcher-parity.rs`:
**109 passed, 0 failed**. Тесты не помечены ignored и не ослаблены. Команда с окружением сборки:

```text
CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w6 cargo test -p hoh-http-proxy --no-fail-fast
```

`cargo build -p hoh-http-proxy`, `cargo clippy -p hoh-http-proxy --all-targets -- -D warnings`
и `cargo fmt -p hoh-http-proxy -- --check` завершились успешно (exit 0), с теми же тремя
переменными `CARGO_BUILD_JOBS=4`, `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR`, что указаны выше.

Windows/macOS не запускались. Свёртка настоящих Windows environment-имён, TLS к https-proxy,
аутентификация proxy и реальное согласование HTTP/2 не проверены исходным набором/этой средой.
Полная побайтная эквивалентность URL.parse Node и url 2.5.8 вне использованных случаев не сверялась.
Различия стандартных заголовков, TLS и распаковки undici/reqwest относятся к будущим потребителям
тел ответов (DEPS.md §5.5.1); этот пакет не добавляет запросы/заголовки/распаковку самостоятельно.

## Вопросы ведущему

Нет. Guard реализован; недостающих API или workspace-зависимостей нет.
