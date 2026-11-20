# hoh-agent-default-model ← packages/core/agent-default-model @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `Config`, сервис и plugin `AgentDefaultModelConfig`, `current_selection`, `save_selection` |
| `tests/agent-default-model.spec.ts` | `tests/agent_default_model.rs` | 3/3 случая; 9 тестов Rust дополнительно проверяют caller realm, lifecycle очереди, обе no-op ветки и settings presentation |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки | прочитаны |

Собственного `LICENSE` и отдельно лицензированных фрагментов нет.

## Отличия от образца

1. Методы сервиса, читающие сервис от имени вызывающего, получают `caller: &Context` по `docs/kernel.md` §7.
   Поэтому `save_selection` находит `configEditor` в realm вызывающего, а owning entry — через сохранённый
   контекст plugin fiber и `LoaderKey::resolve` по полному `Fiber::entry_id()`.
2. Promise-tail `saves` перенесён принадлежащей сервису FIFO-очередью с worker на root-lifecycle ядра.
   Постановка происходит синхронно при вызове `save_selection`, до polling возвращённого future; ошибка
   одного job отправляется только его caller и не мешает следующему. Выгрузка plugin не ждёт активную
   запись и не отбрасывает уже поставленный хвост, как и TS Promise. Когда последний sender исчезает,
   worker дренирует очередь, запускает снятие собственного root-effect и завершается; shutdown ядра
   отменяет активный job с `KernelError::Closed`.
3. Переданный `ModelSelection` в Rust владеет строками, поэтому последующая мутация объекта caller невозможна.
   Значение всё равно переводится в отделённый JSON до постановки в очередь, как spread-object образца.
4. Опциональная интеграция использует публичный типизированный сервис `hoh_settings::Settings`. Presentation
   `{ auto: false }` привязан к fiber экземпляра plugin и снимается child effect при его выгрузке.

## Review 025145

- Причина потери записей при HMR/dispose: worker был эффектом plugin activation, а TS `saves` не входит в
  lifecycle выгрузки сервиса. `queued_saves_outlive_plugin_disposal_without_blocking_it` фиксирует оба
  свойства: dispose завершается при заблокированном A, после освобождения выполняются A и стоящий за ним B.
- `serializes_overlapping_saves_and_continues_after_a_rejected_write` теперь проходит через настоящий
  `save_selection` и `ConfigEditor`: futures ставятся до polling, последний ожидается первым, первый получает
  ошибку испорченного profile patch, а последняя запись сохраняет снимок `final`, не последующую мутацию.
- `keeps_the_entry_when_the_caller_cannot_see_a_configuration_editor` покрывает source no-op при существующей
  owning entry, но изолированном от caller `configEditor`. `kernel_shutdown_closes_an_active_save` закрепляет
  границу root-lifecycle и отсутствие зависания shutdown.

## Review 035011

- `standalone_plugin_with_a_visible_editor_keeps_its_own_selection` воспроизводит точный подслучай source
  spec 2: editor существует, но standalone fiber не имеет composition entry, поэтому save остаётся no-op.
- Замечание о синтетическом тесте spec 3 относилось к состоянию до review 025145. Текущий
  `serializes_overlapping_saves_and_continues_after_a_rejected_write` уже вызывает настоящий
  `save_selection → ConfigEditor::edit`, передаёт ошибку первого edit своему caller и сохраняет второй снимок.

## Не перенесено

Механизмы declaration merging и default export как механизмы TypeScript не переносятся. Публичные значения
`Config`, `AgentDefaultModel`, `AgentDefaultModelConfig` являются Rust-парами исходных интерфейса, Context
service и default class export. Исключённых source test cases нет.

## Не проверено

Ветвей платформ нет. Исключённых интеграций или открытых shim-вопросов нет.
