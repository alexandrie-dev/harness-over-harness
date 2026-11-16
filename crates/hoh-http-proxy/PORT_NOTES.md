# hoh-http-proxy ← packages/util/http-proxy @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/policy.ts` | `src/policy.rs` | Полностью: переменные и их приоритет, диагностика, fallback с различением отсутствия и отказа, NO_PROXY, loopback, маршрут URL. |
| `src/install.ts` | `src/install.rs` | reqwest 0.13.5; состояние, клиент и окружение принадлежат экземпляру, см. отличия. |
| `src/index.ts` | `src/lib.rs` | Все пять функций и оба типа экспортированы в snake_case; модули `policy` и `install` публичны. |
| `tests/policy.spec.ts` | `tests/policy.rs` | 56 исходных случаев, включая развёрнутые `it.each`; ещё 2 проверки точных текстов и граничных значений. Все 58 проходят. |
| `tests/install.spec.ts` | `tests/install.rs` | Все 24 исходных случая представлены. `removes_an_environment_name…` и `restores_the_dispatcher…` проверяют неизменность исходного снимка/внешней установки при layering и Drop: глобального состояния для восстановления нет. Identity транспорта проверяется двумя запросами через общий keep-alive сокет (один accept); in-flight lifetime — отдельным дополнительным случаем. В трёх случаях проверен overlay, но не запуск Node, см. ниже. |
| `tests/install-http1.spec.ts` | `tests/install-http1.rs` | Все 7 случаев проверяют options/http1_only установки, dispatcher и созданных ими routes, включая direct-слой поверх proxy. Some(true) отличается от None в самом сохранённом record; ClientBuilder, построенный отдельно тестом, больше не служит доказательством. |
| `tests/matcher-parity.spec.ts` | `tests/matcher-parity.rs` | Все 9 случаев. Прямой путь использует loopback origin и DNS override вместо внешнего DNS и ожидаемого отказа. |
| Дополнительные проверки fix-round | `tests/transport.rs`, unit test в `src/install.rs` | Настройки fetch-транспорта, Debug трёх публичных типов, не-Unicode snapshot без изменения окружения процесса. Регрессия oversized proxy URL пока красная: ждёт workspace-зависимость http, см. вопросы. |
| `README.md` | эти примечания и rustdoc | Прочитан полностью; публичный контракт и ограничения учтены. |

В образце нет `locale/`, собственного `LICENSE` и указаний на заимствованный код в этом пакете.
Новых внутренних зависимостей нет. Добавлены только разрешённые workspace-зависимости `indexmap`,
`serde`, `url`; для тестов `tokio` с `net`. `reqwest` уже был в манифесте.

## Отличия от образца

1. По решению DEPS.md §5.5.1 и PORTING.md §6 отсутствуют глобальный dispatcher, глобальный `active`
   и изменение `process.env`. `install_proxy_from_environment(&env, report, options)` синхронно
   возвращает `Result<ProxyInstallation, reqwest::Error>`. Построение клиента не выполняет запросов.
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
   вызвать `route_client_builder(policy, options)`, затем `.redirect(reqwest::redirect::Policy::none())`
   перед `.build()`, используя возвращённый таким образом собственный клиент.
5. `EnvLookup::get` возвращает `Option<String>` вместо объекта `{ value }`. Trait можно реализовать
   замыканием, `IndexMap<String, String>` или `HashMap<String, String>`. Как в образце, свёртка
   имён на Windows — обязанность входного снимка, а не резолвера. Тестовый lookup её выполняет.
   Типизированные `ProxySource` и `ProxyDiagnosticKind` заменяют строковые объединения, serde сохраняет
   исходные имена, порядок полей и отсутствие необязательных proxy-полей.
6. Новые публичные символы, необходимые для явного владения: `ProxyInstallation` с методами
   `with_inherited_environment`, `layer`, `client`, `policy`, `environment`, `options`, `http1_only`; `ProxyEnvironment`;
   `ProxySource`, `ProxyDiagnosticKind`; `route_client_builder`. Последний возвращает настроенный
   `ClientBuilder`, чтобы потребитель мог добавить, например, DNS override, не создавая второй
   matcher. Все имена функций образца сохранены с преобразованием camelCase → snake_case.
   Добавлен `RouteDispatcher` с `client`, `options`, `http1_only`, `route_for` и Deref к Client;
   `DispatcherOptions` и `ProxyRoute` также имеют `http1_only`, у route есть `options`.
