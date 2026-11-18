# hoh-credentials ← packages/credentials/credentials @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Все конструкторы/проверки, views, абстрактный provider, inherited notify_updated/notify_record_updated, private fan_out/warn_listener_failure |
| src/types.ts | src/types.rs | Все типы, оба бренда, объединение записей, обе декларации событий |
| tests/credentials.spec.ts | tests/credentials.rs | Все 8/8 исходных случаев |
| ../credentials-local/tests/review-fixes.spec.ts:112–136, блок contained update fan-out | tests/credentials.rs | Оба случая перенесены для seam-only provider, без файлового backend; имена в snake_case сохранены |
| tests/memory.ts | tests/support/memory.rs | Полный test-only provider, все методы обоих пространств ключей |
| — | tests/credentials.rs | Ещё 4 проверки: record failures/commit semantics и доставка обоих событий обычному root-listener при фильтре поставщика, отклоняющем всех |
| — | tests/types.rs | 5 дополнительных тестов грамматик, ошибок, JSON, тегов и порядка полей; TypeError остаётся обычной ошибкой за BoxError |
| README.md, package.json | — | Прочитаны; отдельной LICENSE и locale нет |

## Отличия от образца

- CredentialProvider — object-safe trait, Credentials — ServiceKey с точным именем credentials. Это абстрактный seam, не конкретный плагин: как и в образце, его публикует плагин-поставщик. В тестах это MemoryCredentialsPlugin; production-поставщика в пакете образца нет. Добавлять фиктивный production plugin нельзя.
- CredentialFuture и CredentialMutator — дополнительные публичные псевдонимы для boxed Send future и FnOnce. CredentialFuture использует общий hoh_error::BoxError (это тот же тип, что hoh_kernel::Error). mutate получает owned snapshot, может async-решить замену; None означает отсутствие изменения, не удаление. Контракт сериализации и межпроцессного исключения принадлежит реализации provider.
- Имена Rust — snake_case, но все тексты TypeError, имена сервисов, событий и JSON поля сохранены. TypeError — дополнительный Rust enum вместо встроенного JS TypeError, со стандартными Display/Error и name().
- Оба branded_string! объявлены в src/types.rs и реэкспортированы из корня: подмодуль types сохраняет исходную отдельную точку входа ./types. Грамматики реализованы проверкой ASCII байтов (без расширения диапазонов на Unicode).
- CredentialRecord — закрытый enum; ручной serde делегирует сериализацию ApiKeyRecord/GrantRecord и десериализует через закрытый #[serde(tag = "kind")] helper. Каждый standalone record также сериализуется с kind первым; нет дублированного поля kind. CredentialRecordKind — дополнительное имя исходного типа CredentialRecord['kind']; kind() читает тег. payload — opaque Value, env — IndexMap, None-поля опускаются. Результаты views имеют исходный порядок configured/source/writable/masked и configured/kind/writable.
- CredentialsReferenceUpdated/CredentialsRecordUpdated — дополнительные имена Rust структур для исходных событий, режим **Contained**. Имена остаются `credentials/reference-updated` и `credentials/record-updated`; on_dispatch по-прежнему видит Emit согласно kernel R63. Поле reference представляет исходный параметр ref (ключевое слово Rust); payload события внутри процесса не является протокольным объектом.
- Защищённые методы исходного класса стали object-safe default-методами trait: `notify_updated(&self, ctx: &Context, reference: &CredentialRef)` и `notify_record_updated(&self, ctx: &Context, key: &CredentialKey)`. В Rust нет protected-наследования; Context передаётся явно вместо service Proxy, по docs/kernel.md §7. Поставщик вызывает helper со своим контекстом только после commit. **Отличие имени журнала:** в source `this.ctx.logger` внутри метода сервиса проходит через caller's service proxy (`vendor/cordis/src/utils.ts:153–165`), поэтому warning несёт имя logger вызывающего потребителя; порт берёт logger переданного контекста поставщика и пишет под именем поставщика. Это зафиксированная адаптация explicit-context API, без изменения кода логирования по решению ведущего.
- Private fan_out использует общий реестр kernel через **`ctx.root().dispatch(event).invoke(report)`**. Source `src/index.ts:283–285` передаёт `[event, subject]`, где первый элемент — строка; `vendor/cordis/src/events.ts:162–172` в этом случае не выделяет thisArg и не применяет receiver filter. Поэтому уведомления доставляются всем живым слушателям события, независимо от фильтра контекста поставщика. Context::root создаёт контекст без фильтра, и существующего API достаточно; kernel не менялся. Logger берётся из исходного ctx поставщика до перехода к root (см. отличие имени журнала выше). Синхронный Err (Threw) регистрируется до следующего callback; async Err (Rejected) наблюдается без ожидания операции. Оба вида вызывают одну warn_listener_failure, как в src/index.ts:283–304; собственной таблицы слушателей и собственных задач нет. Снимок/order/alive и отмена async-работы с fiber слушателя — kernel R63–R65. Отдельные уведомления/специальные тексты для Threw/Rejected не добавлялись.
- **Отличие развёртки diagnostic failures:** source `src/index.ts:301–304` вызывает warn дважды, но это **не означает две записи журнала на любую ошибку**. В `vendor/cordis/src/logger.ts:136–146` cause логируется сначала, затем сама ошибка; aggregate без cause с непустым errors разворачивается по одной записи на каждый member (с рекурсивной обработкой причин/вложенных aggregate). Поэтому число source-записей зависит от ошибки и может быть больше двух. Порт сохраняет отдельную строку `credentials: a <event> listener for "<subject>" failed`, а второй вызов пишет Display ошибки и её цепочку source через `: ` **одной форматированной записью**, без разложения aggregate на записи по членам. Для простых ошибок fixtures это две записи; общее число записей source не заявляется эквивалентным. Это согласованное отличие диагностического потока (docs/kernel.md §7), не сравниваемого побайтно; изменений кода для cause/aggregate не требуется. Префикс source `src/index.ts:302` после подстановки `%s` сохранён до символа, ref/key не подменяются секретными значениями; JS util.inspect/идентичность args не воспроизводятся. Ошибки слушателей не меняют результат commit и не препятствуют успешному слушателю.
- Low-level branded_string! разрешает unchecked From<String>, как общий brandString в образце. Гарантии credential_key_scope/id относятся к ключам из validating constructors. На некорректном unchecked ASCII ключе без slash сохранены slice(0,-1)/slice(0); для невалидного Unicode-бренда без slash scope удаляет последний Unicode scalar, а не UTF-16 code unit: Rust String не представляет непарные суррогаты. Валидные ключи всегда ASCII.
- Test-only MemoryCredentials сохраняет исходные Map ordering и синхронные изменения set/unset. Вместо прямого ctx.emit исходного test helper он вызывает default notify helpers: типизированный Contained не допускает emit/on, и этот provider дополнительно служит fixture проверки inherited containment без credentials-local. Это осознанная адаптация тестового поставщика; обычные 8 исходных случаев сохранены. Короткие std::sync::Mutex не удерживаются через await; modify_record сохраняет последовательность snapshot → await mutate → replacement, не добавляя отсутствующий в test helper замок между await. Это не production provider.
- До реализации просмотрены импорты разрешённых packages/src: credentials-local, authorization, deepseek-account, llm-pi-ai, llm-deepseek, llm-deepseek-api-key, mcp-client, web-deepseek-api и web-search-deepseek. Все публичные экспорты используются, кроме GrantRecord как самостоятельного импортируемого имени: потребители читают grant через CredentialRecord (в сгенерированном api-catalog имя присутствует, он не считается потребителем).

