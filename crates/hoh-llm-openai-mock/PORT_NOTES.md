# hoh-llm-openai-mock — заметки

Нового исходника в образце нет (в `docs/porting/MAP.tsv` не значится): тестовый сервер по брифу `plans/briefs/window-glue.md`, пункт «б». Клиент, под который он написан, — `crates/hoh-llm-openai` (`probe.rs`, `adapter.rs`).

- Запрос без `stream:true` — проба (`pong`), сценарий не двигает; каждый потоковый запрос берёт следующий ответ, после конца — HTTP 409.
- `arguments` вызова: JSON-значение сериализуется в строку; **строка уходит как есть** (так сценарий может задать неверные аргументы).
- Журнал: одна строка JSONL = один `write_all`; при отказе записи оборванный хвост отрезается (`set_len`) и позиция записи возвращается к концу (`seek`), иначе следующая строка легла бы после нулевых байтов. Заголовки не пишутся; ключ в `Debug` скрыт.
- Предел размера запроса — 4 МиБ (`MAX_REQUEST_BYTES`), больше — HTTP 400.
- `accept`: временные ошибки (EMFILE/ENFILE/ENOBUFS/ENOMEM, ConnectionAborted/Reset, Interrupted, WouldBlock, TimedOut) пропускаются с паузой 10 мс; остальные заканчивают слушатель.
- Бинарник: `println!` строки `{"type":"ready",…}` — протокол бинарника, не журнал; обработчики SIGINT/SIGTERM ставятся до неё (коды 130/143).
- `RequestRecord.sequence: usize` — снаружи (JSON) не отличается от `u64`.

## Не проверено
- Номера errno в `accept_error_is_fatal` — Linux (на macOS ENOBUFS = 55, на Windows коды другие); проверено только на Linux.
- `an_ipv6_host_is_advertised_in_brackets` требует `::1` на машине (в контейнере без IPv6 упадёт).
- Частичная запись журнала на настоящем диске при ENOSPC: откат проверен на временном файле с ручным обрывом строки, самого переполнения диска не было.
- Гонка SIGTERM сразу после старта (`tests/bin.rs`) надёжно тестом не ловится.
- `tests/scenario_files.rs`: каждый вызов инструмента в `scenarios/*.json` сверяется с обязательными полями схем профиля окна (bash → command, description; read → file_path; write → file_path, content; sandbox_permissions → justification). Новый инструмент в сценарии без записи в таблице теста роняет тест — сначала добавить его схему.
- Пауза между частями текста: `MockOptions.part_delays_ms` (индекс ответа → мс перед каждой частью) и флаг бинарника `--part-delay ИНДЕКС=МС` (повторяемый). Не поле `ScenarioResponse`: структуры сценария строятся литералами в `apps/hoh/tests/window_*.rs`, новое поле их бы сломало. Паузы берёт `Sleeper` (по умолчанию настоящий; `Sleeper::Recorded` пишет `(индекс, мс)` без ожидания — для тестов без привязки ко времени). Тесты: `tests/server.rs::a_part_delay_is_slept_before_every_part_of_that_response_only`, `src/bin.rs::parses_part_delays_and_rejects_malformed_ones`.
