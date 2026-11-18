# hoh-attachment-local ← packages/attachment/attachment-local @ 39163dd3

## Файлы

| Образец | Перенос | Состояние |
|---|---|---|
| src/index.ts | src/lib.rs | LocalAttachmentStore, Config, все константы, сервис Attachments, пакетная подготовка до публикации, общий лимит сжатия, singleflight и отмена отдельных ожидающих |
| src/store.ts | src/store.rs, src/util.rs | Подготовка и проверка ссылок, staging wx/0600, SHA-256, fsync цепочки каталогов, эксклюзивный hard link, проверка EEXIST, chmod 0400, чтение с проверкой digest и заголовка |
| src/file-store.ts | src/file_store.rs | Имена, канонический inode и именованные hard links, запись потоком без агрегирования, чтение чанками до 64 KiB с итоговой проверкой |
| src/image.ts | src/image.rs | Полное декодирование при приёме, header-only probe при чтении, лимиты до декодирования пикселей, EXIF, анимация и метаданные четырёх форматов |
| src/normalization.ts | src/normalization.rs | Условия byte-identical passthrough, геометрия, нормализация, проверка результата и исходные тексты отказов |
| src/encoding.ts | src/encoding.rs | Ленивая лестница 85/75/60, первый подходящий кандидат, иначе наименьший; opaque → JPEG, alpha → WebP, effort 0 |
| src/compression-limiter.ts | src/compression_limiter.rs | FIFO-семафор экземпляра и compression_failure |
| src/request-image.ts | src/request_image.rs | request-image-v6, исходный порядок descriptor JSON, SHA-256 variantId, размеры без увеличения, чтение/восстановление кеша и атомарная замена |
| src/sharp.ts | Прямые зависимости image/webp/moxcms/kamadak-exif | Lazy require исключён по MAP.tsv и решению DEPS.md §5.5 №7 |
| tests/index.spec.ts | tests/index.rs | 7 групп / 8 параметризованных случаев |
| tests/store.spec.ts | tests/store.rs | 15 групп / 18 случаев, включая отдельный cfg(windows) |
| tests/file-store.spec.ts | tests/file_store.rs | 18 случаев |
| tests/image.spec.ts | tests/image.rs | 7 случаев; дополнительно ресемплирование с premultiplied alpha |
| tests/normalization.spec.ts | tests/normalization.rs | 19 групп / 21 случай; дополнительно Gray/GrayAlpha ICC |
| tests/encoding.spec.ts | tests/encoding.rs | 6 случаев, в том числе FIFO и освобождение слота после ошибки |
| tests/request-image.spec.ts | tests/request_image.rs | 15 групп / 16 случаев; дополнительно причина отмены последнего из двух ожидающих |
| tests/normalization-verification.spec.ts | tests/normalization_verification.rs | 1 случай |
| tests/request-image-verification.spec.ts | tests/request_image_verification.rs | 1 случай |
| Дополнительные CMYK-проверки | tests/cmyk.rs | 3 теста: embedded ICC, исходный fallback без ICC, YCCK; каждый проверяет пиксели до и после JPEG-кодирования |
| Встроенный профиль libvips cmyk | assets/cmyk-input.icc.base64, assets/CMYK_PROFILE_LICENSE.txt, assets/README.md | Public-domain Chemical proof; сохранены исходные forward LUT, записаны происхождение, SHA-256 и точный текст лицензии |
| tests/lazy-sharp-failure.spec.ts | Не применим | 2 группы / 3 случая загрузки динамического native binding; причина ниже |
| locale/{de,en,es,fr,ja,pt,ru,zh}.json | locale/ | Все восемь файлов скопированы без изменений; diff -rq завершился с кодом 0 |
| README.md, package.json | PORT_NOTES.md, Cargo.toml | Прочитаны; дополнительные src/* entry points представлены публичными модулями |

Проверены все девять исходных файлов src и все десять файлов tests. Всего в образце
91 it-группа / 99 развёрнутых runtime-случаев. Перенесены 89 групп / 96 случаев;
параметры it.each исполняются в циклах внутри тестов. На Linux один Windows-случай
не компилируется, как исходный skipIf; это не исключение из переноса. Файлы tests/*.rs
подключены как unit-модули через #[path], autotests=false: так исходные vi.mock/spyOn
проверяют закрытые verify-функции и test-only hooks без новых публичных точек входа.
Общие фикстуры находятся в tests/support.rs; реальные пользовательские каталоги не используются.

Поиск импортов в packages/ и apps/ (без client/api/typert/host/experimental/web/desktop)
не нашёл прямых импортов из этого пакета в src других переносимых пакетов. Плагин
подключается композицией; LocalAttachmentStore используется в тестах tool-fs,
tool-office, spill-policy, llm-deepseek и mcp-client, readImageFile — в e2e skill-office.
Именованные функции нормализации/подготовки/кеша, типы и DEFAULT_*/MAX_* не имеют
прямых src-потребителей снаружи владельца; все они всё равно перенесены.

