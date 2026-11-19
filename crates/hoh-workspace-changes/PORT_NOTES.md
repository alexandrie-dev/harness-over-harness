# hoh-workspace-changes ← packages/deliverables/workspace-changes

## Перенос

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin, config, Session/tool/fs lifecycle и сервис `WorkspaceChanges` |
| `src/recorder.ts` | `src/recorder.rs` | последовательные turn snapshots/captures, summaries и lazy diff |
| `src/git.ts` | `src/git.rs` | private index/object directory, snapshots, numstat, blobs, ignores и gitlinks |
| `src/capture.ts` | `src/capture.rs` | ограниченные whole-file captures, SHA-1, binary/oversized/absent |
| `src/compare.ts` | `src/compare.rs` | line diff, три строки контекста, coarse fallback по deadline |
| `src/{numstat,paths,types}.ts` | одноимённые `.rs` | wire types, unusual paths, cwd/home/temp display rules |
| `locale/*.json` | `locale/*.json` | все 8 файлов без смысловых изменений |
| `tests/*.spec.ts` | `src/tests.rs` | объединённые unit/integration regressions основных контрактов |

README, package metadata, все production-файлы и все десять файлов тестов образца
прочитаны целиком. Новых внешних зависимостей нет.

## Сохранённое поведение

- Только top-level Session с `cwd` получает recorder; subagent/delegated sessions пропускаются.
- Начало/конец turn и tool-result observations сериализованы одним worker. До файловой мутации
  исходное содержимое захватывается по `ToolExecutionToken`; после tool result либо stopping
  строится один net summary и публикуется `workspace/changes`.
- Git снимки используют отдельные object directory и index, не меняют рабочий index/status,
  учитывают untracked/ignored/gitlink paths и сохраняют tree ids для ленивого сравнения.
- Без Git продолжают работать захваты write/edit/delete. Binary и oversized не читаются как текст;
  список ограничивается `maxFiles`, а aggregate totals считаются по полному набору.
- Путь wire относительно исходного Session cwd, внешний путь абсолютный; display сортируется по
  workspace/home/absolute правилам образца. Symlink и отсутствующий хвост canonicalize обрабатываются.
- Text diff сохраняет source counts, unified hunks и missing-final-newline semantics; превышение
  `diffTimeoutMs` даёт coarse whole-file comparison.

## Отличия и SHIM

1. Promise-chain recorder образца выражен одним `tokio::mpsc` worker. Это сохраняет порядок и
   дополнительно даёт явный barrier перед файловой операцией и остановкой turn.
2. Захват сначала полностью записывается во временный файл и только затем атомарно публикуется
   hard-link по SHA-1. В образце `writeFile(..., flag: "wx")`; наблюдаемое content-addressed
   поведение то же, но Rust-читатель не может увидеть частичный файл.
3. В Cargo.lock нет отдельной line-diff библиотеки образца, поэтому Myers и unified-hunk builder
   реализованы локально, с теми же bounds/deadline contract. Trace хранит только окно `-d..d`
   каждого слоя; совокупная рабочая память ограничена 32 MiB, а превышение даёт coarse fallback.
4. **SHIM(hoh-subprocess-local):** `git check-ignore --stdin -z` нельзя надёжно использовать через
   текущий subprocess stdin: процесс иногда получает EOF раньше записи, а явная pipe-запись может
   вернуть `EPERM`. Перенос сначала отправляет bounded batch по 128 путей через проверяемый
   `-z -v --non-matching --stdin`: четыре поля на каждый вход позволяют обнаружить потерю stdin.
   Только сбойный batch классифицируется literal argv-запросами с делением группы; табы и переводы
   строк сохраняются. После исправления чужого stdin fallback можно удалить без изменения API.

## Независимое ревью

- Session scratch и вложенные private-index каталоги на Unix создаются с mode `0700`; regression
  читает фактические permissions. Ошибка удаления больше не замалчивается.
