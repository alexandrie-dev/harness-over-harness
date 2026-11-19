# WebKitGTK probe — 2026-11-18

**PASS в настоящем окне Tauri 2.12.5 под xvfb-run.** Страница сообщила runtime-версию WebKitGTK **2.52.6** и три положительных результата. Процесс завершился с кодом 0. Сырой JSON: webkitgtk-tauri-result.json; независимая системная проба: webkitgtk-system-result.json.

| Возможность | Проверка | Результат |
|---|---|---|
| oklch() | CSS.supports + computed style + canvas pixel | PASS: oklch(0.6 0 0), [128,128,128] |
| color-mix() | CSS.supports + computed style + canvas pixel | PASS: color(srgb 0.5 0 0.5), [128,0,128] |
| @layer | Второй объявленный слой должен победить первый независимо от порядка правил | PASS: rgb(0,128,0) |

Версия получена из трёх getter загруженной libwebkit2gtk-4.1, передана странице через query и включена в её JSON. Значение AppleWebKit/605.1.15 в User-Agent не используется как версия WebKitGTK.

Проба использует только наши HTML/JS/CSS. Для неё временно переопределён frontendDist на probe-dist, capability main пуста, CSP совпадает с конфигурацией оболочки. Это доказывает запуск Tauri и поддержку CSS, но не сборку React/Tailwind. Производственная ui/dist и Chromium reference snapshot **не выполнены**: обязательный npm gate отклонил picomatch 4.0.7, high GHSA-fc5q-39xc-vjpf; архивы npm не получены.

Воспроизведение из корня после получения Rust-архивов:

```sh
python3 ui/scripts/stage-native-probe.py
TAURI_CONFIG='{"build":{"frontendDist":"probe-dist"}}' CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui2 CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home-ui2 python3 ui/scripts/offline.py cargo build --locked --offline --manifest-path apps/hoh-desktop/Cargo.toml
apps/hoh-desktop/scripts/run-probe.sh
```

Запускатель создаёт временные HOME/XDG-каталоги, отдельные xvfb и D-Bus, удаляет временные данные; seccomp запрещает IPv4/IPv6 для всех дочерних процессов. Sandbox WebKit не отключался. Окно не подключается к движку. Результат выводится строкой HOH_PROBE_RESULT; любой CSS false даёт код 2.

Обычный Xvfb на этой машине падал в NVIDIA/EGL ещё до открытия окна. Для пробы выставлены LIBGL_ALWAYS_SOFTWARE=1, __GLX_VENDOR_LIBRARY_NAME=mesa, __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json, WEBKIT_DISABLE_COMPOSITING_MODE=1 и -extension GLX. Это параметры headless-проверки, не настройки продукта. Предупреждения portal/FUSE/PipeWire не помешали открытию окна и выполнению страницы.

Длительные паузы сборки включали SIGSTOP от глобального cargo-governor; лимитер не отключался. Финальная сборка выполнена с двумя jobs. Временная приватная копия индекса удалена, исходный индекс в заданном CARGO_HOME восстановлен.
