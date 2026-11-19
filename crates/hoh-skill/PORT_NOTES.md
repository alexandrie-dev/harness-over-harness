# hoh-skill ← packages/skill/skill @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | реестр, плагин, все типы и функции; scoped layers, кеш, отмена, уведомления, rendering |
| `src/error.ts` | `src/error.rs`, re-export из `lib.rs` | все четыре кода и конструктор SkillError |
| `tests/skill.spec.ts` | `tests/skill.rs` | все 37 исходных случаев, дополнительные regression-проверки |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | те же пути | скопированы; `diff -r` не находит отличий |
| `README.md`, `package.json` | эти заметки, Cargo manifest, rustdoc | прочитаны; отдельного LICENSE и дополнительных entry points нет |

Зависимости: исходные `hoh-kernel`, `hoh-scope`, `hoh-llm`, `hoh-error`, `hoh-util-values`;
общий `hoh-timeout` добавлен по PORTING §6. Остальные зависимости взяты из workspace.
Прочитаны актуальные PORTING §4/§7, kernel API и примеры, заметки зависимостей;
использованы `hoh-scope::store::{NamedEntries, ScopedLayers, EffectOptions, EntryUndo}`.
`ScopeFilter` здесь не реализуется: исходник не создаёт scope carriers и не фильтрует receiver;
scope выбирается через `scope_of`, цепочка — через `scope_chain_of` и `chain_layers`.

Проверены импорты переносимых потребителей: skill-filesystem, skill-office, tool-skill,
sandbox-windows-acl/acl-skill и type-only import document/tool-office. Не найдено внешних
прямых импортов имён `SkillRegistry`, `SkillResourceBase`, `SkillRegistration`, `SkillViewOptions`,
`SkillCatalogSnapshot`, `SkillErrorCode`, `SKILL_ERROR_CODES`; все они всё равно доступны.
Класс используется как default plugin и как сервис; методы registry вызываются через `ctx.skills`.
Клиентские, Typert и experimental consumers не определяли интерфейс переноса.

## Отличия от образца

- `SkillRegistry` реализует Plugin с исходным именем класса; `Skills` — дополнительный ключ сервиса
  `skills`. Config сохраняет `collectCacheMaxEntries = 128`; схема разрешает дробное number,
  конструктор отдельно отвергает нецелое/неположительное значение исходным обычным Error.
  `f64` здесь нужен для проверки исходного входа `1.5`; revision/order — целые счётчики.
  Пользовательская сериализация поля выводит целое значение как JSON `128`, а не Rust-форму
  `128.0`, как `z.number().default(128)` в `index.ts:361`.
- Cordis service proxy заменён явным `ctx: &Context` у `register_provider` и `register`.
  Регистрация принадлежит вызывающему fiber; scoped запись попадает в слой этого контекста.
  `register_provider` возвращает точный kernel Disposer; `register` возвращает `Some(Disposer)`
  либо `None` для исходной no-op функции дубликата. Дубликат не снимает победителя.
  `new(ctx, Config::default())` заменяет необязательный аргумент конструктора.
- `SkillProvider` — object-safe trait; `list`/`get` возвращают boxed Send futures.
  Lookup/View options — один тип `SkillViewOptions` и alias `SkillLookupOptions`, передаваемый
  как один и тот же `Arc` всем провайдерам при discovery и loading. Лишнее поле `scope`
  не убирается при передаче провайдеру, как в JS. Отсутствующие options задаются `Default`.
- `SkillCandidate`, `SkillDefinition`, `SkillRegistration`, `SkillSummary` — неизменяемые
  записи с insertion-ordered полями `Arc<Value>`. Это сохраняет сырые неверные значения
  до исходной проверки, missing/null различие, неизвестные поля и identity вложенных
  invocation/resourceBase/metadata. `Clone` сохраняет identity записи; добавление defaults
  и проекция summary создают новую запись, разделяющую прежние поля. Дополнительные методы
  `from_value`, `From<Value>`, `get`, `text`, `shared_field`, `ptr_eq`, `to_value`,
  `with_field`, `with_shared_field`, serde и `Default` заменяют object literals, spread
  и доступ к свойствам. `From<&SkillCandidate> for SkillDefinition` переносит spread с
  сохранением вложенных ссылок. Providers не обязаны сериализовать locator:
  `with_locator(Arc<T>)/locator::<T>()` сохраняют произвольный opaque same-process handle;
  JSON-представление поля locator хранится отдельно, когда оно есть. Каталоги locator не публикуют.
