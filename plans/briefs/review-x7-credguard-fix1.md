# Ревью: credentials-guard, круг 1 (x7, 20.11 02:15)

Ты — независимый ревьюер, только чтение. НЕ запускай тесты, сборки, cargo, браузеры, серверы.

Требование (п.3 `/mnt/toshiba/alexandrie/hoh/logs/lead1118/x7-postmilestone.txt`): в профиле window модель не может читать, перечислять, записывать хранилище движка DSH_HOME (там `.credentials.yaml` с ключом) ни fs-инструментами, ни bash, в т.ч. по симлинку и после эскалации; движок сам доступ сохраняет; рабочая папка работает как прежде.
Задание круга: `/mnt/toshiba/alexandrie/hoh/logs/lead1118/x7-credguard-fix1.txt`. Прошлое ревью: `/mnt/toshiba/alexandrie/hoh/pipeline/reviews/x7-postmilestone.review.md`. Отчёт писателя: `/mnt/toshiba/alexandrie/hoh/logs/x7.credguard-fix1.report.md`.
Код — незакоммиченные изменения (`git diff`) в `crates/hoh-bash-sandbox`, `crates/hoh-fs-sandbox`, `crates/hoh-sandbox-local`, `crates/hoh-sandbox-policy`, `crates/hoh-sandbox`, `crates/hoh-tool-fs/tests`, `apps/hoh` (tests/window_profile.rs, PORT_NOTES.md, assets/bundle/window/cordis.patch.yml). Прочие изменения дерева чужие.

Проверь:
1. Закрыты ли замечания прошлого ревью (hardlink — допустимо как записанное ограничение; DSH_HOME∩workspace; PORT_NOTES hoh-sandbox).
2. Поведение при пересечении DSH_HOME и рабочей папки: писатель закрывает только `.credentials.yaml` и `.env`. Что ещё лежит в DSH_HOME в профиле window (см. `docs/employee-settings-facts.md` п.2, `apps/hoh-desktop`, `apps/hoh` — `profiles/window/model-settings.json`, `window-rules.json`, `cordis.patch.yml`, `sessions/`, другие)? Может ли модель при пересечении прочитать чужие разговоры или ЗАПИСАТЬ настройки/правила/профиль движка и этим расширить свои права или сменить модель/адрес (утечка ключа на чужой адрес)? Оцени, что безопаснее: закрывать весь известный служебный состав или отказывать в такой рабочей папке при старте сессии с понятной ошибкой. Укажи конкретно.
3. Нет ли новых обходов (относительные пути, `..`, /proc/self/root, `find`/`tar`, перенаправления, подстановки, отсутствующий файл, создаваемый позже, гонки canonicalize).
4. Регрессы hoh-tool-fs: восстановлено ли прежнее поведение отказа песочницы без ослабления тестов.

Формат: каждое замечание — сценарий, отклонение, доказательство (файл:строка), что нарушено; пометь дефект / нарушение требования / рекомендацию. Последняя строка: `ВЕРДИКТ: ЧИСТО` или `ВЕРДИКТ: ЗАМЕЧАНИЯ`.
