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
| tests/image.spec.ts | tests/image.rs | 7 случаев; дополнительно premultiplied alpha и 4 review-теста JPEG/PNG/APNG/WebP/grey |
| tests/normalization.spec.ts | tests/normalization.rs | 19 групп / 21 случай; дополнительно Gray/GrayAlpha ICC, таблица 16 PNG/JPEG геометрий, 9 WebP + 1 GIF геометрий и EXIF transpose |
| Дополнительные write-error проверки | tests/write_errors.rs | Отдельный harness=false binary: два изолированных ребёнка, RLIMIT_FSIZE=64, SIGXFSZ ignored; errno, отсутствие публикации/temporary и сохранение прежнего cache |
| tests/encoding.spec.ts | tests/encoding.rs | 6 случаев, в том числе FIFO и освобождение слота после ошибки |
| tests/request-image.spec.ts | tests/request_image.rs | 15 групп / 16 случаев; дополнительно причина отмены последнего из двух ожидающих |
| tests/normalization-verification.spec.ts | tests/normalization_verification.rs | 1 случай |
| tests/request-image-verification.spec.ts | tests/request_image_verification.rs | 1 случай |
| Дополнительные CMYK-проверки | tests/cmyk.rs | 6 тестов: embedded ICC, fallback без ICC, YCCK, Adobe-конвенции, invalid-profile fallback с равенством bytes, header intent; пиксели до/после JPEG |
| Встроенный профиль libvips cmyk | assets/cmyk-input.icc.base64, assets/CMYK_PROFILE_LICENSE.txt, assets/README.md | Public-domain Chemical proof; сохранены исходные forward LUT, записаны происхождение, SHA-256 и точный текст лицензии |
| sharp 0.35.5 LICENSE | assets/sharp/LICENSE | Apache-2.0 для перенесённого поведения ResolveShrink; скопирован без изменений, ведущему для NOTICE |
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
tests/write_errors.rs зарегистрирован отдельно через [[test]], harness=false;
rustix.workspace добавлен только как cfg(unix) dev-dependency для безопасного setrlimit.

Поиск импортов в packages/ и apps/ (без client/api/typert/host/experimental/web/desktop)
не нашёл прямых импортов из этого пакета в src других переносимых пакетов. Плагин
подключается композицией; LocalAttachmentStore используется в тестах tool-fs,
tool-office, spill-policy, llm-deepseek и mcp-client, readImageFile — в e2e skill-office.
Именованные функции нормализации/подготовки/кеша, типы и DEFAULT_*/MAX_* не имеют
прямых src-потребителей снаружи владельца; все они всё равно перенесены.

## Отличия от образца

- Версия moxcms остаётся 0.8.1 из workspace.dependencies, как в существующем
  Cargo.toml; локального pin/upgrade нет. Строка 0.9.2 в таблице docs/porting/DEPS.md
  расходится с workspace; по ответу ведущего её исправляет ведущий, не writer w3.
