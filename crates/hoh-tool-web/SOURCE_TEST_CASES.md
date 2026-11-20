# Source test mapping

В образце 109 развёрнутых cases: 98 в `tool-web.spec.ts` (включая строки
`it.each`), 8 в `integration.spec.ts`, 1 в `spill.spec.ts` и 2 в
`load-path.spec.ts`. 89 cases имеют прямую или адаптированную Rust-проверку ниже;
20 не переносятся либо остаются в крейте-владельце по
указанным причинам. Несколько однотипных строк `it.each` объединены в один Rust
test с циклом, но каждая входная строка сохранена.

| Source cases | Rust coverage |
|---|---|
| search: exact formatting, hostname/raw-url labels, summary-over-snippet, no-results/content-only, truncation | `search::tests::formats_search_contract`, `exact_formatting_presentation_and_meta_contracts_match_source_cases` |
| search args: empty, blank, duplicate, exact 1/2/10-query errors, 10 accepted/11 rejected | `search::tests::parses_deduplicates_and_validates_queries`; `tests::every_numeric_config_boundary_timeout_override_schema_cap_and_query_limit_match_source` |
| search call/result presentation, meta projection/round-trip, absent answer, empty sources, error fallback and all malformed-meta rows | `search::tests::exact_formatting_presentation_and_meta_contracts_match_source_cases`, `malformed_meta_falls_back` |
| round-robin, unequal result lengths, deduplication, combined cap and answer headings | `search::tests::round_robin_merges_deduplicates_and_caps` |
| concurrent multi-query failure aborts siblings and waits for settlement | `search::tests::multi_search_returns_first_temporal_failure_after_siblings_settle` |
| provider request carries `maxResults`; every single/batch request carries a signal | `search::tests::provider_receives_max_results_and_a_signal_for_every_query`; live `StubWeb` assertions in `tests::real_registry_admission_execution_meta_and_guidance_follow_readiness` |
| summary collapse/fallback, configured UTF-16 cap, exact cap, surrogate boundary | `search::tests::summary_cap_counts_utf16_without_splitting_pairs`, `query_and_summary_whitespace_match_ecmascript`; registry meta test keeps display snippet separate |
| ECMAScript `trim`/`\s` boundaries U+0085 and U+FEFF | `search::tests::query_and_summary_whitespace_match_ecmascript` |
| fetch text/HTML dispatch, header/footer, complete-output/source UTF-16 caps and 497-character converted source prefix | `fetch::tests::renders_text_and_bounds_complete_output`, `source_and_final_caps_count_utf16_units`, `source_cap_panic_fallback_and_malformed_boundaries_match_source` |
| hidden/active nodes, entities, GFM tables/alignment/colspan, core Turndown/GFM | `html_to_markdown::tests::removes_active_and_hidden_nodes`, `renders_core_turndown_and_gfm_rules`, `gfm_tables_collapse_pipe_runs_and_only_heading_rows_get_borders` |
| nested lists (2/3 levels and mixed ol/ul), formatted block whitespace, mixed inline siblings and flanking whitespace | `html_to_markdown::tests::preserves_nested_list_structure_at_three_levels_and_across_list_kinds`, `normalizes_ascii_whitespace_with_dom_sibling_context`, `moves_inline_flanking_whitespace_outside_markers_and_links` |
| mark/dl/block tags/highlighted code/blank nodes | `html_to_markdown::tests::matches_default_block_blank_and_highlighted_code_rules` |
| plain pre, multiline inline code, image alt, empty href, quote blank line, 10+ ordered continuation, NBSP | `html_to_markdown::tests::matches_pre_code_image_link_quote_list_and_unicode_whitespace_edges` |
| depth/comment/bad-closer/raw-text/void/quoted markup and boundary forms `</script >`, `</script/>`, unclosed script/comment | `fetch::tests::depth_guard_cannot_be_hidden_by_comments_or_bad_closers`, `ordinary_raw_text_void_and_quoted_markup_passes_preflight`, `source_cap_panic_fallback_and_malformed_boundaries_match_source` |
| malformed `<!--` + 50k `<br>` and `<a` ×100k bounded scans | `fetch::tests::scans_a_large_unclosed_comment_once_in_bounded_time`, `malformed_unterminated_tags_are_scanned_in_bounded_time` |
| converter panic omission and 500-deep block/list/table/pre conversion from a small caller stack | `fetch::tests::source_cap_panic_fallback_and_malformed_boundaries_match_source`, `guarded_recursive_conversion_is_independent_of_the_callers_small_stack` |
| fetch exact INVALID_ARGS, call/result presentation, meta effective truncation, replay overflow and every malformed-meta row | `fetch::tests::exact_fetch_argument_presentation_and_meta_contracts_match_source_cases`, `fetch_meta_tracks_effective_truncation_and_status_overflow_falls_back` |
| render/meta one-conversion hand-off, equal values and bounded cache | `fetch::tests::projection_cache_is_fifo_bounded_and_handles_equal_values`; `ProjectionCache::LIMIT` bounds the hand-off |
| default/enabled registrations, exact timeout overrides, all six numeric config fields × zero/negative/fractional, 100-char fetch output and query-description cap | `tests::plugin_registers_both_tools_with_default_policy`, `plugin_enablement_and_positive_integer_validation_match_source`, `every_numeric_config_boundary_timeout_override_schema_cap_and_query_limit_match_source` |
| real registry formatting/meta, `{}`/`[123]` INVALID_ARGS, exact URL error, UNKNOWN_TOOL, readiness withholding and allowed/blocked guidance | `tests::real_registry_admission_execution_meta_and_guidance_follow_readiness` |
| exact guidance for allow `[]`, search, fetch and both | `tests::scope_allowlist_guidance_matrix_matches_source` |
| provider selection, unavailable/ambiguous routing and “ask only selected provider” | owned by `hoh-web`; this crate tests only its `search_ready`/`fetch_ready` result and forwards to the selected seam. The provider-routing cases remain in `hoh-web` rather than duplicating foreign behavior here. |
| exact JS signal object identity | Rust `AbortSignal` intentionally exposes behavior, not pointer identity. Single calls assert a signal is passed; batch tests prove shared cancellation/settlement. This is the Rust semantic equivalent, not an omitted cancellation branch. |

## Не перенесено или делегировано владельцу: 20 cases

- `tool-web.spec.ts`: namespace “no default export” (1) — ESM-only shape; Rust exports
  are compile-time module items and integration tests compile every public symbol.
- `integration.spec.ts` (8) — owns real loopback HTTP, `web-fetch-http`,
  `web-deepseek-api` and timeout-policy composition. Network/process checks are outside
  this package task and explicitly forbidden for this writer; unit/registry boundaries
  above cover this crate's contribution.
- `spill.spec.ts` (1) — owns the foreign spill-local/spill-policy/token-meter stack and
  filesystem world effects; `tool-web` deliberately contains no spill-specific code.
- `load-path.spec.ts` (2) — Cordis ESM `unwrapExports`/namespace-loader behavior has no
  Rust runtime equivalent; plugin injection is exercised through the real Rust kernel.
- Provider selection/unavailable/ambiguous and “ask only selected provider” (4) — это
  контракт `hoh-web`; `hoh-tool-web` проверяет step-time readiness выбранного seam и
  `UNKNOWN_TOOL`, не дублируя маршрутизацию чужого крейта.
- Exact JavaScript `AbortSignal` pointer-identity assertions (4) — Rust API намеренно
  не раскрывает адрес identity. Адаптированные тесты проверяют наблюдаемое: обязательную
  передачу signal каждому запросу, единое batch cancellation и ожидание siblings.
