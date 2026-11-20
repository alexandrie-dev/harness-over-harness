# Круг исправлений hoh-fs-local (по двум ревью)

Рабочий каталог /home/save/Alexandrie/HarnessOverHarness. Крейт `crates/hoh-fs-local` (писал Codex w4, сейчас он занят
другим — менять только этот крейт, плюс строки в корневом Cargo.toml и docs/porting/DEPS.md, если разрешено ниже).
Источник: /mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/fs/fs-local. Правила: docs/porting/PORTING.md,
plans/briefs/interface-first.md. Ревью (прочитай оба целиком):
/mnt/toshiba/alexandrie/hoh/logs/rv-sandboxfs.full.md (пункты 1–5 про fs-local) и /mnt/toshiba/alexandrie/hoh/logs/rv-fslocal.full.md (1–7).

Исправить все, к каждому регрессионный тест с ожиданием источника:
1. (P1) Ошибка close не должна позволять публикацию: fsio.rs:1064 vs fsio.ts:648; плюс составное сообщение при второй
   ошибке очистки. Решение ведущего по `unsafe`: не нужен — закрывай через безопасную `nix::unistd::close` на
   `OwnedFd::into_raw_fd()` (nix 0.28 уже в Cargo.lock) или через rustix, если там есть безопасная функция; добавь прямую
   зависимость в Cargo.toml крейта (версия из lock) и строку в DEPS.md. Тест с подстановкой ошибки close.
2. Наблюдение: (а) объединение «удалён и тут же создан» в одно изменение как у chokidar 5 (ожидание удаления, порог
   изменений); (б) слежение за целями символьных ссылок прямых детей каталога. Тесты с управляемыми событиями/часами.
3. Порядок имён как `localeCompare` (en-US, ICU): `é` раньше `z`; равные — устойчиво. Разрешено добавить один крейт
   сравнения по UCA/CLDR (предпочтительно `feruca`; если не подходит — `icu_collator`): сверь на тестах с несколькими
   алфавитами (латиница с диакритикой, кириллица, цифры, регистр) против ожидаемых значений Node (возьми их из
   документации ICU/CLDR или выведи по правилам UCA; в отчёте укажи источник ожиданий). Скачать крейт можно
   (единственное разрешение сети — crates.io при сборке, `CARGO_NET_OFFLINE` для этого шага снять). Строку в DEPS.md.
4. maxBytes 9007199254740992 и большие смещения — RangeError ERR_OUT_OF_RANGE как Node (fsio.rs:518).
5. NUL в пути — FsError/FS_IO_ERROR с причиной ERR_INVALID_ARG_VALUE и текстом Node (fsio.rs:289).
6. Абсолютный путь не обращается к cwd (fsio.rs:167).
7. Ошибка чтения подписывается `read`, а не `open` (fsio.rs:462).

Сборка: CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-sonnet (общий, ждать блокировку — нормально), CARGO_BUILD_JOBS=2;
cargo check по ходу; в конце `cargo test -p hoh-fs-local`, clippy `--all-targets -- -D warnings`, fmt. Сборки в T/Tl не
возобновлять. Ничего не устанавливать в систему, не публиковать, не коммитить. Отчёт по-русски: пункт → правка → тест → результаты.
