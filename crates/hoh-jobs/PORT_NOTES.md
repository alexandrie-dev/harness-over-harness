# hoh-jobs ← packages/jobs/jobs @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён: сервисный ключ, object-safe контракт и fail-loud plugin |
| src/types.ts | src/types.rs | Перенесён полностью |
| src/view.ts | src/view.rs | Перенесён полностью |
| src/brand.ts | src/brand.rs | Перенесён через `hoh_brand::branded_string!` |
| src/error.ts | src/error.rs | Перенесён по общему контракту `HarnessError` |
| src/archive-admission.ts | src/archive_admission.rs | Перенесён через временный приватный Workspace shim |
| tests/service.spec.ts | tests/service.rs | 5 из 5 случаев перенесены |
| tests/error.spec.ts | tests/error.rs | 6 параметризованных вариантов перенесены отдельными тестами |
| tests/archive-admission.spec.ts | tests/archive_admission.rs | 3 из 3 случаев перенесены; файл подключён как unit module для доступа к приватному shim |
| README.md, README.zh.md, package.json | Cargo.toml, эти заметки | Прочитаны; `/types` и `/view` сохранены публичными модулями |

Отдельного LICENSE и каталога `locale/` в пакете нет.

## Отличия от образца
- Cordis-класс `JobRegistry` разделён по идиоме ядра на сервисный trait `JobRegistryApi`, ключ `Jobs = "jobs"` и fail-loud plugin `JobRegistry`. Методы, создающие эффекты или читающие scope вызывающего (`JobEvents::subscribe`, `attach_controller`), явно получают `&Context` по `docs/kernel.md` §7.
- `provide_jobs` — обязательная точка регистрации реализации: она безусловно устанавливает оба archive-admission listener до предоставления сервиса, как конструктор абстрактного класса в образце.
- Promise/callback, способные бросить, представлены `Result`: `JobSpec::run`, `JobHooks::cancel`, `JobHooks::done`, `JobOutputSource::read` и `JobEventListener`. `wait` — boxed future для object-safe сервиса.
- Открытые `JobKindMap` и workspace declaration merging представлены строками; сам интерфейс `JobKindMap` как TypeScript-механизм отдельного Rust-символа не имеет (PORTING.md §4/8). Именованный Rust-тип `JobOutcomeStatus` представляет inline union поля `JobOutcome.status`; `JobEventOwners`, `KillOutcome`, `JobFuture`, `JobCancel`, `JobOutputView`, `JobHandle::new` и `append_with_options` — необходимые Rust-пары структурных TS-типов и optional-аргумента.
- `JobStatus::{is_live,is_terminal}` — Rust helpers для повторного использования потребителями; новых статусов они не вводят.
- `JobChunk.channel` при неизвестном или нестроковом wire-значении становится `None`, как требует исходный client-safe контракт. `read_at` принимает `f64`, чтобы реализация могла воспроизвести отказы для отрицательных, дробных и не finite offset.
- `JobToolCall` использует владельца бренда `hoh_llm::ToolCallId`: исходный тип приходит через `SessionEventMap['tool/call']`, а Rust `hoh-session` намеренно хранит открытую карту событий как raw JSON.
- События `job/registered` и `job/settled` остаются открытыми session payloads `JobRegisteredData`/`JobSettledData`; их запись принадлежит реализации registry.

## Не перенесено
Нет. TypeScript declaration merging как механизм заменён открытыми строковыми типами по PORTING.md §4/8.

## Не проверено
Платформенных веток нет. Реальная маршрутизация owner scope, кольцо, settlement и containment listener panic принадлежат `hoh-jobs-local` и проверяются в следующем пакете; этот crate задаёт их контракт.

Экспорты, которыми не пользуется переносимый пакет вне группы jobs: `JobRegisteredData`, `JobSettledData` (их использует только `jobs-local`); остальные экспорты имеют потребителей в `jobs-local`, `tool-jobs`, `tool-workflow` либо shell producers. Клиентские/api/host пакеты при аудите не учитывались.

## Вопросы ведущему
- **SHIM(hoh-workspace):** crate `hoh-workspace` ещё отсутствует (`MAP.tsv`: todo). В `archive_admission.rs` приватно объявлены точные `SessionActivity`, `workspace/session-activity` и `workspace/session-stop`. После появления владельца нужно заменить shim его типами; иначе TypeId событий двух crates не совпадёт.
- `hoh-tool-bash/src/shim.rs` должен быть заменён на `hoh-jobs`: текущий shim не передаёт `JobHandle` в `JobSpec::run`, не содержит `output_limit_bytes`, считает `read_at` целым и делает source/cancel/done infallible. Чужой crate по границам задания не изменялся.