## Отличия от образца

- LocalAttachmentStore реализует hoh_kernel::Plugin с NAME=LocalAttachmentStore,
  Config и Deps=(); apply предоставляет исходный сервис attachments через ключ Attachments
  из hoh-attachment. Наследование заменено реализацией AttachmentStore. Все методы
  файлов, потоков и модельных проекций переопределены; re-export исходной точки входа сохранён.
- Все операции декодирования, изменения размера, ICC и кодирования выполняются
  в tokio::task::spawn_blocking. Подготовки и общие request-преобразования принадлежат
  Context::spawn. Ошибка одного участника пакета не бросает оставшиеся native-подготовки
  и не освобождает их слоты преждевременно; результаты коммитятся строго во входном порядке.
  Явные AbortSignal/AbortController и идентичность SharedError общие с hoh-timeout.
  Регистрация Wake передаёт отмену последнего ожидающего синхронно, до polling его future.
- Нет глобального mutable durableHomes: запрет PORTING.md §6. Свободные функции
  повторно доказывают долговечность home до корня ФС при каждой публикации/alias.
  После первого обращения возможны дополнительные fsync/chmod; порядок staging,
  удаления временного имени до read-only, публикации и возврата ссылки сохранён.
- Пути собираются node_path::{join,dirname,resolve}; JavaScript trim — js_trim.
  Системные причины ошибок оформляются node_fs_error_message, коды и верхние тексты
  AttachmentError сохранены. InvalidCompressionConcurrency и EmptyEncodingCandidates
  — обычные thiserror-ошибки, а не HarnessError, как их базовый Error в образце.
- Дополнительные публичные Rust-типы: DecodedImageLimits для анонимных options,
  EncodingAttempt и EncodingResult::{Fitting,Exhausted} с into_chosen для функций
  и union T | ExhaustedEncoding<T>, CompressionRejection::{Error,Value} и
  NonErrorCompressionFailure для динамического reject, InvalidCompressionConcurrency,
  EmptyEncodingCandidates для исходных простых Error. NormalizedImage — alias
  EncodedImage; его media_type использует общий ImageMediaType. encoding_ladder
  принимает подготовленный DynamicImage вместо Sharp pipeline. Байты — Vec<u8>,
  размеры — u64, ref — поле r#ref. Публикуемые протокольные типы импортированы из hoh-attachment.
- Имена изображений ограничены 255 единицами UTF-16 без разрыва Unicode scalar
  (PORTING.md §4); JS slice мог оставить одиночный surrogate. Имена файлов ограничены
  255 байтами UTF-8, как Buffer/исходный utf8Prefix.
- Типизированный ImageRequestTarget общего crate не представляет 1.5, отрицательные
  значения и NaN. Тест исходного height=1.5 проверяет отказ serde на этой границе,
  а height=0 — исходный текст INVALID_ATTACHMENT_REF. Ноль и значения выше JS safe
  integer проверяются внутри read_request_image_file. Wire-потребитель обязан сначала
  проверять числовые поля, если ему нужен исходный текст ошибки вместо serde-текста.
