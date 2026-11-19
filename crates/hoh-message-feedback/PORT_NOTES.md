# hoh-message-feedback ← packages/feedback/message-feedback @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/types.ts` | `src/types.rs` | перенесён |
| `src/index.ts` | `src/service.rs` | перенесён |
| `tests/message-feedback.spec.ts` | `src/service.rs` tests | перенесены, родственные случаи сгруппированы |
| `tests/loader-composition.spec.ts` | `jsonl_composition_persists_feedback_across_restart` | перенесён |
| `locale/*.json` | `locale/*.json` | перенесены без изменений |

## Отличия от образца

- Typert transport и generated client исключены по §8; публичные операции
  представлены методами `MessageFeedbackService`, а обязательные имена
  зафиксированы маркерами `Remote: messageFeedback.list|put|delete`.
- Принятые операции запускаются owned Tokio task и возвращают waiter. Это
  сохраняет JS Promise-семантику: отмена caller future не отменяет уже принятую
  CAS-операцию и shutdown дренирует её до освобождения persistence handle.
- Ошибка `SessionHandle::close`, как `await handle.close()` в TS `finally`,
  заменяет более раннюю ошибку операции.
- Литеральные `ok: true`/`ok: false`, `absent: true` и допустимые error-union
  проверяются при десериализации отдельно для list/put/delete.
- Неизвестные поля durable `item` сохраняются в flatten-map. Это повторяет
  Zod-проверку исходного объекта без подмены его результатом и последующий
  `snapshotItem({ ...item })`; новые локальные записи создаются без расширений.
- Durable timestamps принимают все JSON-представления целого JS number
  (`1`, `1.0`, `1e0`), но отклоняют дробные, отрицательные и значения выше
  `Number.MAX_SAFE_INTEGER`. Явный `null` для optional `note`/`category`
  отклоняется, как в Zod `.optional()`.
- Проверка пустой заметки использует `hoh_util_values::js_trim`: BOM `U+FEFF`
  считается пробелом JavaScript, а C1 control `U+0085` — нет.

## Круг исправлений review 202802

- Durable `version` принимает специальный lowercase max-UUID
  `ffffffff-ffff-ffff-ffff-ffffffffffff`, как `z.uuid()` исходника. Ветка
  ограничена точным max-значением: соседний `…fff0` остаётся malformed и
  закреплён отрицательным тестом.

## Не перенесено

- Typert transport и его reflection metadata исключены по §8. Remote-имена
  при этом сохранены в обязательных source-маркерах и защищены тестом. Близкие
  TS-проверки объединены в 22 Rust-теста. Матрицы трёх live-checkpoint mismatch
  и malformed durable payload сохранены внутри соответствующих Rust-тестов.
  Дополнительно проверены passthrough ошибок cold read/flush, закрытие handle,
  отсутствие публикации cold-сессии в `Sessions` и точный JSONL-суффикс
  put/delete после перезапуска.
- Ослабленный drain-тест восстановлен до исходного сценария: две операции
  принимаются до disposal, поздняя отклоняется, а после освобождения read обе
  завершаются с точными счётчиками read/append/close `2`. Реализация очереди
  уже поддерживала этот сценарий; дефектом была потеря тестового оракула.

## Не проверено

- Нет. `cargo test --offline -p hoh-message-feedback` — 22/0;
  `cargo clippy --offline -p hoh-message-feedback --all-targets --no-deps --
  -D warnings` и `cargo fmt --check --package hoh-message-feedback` чисты.

## Вопросы ведущему

- `SHIM(hoh-command-feedback)`: крейт ещё отсутствует. В `src/shim.rs`
  временно повторены публичные `FeedbackCategory` и `FEEDBACK_CATEGORIES` из
  TS-образца. После появления владельца заменить shim на path-зависимость и
  re-export его типов.