## Не перенесено

- Нет незавершённого production-кода или исключённых runtime-случаев: исходные credentials.spec.ts 8/8 и дополнительно два исходных contained fan-out случая из credentials-local/tests/review-fixes.spec.ts; пропущено 0. Остальные тесты credentials-local (файл, locks, watcher) относятся к другому crate и в эту передачу не входят.
- TS declaration merging заменён типизированными событиями, абстрактный Service — trait/service key по docs/kernel.md. Других исключений нет.

## Проверка обновлённых правил переноса

- PORTING.md §4, новый пункт о Node/JavaScript: production-код этого пакета не использует trim/\s, path, readdir, fs или запуск процессов. Для него нет применимых вызовов общих js_trim/node_path/node_fs_error_message; прямых Rust-подмен таких операций не найдено. ASCII-грамматики соответствуют исходным диапазонам без Unicode-расширений. Типы JSON и insertion order сохранены; библиотека не пишет JSON-текст наружу, поэтому serde используется только для значений, а не вместо JSON.stringify.
- PORTING.md §7, базовый класс ошибки: source бросает встроенный TypeError, а test helper — обычный Error для пустого значения. Они не наследуют HarnessError и не превращаются в него. Собственный enum TypeError сохранён; добавлена проверка, что все три его варианта за BoxError узнаются как TypeError и **не** HarnessError. Arbitrary failures методов и слушателей используют общий BoxError. Других собственных классов ошибок, требующих переклассификации, в пакете нет.

## Не проверено

- Проверки notification fan-out не используют wall-clock ожиданий: paused Tokio, gate и уведомление logger exporter доказывают завершение commit до async rejection, продолжение слушателей и две записи для простых ошибок fixtures. Различие развёртки cause/aggregate описано выше и не считается побайтно сравниваемым выводом. Durable/file assertions credentials-local заменены проверкой уже committed memory-state — здесь проверяется абстрактная capability, не хранение на диске.
- Сценарии durable storage, locks между процессами и ambient fallback принадлежат concrete providers и здесь не реализуются. Платформенных веток нет.

## Вопросы ведущему

Нет. Оба прежних вопроса закрыты добавленным API hoh-kernel: Contained/on_contained/on_contained_async и dispatch(event).invoke(report) с ListenerFailure::Threw/Rejected. Свои обходные механизмы не понадобились; чужие crates не менялись.

## Проверки

Все Cargo-команды при передаче writer w5: CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5.

- Независимое review, воспроизведение до изменения production-кода: `cargo test -p hoh-credentials` — credentials.rs `test result: FAILED. 12 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out`. Оба новых теста (`reference_notifications_ignore_the_provider_context_filter`, `record_notifications_ignore_the_provider_context_filter`) проверили успешный commit, затем получили пустой список вместо ref/key обычного root-listener. Поставщик смонтирован под `with_filter(|_| false)`; слушатели не global. После подтверждения красного результата dispatch заменён на ctx.root().dispatch, без изменения kernel.
- После исправления `cargo test -p hoh-credentials`: credentials.rs — `test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`; types.rs — `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`. Всего 19 passed; 0 failed; lib/doc-tests 0/0.
- cargo clippy -p hoh-credentials --all-targets -- -D warnings: exit 0.
- cargo fmt -p hoh-credentials -- --check: exit 0.