7. Тестовые серверы слушают только `127.0.0.1:0`; порт выбирает ОС. Прямые запросы к тестовым доменам
   используют `ClientBuilder::resolve` с loopback-адресом fixture. Внешних DNS/HTTP-запросов нет.
   Тест lifetime ждёт получения запроса сервером до удаления установки и затем разрешает ответ.
8. Debug для ProxyPolicy, ProxyInstallation и ProxyRoute скрывает пароль URL как `***`, включая
   унаследованные и публикуемые proxy-env значения. Сериализация, реальные поля и значения ребёнка
   не изменяются. Debug вложенного RouteDispatcher использует тот же redacted ProxyPolicy.
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
11. При чтении зависимостей подтверждён риск из §3 fix-брифа:
    `reqwest-0.13.5/src/proxy.rs:786–793` отбрасывает ошибку conversion и результат matcher;
    `hyper-util-0.1.21/src/client/proxy/matcher.rs:336` вызывает parse::<http::Uri>().ok().
    `http-1.5.0/src/uri/mod.rs:145,296` ограничивает URI 65 534 байтами, но url::Url принимает
    `http://proxy.test/` с 65 535 символами пути. Красный regression test воспроизводит отсутствие
    отказа при installation/route construction. Сам guard ещё не реализован: `http` отсутствует
    в workspace.dependencies, а менять корневой манифест или добавлять стороннюю зависимость вне
    workspace запрещено writer-брифом. Запрошено добавление `http = "1.5.0"` ведущим; библиотека
    уже перечислена в DEPS.md. До этого fix-round не завершён.

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

Полный прогон заблокирован средой: sandbox отвергает `TcpListener::bind(127.0.0.1:0)` с
`Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }` до первого запроса.
Тесты не пропущены, не помечены ignored и не ослаблены ради зелёного результата.

Команда:

```text
CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w6 cargo test -p hoh-http-proxy --no-fail-fast
```

Результаты fix-round: unit tests — `1 passed; 0 failed`; `policy` — `58 passed; 0 failed`;
`install` — `17 passed; 8 failed`; `install-http1` — `7 passed; 0 failed`;
`matcher-parity` — `0 passed; 9 failed`; `transport` — `5 passed; 1 failed`.
Всего 88 passed, 18 failed. Из них 17 отказов — запрет loopback listener, один — сохранённая
красная регрессия `rejects_a_url_the_policy_accepts_but_reqwests_uri_matcher_would_drop` (guard ждёт http).

Не удалось выполнить проверки после создания listener:

- `carries_the_dispatcher_already_routing_so_a_branch_and_its_request_agree`;
- `captured_route_finishes_in_flight_request_after_installation_is_dropped`;
- `connects_directly_when_the_bypass_list_covers_the_target`;
- `keeps_a_scheme_direct_when_the_policy_refused_the_proxy_the_user_named_for_it`;
- `reports_a_value_it_cannot_use_and_installs_the_rest`;
- `restores_the_dispatcher_the_route_and_the_environment_on_disposal`;
- `routes_the_explicitly_shared_fetch_client_through_the_proxy`;
- `stops_proxying_when_the_mounted_policy_proxies_nothing`;
- все 9 `agrees_on_*` случаев в `tests/matcher-parity.rs` (empty, exact host, dot suffix,
  star suffix, other host, star, default port, wrong port, comma list).

`cargo build -p hoh-http-proxy`, `cargo clippy -p hoh-http-proxy --all-targets -- -D warnings`
и `cargo fmt -p hoh-http-proxy -- --check` завершились успешно (exit 0), с теми же тремя
переменными `CARGO_BUILD_JOBS=4`, `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR`, что указаны выше.

Windows/macOS не запускались. Свёртка настоящих Windows environment-имён, TLS к https-proxy,
аутентификация proxy и реальное согласование HTTP/2 не проверены исходным набором/этой средой.
Полная побайтная эквивалентность URL.parse Node и url 2.5.8 вне использованных случаев не сверялась.
Различия стандартных заголовков, TLS и распаковки undici/reqwest относятся к будущим потребителям
тел ответов (DEPS.md §5.5.1); этот пакет не добавляет запросы/заголовки/распаковку самостоятельно.

## Вопросы ведущему

Внутренние crates ничего не блокируют. Для guard из §3 fix-брифа требуется разрешённая workspace-строка
`http = "1.5.0"`: сейчас её нет; корневой манифест writer не меняет. Известный silent bypass по oversized
URI до этого не закрыт. Ведущий сообщил, что исходный набор из 98 тестов прошёл вне sandbox.
После исправлений также нужен его прогон loopback-тестов, включая новое доказательство reuse соединения.
