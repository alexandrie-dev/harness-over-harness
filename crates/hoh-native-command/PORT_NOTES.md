# hoh-native-command ← packages/util/native-command @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | все экспорты |
| src/runner.ts | src/runner.rs | запуск argv без оболочки, потоки, коды, отмена, maxBuffer |
| src/host-environment.ts | src/host_environment.rs | platform/env/uname и WSL |
| src/path-opener.ts | src/path_opener.rs | все платформы, браузер, три намерения, reveal, Explorer/WSL |
| src/desktop-entry.ts | src/desktop_entry.rs | поля секции, XDG, PNG/SVG |
| src/file-applications-linux.ts | src/file_applications_linux.rs | GIO, локали, вложенные desktop-id, иконки |
| src/file-applications-windows.ts | src/file_applications_windows.rs + src/windows_associations.cs | PowerShell STA, исходный C# без изменений, BOM/base64 и очистка |
| src/file-applications.ts | src/file_applications.rs + src/mac_applications.js | запрос/проверка обработчика, исходный JXA, дедупликация macOS |
| src/types.ts | src/types.rs | serde-тип и строгий парсер метаданных |
| tests/host-environment.spec.ts | tests/host_environment.rs | 2 объявления, 8 параметризованных случаев |
| tests/native-command.spec.ts | tests/native_command.rs | 4 случая + 2 собственных |
| tests/path-opener.spec.ts | tests/path_opener.rs | 38 объявлений, 60 параметризованных случаев |
| tests/file-applications.spec.ts | tests/file_applications.rs | 13 объявлений, 18 параметризованных случаев |
| tests/file-applications-linux.spec.ts | tests/file_applications_linux.rs | 4 случая + 1 собственный |
| tests/file-applications-linux-native.spec.ts | tests/file_applications_linux_native.rs | настоящий GIO, закрытые XDG-каталоги, проверка пути и завершения PID |
| tests/file-applications-windows.spec.ts | tests/file_applications_windows.rs | Windows-only, компиляция приватного обработчика, регистрация/очистка и проверка PID |

Итого: перенесены 63 объявления it (96 случаев после разворачивания it.each),
не перенесено 0. На Linux выполняются 65 Rust-тестов; циклы сохраняют наборы входов
параметризованных тестов. Один Windows-only тест на Linux исключается cfg.
locale и отдельного LICENSE у исходного пакета нет.

## Отличия от образца
- По прямому заданию манифест не содержит HoH crates. Поэтому CommandSignal —
  только трейт-адаптер отмены вызывающего кода, без собственного состояния и без
  CancellationToken. Приложение должно связать его со своим общим AbortSignal.
  Причина команды сохраняется в cause; проверка сигнала при WSL/Explorer возвращает
  ошибку адаптера. JS-идентичность объекта ошибки в Rust не является частью API.
- NativeCommandRunner/PathOpenerRunner — object-safe трейт с boxed Future;
  необязательные аргументы передаются как &PathOpenerInternals::default().
  NativeFileManager и Window — enum вместо строк. Коды — CommandCode::Exit/Name.
  NativeCommandError, CommandOutput, CommandFuture, CommandSignal, SystemCommandRunner,
  Window, CommandCode, Environment и функции platform/os_release — вспомогательные
  публичные символы для типизированного адаптера и тестирования. Перевод имён полей
  Rust в snake_case не меняет JSON NativeFileApplication (id/name/default/icon).
- Модульные моки Node execFile заменены инъекцией NativeCommandRunner; настоящий
  runner отдельно проверен на stdout/stderr, ENOENT, exit 3, отмене и лимите 1 МиБ.
  Тест default reveal использует pre-abort перед реальным desktop-запуском и
  проверяет сам handoff инъекцией. Проверки Node-объектов заменены полями Rust-ошибки.
  Фикстуры Node заменены sh на Unix и PowerShell/C# на Windows; оболочка запускается
  только явно в тестовой фикстуре, библиотека никогда не добавляет её к командам.
- Разбор JSON ответов приватных macOS/Windows-хелперов — serde_json: валидные
  метаданные сохраняются, текст синтаксической ошибки и лимит вложенности отличаются
  от JSON.parse V8. HoH JSON-утилиты исключены запретом HoH-зависимостей задания.
  Нестандартные системные ошибки ОС могут иметь текст Rust вместо Node/libuv.
- На Windows Hidden использует безопасный CREATE_NO_WINDOW для консольных
  помощников; Visible не задаёт флаг. Это не Win32 STARTUPINFO/SW_HIDE из Node:
  скрытие произвольного GUI-процесса требует другой безопасной обёртки. Explorer
  получает Visible, как в образце. Фактическая видимость на Windows не проверена.
- homedir при отсутствии HOME получается стандартной безопасной функцией Rust;
  пакет образца не импортирует util/home-paths, новых HoH-зависимостей нет.
  На non-Unix os_release возвращает пустую строку: WSL определяется только на Linux;
  специально подставленный platform=linux на Windows требует os_release override.

Из переносимых потребителей только app-boot/bundle-platform импортирует isWsl и
HostEnvironmentFacts. Остальные экспорты вне исключённых client/api/host/typert/
experimental-пакетов не используются; все они всё равно перенесены.

## Не перенесено
Нет. Ветки не вызывают пакеты этапа 5: C# Shell-хелпер является исходным
текстом внешнего PowerShell-процесса, а не Rust unsafe или зависимостью pwsh-*.
Зависимости берутся только из workspace.dependencies; чужие crates не менялись.

## Не проверено
- Компиляция и выполнение под Windows: PowerShell 5.1, C# COM/Shell, STA message
  loop, indirect icons, private registry fixture, фактическое окно/делегирование
  Explorer и завершение Windows-процессов. Windows-only тест перенесён, но здесь
  не запускался. WSL проверен инъекцией; реального WSL/Windows desktop здесь нет.
- Настоящие macOS LaunchServices/AppKit/JXA и open/defaults. Командные адаптеры,
  разбор и дедупликация протестированы на Linux по исходным фикстурам.
- Произвольные относительные Windows-пути с drive-relative окружением (C:foo),
  IDN-имена UNC и редкие правила нормализации Node pathToFileURL вне исходных
  наборов входов. Обычные drive/UNC, verbatim/device, Unicode, пробелы, %, #,
  запятые, равенство и корень диска проверены исходными тестами.
- Крайний случай лимита maxBuffer посередине многобайтного символа: реализация
  ограничивает байты и декодирует UTF-8 с заменой, как обычный вывод Node; точная
  обрезка Node для этого случая не сверена без запуска эталона.

## Вопросы ведущему
Подтвердить CommandSignal-адаптер для задания без HoH-зависимостей либо разрешить
hoh-timeout/hoh-error и hoh-util-values: это позволит напрямую выполнить общие
правила PORTING §4/§6 для AbortSignal и V8 JSON-диагностики. Недостающего API в
зависимых HoH crates нет — зависимостей от них по заданию нет.
Для Windows позднее нужна сверка безопасного Hidden с Node windowsHide.
