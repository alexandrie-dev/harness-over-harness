# PORT_NOTES — hoh-spill-policy

Источник: `packages/spill/spill-policy` из зафиксированного дерева
`refs/deepseek-harness`. Перенесены `src/index.ts`, `src/notice.ts`,
`src/retention.ts`, `src/types.ts` и восемь файлов `locale/`.

## Контракт и точность поведения

- `SpillPolicy` — listener-only plugin с именем `spill-policy` и обязательной
  зависимостью `tools`. Отсутствующий `maxInlineTokens` действительно не
  регистрирует ни одного listener. Число проверяется при загрузке как
  неотрицательный JavaScript safe integer; текст ошибки и JavaScript-печать
  числа сохранены. Явный `maxInlineTokens: null` теперь доходит до пользовательской
  проверки и отвергается с точным TS-текстом `(got null)`, а не ошибочно
  трактуется как отсутствие поля.
- Оба listener зарегистрированы с `prepend`: `tools/post-execute` сначала
  вызывает downstream, а `tools/ptc-dispatch-log` ограничивает только durable
  копию. Accept с `value`, block, `read`, неподдерживаемые content block и
  text-only nested result проходят по тем же веткам, что в TypeScript.
- Стоимость текста считается через `hoh_token_meter::estimate_content`.
  Изображения оценивает calculator текущего route: provider и model независимо
  берутся сначала из `request/header`, затем из live `AgentOptions`.
  Отсутствующий calculator и несовпадающее число occurrence сохраняют исходный
  успешный result. Warning сохраняет JavaScript-оболочку `Error: ...`.
- Полный spill сохраняет исходный порядок и заменяет изображение точной
  строкой с JSON-адресом, media type и размерами. Неразрешимый путь, отсутствие
  attachments/fs/spill store/owner или ошибка save оставляют content inline.
  Head/tail retention использует UTF-16 длины для выбора разреза, UTF-8 bytes
  для omitted count, не режет surrogate pair и не делит изображение.
- Notice, gap и footer резервируются внутри лимита; notice, который сам дороже
  лимита, оставляет оригинал после best-effort save, как исходник. Соседние
  text blocks после retention объединяются. Распознаватель notice принимает
  только канонические ASCII-цифры и отвергает `+N`, ведущие нули и unsafe count.
- Успешный nested result, из которого удалено последнее изображение, получает
  дополнительный `UserMessage` с source `ptc-mode`; error result такого
  контекста не получает.

## Interface-first

Крейта-владельца `hoh-tool-executor-ptc` в workspace пока нет. Поэтому
`src/shim.rs` объявляет ровно необходимый source event
`PtcDispatchLog = "tools/ptc-dispatch-log"` с полями `exec`, `name`,
`sub_call_id`. Будущий владелец PTC должен принять этот event либо заменить shim
с сохранением имени и payload; чужие крейты в этой поставке не менялись.

После появления реального `Agent` в `hoh-tools::ToolExecution` route fallback
читается прямо из `exec.agent.options()`, а не через отдельный registry lookup.
Это эквивалентно `exec.agent?.options.provider/model` из `src/index.ts` и не
требует дополнительного service.

Rust не имеет объектной identity для клонированных `ImageBlock`. Цена хранится
по полному равенству image block; для повторов одного и того же durable image
это совпадает с calculator-ами workspace, цена которых является функцией
attachment/offloaded. Calculator всё равно обязан вернуть одну цену на каждый
occurrence, и это проверяется до retention.

## Тесты и исключения

В исходнике 55 развёрнутых test cases (два `it.each` дают по два случая).
Связанные варианты объединены в Rust table/compound tests без потери отдельных
assertions. После ревью отдельно закреплены все доступные ветки
`ptc-dispatch-log` (успех, unsupported content, within-cap и save failure), оба
listener при disposal, cap-invariant для огромного тела, точные best-effort
warnings, config/image/notice/retrieval/source строки и сохранение canonical
`value` при multimodal retention:

- все 6 notice cases и все 8 retention cases;
- disabled/config validation, bounded spill, notice-only fallback, small и
  unsupported content, `read`, owner/backend/save fallback, downstream
  content/context/value composition, nested skip и disposal;
- durable dispatch projection, неизменность program copy, unsupported и
  within-cap passthrough, а также save failure;
- ordered end images, whole middle-image omission, recovery descriptors,
  точные warning-причины missing path/pricing/fs и inconsistent occurrence,
  direct-programmatic no-route fallback, а также success/failure nested PTC
  recovery context;
- composition с делегированием в `next()` до generic cap и независимый fallback
  provider/model для частичного request header.

Полностью end-to-end не перенесены только окруженческие cases:

- export-shape через TypeScript `Loader.unwrapExports` — не существует в Rust;
- реальный Node PTC output-limit и два теста overlap/backpressure — владелец
  `hoh-tool-executor-ptc` ещё отсутствует; pure waterfall contract, включая
  non-text/within-cap/save-failure, проверен без Node runtime;
- все 6 `multimodal-recovery.spec.ts` требуют интеграции с реальным
  `hoh-spill-local`, Node PTC runtime, real local attachment stack и DeepSeek
  request serialization. Core retention/recovery path проверен локальными
  service doubles без сети, API key и desktop.
