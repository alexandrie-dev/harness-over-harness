# hoh-message-quotes ← packages/session/message-quotes @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | пустой Host `apply()` перенесён без зависимостей и hooks |
| `src/client/quote-text.ts` | `src/client/quote_text.rs` | CR/CRLF normalization, explicit quote recognition и UTF-16 source ranges перенесены |
| `src/client/quote-rule.ts` | `src/client/quote_rule.rs` | atomic quote nodes, adjacent separator replacement и `from` semantics перенесены |
| `src/client/QuoteSelection.tsx`, `src/client/index.ts` | `src/client/selection.rs`, `src/client/mod.rs` | browser-independent admission/action outcomes и точная registration metadata перенесены |
| `src/client/QuotedUserText.tsx` | `UserTextNode::Quote` | decoded excerpt остаётся одним literal `String`, без повторной интерпретации |
| `src/client/locales.ts` | `src/client/locales.rs` | namespace и все 8 строк `quote.add` перенесены |
| `src/client/QuoteSelection.module.css` | `assets/quote-selection.css`, `QUOTE_SELECTION_CSS` | CSS скопирован без изменений и доступен Tauri UI adapter |
| `locale/*.json` | `locale/*.json` | все 8 package metadata locale скопированы без изменений |
| `tests/*.spec.*` | `tests/message_quotes.rs` | pure/core наблюдаемое поведение сведено в 13 Rust-тестов; browser-only исключения ниже |

Из 18 исходных test case 11 полностью адаптированы к Rust core assertions, 7 browser-runtime case не перенесены.

## Отличия от образца

1. В workspace нет Rust-эквивалентов `dsh-client-ui-conversation`, `slots`, React и DOM: эти пакеты имеют
   статус `skip` в `MAP.tsv`, а `ui/` принадлежит другому исполнителю. Поэтому крейт экспортирует чистые
   admission/action/rule данные для Tauri adapter, но сам не регистрирует browser Slots.
2. `MessageQuoteRange` хранит смещения в UTF-16 code units, как JavaScript. Внутренняя конвертация в byte
   offsets допускает только Unicode scalar boundaries; все offsets, порождённые matcher, таким условием обладают.
3. Literal presentation выражена закрытым `UserTextNode::Quote { text }`: содержимое credential/reference/
   Markdown не проходит через decorators внутри этого крейта. Внешнее оформление ordinary text остаётся UI layer.

## Не перенесено

- `browser-plugin.client.spec.tsx` — 1 случай lifecycle поздних/replaced Slot declarations и HMR dispose/reload:
  соответствующие client registries отсутствуют в Rust workspace. Имена, inject, keys, order и priority
  сохранены константами; admission покрыт pure-тестом.
- `quote-user-text.client.spec.tsx` — 5 случаев DOM decorators/popover и identity cache неизменной оценки:
  `UserTextRegistry`, secret mask, reference chips и QuotePreview принадлежат пропущенным client-ui пакетам и
  Tauri presentation. Literal quote data, nested markers и точные source ranges проверены без DOM.
- `quote-selection.client.spec.tsx` — 1 случай stateful React rerender «dismissed surface не открывается
  снова» принадлежит владельцу component state;
  крейт выдаёт `dismiss=true` при invalidation, повторное открытие из core не инициирует.

## Не проверено

- Реальный DOM selection, pointer events, React rendering и Slot replacement; для них в Rust crate нет runtime.
- Интеграция `QUOTE_SELECTION_CSS` и exported outcomes с Tauri UI — обязанность UI adapter.

## Вопросы ведущему

- Для включения controls в окно нужен glue в принадлежащем другому исполнителю `ui/`; чужой API не менялся.
