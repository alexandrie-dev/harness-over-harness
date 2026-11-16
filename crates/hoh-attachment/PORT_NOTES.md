# hoh-attachment ← packages/attachment/attachment @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/brand.ts | src/brand.rs | оба бренда перенесены |
| src/error.ts | src/error.rs | все 17 кодов, конструктор и обе функции распознавания |
| src/types.ts | src/types.rs | все типы, включая SaveFileStreamAttachment |
| src/request-projection.ts | src/request_projection.rs | обе функции геометрии перенесены |
| src/admission.ts | src/admission.rs | обе функции перенесены |
| src/index.ts | src/lib.rs | полный сервисный контракт, admission, batch validation, host paths, потоковые операции и request projection |
| tests/admission.spec.ts | tests/admission.rs | все 13 исходных случаев; ещё 2 проверки неполного/избыточного ответа поставщика |
| tests/index.spec.ts | tests/index.rs | все 10 исходных случаев; ещё 2 проверки момента отмены и отсутствия чтения входного потока |
| tests/request-projection.spec.ts | tests/request_projection.rs | все 8 случаев, включая четыре параметризованных |
| дополнительные проверки Rust-контракта | tests/contracts.rs | 5 тестов сериализации, брендов и кодов |
| README.md | — | прочитан полностью; пользовательская документация не копируется |
| package.json | Cargo.toml, публичные модули | root и ./types доступны; исходные модули доступны по snake_case |

Собственного LICENSE и locale/ в исходном пакете нет. Заимствований из стороннего проекта в исходниках и README нет.

## Отличия от образца

- Абстрактный класс AttachmentStore представлен объектно-безопасным trait с тем же именем; дополнительный ключ `Attachments` предоставляет `dyn AttachmentStore` под исходным именем `attachments`. Отдельного плагина, `name`, `inject`, `Config`, `apply` у образца нет: поставщик публикует сервис через `ctx.provide::<Attachments>(Arc::new(provider)).await?`. Не создаётся вымышленный плагин для абстрактного шва.
- Асинхронные методы используют `futures_util::future::BoxFuture<'a, Result<T, hoh_error::BoxError>>`, чтобы сервис оставался объектно-безопасным. Входы byte-array — Vec<u8>, аргументы обычных операций заимствуются. Реализация поставщика может переопределить save_images и validate_image_batch, как в образце.
- Намеренное отличие, принятое пользователем после независимого ревью: если save_images возвращает меньше ссылок, чем было закодированных изображений, admit_prompt_content возвращает HarnessError с именем AttachmentError, кодом `ATTACHMENT_WRITE_FAILED` и точным сообщением `Attachment store returned N image references for M images.`; N — число полученных ссылок, M — число закодированных изображений, без уже сохранённых ссылок. Образец `refs[next++] as ImageAttachmentRef` молча подставляет undefined; перенос возвращает Err без частичного результата. Паника прежнего expect устранена. Избыточные ссылки игнорируются, как в образце. Регрессионный тест сначала воспроизвёл панику на пустом ответе для одного изображения, затем закрепил ошибки 0/1 и 1/2; отдельный тест закрепляет игнорирование лишних ссылок.
- `SaveFileStreamAttachment<'a>` владеет `BoxStream<'a, Result<Vec<u8>, BoxError>>`, необязательным `hoh_timeout::AbortSignal` и именем. Ошибка элемента соответствует исключению AsyncIterable; lifetime допускает заимствованные источники. Поток передаётся save_file_stream по значению, без сбора чанков. read_file_stream возвращает тот же вид BoxStream. Его исходный default при первом poll проверяет сигнал и возвращает ровно один Err без чанков; следующий poll даёт None. Отмена после создания потока, но до первого чтения, сохраняет причину.
- `read_image` остаётся обязательным методом поставщика, как abstract readImage; контракт требует проверять ID, формат, длину и размеры и сохранять причину отмены. Default `read_image_request` проверяет сигнал уже при вызове, как неасинхронный метод образца. Rust не бросает синхронное исключение: выбранная при вызове ошибка возвращается через Result при ожидании future. Причина сохраняет SharedError::ptr_eq; отмена между вызовом и polling не заменяет уже выбранный ATTACHMENT_PROJECTION_UNSUPPORTED. Default save_file_stream не читает источник и не проверяет отмену, как образец, сохраняя ATTACHMENT_FILES_UNSUPPORTED даже с отменённым входом.
- Синхронные image_host_path/file_host_path возвращают Result<Option<String>, HarnessError>: None соответствует undefined, Err — исходному throw. Значение по умолчанию — Ok(None).
- AttachmentError — пространство конструктора с NAME и new, возвращающим HarnessError, по PORTING §7. Причина добавляется через with_cause; типизированные коды имеют as_str/from_code и serde с исходными строками. ImageAdmissionErrorCode преобразуется в AttachmentErrorCode через From. Различия имён ошибок не влияют на распознавание кода. Произвольных динамических полей у std::error::Error нет: чужой код передаётся через общий HarnessError, в том числе в SharedError; цепочка cause не просматривается. Все переносимые ошибки с кодом обязаны использовать HarnessError по общим правилам.
- AttachmentId/ImageVariantId — newtype через hoh_brand::branded_string!, конструктор From<&str/String> вместо одноимённой TS-функции. AttachmentIdType сохранён как type alias.
- Размеры и лимиты — u64, промежуточная геометрия — f64, как JS Number; функции геометрии принимают положительные размеры и бюджет согласно исходному контракту. Отрицательные, дробные и нечисловые размеры не входят в Rust API. Суммы байтов насыщаются при переполнении u64, недостижимом для фактических буферов.
- Вложенная пара originalDimensions использует ProjectedDimensions вместо анонимного TS-объекта. Дополнительные типы RequestImageDepth/RequestImageSpace выражают исходные литералы 'uchar'/'srgb'. Enum ImageMediaType имеет as_str/Display с исходным MIME. Отсутствующие поля JSON пропускаются; порядок полей reference, limits, uploads, prompt parts и target сохранён. Бинарные процессные payload не сериализуются в журнал.
- Требование к потребителям wire-content (ACP content adapter и file upload): `ImageMediaType` остаётся закрытым enum по типу образца. Прямой serde-разбор `image/bmp` даст общий текст unknown variant раньше вызова store и потеряет исходную машинную маршрутизацию ошибки. Wire DTO должен сохранять MIME как String до явной проверки типа и политики deployment; только принятый MIME преобразуется в ImageMediaType. Нельзя напрямую разбирать непроверенный wire-content в EncodedImageAttachment/PromptContentPart и отдавать serde-ошибку пользователю вместо исходной ошибки адаптера. Отказ store для такого MIME — код `UNSUPPORTED_IMAGE_TYPE` и текст `Image type image/bmp is not accepted by this deployment.`. Сам ACP до store проверяет `mimeType` в `packages/acp/acp/src/content.ts`, decodeImage, и использует собственный AcpContentError с кодом `invalid` и текстом `image mimeType must be image/png, image/jpeg, image/webp, or image/gif`: Rust-адаптер сохраняет именно этот исходный контракт. Изменений чужих адаптеров в данном crate нет.
- AttachmentAdmissionPart имеет отдельные Rust-варианты EncodedImage и StoredImage с одинаковым wire-tag `image`. Разбор различает payload по attachment; порядок сериализации type/attachment/encodedBytes и type/mediaType/data/name сохранён. Остальные закрытые объединения представлены enum.
- Предварительная оценка base64-байтов в смешанном prompt повторяет Buffer.byteLength(data, 'base64') до проверки каноничности: учитывает длину UTF-16 и конечные '='. Каноничное декодирование выполняется разрешённым base64; повторное кодирование должно точно совпасть со входом. Пустой файл допустим; пустое изображение отклоняется.
- Проверены прямые импорты из @deepseek-ai/dsh-attachment и /types в переносимых src/ пакетов (без client, api, typert, host, experimental), включая attachment-local, llm, llm-deepseek, llm-pi-ai, commands, acp, mcp-client, tool-fs, tool-office, subagent, sdk/server, session-log-export и ptc-runtime. Отдельных прямых импортов `AttachmentIdType`, `AttachmentErrorCode`, `ImageAdmissionErrorCode`, `AdmittedPromptContentPart`, `AttachmentAdmissionPart`, `EncodedFileAttachment`, `admitEncodedFile`, `isAttachmentError` вне владельца не найдено. Методы admit_prompt_content/admit_encoded_file/is_attachment_error могут вызываться через сервис; отсутствие импорта не означает отсутствие использования метода. Все эти экспорты сохранены.