- ImageAttachmentRef возвращается типом hoh-attachment. Его derive Serialize задаёт
  attachmentId/mediaType/bytes/width/height, тогда как prepareImageFile образца
  создаёт attachmentId/mediaType/width/height/bytes. Ведущий закрыл этот вопрос:
  журналы dsh содержат оба порядка (50 ссылок с bytes перед width и 97 с dimensions
  перед bytes), поскольку JavaScript сохраняет порядок построения каждого producer.
  Сценарии сравнивают распарсенные JSON-значения, порядок ключей не сравнивается
  (crates/hoh-conformance/README.md §6.2). Общий тип и сериализаторы не менялись;
  это не незавершённая работа local-store. Порядок descriptor для variantId сохранён.
- AbortSignal общего crate представляет только SharedError. Поэтому исходный случай
  controller.abort('cancelled') представлен уже обёрнутой обычной ошибкой с тем же
  message и cause='cancelled'; проверяются сохранение причины и замена отменённого
  singleflight. Само преобразование произвольной причины в Error — ответственность
  адаптера нетипизированной границы; ложного теста на поддержку raw String в сигнале нет.
- Фикстуры генерируются image в blocking worker вместо sharp, размеры и проверки
  сохранены. Antialiased screenshot использует детерминированный растр глифов вместо
  SVG/font renderer; остаются JPEG, 512×256 и исходные границы min<80/max>240.
  Unsupported TIFF представлен заголовком неподдерживаемого контейнера. Два теста
  несовпадения encoder/decoded metadata вызывают те же закрытые verify-функции
  с несовпадающим width вместо глобальной подмены модуля. FS traces task-local,
  read-hook принадлежит экземпляру и существует только при cfg(test). Конфликт
  DSH_HOME проверяется внедрённой картой окружения общего hoh-home-paths, без set_var.
  Дополнительные CMYK-фикстуры записывают baseline JPEG из точных DC-блоков 8×8
  с APP14 и необязательным APP2 ICC: image не предоставляет CMYK JPEG encoder.
  Два MCU позволяют проверить отдельные каналы и состояние DC predictor.

### Отличия растра и кодированных байтов от libvips

Решение ведущего DEPS.md §5.5 №7 разрешает другое перекодирование. Условия выбора
passthrough и формата, лестница качеств, размеры и лимиты этим разрешением не заменены:
они реализованы непосредственно по исходному коду. Локальные причины записаны также
в EXPECTED.tsv; это не разрешение игнорировать любые различия всех изображений.

1. **Passthrough:** чистые single-frame uchar sRGB PNG/JPEG/WebP внутри трёх лимитов
   хранятся byte-for-byte, включая допустимые контейнерные поля. Никакого повторного
   PNG/JPEG/WebP кодирования нет; SHA-256 attachmentId совпадает с образцом. Запрос,
   помещающийся по размерам и байтам, также возвращает исходные байты и не пишет кеш.
2. **JPEG в normalization и request-image:** image::JpegEncoder использует собственные
   DCT, квантование, субдискретизацию и контейнер вместо кодировщика libvips/libjpeg.
   Поэтому меняются сжатые байты, их длина и декодированные пиксели; вслед за ними
   attachmentId нормализованного объекта. При бюджете между размерами двух движков
   может отличаться выбранная ступень качества, хотя порядок 85→75→60 и правило
   выбора первого fitting / smallest exhausted те же.
3. **WebP в обоих путях:** webp::Encoder вызывает libwebp с quality 85/75/60,
   method=0. Версия libwebp и подготовка пикселей отличаются от сборки sharp;
   точные байты, длина и пограничная ступень качества не гарантированы. Alpha
   сохраняется; полностью opaque alpha-плоскость может исчезнуть, что разрешает
   и encodedAlphaIsCompatible образца. GIF превращается в первый неподвижный кадр.