- `SkillProviderOutput::{Candidates, Observation, Raw}` — дополнительный вход для array shorthand,
  typed observation и необработанного результата парсера. `Raw` не превращает malformed output
  в пустой успешный каталог. `SkillMetadata` — дополнительный общий доступ к invocation;
  model/user helpers принимают summary/candidate/definition/registration и возвращают Result,
  чтобы исключение при отсутствующей policy не стало panic. Provider policy не получает
  runtime defaults: исходный validator тоже разрешает отсутствие invocation целиком.
- `SkillResourceBase` — закрытый tagged enum; rendering использует исчерпывающий match.
  `SkillSource` остаётся открытой строкой. `SkillInvocationSource` имеет фиксированные
  KIND/FORM, serde и From в открытый `hoh_llm::MessageSource`: `kind,name,form`.
- `SkillError::new` возвращает непосредственно общий HarnessError с name `SkillError`,
  точным code/message; причины добавляются `with_cause`. Wrapper над HarnessError отсутствует.
  `SKILL_ERROR_CODES` дополнительно реэкспортирован в корне crate (в образце экспортируется
  из внутреннего error.ts); `SkillErrorCode::as_str()` даёт исходное строковое значение.
  Дополнительный enum `NativeError::{Error, Type}` соответствует обычным Error/TypeError;
  SharedError сохраняет identity исходной Error при отмене и отказе регистрации.
  Дополнительный `SkillThrownValue::{Json, Unrenderable}` явно представляет JS throw,
  не являющийся Error: JS String через общий `js_string`, защитный fallback и нормализация
  в обычный Error в тех же abort/wait ветках. Rust не имеет Symbol.toPrimitive/proxy traps;
  hostile-case проверяется этим bridge, а hostile Display провайдера — catch_unwind.
- Ключ кеша — `(cwd, Vec<ScopeKey>, revision)` вместо JSON с WeakMap scope ids. Равенство
  scope identity и цепочка сохранены; FIFO ограничен cap, чтение не обновляет позицию.
  Ключ Rust удерживает scope keys до eviction/invalidation вместо слабых ключей WeakMap;
  это ограниченное временем жизни кеша отличие удержания памяти, не порядок или текст каталога.
  Rebind без мутации registry меняет ключ. Retry ограничен двумя попытками; второй конфликт
  revision возвращает incomplete и не заполняет кеш. Неполные наблюдения тоже не кешируются.
- Отмена lookup прекращает ожидание, сохраняя уже начатую работу провайдера. Для signal lookup
  её driver — эффект контекста registry; в отличие от обычного JS Promise, выгрузка этого
  fiber завершает оставшуюся работу. Без signal await выполняется непосредственно.
  Неожидаемый отказ driver при unload — обычный Error `skill provider work disposed`.
  Проверка отмены после завершившегося `collectFresh` вынесена в `finish_collect`. Её вызов
  закреплён интеграционным тестом: provider отменяет signal во втором `name()` — уже после
  успешного provider `wait`, но до возврата каталога. Поэтому удаление вызова `finish_collect`
  из production `collect` делает тест красным без гонки планировщика. Отдельная повторная
  проверка после `collect` в `get` также присутствует, как в `index.ts:506`.
- `skills/change` объявлен в Contained mode. `ctx.root().dispatch(...).invoke(report)`
  не применяет receiver filter контекста provider, не ждёт promises, изолирует каждого
  listener и различает Threw/Rejected. Два текста warn сохранены дословно, включая `Error: `.
  Journal получает одну готовую строку на вызов, согласно docs/kernel.md §7.
  Ошибки discovery/observer записываются под контекстом владельца сервиса; Cordis proxy
  в отдельных путях использует caller logger name. Duplicate runtime warn использует
  переданный caller ctx. Это диагностическое отличие имён logger, не model/session output.
- Имена сортируются обычным сравнением ASCII (валидная грамматика только ASCII), что совпадает
  с исходным JS `<`/`>`. `localeCompare` нет. Внутри слоя rank дробный, затем providerOrder,
  затем localOrder; ближайший scope выигрывает без учёта rank. Summary serializes в порядке
  `name,path?,description,whenToUse?,invocation,source,provider,resourceBase?`.
  Wrapper rendering, escapeText, resource hints, переносы и отсутствие конечного LF сохранены.
  Body вставляется дословно, в том числе закрывающие теги. Raw resourceBase с неизвестным kind
  получает исходный `unreachable variant in SkillResourceBase.kind: ...` через общий
  `js_json_stringify`; тот же helper используется в regression goldens. Native failures
  renderer и порядок чтения resource hint перед name сохранены; пустой `Error` рендерится как
  `Error` без двоеточия, как `String(new Error(''))` в `index.ts:865`; null content в Array.join
  даёт пустую строку. Файлов, путей Node, readdir,
  trim/whitespace-классов, системных ошибок и часов в исходнике нет.