- Происхождение CMYK-профиля подтверждено: libvips/colour/profiles/cmyk.icm,
  961 644 байта, SHA-256 156e7c14f244cfc4ed83a755ca4803d80e15dd249b40fae82cb127d3902e15c7.
  Отдельного LICENSE для профиля в libvips нет: assets/CMYK_PROFILE_LICENSE.txt
  воспроизводит текст cprt о public domain Graeme W. Gill. Наш asset — производный
  input-only профиль: forward A2B0/A2B1/A2B2 payload сохранён, удалены B2A0/B2A1/B2A2,
  gamt, clrt и characterization targ/DevD/CIED; пересобраны таблица тегов, смещения,
  длина и очищен старый profile ID. Цветовые forward LUT не изменены.

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
4. **Размер и декодирование:** image Lanczos3 заменяет libvips Lanczos3.
   Размеры вычисляет перенос ResolveShrink, а не image::resize: inside берёт
   больший shrink по осям, withoutEnlargement ограничивает его снизу единицей,
   каждая ось ограничена размером входа. Размер результата — floor(in/shrink+0.5).
   JPEG сначала выбирает DCT shrink 2/4/8 и делит его пополам при trunc(shrink)==scale,
   считает loaded axes через floor(in/scale), затем пересчитывает shrink.
   Это округление libvips jpeg2vips.c:639–640, исправляющее ошибочный ceil
   из предыдущего задания; libjpeg IDCT сам по себе округлял бы иначе.
   zune не предоставляет scaled IDCT: перенос декодирует полный растр и уменьшает
   его Lanczos до тех же loaded dimensions. Это необходимое отличие пикселей от
   native DCT shrink; размеры последующего преобразования совпадают с формулами
   sharp/libvips. WebP при shrink=min(h,v)>1 использует loaded axes
   rint(in*(1/shrink)), с ties-to-even, после чего shrink пересчитывается.
   Полный WebP decode + Lanczos эмулирует эти размеры вместо native scale-on-load:
   значения пикселей и следующие encoded bytes могут отличаться от libwebp/libvips.
   GIF не имеет preload-ветки и меняет размер только первого полного кадра.
   Само конечное изменение размера выполняется resize_exact.
   JPEG декодирует
   zune через image, GIF/PNG/WebP — соответствующие image-бэкенды; при декодировании
   с потерями и сборке первого кадра возможны другие значения пикселей. Геометрия
   начинает с исходного request_image_dimensions для normalization и source long-edge
   target для request-image, затем учитывает ResolveShrink и JPEG/WebP preload. EXIF
   применён перед нормализацией: вычисление на oriented axes эквивалентно обмену
   target width/height до resize при повороте 90°/270°; perceived axes учтены в лимитах.
   Как sharp, перенос premultiply/unpremultiply alpha вокруг ресемплирования;
   float-округление image отличается от libvips, но скрытые цвета прозрачных
   пикселей не смешиваются с цветами видимых границ.
5. **Цвет и глубина:** RGB/Gray/CMYK ICC преобразуются moxcms в sRGB вместо цветового
   движка libvips. По умолчанию intent perceptual; CMYK без perceptual LUT использует
   intent заголовка. Интерполяция и округление не побайтные;
   промежуточные каналы приводятся image к 8 битам, включая 16-битный PNG, прежде
   чем выполняется 8-битный ICC transform. Без ICC image разворачивает grayscale
   и переводит 16-bit в uchar RGB/RGBA. Метаданные, EXIF, ICC/XMP/IPTC/Photoshop,
   видимые sharp PNG-комментарии и orientation делают вход нечистым, а кодировщики
   их не сохраняют. JPEG COM и PNG-текст после первого IDAT не являются retained metadata.
6. **CMYK:** разрешённые ведущим zune-jpeg 0.5.15 и zune-core 0.5.3 добавлены
   как workspace-зависимости. JPEG декодируется в четыре исходных канала до ICC,
   инвертированный JPEG CMYK переводится в ink coverage, YCCK — в CMYK с отдельным K.
   Затем moxcms выполняет CMYK→sRGB с perceptual intent либо intent заголовка,
   когда perceptual не поддержан; RGB, полученный прежним
   декодером image, в CMYK ICC не подаётся. Нормализация применяет EXIF и удаляет ICC.
   Правило passthrough не менялось: src/normalization.ts:41–48 требует space=srgb,
   а src/normalization.ts:80–83 всегда строит rotate→toColourspace('srgb')→resize.
   При наличии пригодного CMYK-профиля применяется embedded ICC; при отсутствии,
   повреждении, неверном цветовом пространстве либо отказе построения transform используется тот же
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

### Раунд исследования sharp 0.35.5 / libvips 8.18.7 / libpng 1.6.58 / mozjpeg 0826579

Ниже использованы результаты исследования ведущего с указанными строками exact
версий. В sharp-libvips 1.3.4 libspng отключён, PNG loader использует libpng 1.6.58.

Перед исправлениями новые тесты `cargo test -p hoh-attachment-local review_`
воспроизвели 8 отказов / 1 успех: JPEG COM, APNG, PNG metadata cut-off, три
CMYK-сценария и два resize-сценария (включая autorotation). Сначала были добавлены
и запущены тесты против прежней реализации, затем изменён production-код.

