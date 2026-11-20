# Зависимости окна

Разрешение основателя: пакеты §1 docs/window.md и Tauri 2.12.x. Дополнения ниже нужны для типов и локальных бинарных backend при полном отказе от optionalDependencies. Других прямых зависимостей нет. Потолка числа пакетов нет.

## Машинная политика

```json
{
  "licenses": [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 OR MIT",
    "MIT OR Apache-2.0",
    "ISC",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "OFL-1.1",
    "MPL-2.0",
    "CC0-1.0",
    "0BSD",
    "Apache-2.0 WITH LLVM-exception",
    "Unicode-3.0",
    "Zlib"
  ],
  "direct": {
    "react": "19.3.1",
    "react-dom": "19.3.1",
    "@base-ui/react": "1.9.0",
    "zustand": "5.1.0",
    "lucide-react": "1.71.0",
    "class-variance-authority": "0.7.1",
    "tailwind-merge": "3.7.0",
    "@fontsource-variable/inter": "5.3.0",
    "@legendapp/list": "3.7.2",
    "pdfjs-dist": "6.5.438",
    "uplot": "1.6.32",
    "@tauri-apps/api": "2.12.4",
    "vite": "8.3.7",
    "@vitejs/plugin-react": "6.3.0",
    "tailwindcss": "4.3.5",
    "@tailwindcss/vite": "4.3.5",
    "typescript": "6.0.3",
    "playwright-core": "1.65.0",
    "@tauri-apps/cli": "2.12.4",
    "@types/react": "19.3.0",
    "@types/react-dom": "19.3.0",
    "@types/node": "24.19.2",
    "@rolldown/binding-linux-x64-gnu": "1.2.19",
    "@tailwindcss/oxide-linux-x64-gnu": "4.3.5",
    "lightningcss-linux-x64-gnu": "1.33.0",
    "@tauri-apps/cli-linux-x64-gnu": "2.12.4",
    "oxc-transform-react": "0.156.0",
    "@oxc-transform-react/binding-linux-x64-gnu": "0.156.0",
    "react-markdown": "10.1.0",
    "unified": "11.0.5",
    "remark-parse": "11.0.0",
    "remark-gfm": "4.0.1",
    "remark-math": "6.0.0",
    "micromark-extension-llm-math": "3.1.0",
    "remark-breaks": "4.0.0",
    "remark-rehype": "11.1.2",
    "rehype-raw": "7.0.0",
    "rehype-sanitize": "6.0.0",
    "vfile": "6.0.3",
    "hast-util-to-jsx-runtime": "2.3.6",
    "hast-util-to-html": "9.0.5",
    "html-url-attributes": "3.0.1",
    "mdast-util-from-markdown": "2.0.3",
    "micromark-util-combine-extensions": "2.0.1",
    "micromark-util-types": "2.0.2",
    "micromark-util-character": "2.1.1",
    "micromark-util-decode-string": "2.0.1",
    "micromark-extension-directive": "4.0.0",
    "mdast-util-directive": "3.1.0",
    "property-information": "7.2.0",
    "@types/hast": "3.0.5",
    "@types/mdast": "4.0.4",
    "katex": "0.18.10",
    "dompurify": "3.4.17",
    "pele": "1.8.0"
  },
  "scriptExceptions": {
    "lightningcss@1.33.0": [
      "prepare"
    ],
    "enhanced-resolve@5.26.1": [
      "prepare"
    ],
    "dompurify@3.4.17": [
      "prepare"
    ],
    "katex@0.18.10": [
      "prepare"
    ],
    "inline-style-parser@0.2.7": [
      "prepare"
    ],
    "style-to-js@1.1.21": [
      "prepare"
    ],
    "style-to-object@1.0.14": [
      "prepare"
    ]
  },
  "licenseExceptions": {
    "dompurify@3.4.17": {
      "license": "(MPL-2.0 OR Apache-2.0)",
      "reason": "двойная лицензия, используется ветка Apache-2.0; решение CEO 20.11.2026"
    }
  }
}
```

