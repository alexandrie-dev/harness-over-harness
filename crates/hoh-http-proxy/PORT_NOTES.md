# hoh-http-proxy ← packages/util/http-proxy @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/policy.ts` | `src/policy.rs` | Полностью: переменные и их приоритет, диагностика, fallback с различением отсутствия и отказа, NO_PROXY, loopback, маршрут URL. |
| `src/install.ts` | `src/install.rs` | reqwest 0.13.5; состояние, клиент и окружение принадлежат экземпляру, см. отличия. |
| `src/index.ts` | `src/lib.rs` | Все пять функций и оба типа экспортированы в snake_case; модули `policy` и `install` публичны. |
| `tests/policy.spec.ts` | `tests/policy.rs` | 56 исходных случаев, включая развёрнутые `it.each`; ещё 2 проверки точных текстов и граничных значений. Все 58 проходят. |
| `tests/install.spec.ts` | `tests/install.rs` | Все 24 исходных случая представлены с явным владением вместо глобальных переменных. В трёх случаях проверен overlay, но не запуск Node, см. ниже. |
| `tests/install-http1.spec.ts` | `tests/install-http1.rs` | Все 7 случаев: вместо mock undici проверяется параметр `http1_only` реального ClientBuilder; для двух случаев добавлены реальные HTTP/1.1 запросы через proxy и напрямую. |
| `tests/matcher-parity.spec.ts` | `tests/matcher-parity.rs` | Все 9 случаев. Прямой путь использует loopback origin и DNS override вместо внешнего DNS и ожидаемого отказа. |
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
   `create_route_dispatcher(&env, options)?`; возвращаемый тип — `reqwest::Client`.
2. `proxy_route_for(Some(&installation), &url)` возвращает `ProxyRoute::Proxied { proxy, dispatcher }`
   либо `ProxyRoute::Direct`; `None` означает отсутствие установки. `dispatcher` — дешёвая копия
   того же pooled Client, а не новый транспорт. Такой route можно удерживать после удаления
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
4. Один `reqwest::Proxy::custom` вызывает `proxy_for_url` для каждого URL. Встроенное чтение proxy-env
   reqwest отключено через `no_proxy()`, включая direct-клиент и отвергнутую схему. `allowH2: false`
   (`DispatcherOptions { allow_h2: Some(false) }`) вызывает `http1_only()` для всего клиента;
   `None` и `Some(true)` оставляют стандартное согласование HTTP/1.1 и HTTP/2. В первоначальном
   direct-случае всё равно возвращается direct Client, поскольку глобального клиента Rust нет.
5. `EnvLookup::get` возвращает `Option<String>` вместо объекта `{ value }`. Trait можно реализовать
   замыканием, `IndexMap<String, String>` или `HashMap<String, String>`. Как в образце, свёртка
   имён на Windows — обязанность входного снимка, а не резолвера. Тестовый lookup её выполняет.
   Типизированные `ProxySource` и `ProxyDiagnosticKind` заменяют строковые объединения, serde сохраняет
   исходные имена, порядок полей и отсутствие необязательных proxy-полей.
6. Новые публичные символы, необходимые для явного владения: `ProxyInstallation` с методами
   `with_inherited_environment`, `layer`, `client`, `policy`, `environment`; `ProxyEnvironment`;
   `ProxySource`, `ProxyDiagnosticKind`; `route_client_builder`. Последний возвращает настроенный
   `ClientBuilder`, чтобы потребитель мог добавить, например, DNS override, не создавая второй
   matcher. Все имена функций образца сохранены с преобразованием camelCase → snake_case.
7. Тестовые серверы слушают только `127.0.0.1:0`; порт выбирает ОС. Прямые запросы к тестовым доменам
   используют `ClientBuilder::resolve` с loopback-адресом fixture. Внешних DNS/HTTP-запросов нет.
   Тест lifetime ждёт получения запроса сервером до удаления установки и затем разрешает ответ.

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

Результаты: `policy` — `58 passed; 0 failed`; `install` — `17 passed; 7 failed`;
`install-http1` — `5 passed; 2 failed`; `matcher-parity` — `0 passed; 9 failed`.
Всего 80 passed, 18 failed; все 18 отказов — запрет создания loopback listener.
Обычный `cargo test -p hoh-http-proxy` также запущен: остановился на `install`, 17 passed / 7 failed.

`cargo build -p hoh-http-proxy`, `cargo clippy -p hoh-http-proxy --all-targets -- -D warnings`
и `cargo fmt -p hoh-http-proxy -- --check` завершились успешно (exit 0), с теми же тремя
переменными `CARGO_BUILD_JOBS=4`, `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR`, что указаны выше.

Windows/macOS не запускались. Свёртка настоящих Windows environment-имён, TLS к https-proxy,
аутентификация proxy и реальное согласование HTTP/2 не проверены исходным набором/этой средой.
Полная побайтная эквивалентность URL.parse Node и url 2.5.8 вне использованных случаев не сверялась.
Различия стандартных заголовков, TLS и распаковки undici/reqwest относятся к будущим потребителям
тел ответов (DEPS.md §5.5.1); этот пакет не добавляет запросы/заголовки/распаковку самостоятельно.

## Вопросы ведущему

Внутренние crates ничего не блокируют; недостающих API зависимостей нет. Требуется прогнать
сохранённые 18 transport-тестов в среде, разрешающей loopback listener. Пока они не пройдут,
условие готовности брифа по полному `cargo test` не выполнено.