1. JPEG COM не является metadata. Sharp metadata.cc:152–153,336–346,391–401
   читает comments только из png-comment-<i>-<key>; jpeg2vips.c:963–965 сохраняет
   APP1/APP2/APP13. FE удалён из retained metadata; APP13 учитывается только
   с payload длиннее пяти байтов и префиксом Photo (уточнение третьего раунда ниже). В исходном
   reports_color_profiles_and_encoder_metadata_as_metadata восстановлен PNG с
   текстовым chunk; отдельный JPEG COM проходит byte-for-byte и сохраняет digest.
2. PNG всегда имеет animated=false, в том числе APNG: libpng pngrutil.c:2962–2973
   игнорирует acTL/fcTL/fdAT, libvips не выставляет PNG n-pages. Тест строит валидный
   двухкадровый APNG с fcTL/fdAT и проверяет probe/full decode и passthrough bytes.
3. PNG tEXt/zTXt/iTXt/eXIf учитываются только до первого IDAT; скан прекращается
   на IDAT/IEND. В vipspng.c:210–211,354,578 используются png_read_info и png_get_text,
   но нет png_read_end. Keyword XML:com.adobe.xmp перед IDAT остаётся metadata
   (vipspng.c:363–370). Тесты размещают каждый вид chunk до IDAT, между IDAT/IEND
   и после IEND и проверяют факты и точное совпадение/изменение байтов.
4. CMYK-конвенции: jdapimin.c:186–205, jdcolor.c:574–579 и jpeg2vips.c:547–552,881–886.
   zune без APP14 и при transform 0 возвращает CMYK; при 2 — YCCK; при 1 возвращает
   CMYK, при 3 отказывает. Для four-component JPEG все нестандартные nonzero Adobe
   transforms адаптируются к 2 только в decoder input; исходный буфер не переписывается.
   Итоговые C/M/Y равны YCbCr→RGB для YCCK, K инвертируется; CMYK инвертирует все
   четыре канала, как libjpeg+libvips. Пиксели проверены во всех пяти случаях.
   Invalid/non-CMYK ICC отбрасывается с fallback на cmyk (icc_transform.c:555–563,691–735);
   тесты проверяют также точное равенство нормализованных байтов входу без ICC.
   При отсутствии perceptual LUT выбирается intent заголовка (icc_transform.c:596–612),
   проверены относительный colorimetric и saturation с разными LUT, а не builtin fallback.
5. Геометрия: common.cc:1091–1153, pipeline.cc:195–199,218–247,353–355,
   reduceh.cpp:424–425, reducev.cpp:855. Уточнение третьего раунда: loaded axes
   берутся из jpeg2vips.c:639–640, не из jdmaster.c. Таблица из 16 случаев
   проверяет размеры фактических bytes, reference и originalDimensions, а также digest.
   Включены PNG 4000×3→1333×1 и JPEG 4000×3→2000×1 при target 2000×1;
   PNG/JPEG 3000×2001→2000×1334 (также явно вызван target 2000×2000), transpose,
   withoutEnlargement, тонкая сторона, shrink=8.9 и отдельная EXIF orientation=6.
6. В always_re_encodes_gif восстановлены проверки depth=uchar, space=srgb.
   Дополнительный тест проверяет animated WebP и 8-bit grey PNG с space=b-w:
   оба требуют re-encoding, в отличие от чистого APNG.
7. moxcms остаётся workspace 0.8.1; исправление таблицы DEPS.md на стороне ведущего.
8. assets/README.md называет профиль производным forward-only input asset,
   перечисляет удалённые теги и происхождение public-domain cprt вместо отдельного
   upstream LICENSE, которого нет.

### Третий раунд: write errors, APP13 и scale-on-load

Факты ниже предоставлены ведущим по исходникам точных версий; сеть и Sharp/libvips
не запускались. Permalinks — ссылки на установленное исследованием поведение.

