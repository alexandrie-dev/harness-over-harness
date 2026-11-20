# Исследование HOH-31: OmniBrowser как браузер помощника (только чтение)

Ты — исследователь, ТОЛЬКО ЧТЕНИЕ. Чужой репозиторий `/home/save/Work/OmniBrowser` не менять, ничего в нём не собирать и не запускать, КРОМЕ `/home/save/Work/OmniBrowser/zig-out/bin/lightpanda help <команда>` (и `lightpanda help` без аргумента). Не запускай cargo, npm, браузеры, серверы; ничего не устанавливай и не пиши на диск — результат в ответ.

Слова основателя: «предлагаю заменить headless chrome на Omnibrowser, он у меня на компе есть». Это его сборка Lightpanda (Zig, AGPL-3.0-only, без Chromium и без движка раскладки), бинарь `zig-out/bin/lightpanda` (11.11), команды `serve` (CDP по WebSocket), `mcp` (MCP по stdio или HTTP), `fetch`, `agent`, `run`; описание `OMNI.md`.

У нас: контракт `crates/hoh-browser-use` (65 строк TS-источника; исходник dsh — колонка 3 `docs/porting/MAP.tsv`, ref-дерево `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/browser-use/browser-use`), MCP-клиент `crates/hoh-mcp-client`, менеджер `crates/hoh-mcp-manager`; экспериментальные пакеты dsh с Chrome (`packages/experimental/browser-use-chrome-devtools-mcp`, `browser-use-playwright-mcp`, `browser-use-runtime`, `browser-use-stagehand-native` — в MAP «later»). Правила зависимостей — `docs/porting/DEPS.md`; распространение HoH закрытым бинарником — карточка HOH-03 (см. `PLAN.md`/`plans/vision-1118.md`, если есть).

Вопросы (коротко, по пунктам, с путями файл:строка):
1. Как подключить OmniBrowser за контрактом hoh-browser-use: через его MCP (stdio/HTTP; какие инструменты он отдаёт — по `lightpanda help mcp` и исходникам) с нашими hoh-mcp-client/manager, или через CDP; что из экспериментальных пакетов dsh тогда не нужно.
2. Чего он не умеет из того, что ждёт контракт и сценарии dsh browser-use (снимки экрана — раскладки нет; клики по координатам; формы; JS; загрузки; куки/профили; несколько вкладок) — по фактам из исходников/документации, остальное «не проверено».
3. Лицензия: только факты из `LICENSE` и `LICENSING.md` (и заголовков файлов) — AGPL-3.0-only; варианты для HoH (отдельный процесс, который пользователь ставит сам; поставка вместе с исходниками; иное) без юридических выводов.
4. Оценка в писателе-часах (писатель = одна сессия Codex; ориентир: перенос пакета ~1000 строк TS = 2–4 ч с кругами ревью) и что для этого нужно в нашем движке.
Формат: по-русски, до ~60 строк.
