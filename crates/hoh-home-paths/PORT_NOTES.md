# hoh-home-paths ← packages/util/home-paths @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/home-paths.spec.ts | tests/home-paths.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- Пути — PathBuf, функции возвращают io::Result, ошибки ОС остаются стандартными io::Error. Node join воспроизведён объединением сегментов до нормализации: абсолютный child не сбрасывает home. `~\\` расширяется и на Unix. Whitespace окружения использует набор ECMAScript trim.
- DshHomeCategory — enum с lowercase serde; DshPathLocation сохраняет category/dshHome, отсутствие dshHome опускается.
- Variadic segments заменены &[&str], overload cache — явным Option<&str> dsh_home. Необязательное окружение во всех path helpers явно передаётся как Option<&HashMap<String,String>>; None читает process environment, Some(empty) означает пустое окружение. Этот дополнительный аргумент позволяет проверять исходные env-сценарии без изменения глобального окружения.
- На Unix явно читается HOME: установленный пустой HOME возвращает пустую строку, fallback std::env::home_dir/passwd используется только при отсутствии HOME. Windows использует std::env::home_dir (USERPROFILE/профиль). При отсутствии OS home возвращается явная ошибка. default_dsh_home/default_agents_home используют нормализующий join, как Node path.join, включая повторные разделители и пустой HOME (относительные .dsh/.agents).
- join игнорирует пустые сегменты при определении завершающего разделителя; join('/a','b/','') сохраняет /a/b/. Display сравнивает точную OS-строку, поэтому завершающий разделитель не совпадает с default root. ENOENT из opendir внутри canonicalize обрабатывается тем же подъёмом к родителю, что ENOENT из realpath; проверено отдельной детерминированной инъекцией.
- Дополнительные тесты проверяют env:None с полностью контролируемым окружением дочернего test executable, нормализацию/пустой HOME на Unix, ~/ и ~\\ с пустым остатком, Some("") configured, trailing separators и точное сравнение display. Ignored helper home_environment_child ничего не делает без marker от родителя; реальные пользовательские каталоги в child-тестах не используются.
- Все 15 it-групп сохранены, параметризованные cache/category случаи исполняются внутри тестов.

## Не перенесено
Нет. Каталога locale нет.

## Не проверено
Windows/macOS не запускались. Windows drive-relative/UNC/длинные пути и native canonicalize-префикс требуют платформенной сверки с Node; symlink-тест на Windows требует права создавать symlink. Ошибки чтения запрещённых каталогов не инъецированы.

## Вопросы ведущему
Нужна платформенная сверка Windows path/canonicalize (включая extended-length prefix).