## Не перенесено

Нет исключённых исходных файлов, экспортов или случаев тестов. Все 31 runtime-случай перенесены: 13 admission, 10 index и 8 geometry (четыре варианта it.each считаются отдельно). locale/ отсутствует. Typert, HMR и npm build-проверок в этом пакете нет.

## Не проверено

В пакете нет платформенных веток. Реального хранения и декодирования растра в образце нет: это контракт, реализация принадлежит attachment-local. Поэтому фактический backpressure, запись/проверка digest и кодирование request-image принадлежат тестам поставщика. Здесь проверены default-rejection, точные тексты/коды, идентичность причины отмены, момент проверки сигнала, завершение потока после ошибки и отсутствие polling источника при неподдерживаемой записи. Геометрия для значений вне положительного целочисленного JS-safe диапазона не проверялась. Неполный и избыточный ответы save_images проверены регрессионными тестами; wire-адаптеры должны отдельно проверить описанную выше валидацию MIME.

## Вопросы ведущему

Нет. Пользователь разрешил зависимость hoh-timeout и исправил Dependencies в brief; зависимость добавлена. Общий AbortSignal используется без собственной замены. Зависимые crates имеют все необходимые API; правок вне hoh-attachment не требуется. Незавершённой работы нет.

## Проверки

Финальные команды выполнены с `CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3`:

- `cargo build -p hoh-attachment`: exit 0.
- До исправления: `cargo test -p hoh-attachment --test admission rejects_short_image_reference_batches` — `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 13 filtered out`; паника `one stored reference per encoded image` в src/lib.rs.
- `cargo test -p hoh-attachment`: 40 passed; 0 failed; 0 ignored (15 admission + 12 index + 8 geometry + 5 contracts; unit/doc-tests 0/0). Это все 31 исходных случая и 9 дополнительных. Итоговые строки test binaries: `15 passed; 0 failed`, `5 passed; 0 failed`, `12 passed; 0 failed`, `8 passed; 0 failed`.
- `cargo clippy -p hoh-attachment --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-attachment -- --check`: exit 0.
