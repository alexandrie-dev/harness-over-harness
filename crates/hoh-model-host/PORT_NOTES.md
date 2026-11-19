# hoh-model-host (без образца в DSH: склейка окна-чата)

Образца в `refs/deepseek-harness` у крейта нет: он собирает для окна пять методов `hoh/*` и поставщика модели
из готовых пакетов (`hoh-llm`, `hoh-llm-openai`, `hoh-credentials`). Эталон поведения — заглушка окна
`crates/hoh-window-stub` (`engine.rs`, `credentials.rs`), UI `ui/src/screens/firstrun`, `docs/window.md` §4,
типы `hoh-protocol`.

## Файлы

| что | где |
|---|---|
| плагин `ModelHostPlugin` (`model-host`), `ModelHost`, `ModelHostConfig`, службы `ModelHostKey`, адаптер `HostedOpenAiAdapter` | `src/lib.rs` |
| константы: `MODEL_API_KEY_REF` (ссылка ключа в credentials), `DEFAULT_PROVIDER`, `DEFAULT_BASE_URL`, `DEFAULT_MODEL` (значения конфига по умолчанию), `METHOD_PROBE`, `METHOD_KEY_SET`, `METHOD_SETTINGS_GET`, `METHOD_SETTINGS_SET`, `METHOD_RULES_SET` (имена методов) | `src/lib.rs` |
| тесты на локальном HTTP-сервере внутри теста | `tests/model_host.rs`, `tests/support/mod.rs` |

Методы регистрируются в таблице `HohRpcMethods` (`hoh-sdk-jsonrpc-server`): `hoh/model/probe`,
`hoh/model/key/set`, `hoh/settings/get`, `hoh/settings/set`, `hoh/settings/rules/set`. Подтверждение хоста
(`requires_host_approval`) выдаёт сервер до вызова обработчика; здесь его не проверяют. Привилегированны (всегда
или при заданных полях, `PRIVILEGED_METHODS`): `key/set`, `rules/set`, `probe` с `baseUrl`, `settings/set` с `model` или `security`.

## Поведение

- Ключ только в `hoh-credentials` (`model_api_key`), читается на каждый вызов; в конфиге, `Debug` и ошибках его нет.
  Текст ошибки параметров не содержит значений (serde их цитирует).
- Адаптер `OpenAiAdapter` (и его `reqwest::Client`) кэшируется и пересоздаётся только при смене адреса или ключа:
  иначе теряется выученное написание поля лимита (`max_tokens`/`max_completion_tokens`) и соединения.
- `probe` с пустым `model` (так делает первый запуск окна): `GET /models` один раз, completion на первую модель из
  списка, при недоступном списке — на модель из настроек (`hoh_llm_openai::probe_first_listed`). Проба привязана
  к `ctx.token()`: выгрузка плагина прерывает её и закрывает соединения.
- `hoh/settings/rules/set {internet:"ask"|"public", folder}` пишет `security.internet` и `security.folders=[folder]`;
  `settings/get` их отдаёт. Папка сохраняется как есть (пробелы по краям — часть пути); отказ: пустая или из пробелов,
  больше 4096 байт, с NUL. Для сохранения между запусками — `ModelHost::rules()` (`None`, пока первый запуск не
  выбрал) и `ModelHost::set_rules(..)`; владелец композиции сохраняет `rules()` рядом с настройками модели и
  повторяет `set_rules` при старте.
- Имя модели на проводе — сохранённое (`settings.model.model`), а не имя из маршрута сессии: окно объявляет в `initialize`
  заглушку (`local`), `HostedOpenAiAdapter::prepare_call` подменяет `options.model` в `dispatch` (тест
  `saved_model_name_goes_on_the_wire_instead_of_the_session_placeholder`).
- Подписка на изменения: `ModelHost::subscribe() -> watch::Receiver<PersistedSettings>` (`model` и `rules`); текущее значение
  доступно сразу, `changed()` срабатывает после реального изменения `settings/set` или `rules/set` (повтор тех же значений,
  отказ и смена ключа — не изменения). Нужна `apps/hoh`, чтобы писать `model-settings.json` и правила по событию.
- Вызов, подготовленный (`prepare_call`) для одного адреса, уходит на этот адрес с именем модели из того же снимка настроек;
  `settings/set` между подготовкой и отправкой шага его не смешивает, следующий шаг берёт новые настройки
  (тест `call_prepared_for_one_address_keeps_that_addresss_model_name_when_settings_change`).
- После успешного сохранения ключа выгрузка плагина ошибку не возвращает.

## Отличия от заглушки окна

- `settings/set` с `security` или `view` отвергается (заглушка их применяет); `settings/get` отдаёт для `view` и для
  `security` кроме `internet`/`folders` значения по умолчанию. **GLUE-SHIM(settings)**: `hoh-settings` пуст; при
  стыковке заменить `default_settings` агрегатором настроек (метка стоит в `src/lib.rs`).
- `settings/set {}` возвращает текущие настройки (как заглушка); заглушка принимает и пустое имя модели, здесь — отказ.
- Ключ из одних пробелов отвергается (`model credential must contain 1..8192 bytes`); заглушка принимает любую
  непустую строку и пустую отвергает только по байтам. Недопустимые для заголовка символы — отдельная ошибка без цитаты.
- `key/set` с неверным `baseUrl` отвергается после подтверждения; заглушка ключ сохраняет.
- `rules/set`: заглушка принимает любую строку папки, включая пустую; здесь пустая, из пробелов, длиннее 4096 байт
  или с NUL — отказ, состояние не меняется (тесты `invalid_rules_change_nothing`, границы 4096/4097).
- `probe` ходит по настоящей сети (список моделей и один токен completion с инструментом), заглушка лишь проверяет,
  что ключ сохранён. Недоступный сервер — `ok:false`, не ошибка вызова.
- Настройки модели и правила живут в памяти и стартуют из конфига плагина (`baseUrl`, `model`); между запусками
  модель (`model-settings.json`) и правила (`window-rules.json`) сохраняет `WindowModelSettings` в `apps/hoh`: восстановление
  через `set_settings`/`set_rules`, запись по уведомлениям `ModelHost::subscribe`.

## Правка вне крейта

`hoh-llm-openai`: добавлена `probe_first_listed` (общая внутренняя `probe_choosing`; `probe` без изменений поведения)
и два теста в `tests/probe.rs` — иначе пустое имя модели давало два запроса `GET /models`.

## Не проверено

- Сквозной путь через stdio и окно (первый запуск: probe, ключ, правила) — за сквозным тестом `apps/hoh`.
- Тест выгрузки (`unloading_the_plugin_aborts_a_probe_in_flight`) ждёт по настоящим часам (не более 3–5 с).
- Реальные серверы (vLLM, llama.cpp, облако) не пробовались, только локальный mock в тестах.
- Тесты «закрытого порта» (`probe_of_a_dead_address_is_a_typed_failure`, а также `connection_refused…` в `hoh-llm-openai`)
  занимают порт `127.0.0.1:0` и сразу отпускают его: теоретически порт может занять другой тест. Решение: оставлено как
  есть (нестабильность только тестов, в продукт не попадает; надёжной замены без сторонних зависимостей нет).

## Заметки ревью 154053: решения

- README `hoh-llm-openai` — обновлён (`probe_first_listed`, `modelListed`).
- Порты в тестах — оставлено, см. «Не проверено».
- Комментарий о привилегированных методах — исправлен (`settings/set` привилегирован и с `security`).
