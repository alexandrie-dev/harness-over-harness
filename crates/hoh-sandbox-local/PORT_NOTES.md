# hoh-sandbox-local ← packages/sandbox/sandbox-local @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | Linux provider: config, custom runner, bwrap→Landlock ladder, probes, cache, fail-closed |
| src/profiles.ts | src/profiles.rs | bwrap и Landlock profiles; Seatbelt исключён по Linux-only заданию |
| tests/local.spec.ts | tests/local.rs | 25 из 42 объявлений; Linux и общая механика selection/config |
| tests/bwrap.e2e.ts | tests/e2e.rs | все 9 объявлений / 10 развёрнутых случаев, условно по probe |
| tests/landlock.e2e.ts | tests/e2e.rs | все 7 объявлений, условно по probe bundled launcher |
| locale/{de,en,es,fr,ja,pt,ru,zh}.json | locale/ | скопированы побайтно |
| README.md, README.zh.md, package.json | Cargo.toml, эти заметки | прочитаны; собственного LICENSE нет |

Всего перенесено 41 из 81 исходного объявления, или 44 из 85 случаев после
разворачивания `it.each`. Все 41 Linux-применимое runtime-объявление перенесены;
остальные 40 перечислены ниже.

## Отличия от образца

- Перенос только Linux по заданию ведущего. Product ladder ровно `bwrap`, затем
  `landlock-run`; неизвестная платформа получает исходный `SANDBOX_UNAVAILABLE`.
- `SandboxProvider`, политики, coded errors, canonicalization и проверка physical
  additional roots импортированы из `hoh-sandbox`. Landlock launcher, grants,
  probe parsing и константы импортированы из `hoh-node-addon-system`.
- Source `spawnSync` заменён bounded bwrap owner на blocking worker с обязательными
  kill/reap при timeout и через cancellation-флаг при drop async waiter;
  синхронный owner Landlock probe также исполняется на blocking worker.
  Инфраструктурный отказ probe означает `unusable`, не утечку запуска без sandbox.
  Ошибка разрешения пути bundled Landlock launcher также означает `unusable` и
  кэшируется как `SANDBOX_UNAVAILABLE`; в source `landlockLauncherPath()` бросает
  исходную ошибку без кэширования. Это намеренный fail-closed результат Rust owner.
- Promise selection в source не отменяется отказом caller от ожидания. Rust
  запускает один shared bounded selection task; concurrent и dropped callers
  получают один cached verdict, включая `unavailable`. После async границ
  `AbortSignal` проверяется повторно, причина сохраняет `SharedError::ptr_eq`.
- Из-за async owner helper функции profiles тоже async и возвращают `Result`.
  Non-UTF-8 launcher/profile path отклоняется fail-closed; wire argv остаётся
  `Vec<String>`. `SandboxInternals`, `LocalRunner`, `ProbeVerdict` и
  `new_with_internals` — Rust-имена source test seam.
- Один source test для sole-candidate selection перенесён на Landlock вместо
  macOS Seatbelt: проверяется сама общая механика «единственная ступень без probe».
- Оконное расширение относительно dsh: каталоги из `protectedRoots` маскируются последними bwrap
  mount-ами (`tmpfs` + mode 000), а существующие файлы — read-only bind `/dev/null`. Отсутствующий
  protected mountpoint даёт ясный fail-closed отказ подготовки: маска никогда не расширяется до
  предка, который мог бы накрыть sibling-workspace. Window передаёт уже созданный целый DSH_HOME,
  поэтому ещё отсутствующие файлы внутри него закрывает directory-mask. Пересечение workspace с
  таким каталогом раньше отклоняет `hoh-sandbox-policy`. Для approved full access
  корень `/` остаётся host-rw и `/tmp` не
  подменяется. Landlock не умеет безопасно вычесть same-user path из широкого read grant без
  гонки открытия, поэтому такая policy требует bwrap и при его отсутствии даёт
  `SANDBOX_UNAVAILABLE`, а не запускает команду.
- Если предыдущая approved full-access команда переименовала предка protected root, следующая
  команда не запускается: отсутствие исходного mountpoint либо несовпадение сохранённой startup
  identity после его подмены даёт явный fail-closed отказ вместо маскировки ложного объекта или
  ослабления маски до предка.

## Не перенесено

- `acl-grants.spec.ts`: 11 Windows ACL случаев; весь Windows rung ждёт этапа 5 и
  исключён явным Linux-only заданием.
- `seatbelt.e2e.ts`: 7 macOS случаев; Seatbelt profile/probe/runner не входят в
  назначенную Linux-волну.
- Из `local.spec.ts` исключены 17 объявлений: четыре Seatbelt profile, darwin
  selection, два Seatbelt probe, восемь Windows runner/probe и два JS-only
  closed-union cast/exhaustiveness случая. Sole-candidate rule проверен Landlock.
- `packed-install.e2e.ts` (3) и `packed-workspace-closure.spec.ts` (2) — npm pack,
  workspace inventory и Node installation checks, исключённые PORTING §8/§10.

## Не проверено

- Native bwrap/Landlock tests исполняют world-effect assertions только если
  функциональный probe текущего host проходит. Иначе они печатают явную строку
  `SKIP: …`; `HOH_REQUIRE_NATIVE_SANDBOX=1` превращает отсутствие launcher/bwrap
  или unusable probe в падение, чтобы обязательная native-приёмка не могла пройти
  молча. В этой среде результат strict-прогона указан в итоговом отчёте.
- Нет macOS/Windows ветвей. Системная статическая упаковка `landlock-run` проверяется
  crate-владельцем и его release script, не дублируется здесь.

## Вопросы ведущему

Нет отсутствующих Linux API в зависимостях. Для будущего полного cross-platform
переноса нужны владельцы Windows ACL и отдельная macOS-приёмка Seatbelt.

## ready_dir (credguard, круг 5)

- `SandboxPolicy.ready_dir: Option<String>` — каталог маркера готовности для исполнителя оболочки. Только `hoh-sandbox-local` (bwrap) монтирует его на запись `--bind d d` после корней рабочей области и перед защищёнными путями. Образца нет: расширение для замка `protected_effect` (см. PORT_NOTES `hoh-bash-sandbox`).