4. **Размер и декодирование:** image Lanczos3 заменяет libvips Lanczos3 с его
   предварительным shrink-on-load и другим округлением отсчётов. JPEG декодирует
   zune через image, GIF/PNG/WebP — соответствующие image-бэкенды; при декодировании
   с потерями и сборке первого кадра возможны другие значения пикселей. Геометрия
   выбирается исходными request_image_dimensions/long_edge_dimensions, inside без
   увеличения; EXIF применён перед нормализацией, perceived axes учитываются в лимитах.
   Как sharp, перенос premultiply/unpremultiply alpha вокруг ресемплирования;
   float-округление image отличается от libvips, но скрытые цвета прозрачных
   пикселей не смешиваются с цветами видимых границ.
5. **Цвет и глубина:** RGB/Gray/CMYK ICC преобразуются moxcms в sRGB вместо цветового
   движка libvips. Perceptual intent тот же; интерполяция и округление не побайтные;
   промежуточные каналы приводятся image к 8 битам, включая 16-битный PNG, прежде
   чем выполняется 8-битный ICC transform. Без ICC image разворачивает grayscale
   и переводит 16-bit в uchar RGB/RGBA. Метаданные, EXIF, ICC/XMP/IPTC/Photoshop,
   комментарии и orientation делают вход нечистым, а кодировщики их не сохраняют.
6. **CMYK:** разрешённые ведущим zune-jpeg 0.5.15 и zune-core 0.5.3 добавлены
   как workspace-зависимости. JPEG декодируется в четыре исходных канала до ICC,
   инвертированный JPEG CMYK переводится в ink coverage, YCCK — в CMYK с отдельным K.
   Затем moxcms выполняет CMYK→sRGB с perceptual intent; RGB, полученный прежним
   декодером image, в CMYK ICC не подаётся. Нормализация применяет EXIF и удаляет ICC.
   Правило passthrough не менялось: src/normalization.ts:41–48 требует space=srgb,
   а src/normalization.ts:80–83 всегда строит rotate→toColourspace('srgb')→resize.
   При наличии профиля применяется embedded ICC; без него используется тот же
   Chemical proof, что встроен в libvips под именем cmyk. Это следует из Sharp
   0.35.5 src/pipeline.cc:374–397: строки 382–387 используют embedded=true,
   строки 391–397 — input_profile="cmyk". Исходник прочитан в уже имевшемся npm
   cache, без сети, установки или запуска Sharp. Не используется упрощённая формула
   RGB=(1-CMY)*(1-K): например, исходный fallback даёт cyan [0,156,228].
   Public-domain LUT Graeme W. Gill сохранён с неизменными forward payload;
   обратные LUT и большой characterization text не нужны для input transform.
   assets/README.md фиксирует исходные/сокращённые SHA-256 и независимые golden
   Lab→sRGB значения. Допуск каждого канала — 3/255 до кодирования, 6/255 после
   JPEG quality 85; проверяются все пиксели двух MCU. Тест с синтетическим embedded
   ICC содержит также смешанный CMYK [64,128,192,32]→sRGB [212,176,128], поэтому
   игнорирование embedded ICC или потеря K не маскируются fallback-тестом.
   Пиксели могут отличаться от lcms/libvips из-за интерполяции и округления moxcms
   и YCbCr→CMY, а финальные JPEG-байты — также по причинам пункта 2.

## Не перенесено

- src/sharp.ts: динамический createLazyRequire/requireSharp исключён по MAP.tsv
  (lazy-require: drop) и прямому указанию использовать выбранные crates.
- Три runtime-случая tests/lazy-sharp-failure.spec.ts: ошибка загрузки Sharp при
  header probe, full decode и старте normalization. Нативные библиотеки связаны
  при сборке Rust; такой runtime-отказ загрузки отсутствует. Его подмена не проверяла
  бы поведение поставляемого crate. Остальные 96 исходных случаев представлены.

## Не проверено

Windows/macOS не запускались: cfg(windows) smoke-test, Windows read-only hard-link
поведение и отсутствие directory fsync требуют проверки там. POSIX chmod, inode
dedup, порядок fsync до возврата, снятие staging, потоковые ошибки и причины отмены
проверяются на Linux. Аварийное выключение питания не моделировалось.

