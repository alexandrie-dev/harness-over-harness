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