1. Перед fsync staging и перед закрытием/rename request-cache добавлен flush,
   его отказ оформляется тем же fs_error(...,"write",...), что write_all.
   Tokio sync_all теряет ошибку pending write: [tokio 1.54.1 file.rs:317–323](https://github.com/tokio-rs/tokio/blob/tokio-1.54.1/tokio/src/fs/file.rs#L317-L323)
   и [file.rs:1194–1204](https://github.com/tokio-rs/tokio/blob/tokio-1.54.1/tokio/src/fs/file.rs#L1194-L1204),
   PORTING.md §6. Это не новый источник отказа: исходные handle.writeFile(chunk)
   и fs.writeFile(temporary,data) уже ожидают реальной записи. Аудит всех writable
   tokio::fs::File нашёл только эти два места; directory sync открывается read-only.
   В tests/write_errors.rs дети запускаются через sh trap/exec с ignored SIGXFSZ,
   безопасный rustix::process::setrlimit ограничивает только ребёнка. Проверены
   EFBIG в исходной цепочке io::Error, текст "EFBIG: file too large, write",
   ATTACHMENT_WRITE_FAILED для staging, отсутствие опубликованного короткого
   объекта, удаление temporary и сохранение прежних cache bytes. До исправления
   оба ребёнка обнаружили Ok(()) после короткой записи и завершились с ошибкой.
2. У APP13 ровно условие data_length>5 && starts_with(b"Photo"):
   [libvips jpeg2vips.c:719–735](https://github.com/libvips/libvips/blob/v8.18.7/libvips/foreign/jpeg2vips.c#L719-L735),
   sharp [metadata.cc:120–126](https://github.com/lovell/sharp/blob/v0.35.5/src/metadata.cc#L120-L126)
   и [315–316](https://github.com/lovell/sharp/blob/v0.35.5/src/metadata.cc#L315-L316).
   Photoshop 3.0\0IPTC и PhotoXYZ требуют re-encoding; ровно Photo (5 байтов)
   и иной prefix проходят byte-for-byte. Проверены probe/full decode и normalization;
   JPEG COM по-прежнему чистый. До исправления тест падал на metadata=true вместо false.
3. JPEG preload теперь floor(in/scale), как [jpeg2vips.c:639–640](https://github.com/libvips/libvips/blob/v8.18.7/libvips/foreign/jpeg2vips.c#L639-L640).
   Исправлены четыре JPEG строки таблицы и EXIF transpose: 4000×3 → 2000×1,
   transpose → 1×2000, 99×33 → 9×3, 267×89 → 30×10 при прежних limits.
   WebP preload: [sharp pipeline.cc:218–247](https://github.com/lovell/sharp/blob/v0.35.5/src/pipeline.cc#L218-L247),
   loaded size rint(in*(1/min(h,v))) из [webp2vips.c:513–514](https://github.com/libvips/libvips/blob/v8.18.7/libvips/foreign/webp2vips.c#L513-L514),
   повторный shrink — [pipeline.cc:353–355](https://github.com/lovell/sharp/blob/v0.35.5/src/pipeline.cc#L353-L355),
   итог floor(in/shrink+0.5) — [resize.c:170–204](https://github.com/libvips/libvips/blob/v8.18.7/libvips/resample/resize.c#L170-L204).
   Девять WebP строк включают 4828×408 при cap107 → 106×9, три прежних golden
   геометрии и остальные уникальные строки PNG/JPEG таблицы. GIF с теми же
   4828×408/cap107 даёт 107×9: GIF branch в pipeline отсутствует, nsgifload не
   имеет scale. До исправления WebP возвращал 107×9; JPEG/EXIF — старые 1000×1/1×1000.
   Проверяются и объявленные, и реально декодированные размеры; прежние 16 строк
   продолжают проверять reference, originalDimensions и SHA-256.
4. У resize_srgb удалены bool inside и мёртвая ветка; функция только resize_exact
   и premultiply/unpremultiply. Геометрия живёт в resize_for_target/resolve_shrink.
   Существующий тест прозрачного ресемплирования использует прямой exact resize.

## Не перенесено

- src/sharp.ts: динамический createLazyRequire/requireSharp исключён по MAP.tsv
  (lazy-require: drop) и прямому указанию использовать выбранные crates.
- Три runtime-случая tests/lazy-sharp-failure.spec.ts: ошибка загрузки Sharp при
  header probe, full decode и старте normalization. Нативные библиотеки связаны
  при сборке Rust; такой runtime-отказ загрузки отсутствует. Его подмена не проверяла
  бы поведение поставляемого crate. Остальные 96 исходных случаев представлены.

## Не проверено

Обработка eXIf после IDAT самим libpng не прослежена: по решению ведущего
применён тот же cut-off первого IDAT, что к PNG text chunks.

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
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3. Запуски первого hand-in
2026-11-18 (история; результаты текущего review-раунда ниже):

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

Финальные проверки review-раунда sharp/libvips/libpng/mozjpeg, с тем же окружением:

- До production-изменений `RUST_TEST_THREADS=4 cargo test -p hoh-attachment-local review_`:
  `test result: FAILED. 1 passed; 8 failed; 0 ignored; 0 measured; 94 filtered out; finished in 0.74s`.
- После исправлений тот же отбор: `9 passed; 0 failed; 94 filtered out`.
- Финальный `RUST_TEST_THREADS=4 cargo test -p hoh-attachment-local`:
  `test result: ok. 103 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 40.42s`.
  Doc-tests: `0 passed; 0 failed`. Добавлены 9 review test-функций; таблица размеров
  содержит 16 hand-derived случаев. Количество исходных случаев остаётся 96/99
  (95 на Linux, один под cfg(windows)); исключены только три lazy-Sharp случая.
- `cargo clippy -p hoh-attachment-local --all-targets -- -D warnings`: exit 0,
  `Finished dev profile [unoptimized + debuginfo] target(s) in 3m 00s`.
  Предварительный запуск потребовал заменить `% 2 != 0` на `!is_multiple_of(2)`
  в новом WebP test-helper; финальный полный test-run выполнен после этой замены.
- `cargo fmt -p hoh-attachment-local -- --check`: exit 0, без вывода.
- `git diff --check -- crates/hoh-attachment-local`: exit 0; в src нет todo!,
  unimplemented!, panic!, expect или unwrap. Sharp LICENSE скопирован byte-for-byte.

Незавершённой работы и открытых вопросов нет. Изменений общего ImageAttachmentRef,
чужих crates, корневого manifest или глобального EXPECTED.tsv не делалось.

Проверки третьего раунда, с тем же offline/4-job окружением и target-w3:

- До исправлений `RUST_TEST_THREADS=4 cargo test -p hoh-attachment-local review_`:
  `test result: FAILED. 6 passed; 4 failed; 0 ignored; 0 measured; 94 filtered out; finished in 2.00s`.
  Это APP13, JPEG table, EXIF transpose и WebP scale-on-load.
- До исправлений `cargo test -p hoh-attachment-local --test write_errors`: exit 101;
  оба ребёнка (immutable и request-cache) поймали неверный Ok(()) вместо write errno.
  Первый compile нового binary выявил неподдерживаемый LowerHex у workspace sha2;
  test-helper исправлен до этого runtime red-run, production ещё не менялся.
- После исправлений `RUST_TEST_THREADS=4 cargo test -p hoh-attachment-local`:
  `test result: ok. 104 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 41.57s`.
  Отдельный tests/write_errors.rs (harness=false): exit 0, оба изолированных случая
  выполнены и прошли. Они не включены в 104 unit-теста. Doc-tests: `0 passed; 0 failed`.
  Исходные 96/99 случаев и три исключения lazy-Sharp не изменились.
- `cargo fmt -p hoh-attachment-local -- --check`: exit 0, без вывода.
- `cargo clippy -p hoh-attachment-local --all-targets -- -D warnings`: exit 0,
  `Finished dev profile [unoptimized + debuginfo] target(s) in 7m 41s`.
- Финальный `git diff --check -- crates/hoh-attachment-local`: exit 0.
  В src нет todo!, unimplemented!, panic!, expect или unwrap; inside-ветки не осталось.
  Код зависимостей, чужие crates и глобальные документы не редактировались; единственное
  добавление manifest — разрешённый workspace rustix для Unix dev-теста.