Sharp/libvips здесь не запускался: эталонные JPEG/WebP байты и точность отдельных
ICC/Lanczos пикселей не сравнивались. Поддержка всех возможных повреждённых ICC и
необычных/неправильных контейнеров двух движков может различаться; верхние коды
и тексты ошибок стабильны, низкоуровневые причины native-декодеров другие.
Нестандартные ICC-пространства за RGB/Gray/CMYK и CMYK DeviceLink не проверены.
CMYK проверен на синтетическом embedded ICC и реальном исходном Chemical proof;
сравнение с запущенным lcms/Sharp oracle не выполнялось: golden-пиксели вычислены
из определения синтетического профиля и Lab-узлов исходного fallback-профиля.

## Вопросы ведущему

Открытых вопросов нет. Три ответа ведущего выполнены:

1. Прямые zune-зависимости разрешены и добавлены; CMYK ICC и исходный fallback
   реализованы и покрыты отдельными пиксельными тестами. Недостающих API зависимостей нет.
2. Порядок ключей ImageAttachmentRef не меняется: сравнение сценариев идёт по JSON
   значениям, как README conformance §6.2; это не ответственность данного crate.
3. Локальный список EXPECTED.tsv остаётся в crate; ведущий переносит его в
   conformance/EXPECTED.tsv при первом запуске соответствующих сценариев.

## Проверки

Команды выполняются с CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3. Финальные запуски текущего
дерева 2026-11-18:

- `RUST_TEST_THREADS=4 cargo test -p hoh-attachment-local`:
  `test result: ok. 91 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.52s`.
  Doc-tests: `0 passed; 0 failed`. Это 88 исходных Linux test-функций, включающих
  параметры 95 исходных runtime-случаев, и 3 дополнительные функции. Один исходный
  Windows runtime-случай присутствует под cfg(windows), здесь не запускался.
- `cargo clippy -p hoh-attachment-local --all-targets -- -D warnings`: exit 0,
  `Finished dev profile ... in 1m 00s`.
- `cargo build -p hoh-attachment-local`: exit 0,
  `Finished dev profile ... in 55.02s`.
- `cargo fmt -p hoh-attachment-local -- --check`: exit 0.
- Усиленный тест aborts_the_underlying_request_transform_after_its_only_waiter_cancels
  сначала воспроизвёл прежнюю задержку отмены (`0 passed; 1 failed`), затем прошёл
  после синхронной регистрации Wake. Добавлен отдельный тест последнего из двух
  ожидающих с проверкой причины до polling обеих future.
- `diff -rq locale` с исходным locale: exit 0; поиск todo!/unimplemented!/panic!/
  expect/unwrap в src не нашёл совпадений. Заглушек и отсутствующих include-файлов нет.

Проверки первого hand-in приведены выше. После закрытия CMYK, с тем же окружением:

- `RUST_TEST_THREADS=4 cargo test -p hoh-attachment-local`:
  `test result: ok. 94 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.20s`.
  Doc-tests: `0 passed; 0 failed`. Добавлены 3 CMYK/YCCK test-функции сверх первого
  hand-in; количество перенесённых исходных случаев и исключения lazy Sharp те же.
- `cargo clippy -p hoh-attachment-local --all-targets -- -D warnings`: exit 0,
  `Finished dev profile [unoptimized + debuginfo] target(s) in 10.92s`.
- `cargo fmt -p hoh-attachment-local -- --check`: exit 0, без вывода.
- До исправления новые пиксельные тесты обнаружили прежнюю ошибку: CMYK с ICC
  не преобразовывался, а без ICC cyan давал [0,255,255] вместо [0,156,228]. После
  прямого четырёхканального декодирования и ICC/fallback-преобразования все три
  новые функции проходят, затем проходит полный набор из 94 тестов.

Незавершённой работы и открытых вопросов нет. Изменений общего ImageAttachmentRef,
чужих crates, корневого manifest или глобального EXPECTED.tsv не делалось.
