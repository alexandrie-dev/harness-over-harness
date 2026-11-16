# hoh-credentials ← packages/credentials/credentials @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Все публичные конструкторы/проверки ключей, views, абстрактный provider; inherited notifications заблокированы API ядра |
| src/types.ts | src/types.rs | Все типы, оба бренда, объединение записей, обе декларации событий |
| tests/credentials.spec.ts | tests/credentials.rs | Все 8/8 исходных случаев |
| tests/memory.ts | tests/support/memory.rs | Полный test-only provider, все методы обоих пространств ключей |
| — | tests/types.rs | 4 дополнительных теста грамматик, ошибок, JSON, тегов и порядка полей |
| README.md, package.json | — | Прочитаны; отдельной LICENSE и locale нет |

## Отличия от образца

- CredentialProvider — object-safe trait, Credentials — ServiceKey с точным именем credentials. Это абстрактный seam, не конкретный плагин: как и в образце, его публикует плагин-поставщик. В тестах это MemoryCredentialsPlugin; production-поставщика в пакете образца нет. Добавлять фиктивный production plugin нельзя.
- CredentialFuture и CredentialMutator — дополнительные публичные псевдонимы для boxed Send future и FnOnce. mutate получает owned snapshot, может async-решить замену; None означает отсутствие изменения, не удаление. Контракт сериализации и межпроцессного исключения принадлежит реализации provider.
- Имена Rust — snake_case, но все тексты TypeError, имена сервисов, событий и JSON поля сохранены. TypeError — дополнительный Rust enum вместо встроенного JS TypeError, со стандартными Display/Error и name().
- Оба branded_string! объявлены в src/types.rs и реэкспортированы из корня: подмодуль types сохраняет исходную отдельную точку входа ./types. Грамматики реализованы проверкой ASCII байтов (без расширения диапазонов на Unicode).
- CredentialRecord — закрытый enum; ручной serde делегирует сериализацию ApiKeyRecord/GrantRecord и десериализует через закрытый #[serde(tag = "kind")] helper. Каждый standalone record также сериализуется с kind первым; нет дублированного поля kind. CredentialRecordKind — дополнительное имя исходного типа CredentialRecord['kind']; kind() читает тег. payload — opaque Value, env — IndexMap, None-поля опускаются. Результаты views имеют исходный порядок configured/source/writable/masked и configured/kind/writable.
- CredentialsReferenceUpdated/CredentialsRecordUpdated — дополнительные имена Rust структур для исходных событий, режим Emit. Поле reference представляет исходный параметр ref (ключевое слово Rust); payload события внутри процесса не является протокольным объектом.
- Low-level branded_string! разрешает unchecked From<String>, как общий brandString в образце. Гарантии credential_key_scope/id относятся к ключам из validating constructors. На некорректном unchecked ASCII ключе без slash сохранены slice(0,-1)/slice(0); для невалидного Unicode-бренда без slash scope удаляет последний Unicode scalar, а не UTF-16 code unit: Rust String не представляет непарные суррогаты. Валидные ключи всегда ASCII.
- Test-only MemoryCredentials сохраняет исходные Map ordering, синхронные эффекты set/unset, отсутствие notify helpers (он и в образце напрямую вызывает ctx.emit). Короткие std::sync::Mutex не удерживаются через await; modify_record сохраняет исходную последовательность snapshot → await mutate → replacement, не добавляя отсутствующий в test helper замок между await. Это не production provider.
- До реализации просмотрены импорты разрешённых packages/src: credentials-local, authorization, deepseek-account, llm-pi-ai, llm-deepseek, llm-deepseek-api-key, mcp-client, web-deepseek-api и web-search-deepseek. Все публичные экспорты используются, кроме GrantRecord как самостоятельного импортируемого имени: потребители читают grant через CredentialRecord (в сгенерированном api-catalog имя присутствует, он не считается потребителем).

## Не перенесено

- Из credentials.spec.ts не пропущено ничего: 8/8; дополнительно tests/types.rs — 4 случая.
- **Не завершено:** protected notifyUpdated/notifyRecordUpdated и private fanOut/warnListenerFailure из src/index.ts:255–304. Ядро предоставляет только Context::emit, который прекращает обход при панике и не принимает async возвращаемые значения. Его приватный dispatch недоступен. Catch вокруг emit не исполняет оставшихся слушателей; смена режима на Parallel меняет контракт события и момент завершения. Свой параллельный реестр или копирование ядра запрещены границами задания и ломают общий ctx.on. Поэтому нет ложной реализации/пустой заглушки этих helpers; см. вопросы.
- Обязательные будущие warning строки исходника: `credentials: a %s listener for "%s" failed` (event/subject подставляются) и отдельная запись самого error. Сохранить их после появления нужного API.
- TS declaration merging заменён типизированными событиями, абстрактный Service — trait/service key по docs/kernel.md. Других исключений нет.

## Не проверено

- Contained notification fan-out требует расширения ядра; sync throw/async rejection и продолжение последующих слушателей не объявляются работающими.
- Сценарии durable storage, locks между процессами и ambient fallback принадлежат concrete providers и здесь не реализуются. Платформенных веток нет.

## Вопросы ведущему

1. Нужен публичный contained Emit/dispatch API hoh-kernel: snapshot слушателей, respect filter/alive/disposal/order; синхронное исполнение каждого отдельно с продолжением после паники/ошибки, наблюдение async ошибок без ожидания commit, логирование каждой ошибки. Он должен использовать общий реестр ctx.on. Простого catch_unwind вокруг Context::emit недостаточно. После этого можно реализовать inherited notify_updated/notify_record_updated без изменения режима и поведения событий. Чужой kernel не менялся.
2. Нужен согласованный Rust контракт async listener для Emit: текущая сигнатура on Fn(&E) -> () принципиально не возвращает future, в отличие от допускаемого Cordis Promise-like возврата.

## Проверки

Все Cargo-команды: CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4.

- cargo test -p hoh-credentials: credentials.rs — test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. types.rs — test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. Всего 12 passed; 0 failed; lib/doc-tests 0/0.
- cargo clippy -p hoh-credentials --all-targets -- -D warnings: exit 0.
- cargo fmt -p hoh-credentials -- --check: exit 0.