- Content-addressed capture публикуется атомарно, а dispose abort-gate не позволяет queued work
  снова наполнить очищенный record cache.
- Небинарный invalid UTF-8 декодируется с U+FFFD, как Node `readFile(..., 'utf8')`, и покрыт summary
  плюс lazy-diff regression.
- Myers получил строгий memory guard и safe `Instant::checked_add`; config ограничен JS
  `Number.MAX_SAFE_INTEGER`, как `Number.isSafeInteger` образца.
- Actor correlation хранит weak entry сразу на `ToolExecution` и recorder; мёртвые entries
  удаляются при следующем insert/capture, а `ToolsResult` выполняет быстрый явный cleanup после
  dispatch. Запись намеренно живёт дольше pre-execute, иначе настоящий tool body потерял бы capture.
- Добавлена интеграция настоящего minted `ToolExecution → tools/pre-execute → fs/write-intent`:
  `prepare → dispatch → complete` исполняет зарегистрированный tool body, мутация происходит только
  после capture, summary и lazy diff подтверждают absent→created.

## Исправления после ревью

- Verbose-ответ `git check-ignore -v --non-matching` теперь отличает положительный шаблон от
  отменяющего `!pattern`. Unit/integration regressions проверяют одновременно `{x.log}` и отсутствие
  ложного B→A после bash-изменения с возвратом `keep.log` к snapshot-содержимому.
- Myers хранит компактные окна trace. Regression на 10 000 общих строк и замену 60 строк требует
  точный один hunk с `added == deleted == 60`, а не coarse whole-file diff.
- Lazy diff после успешных чтений возвращает `None`, если lifetime был остановлен до сравнения;
  deterministic regression прерывает lifetime именно в этой точке.
- SHA-1 захвата и оба вызова `compare_text` вынесены в `spawn_blocking`, поэтому поднятый
  `diffTimeoutMs` не удерживает поток Tokio runtime.
- Добавлены точные assertions для unified hunks, missing-side случаев, far-hunk coordinates,
  created coarse fallback, сортировки display по UTF-16 code units и дробного `diffTimeoutMs`.

## Отложенные проверки образца

- Реальный macOS `/usr/bin/git` + четыре варианта `xcode-select`, а также Windows filesystem/process
  behavior: ветви платформозависимы, текущая среда Linux.
- Git cases с нечитаемым index/repository: процесс тестов идёт с правами, при которых `chmod 000`
  не моделирует отказ. Production-пути ошибок не замалчиваются. Timeout/abort самого subprocess,
  unmerged-index preservation и очистка scratch/object directories принадлежат общим контрактам
  `hoh-subprocess`/private-index; здесь оставлены integration snapshot/blob/status/cleanup проверки
  без дублирования медленных либо permission-dependent fixtures.
- Полная восьмисценарная матрица настоящего apply-patch, `current-directory` adapters для
  write/edit/str-replace и loader-composition: этот крейт потребляет общий `Fs*Intent` seam и не
  владеет конкретными tool/loader реализациями. Здесь проверены реальный minted `ToolExecution`,
  pre-execute waterfall, write intent, policy-preserving dispatch, Git turn, публикация события и
  lazy diff; provider-specific edit/delete/apply-patch fixtures остаются в крейтах инструментов.
- Редкие plugin orchestration варианты (удалённый cwd, следующий turn после прерванного,
  повторный start после dispose, ровно один info при отсутствии git) отложены: жизненный цикл
  покрыт общими Session hooks и локальными recorder dispose/net-change regressions; отдельные
  end-to-end сценарии требуют искусственных runtime/process doubles, которых в workspace API нет.

## Проверка

Команды выполняются offline с `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`; тесты только под `timeout`.

- `cargo test -p hoh-workspace-changes --offline`: 17 passed, 0 failed; doc-tests 0/0.
- `cargo clippy -p hoh-workspace-changes --all-targets --no-deps --offline -- -D warnings` — чисто.
- `cargo fmt -p hoh-workspace-changes -- --check` — чисто.
