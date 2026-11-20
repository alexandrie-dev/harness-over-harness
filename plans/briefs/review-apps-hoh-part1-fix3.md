Ты независимый ревьюер, только чтение. Ничего не запускай (тесты, сборки, браузеры, серверы). Пиши по-русски.

Проект HoH: порт DeepSeek Harness (TS, эталон /mnt/toshiba/alexandrie/hoh/refs/deepseek-harness) на Rust,
рабочее дерево /home/save/Alexandrie/HarnessOverHarness. Проверка закрытия круга fix3 части 1 склейки.
Прошлое ревью: /mnt/toshiba/alexandrie/hoh/pipeline/reviews/apps-hoh-part1.170744-fix2.md (замечания 1–4).
Отчёт исполнителя: /mnt/toshiba/alexandrie/hoh/logs/sg1-fix3.glue.sonnet.md.
Изменённые файлы: apps/hoh/{PORT_NOTES.md,src/profile_boot.rs,src/window_glue.rs,tests/window_profile.rs,
tests/exit_during_boot.rs}, crates/hoh-sdk-jsonrpc-server/{PORT_NOTES.md,src/lib.rs,tests/plugin_apply.rs}.
Сервер уже принят и закоммичен: оцени, не нарушает ли новый хук `with_exiting` его поведение и порядок
(flush → exiting → dispose корня → exit) и нет ли нового пути, где выход с кодом 0 скрывает настоящий сбой.

Для каждого замечания 1–4: закрыто / нет, file:line, упал бы новый тест на старом коде. Затем новые дефекты.
Каждое замечание: сценарий, отклонение, доказательство, уровень (блокер / ошибка / заметка).

Последняя строка строго: `ВЕРДИКТ: ЧИСТО` или `ВЕРДИКТ: ЗАМЕЧАНИЯ`.
