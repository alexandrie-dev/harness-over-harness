# hoh-destructive-classifier — нет образца в dsh

Общий классификатор опасных действий (HOH-18, часть 1, пункт c). Логика вынесена без изменений
из `hoh-window-destructive-guard`: файлы `classify.rs`, `shell.rs`, `text.rs`, `fs.rs` перенесены
целиком, тексты вопросов те же. Правила и принцип описаны в
`crates/hoh-window-destructive-guard/PORT_NOTES.md`.

## Файлы
| было | стало | состояние |
|---|---|---|
| `hoh-window-destructive-guard/src/classify.rs` | `src/classify.rs` | без изменений |
| `hoh-window-destructive-guard/src/shell.rs` | `src/shell.rs` | без изменений |
| `hoh-window-destructive-guard/src/text.rs` | `src/text.rs` | без изменений |
| `hoh-window-destructive-guard/src/fs.rs` | `src/fs.rs` | без изменений |
| `decide` из `hoh-window-destructive-guard/src/lib.rs` | `decide_command` в `src/lib.rs` | то же поведение; страж вызывает его |

## Что добавлено
- Служба `destructiveClassifier` (`DestructiveClassifierService`, плагин
  `DestructiveClassifier`, без настроек) — для пакетов и гнезда внешних исполнителей:
  - `command(command, cwd)` — вопрос для команды оболочки или `None` (то же, что страж);
  - `command_risks(command, cwd)` — список рисков без текста вопроса;
  - `deletion(paths, cwd)` — удаление перечисленных файлов (всегда вопрос, если список не пуст);
  - `overwrite(paths, cwd)` — вопрос, только если какой-то из путей уже существует.
- Тексты для `deletion` / `overwrite` строятся тем же `question()` из `text.rs`.
- `command_reach(command) -> Option<CommandReach>` (`src/reach.rs`, ревью 214304 п. 1): куда
  команда достаёт — все слова, которые могут быть путями (аргументы, значения флагов и
  присваиваний, цели перенаправлений, путь программы), и сетевые программы. `None` — строка не
  разобрана наверняка (переменные, подстановки, шаблоны, `~`, `cd`, обёртки `sudo`/`env`/`xargs`,
  код в аргументах `sh -c`/`python -c`/`awk`/`find -exec`, `sed` кроме `-n '<строки>p'`).
  Использует тот же `shell::parse`, `KEYWORDS` и `is_assignment` из `classify.rs` (стали
  `pub(crate)`, поведение не менялось). Пути не проверяет: это делает вызывающий (слот
  внешних исполнителей сверяет их с папкой сессии со ссылками). Страж окна её не вызывает.

## Отличия от образца
Образца в dsh нет. Поведение стража не изменилось: его тесты (`blocking`, `table`) проходят
без правок.

## Папка в тексте вопроса
Заголовок и объяснение называют папку полным путём, домашний каталог — `~`
(«Удалить 2 файла: … (папка ~/Documents/HoH)…»), а не двумя последними именами
(«Documents/HoH», замечание QA окна P2, отчёт u1). Путь вне дома — целиком (`/srv/…`);
`/home/u2` при доме `/home/u` не сокращается; дом `/` не используется. Тесты: модульный
`the_folder_is_named_by_its_whole_path_with_home_as_tilde` (`text.rs`) и проверка заголовка в
`tests/service.rs`. Длина по-прежнему ограничена пределами заголовка (600) и объяснения (2000).
Это меняет текст и вопроса стража окна (тот же `question()`).

## Слова для людей без опыта (HOH-35, 20.11)
`text.rs`: заголовок начинается с действия, объяснение не повторяет заголовок, «песочница» убрана
(правки 15 и 17 отчёта `research/ux-noncoder-audit.md`). Классификация не менялась. Подробно и
с тестами — в `hoh-window-destructive-guard/PORT_NOTES.md`, раздел «Слова для людей без опыта».

## Один источник правил
Страж окна больше не держит своей копии: в `hoh-window-destructive-guard/src` остался только
`lib.rs` (плагин и реэкспорт), его `decide` — одна строка вызова `decide_command`. Правило,
исправленное здесь, действует и в окне, и в гнезде внешних исполнителей. Поэтому правка этого
крейта входит в один пакет с `hoh-window-destructive-guard` (его `Cargo.toml`, `src/lib.rs`,
`PORT_NOTES.md`): без пакета стража в основном дереве останется вторая копия правил.

## Тесты
- `tests/service.rs` — точки входа службы на настоящей папке: команда, удаление, перезапись.
- `tests/reach.rs` — `command_reach`: что не разбирается, какие слова-пути и какая сеть
  находятся (строки ревью 214304: `make -C /etc`, `git -C /x`, `tar -C /`, `cat ~/…`, `curl -T ~/…`).
- Таблица случаев разбора (`hoh-window-destructive-guard/tests/table.rs`, `blocking.rs`) осталась
  у стража и проверяет ту же логику через `decide` → `decide_command`.
- `deletion` / `overwrite` в работе гнезда — сценарии `hoh-external-harness/tests/harness.rs`.