Разрешённые лицензии перечислены в licenses; выражение OR допускает выбор разрешённой ветки. MPL-2.0 — lightningcss (build tool), OFL-1.1 — шрифт.

Исключение lightningcss@1.33.0: поле prepare=patch-package в опубликованных метаданных. Это подготовка релиза, при установке tarball из npm не нужна; скрипт НЕ запускается. allowBuilds остаётся {}. preinstall/install/postinstall запрещены без исключений.

## Прямые пакеты

| Пакет | Точная версия | Дата UTC | Лицензия | Зачем / почему не свой код |
|---|---|---|---|---|
| `react` | 19.3.1 | 2026-10-14 | MIT | Компоненты оболочки |
| `react-dom` | 19.3.1 | 2026-10-14 | MIT | DOM-рендер React |
| `@base-ui/react` | 1.9.0 | 2026-10-09 | MIT | Доступные элементы управления для следующих экранов |
| `zustand` | 5.1.0 | 2026-11-06 | MIT | Состояние экранов и история |
| `lucide-react` | 1.71.0 | 2026-11-10 | ISC | Только именованные иконки, tree shaking |
| `class-variance-authority` | 0.7.1 | 2024-11-26 | Apache-2.0 | Варианты компонентов |
| `tailwind-merge` | 3.7.0 | 2026-09-12 | MIT | Слияние классов Tailwind |
| `@fontsource-variable/inter` | 5.3.0 | 2026-07-19 | OFL-1.1 | Локальный Inter Variable |
| `@legendapp/list` | 3.7.2 | 2026-11-07 | MIT | Виртуальная лента |
| `pdfjs-dist` | 6.5.438 | 2026-11-01 | Apache-2.0 | Просмотр PDF, canvas запрещён |
| `uplot` | 1.6.32 | 2025-03-14 | MIT | Временные ряды |
| `@tauri-apps/api` | 2.12.4 | 2026-10-14 | Apache-2.0 OR MIT | Типизированный IPC Tauri |
| `vite` | 8.3.7 | 2026-11-09 | MIT | Локальная сборка статических файлов |
| `@vitejs/plugin-react` | 6.3.0 | 2026-11-09 | MIT | Преобразование TSX для Vite |
| `tailwindcss` | 4.3.5 | 2026-11-06 | MIT | Токены и утилиты CSS |
| `@tailwindcss/vite` | 4.3.5 | 2026-11-06 | MIT | Интеграция Tailwind с Vite |
| `typescript` | 6.0.3 | 2026-04-16 | Apache-2.0 | Проверка типов (JS-реализация) |
| `playwright-core` | 1.65.0 | 2026-11-05 | Apache-2.0 | Снимки установленным Chromium без загрузки браузера |
| `@tauri-apps/cli` | 2.12.4 | 2026-10-21 | Apache-2.0 OR MIT | Локальный CLI оболочки |
| `@types/react` | 19.3.0 | 2026-09-09 | MIT | Типы React для tsc, нельзя заменить DOM-типами |
| `@types/react-dom` | 19.3.0 | 2026-09-09 | MIT | Типы ReactDOM для tsc |
| `@types/node` | 24.19.2 | 2026-10-09 | MIT | Типы конфигурации Vite и проверок Node |
| `@rolldown/binding-linux-x64-gnu` | 1.2.19 | 2026-11-11 | MIT | Необходимый бинарный backend Vite, явный Linux x64 glibc пакет при optional=false |
| `@tailwindcss/oxide-linux-x64-gnu` | 4.3.5 | 2026-11-06 | MIT | Сканер Tailwind, явный Linux x64 glibc пакет при optional=false |
| `lightningcss-linux-x64-gnu` | 1.33.0 | 2026-07-20 | MPL-2.0 | CSS backend Vite/Tailwind, явный Linux x64 glibc пакет при optional=false |
| `@tauri-apps/cli-linux-x64-gnu` | 2.12.4 | 2026-10-21 | Apache-2.0 OR MIT | Бинарный CLI Tauri, явный Linux x64 glibc пакет при optional=false |
| `oxc-transform-react` | 0.156.0 | 2026-10-27 | MIT | Обязательный peer plugin-react 6.3; TSX-преобразование |
| `@oxc-transform-react/binding-linux-x64-gnu` | 0.156.0 | 2026-10-27 | MIT | Backend обязательного TSX peer, явный Linux x64 glibc пакет при optional=false |
| `react-markdown` | 10.1.0 | 2025-03-07 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `unified` | 11.0.5 | 2024-06-19 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `remark-parse` | 11.0.0 | 2023-09-18 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `remark-gfm` | 4.0.1 | 2025-02-10 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `remark-math` | 6.0.0 | 2023-09-19 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `micromark-extension-llm-math` | 3.1.0 | 2024-11-24 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `remark-breaks` | 4.0.0 | 2023-09-22 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `remark-rehype` | 11.1.2 | 2025-04-02 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `rehype-raw` | 7.0.0 | 2023-08-26 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `rehype-sanitize` | 6.0.0 | 2023-08-26 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `vfile` | 6.0.3 | 2024-08-27 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `hast-util-to-jsx-runtime` | 2.3.6 | 2025-03-05 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `hast-util-to-html` | 9.0.5 | 2025-02-19 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `html-url-attributes` | 3.0.1 | 2024-09-27 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `mdast-util-from-markdown` | 2.0.3 | 2026-02-21 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `micromark-util-combine-extensions` | 2.0.1 | 2024-11-12 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `micromark-util-types` | 2.0.2 | 2025-02-27 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `micromark-util-character` | 2.1.1 | 2024-11-12 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `micromark-util-decode-string` | 2.0.1 | 2024-11-12 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `micromark-extension-directive` | 4.0.0 | 2025-02-27 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `mdast-util-directive` | 3.1.0 | 2025-01-22 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `property-information` | 7.2.0 | 2026-06-01 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `@types/hast` | 3.0.5 | 2026-07-09 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `@types/mdast` | 4.0.4 | 2024-05-14 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `katex` | 0.18.10 | 2026-09-30 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `dompurify` | 3.4.17 | 2026-10-12 | (MPL-2.0 OR Apache-2.0) | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |
| `pele` | 1.8.0 | 2026-10-20 | MIT | Отображение чата как в T3, установка разрешена основателем 20.11.2026 |

