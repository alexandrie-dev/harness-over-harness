# Port notes

- Source: `packages/web/tool-web` (`src/index.ts`, `search.ts`, `fetch.ts`, `trust.ts`).
- `TurndownService` 7.2.4 plus `@joplin/turndown-plugin-gfm` is implemented locally over
  `html5ever`/`markup5ever_rcdom`, as approved by `docs/porting/DEPS.md` §14. It includes the
  core Turndown block/inline/list/link/image/code rules and the GFM table, task-list,
  strikethrough and highlighted-code rules. A DOM-aware prepass collapses only Turndown's ASCII
  whitespace with block/sibling context; NBSP and other Unicode spaces remain content. Browser
  error recovery can differ from Domino on malformed HTML; the source's 512-element lexical guard
  and fixed safe omission marker remain.
- Recursive DOM conversion runs on an 8 MiB bounded-stack worker after the lexical depth guard, so
  a small Tokio/caller stack cannot turn the source's recoverable conversion boundary into process
  stack exhaustion. Panics still map to the fixed omission marker. Comment preflight caches the
  nearest `-->` once and preserves the source's conservative malformed-comment depth counting,
  eliminating the former quadratic rescan.
- Turndown parity regressions cover nested/mixed lists, block and inline whitespace, blank/default
  rules, highlighted code, plain `pre`, inline code, links/images, blockquotes, ordered continuation,
  td-only/aligned/colspan tables and GFM pipe-run escaping.
- JavaScript character limits are counted as UTF-16 code units. A cut that would leave an
  unrepresentable lone surrogate drops the whole scalar, following the workspace
  `truncate_without_splitting_surrogate_pair` convention.
- Search validation and summary collapse use the shared ECMAScript whitespace helpers from
  `hoh-util-values` (U+FEFF is whitespace; U+0085 is not), rather than Rust `trim`/
  `split_whitespace`.
- The registry calls output rendering before presentation-meta projection. A bounded FIFO
  hand-off shares that conversion without address identity, supports concurrent equal values,
  releases entries when metadata consumes them, and never performs fallback HTML conversion while
  holding its mutex. It intentionally stores at most 32 JSON keys/results.
- `WebFetchResult.status_code` is wider than the UI `u16`; an out-of-range replay meta falls back
  to the generic card instead of truncating the value.
- No foreign API changes or new external dependencies were required; `hoh-util-values` is an
  existing workspace crate already present in `Cargo.lock`.

## Тесты образца

`SOURCE_TEST_CASES.md` maps all 109 expanded source cases. 89 have direct or adapted Rust coverage
(grouped where one Rust loop represents an `it.each` table); 20 are explicitly excluded or delegated:
8 real-backend/network integration cases, 1 foreign spill-stack case, 3 ESM namespace/Loader cases,
4 provider-routing cases owned by `hoh-web`, and 4 JavaScript signal pointer-identity assertions.
Those identity assertions are replaced by observable signal forwarding, shared batch cancellation
and settlement.

## Проверки

Все команды выполняются offline, jobs=2, target
`/mnt/toshiba/alexandrie/hoh/target-x8`; тесты — только под внешним `timeout`.

- `timeout 180s cargo test -p hoh-tool-web --offline --no-fail-fast`: 37 passed, 0 failed;
  doc-tests 0/0.
- `cargo clippy -p hoh-tool-web --all-targets --no-deps --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-tool-web -- --check`: exit 0.

## Круг исправлений по ревью 034911 (N1–N5)

Код Turndown и gfm-плагина в образце отсутствует; правила воспроизведены по памяти о Turndown 7.2 и
`@joplin/turndown-plugin-gfm` и не сверены с их исходниками.

- N1: `highlighted code` — `highlight-(?:text|source)-([a-z0-9]+)` где угодно в `class`, первый ребёнок `div` —
  `pre`, забор фиксированный ```` ``` ````, текст без срезания переводов строки.
- N2: ширина ячейки — в единицах UTF-16, обрезка — `js_trim`.
- N3: `fenced code` — только если `code` первый ребёнок `pre`; срезается один хвостовой `\n`; забор растёт от
  серий из трёх и более обратных кавычек в начале строки; язык — `language-(\S+)` в любом месте `class`.
- N4: список, завершающий `li`, добавляет один перевод строки (пустая строка остаётся, если перед ним был блок).
- N5: `summary` и `title` — не блочные; заголовок картинки не экранируется. Не менялось: смещение `offset = end + 3`
  в `fetch.rs` (совпадает с html5ever, вреда нет).
