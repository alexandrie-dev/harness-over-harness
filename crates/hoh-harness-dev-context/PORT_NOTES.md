# hoh-harness-dev-context ← packages/context/harness-dev-context @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/shim.rs` | плагин, проекция legacy prompt, две секции и два runtime-context provider перенесены |
| `tests/harness-dev-context.spec.ts` | `tests/harness_dev_context.rs`, `tests/production_loop.rs` | все 7 развёрнутых случаев и production AgentLoop-регрессии для всех ветвей resume/consolidation |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | `locale/` | восемь файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `package.json` | `PORT_NOTES.md`, `Cargo.toml` | прочитаны; отдельного LICENSE нет, лицензия пакета MIT |

Сохранены точные имена `harness-dev-context`, `harness:source`, `app:web-surface`,
`harnessDevLegacyPrompt`, порядки секций/контекстов, `interpolate: false`, тексты текущих
и замороженных legacy-параграфов, однострочное распознавание и игнорирование пустой версии.
URL читается при каждой сборке; путь и URL сериализуются общим JavaScript JSON codec.

## Отличия от образца

1. Function-plugin представлен `HarnessDevContext`, `Deps`, `NoConfig` и константами
   `NAME`/`INJECT`. `AssembleContext.agent` представлен общим расширением `AssembleAgent`.
2. `SOURCE_ROOT` вычисляется во время запуска из необязательного `InstallAnchorKey`, который
   launcher задаёт как `<корень>/package.json`; fallback — каталог текущего исполняемого файла.
   Это сохраняет runtime-семантику `fileURLToPath(new URL('../../../..', import.meta.url))` после
   переноса уже собранного бинаря и всегда добавляет платформенный завершающий разделитель.
3. `WebAppApi` и service key `WebApp` — временные публичные Rust-символы из `src/shim.rs`.
   Они сохраняют точное имя сервиса `webApp` и единственный используемый метод
   `get_advertised_url`; убрать после появления crate-владельца `packages/bundle/web-app`.
4. Zod-схема host-only состояния выражена закрытым Rust-типом и ручной проверкой обязательных
   `source`/`web`: принимаются только строка или `null`, неизвестные поля отбрасываются.

## Не перенесено

Нет пропущенных production-экспортов или исходных тестовых случаев. TypeScript declaration
merging заменён kernel service и `AssembleAgent`; npm export `./src/*` не создаёт отдельной Rust-точки входа.

## Не проверено

- Отдельной платформенной ветки в исходнике нет. Windows/macOS не запускались; завершающий
  разделитель runtime-пути выбирается платформой Rust.
- Web bundle отсутствует в Rust по решению MAP (`packages/bundle/web-app` имеет статус `skip`),
  поэтому интеграция с настоящим Tauri/Web host не проверена. Тестовый provider использует тот же service key.
- Оба режима обновления system prompt, консолидация серии, две похожие legacy-формы и свежая
  сессия проверены настоящим AgentLoop и фактическими запросами адаптера. Перенос бинаря между
  checkout не запускался; runtime anchor проверен отдельным значением `/test/install/package.json`.

## Вопросы ведущему

1. Нужен владелец сервиса `webApp` с `get_advertised_url` для удаления SHIM. До этого crate
   корректно остаётся Pending без provider, как исходный plugin при отсутствии injected service.

## Проверка

- `cargo test -p hoh-harness-dev-context --offline`: 11 passed, 0 failed.
- `cargo clippy -p hoh-harness-dev-context --all-targets --no-deps --offline -- -D warnings`: clean.
- `cargo fmt -p hoh-harness-dev-context -- --check`: clean.