Исключение enhanced-resolve@5.26.1: prepare=husky — подготовка checkout для разработчика upstream; при установке tarball не нужен и никогда не запускается. Исключения только для присутствия поля prepare, не для выполнения скриптов.

prepare не выполняется при установке из реестра; установка с ignore-scripts (решение CEO 20.11.2026). Исключения prepare: `dompurify@3.4.17`, `katex@0.18.10`, `inline-style-parser@0.2.7`, `style-to-js@1.1.21`, `style-to-object@1.0.14`.

Лицензионное исключение `dompurify@3.4.17`: двойная лицензия `(MPL-2.0 OR Apache-2.0)`, используется ветка Apache-2.0; допускается только эта строка для этой версии, решение CEO 20.11.2026.

Шрифт окна (не npm-пакет): Inter Regular, Medium и SemiBold, woff, урезаны до латиницы и кириллицы, лежат в `ui/src/assets/fonts/` и подключаются из `ui/src/styles/tokens.css`; источник — Inter 4 из `~/.local/share/fonts/Inter`, лицензия SIL OFL 1.1, текст — `ui/src/assets/fonts/LICENSE-Inter.txt`. Пакет `@fontsource-variable/inter` остаётся в списке выше, но окно его больше не подключает.

## Изменения

- marked удалён: отображение чата перешло на конвейер T3, решение CEO 20.11.2026.

## Rust оболочки

Это отдельный workspace, без зависимостей на crates движка и без плагинов Tauri. Корневой Cargo.toml и Cargo.lock не меняются этой работой.

| Crate | Версия | Features | Зачем |
|---|---|---|---|
| tauri | =2.12.5 | default-features=false; wry, x11, custom-protocol | Системное окно и встроенные статические файлы; без сети, updater, tray, devtools, dynamic-acl |
| tauri-build | =2.7.4 | default-features=false; config-json | Конфигурация JSON и явный пустой список команд; версии tauri-build идут по собственной линии, 2.12.x у него нет |
| webkit2gtk | =2.0.2 | default-features=false; v2_40 | Нативный сигнал заголовка для независимой CSS-пробы и три C-getter runtime-версии WebKitGTK; не JS-команда |

