# hoh-native-command ← packages/util/native-command @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | все экспорты |
| src/runner.ts | src/runner.rs | argv без оболочки, UTF-8, отмена, коды/тексты ошибок, maxBuffer |
| src/host-environment.ts | src/host_environment.rs | platform/env/uname и WSL |
| src/path-opener.ts | src/path_opener.rs | все платформы, браузер, три намерения, reveal, Explorer/WSL |
| src/desktop-entry.ts | src/desktop_entry.rs | поля секции, XDG, PNG/SVG |
| src/file-applications-linux.ts | src/file_applications_linux.rs | GIO, локали, вложенные desktop-id, иконки |
| src/file-applications-windows.ts | src/file_applications_windows.rs + src/windows_associations.cs | PowerShell STA, исходный C#, BOM/base64 и очистка |
| src/file-applications.ts | src/file_applications.rs + src/mac_applications.js | запрос/проверка обработчика, исходный JXA, дедупликация macOS |
| src/types.ts | src/types.rs | serde-тип и строгий парсер метаданных |
| tests/host-environment.spec.ts | tests/host_environment.rs | 2 объявления, 8 параметризованных случаев |
| tests/native-command.spec.ts | tests/native_command.rs | 4 исходных случая и дополнительные проверки |
| tests/path-opener.spec.ts | tests/path_opener.rs | 38 исходных объявлений, 60 параметризованных случаев, приватный процесс для реального default runner |
| tests/file-applications.spec.ts | tests/file_applications.rs | 13 исходных объявлений, 18 параметризованных случаев |
| tests/file-applications-linux.spec.ts | tests/file_applications_linux.rs | 4 исходных случая и проверка desktop-полей/иконок |
| tests/file-applications-linux-native.spec.ts | tests/file_applications_linux_native.rs | настоящий GIO, закрытые XDG-каталоги, путь и завершение PID |
| tests/file-applications-windows.spec.ts | tests/file_applications_windows.rs | Windows-only, приватный обработчик, регистрация/очистка и PID |
| дополнительные случаи fix brief | tests/review_regressions.rs | whitespace, GIO regex, порядок readdir, пути, 136 Explorer targets, тексты/форма ошибок, OS env, JS JSON |

Перенесены все исходные объявления it: 38 + 13 + 4 + 2 + 4 и две
платформенные интеграционные проверки; ни одно не исключено из переноса.
Параметризованные строки сохранены циклами. Linux-интеграция выполняется при
наличии GIO; Windows-интеграция исключается на Linux через cfg. locale и отдельного
LICENSE у исходного пакета нет.

## Отличия от образца
- NativeCommandRunner/PathOpenerRunner — object-safe трейт с boxed Future;
  необязательные аргументы передаются как &PathOpenerInternals::default().
  NativeFileManager и Window — enum вместо строк. Коды — CommandCode::Exit/Name.
  NativeCommandError — обычная ошибка, не HarnessError: наличие поля code
  не меняет исходный базовый класс Error.
- По исправленному решению ведущего используется hoh_timeout::AbortSignal.
  Собственного CommandSignal и собственного состояния отмены больше нет.
  Границы вызовов и парсеры возвращают hoh_error::SharedError: причина отмены и
  проброшенная ошибка runner сохраняют ptr_eq. Возврат исходной причины вместо
  Node execFile AbortError-обёртки — прямое требование fix brief, пункт 1.
  NativeCommandError доступна через downcast_ref; её stdout/stderr — Option:
  Some("") у ошибок callback ENOENT/EACCES и None у синхронного отказа ENOTDIR.
  signal хранит имя завершившего процесс сигнала, code=None соответствует null.
- Вспомогательный публичный API Rust: NativeCommandError (message/io),
  CommandOutput, CommandFuture, SystemCommandRunner, Window, CommandCode,
  Environment, platform/os_release и environment_from_os. Последняя функция —
  seam для vars_os: ключи и значения переводятся to_string_lossy, не меняя
  окружения процесса в тестах. JSON NativeFileApplication сохраняет имена
  id/name/default/icon.
- Поведение среды не реализуется заново: whitespace и js_json_parse берутся
  из hoh-util-values, POSIX join/relative/dirname/extname/is_absolute — из
  hoh-util-workspace-path::node_path, errno и тексты fs/spawn — из hoh-error.
  На Unix readdir сортируется по байтам имени перед проходами «файлы, каталоги».
  Тексты JXA/C#, шаблон PowerShell, argv хелперов, порядок иконок и macOS
  дедупликация сохранены. Создание и удаление временного каталога PowerShell
  выполняются через spawn_blocking, запись файла — tokio::fs.
- Входы Windows-opener: абсолютные Windows drive/UNC-пути и абсолютный результат
  wslpath -w. Все 136 строк неизменённой ведущей фикстуры
  tests/fixtures/explorer_target.jsonl проверяются через публичный open_native_path
  с platform=win32. Фикстура снята на POSIX-хосте: drive root C:\ имеет target
  file:///C://; cfg(windows) сохраняет исходное host-зависимое file:///C:/.
  Относительные и drive-relative пути требуют окружения Windows process.cwd()
  и не считаются проверенными.
