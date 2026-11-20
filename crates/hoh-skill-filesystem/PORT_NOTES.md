# hoh-skill-filesystem ← packages/skill/skill-filesystem @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs`, `src/frontmatter.rs`, `src/watch.rs` | перенесён |
| `tests/skill-filesystem.spec.ts` | `tests/skill_filesystem.rs`, unit-тесты модулей | наблюдаемые контракты сгруппированы в integration- и unit-тестах; отдельные регрессии покрывают malformed YAML, native invalid UTF-8, abort и host mutation |
| `tests/skill-filesystem-watcher.spec.ts` | `tests/skill_filesystem.rs`, `src/watch.rs` | сценарии перенесены как integration- и deterministic mode/filter/lifecycle tests, включая child-symlink и LRU без открытого watcher |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |

## Отличия от образца

- `yaml`/`serde_yaml` отсутствуют в workspace и `Cargo.lock`; новая
  внешняя зависимость запрещена. `frontmatter.rs` разбирает используемый
  skill-контракт: top-level scalars, YAML booleans/numbers, quoted strings,
  comments, flow sequences/maps, `|`/`>` с chomping, одноуровневые scalar
  block sequences (включая YAML indentationless form), metadata maps с
  произвольным единым отступом, многострочные plain/quoted scalars с YAML-свёрткой,
  plain-скаляры с апострофами/комментариями и CRLF. Anchors, tags, aliases, complex
  keys и вложенность глубже одного уровня намеренно отклоняются как invalid YAML.
  Редкие YAML-only escapes двойных кавычек (`\\x`, `\\e` и литеральный tab)
  также не входят в подмножество; JSON-совместимые escapes поддержаны. Для
  полного синтаксиса YAML
  без общего YAML crate нельзя точно воспроизвести их семантику.
- Chokidar заменён уже закреплённым в workspace `notify`: watcher
  канонизирует существующий root; для отсутствующего root polling наблюдает
  только следующий компонент пути (не всё дерево предка). События фильтруются
  до flat `.md` и `<dir>/SKILL.md`; stability debounce перезапускается каждым
  write-event, runtime error помечает watcher для замены. Abort provider control
  закрывает handles и поздние callbacks. Чтобы воспроизвести `depth: 1`, native
  watcher открывается non-recursive для корня и его непосредственных каталогов;
  при `watchFollowSymlinks` это включает непосредственные symlink-каталоги.
  На Linux `notify` маркирует add/unlink непосредственного symlink-каталога как
  file-event; при `watchFollowSymlinks:true` watcher хранит множество известных
  directory-like entries, чтобы воспроизвести chokidar `addDir`/`unlinkDir`.
  Та же проверка depth и `.system` применяется до классификации, поэтому
  каталоги ресурсов и отключённые системные skills не дают лишних invalidation.
- Корни отдельно регистрируются для синхронной host-инвалидации даже при
  `watch:false` и ошибке открытия watcher. Project-root записи участвуют в том
  же LRU `watchMaxProjects`, поэтому отключённый watcher не оставляет
  неограниченный список путей.
- TypeScript structural actor `{name:'write'|'edit'}` в Rust сопоставляется с
  владельцем `hoh_tools::ToolExecution`, которого передают Rust fs-tools.

## Не перенесено

- ESM exports и npm README assets не имеют runtime-аналога в Rust.
- Chokidar-specific injected callback spies не экспонируются `notify`; их
  наблюдаемые canonicalization, filtering, debounce, recovery, teardown и
  missing-root contracts покрыты через Rust integration и pure mode tests.
- Полный синтаксис YAML вне явно перечисленного подмножества выше; файлы с
  ним изолированно пропускаются, не скрывая корректные соседние skills.

## Вопросы ведущему

- Нет; новых external crates и правок чужих API нет.