417 сторонних crates в Cargo.lock (418 записей с самим hoh-desktop). Сборка только для Linux. Обычный cargo fetch запускался с --target x86_64-unknown-linux-gnu; из-за сбоев DNS/TLS недостающие архивы восстановлены по отдельной блокировке с проверкой SHA256. В кэше теперь все 417 архивов, включая другие цели; их наличие в кэше не означает установку или сборку для этих целей. Контрольные суммы локально повторно используемых архивов сверяются с отдельным Cargo.lock; общий CARGO_HOME только читается, сборка использует собственный CARGO_HOME. Rust build.rs и proc-macro технически не выключаются; сборка с --locked --offline и seccomp-запретом IPv4/IPv6. cargo-deny пока не установлен: проверки его advisories/licenses/sources не выдаются за выполненные.

## Зафиксированный отказ npm audit

2026-11-18 live pnpm audit: picomatch 4.0.7, high, GHSA-fc5q-39xc-vjpf (ReDoS через repeated extglobs). Он приходит через Vite 8.3.7 и tinyglobby. Ответ registry: vulnerable_versions <=4.0.7, patched_versions=null, patched_versions_unpublished=true. В метаданных picomatch latest=4.0.7. Отчёт: pnpm-audit.json, привязан к SHA256 блокировки. Gate правильно красный. Уязвимость не игнорируется; npm-архивы и node_modules этим заданием не получены. Для продолжения нужен исправленный пакет с выдержкой 7 дней либо новое решение по зависимостям.

Число в текущей npm-блокировке: 65 (29 прямых, 36 транзитивных), без optional. Полные зафиксированные метаданные выбранных версий: npm-lock-metadata.json. Оба поля prepare — отличие от утверждения в docs/window.md об одном fsevents-скрипте; ни одно из них не выполняется.

Итог получения Rust: 153 архива скачано с crates.io, 264 повторно использовано из общего read-only кэша после сверки SHA256. В Cargo.lock 417 сторонних crates + запись приложения. Восстановление DNS использовало адреса static.crates.io, полученные системным resolver; TLS-проверка сертификата оставалась включена. Скрипт восстановления: apps/hoh-desktop/scripts/fetch-locked.py (2 загрузки одновременно).

Проверка native minimumReleaseAge в pnpm 12.21.0: scripts/tests/ui-supply/native-age.py; усиленный порог 100800 минут, react@19.3.1, только offline lockfile resolve без архивов. Получен ERR_PNPM_NO_MATURE_MATCHING_VERSION с явным указанием minimumReleaseAge cutoff. Основные 17 fixtures отдельно проверяют нашу политику 10080 минут и каждый требуемый отказ.

Среда сборки окна: Node 22.23.1 (`/home/save/.nvm/versions/node/v22.23.1`, уже стоял, новых установок нет), pnpm 12.21.0. Причина: `engines` у pdfjs-dist@6.5.438 и commander@15.1.0 требуют Node не ниже 22; решение ведущего 20.11.2026. PATH задаёт `ui/scripts/build-env.allow`.

На этой машине Cargo распаковал 272 Linux-crates, остальные архивы остались только в кэше. Для независимой пробы используются те же Tauri capabilities/CSP и те же исходники probe, что в будущей производственной сборке, но frontendDist временно заменён на authored probe-dist, без React/Vite. Это не заменяет заблокированные npm build/snapshot.

Разрешённые лицензии Rust в deny.toml — подмножество общей licenses-политики выше: `MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Unicode-3.0`, `Zlib`, `MPL-2.0`, `CC0-1.0`, `OFL-1.1`. Supply gate проверяет это соответствие даже без cargo-deny; проверка лицензий самих Rust-crates пропущена до установки cargo-deny. Unicode-3.0, Zlib и Apache-2.0 WITH LLVM-exception добавлены для Rust-дерева.