- Намеренное отличие exec: файл без строки интерпретатора Node запускает через
  /bin/sh (execvp), а Rust Command отказывает с ENOEXEC. Это существенно для
  BROWSER, если он указывает на такой скрипт; автоматическая оболочка не добавляется.
  stdin дочернего процесса в переносе закрыт (Stdio::null), а у Node это открытая
  труба, в которую никто не пишет.
- На Windows Hidden использует безопасный CREATE_NO_WINDOW для консольных
  помощников, Visible не задаёт флаг. Это не STARTUPINFO/SW_HIDE из Node:
  скрытие произвольного GUI-процесса требует другой безопасной обёртки.
  Explorer всегда получает Visible, включая WSL. Фактическая видимость
  Windows-окон здесь не проверена.
- homedir при отсутствии HOME получает стандартная безопасная функция Rust;
  исходный пакет не импортирует util/home-paths. На non-Unix os_release возвращает
  пустую строку: обычная WSL-детекция работает только на Linux; искусственный
  platform=linux на Windows требует os_release override.

Потребители: из переносимых пакетов только app-boot/bundle-platform напрямую
импортирует isWsl и HostEnvironmentFacts. Прочие экспорты вне исключённых
client/api/host/typert/experimental-пакетов не используются, но перенесены.

Проверки fix brief:
- Для пунктов 5–10 сначала добавлены 11 регрессионных тестов: до исправлений
  1 passed (точный exit 3 уже совпадал), 10 failed. Фикстура Explorer тогда
  выявила 14 несовпадений из 136.
- Проверены BOM/U+0085, JS \s в content type и обоих regex GIO; пять заданных
  строк Registered applications и дополнительные FEFF/0085; последний двоеточие
  preferred и захват с ведущим пробелом; оба порядка создания неоднозначного id;
  reveal("/") и reveal("/a/."); XDG root с //.
- Полные argv/calls, включая defaults read и BROWSER без переменной, проверяются
  целиком. Mock фиксирует адрес переданной ссылки на общий AbortSignal:
  WSL- и file-applications-тесты проверяют, что runner получил именно ссылку
  вызывающего кода. Ошибки Explorer, отмены и отказы launcher проверены ptr_eq.
- В macOS-фикстурах отсутствующие bundle/version не заменяются null:
  script-a имеет отсутствующие поля, script-b — явный null; N/W имеют
  отсутствующую version, V — исходную пустую строку.
- Реальный SystemCommandRunner выполняет /bin/sh -c и получает данные argv
  с пробелами, кавычками, $, Unicode и текстом $(touch nope) без интерпретации.
  Проверены UTF-8 stdout/stderr, точный command failure, sleeping child с
  проверкой завершения PID и ptr_eq причины. Default opener проверяется в
  дочернем тестовом процессе с частным PATH и временными executable-фикстурами
  xdg-open/open/explorer.exe; globals/окружение родительского теста не меняются.
- Случай исчезнувшего desktop-файла воспроизводится без гонки: desktop-запись —
  символьная ссылка на только что удалённую цель. Обнаружение id успешно,
  последующий readFile получает тот же ENOENT/open с точным путём записи.
- Не проверяется тождество Node Error-объекта из execFile: в Rust его нет.
  Эквивалент причины реальной ошибки проверяется как Error/source и точные поля;
  тождество пробрасываемых ошибок и причин отмены проверяется SharedError::ptr_eq.
  Проверка фактического Windows windowsHide остаётся слабее Node-мока: на Linux
  проверены Visible при маршрутизации и реальный запуск с обоими Window, но
  не Win32 STARTUPINFO. Остальные перечисленные review ослабления устранены.

## Не перенесено
Нет исходных файлов/тестов, исключённых из переноса. Ветки не вызывают пакеты
этапа 5: исходный C# Shell-хелпер — текст внешнего процесса, не Rust unsafe.

## Не проверено
- Компиляция и выполнение под Windows: PowerShell 5.1, COM/Shell, STA message
  loop, indirect icons, private registry fixture, видимость/делегирование
  Explorer и Windows termination. Реальный WSL/Windows desktop не запускался.
- Настоящие macOS LaunchServices/AppKit/JXA и open/defaults. Адаптеры, metadata
  parsing и дедупликация проверены на Linux по исходным фикстурам.
- Относительные Windows-пути, drive-relative C:foo, IDN UNC и одиночные UTF-16
  суррогаты вне контракта входных UTF-8 абсолютных путей фикстуры.
- Крайний maxBuffer посередине многобайтного символа отдельно с Node не сверен;
  установленный лимит 1 МиБ и код отказа сохранены.
- Ограничения js_json_parse и платформенные fallback Node error helpers
  наследуются из PORT_NOTES.md соответствующих общих crates.

## Вопросы ведущему
Недостающего API общих crates нет. Прежние вопросы о разрешении HoH-зависимостей
закрыты исправленным brief. Для Windows остаётся будущая сверка Hidden/windowsHide;
за пределы разрешённого Linux-прогона эта работа не выдаётся.