## Не перенесено

Пропущенных исходных файлов и test cases нет: 37 из 37 представлены с узнаваемыми snake_case
именами. JS-механизм hot replacement `vi.spyOn(effect)` заменён реальным inactive context:
проверяются отказ регистрации и та же identity причины в lifecycle signal. Шпионы на
Array.sort/String.localeCompare заменены проверкой результата обычного Rust comparator;
механизм подмены методов не переносится по PORTING §8/§10. NaN в исходном malformed-rank
case представлен JSON null (serde_json не хранит NaN), отказ и сообщение те же.
Hostile abort reason — явный Unrenderable bridge вместо JS Symbol; Promise timeout races
проверяются с paused time и Notify gates. npm-документация не копируется.

## Не проверено

Платформенных веток, внешнего I/O и сети нет. Реальные JavaScript proxy/getter/Symbol traps
не исполнялись; их перенос не требуется §8. Строки Rust не содержат lone UTF-16 surrogates.
Произвольные нетипизированные runtime registrations вне заявленных source interfaces имеют
явную границу. В частности, JavaScript RegExp в `index.ts:745` преобразует числовое `name: 123`
в строку для самой проверки, после чего исходник сохраняет число в `Map<string, ...>` вопреки
объявленному `SkillRegistration`; нативный реестр использует строковый ключ и отвергает такой
`SkillRegistration::from_value`. Это malformed-вход вне TypeScript-контракта, закреплённый
boundary-тестом, а не расширение Rust API до разнородных ключей.

Замечание ревью об отсутствии `invocation` было основано на неверном прочтении источника:
`isModelInvocable`/`isUserInvocable` в `index.ts:129,138` обращаются к
`skill.invocation.modelInvocable/userInvocable` без optional chaining и потому выбрасывают
TypeError. Rust helpers также возвращают `NativeError::Type`; отдельный тест проверяет класс.
Напротив, `validateRuntimeSkill` (`index.ts:744-748`) действительно не проверяет обязательный
по типу `source`; порт сохраняет это runtime-поведение и тестирует, что поле остаётся absent.
Остальные provider scalar validations покрыты.
Полные recorded dsh sessions проверит общий conformance runner; здесь проверены байтовые
goldens renderer, summary и message-source, а не заявлен запуск conformance.

## Вопросы ведущему

Нет. Недостающих API зависимостей не обнаружено; чужие crates не менялись.

## Проверки

Все cargo-команды выполняются из корня с `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4` и `--offline`.

- `cargo test -p hoh-skill --offline` — unit `1 passed; 0 failed`, integration
  `48 passed; 0 failed`, doc-tests `0 passed; 0 failed`: всего 49/0. Все 37 исходных случаев
  и двенадцать regression-проверок. Финальный test profile: `Finished ... in 1.40s`.
- `cargo clippy -p hoh-skill --all-targets --no-deps --offline -- -D warnings` — exit 0,
  `Finished dev profile [unoptimized + debuginfo] target(s) in 0.75s`.
- `cargo fmt -p hoh-skill -- --check` — exit 0, вывода нет.
- `diff -r <source>/locale crates/hoh-skill/locale` — exit 0, файлы совпадают побайтно.
- `git diff --check -- crates/hoh-skill` — exit 0; заглушек/todo в src нет.

Первый исходный набор прошёл 37/37. Добавленная проверка plugin.raw_config сначала ошибочно
ожидала defaults в сыром input: raw_config действительно сохраняет `{}`; assertion исправлен
на исходный input, а отдельный wire-тест проверяет `Config::default()` как JSON-число `128`.
Все проверки выполнены повторно для актуального состояния; ошибок в чужих crates в этих
запусках не было. Для новой cancellation-регрессии отдельно выполнена mutation-проверка:
временное удаление production-вызова `finish_collect` дало `MUTATION_EXIT=101`, потому что
тест получил успешный каталог вместо общей причины отмены; после этого вызов восстановлен.
Изменения только в `crates/hoh-skill`.
