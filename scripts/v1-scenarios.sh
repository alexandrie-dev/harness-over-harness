#!/usr/bin/env bash
# Отбор сценариев dsh (conformance/EXPECTED.tsv) для «ядро готово» → conformance/V1-SCENARIOS.tsv.
#   bash scripts/v1-scenarios.sh            перезаписывает V1-SCENARIOS.tsv (правило — в шапке файла)
# Решение CEO 20.11: отбор по теме, правило записано до отбора, у исключённого одна причина (первое сработавшее правило).
set -euo pipefail
cd "$(dirname "$0")/.."
S=/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/snapshots
out=conformance/V1-SCENARIOS.tsv
rules=(   # шаблон по «suite/имя» или «tool:имя_инструмента»<TAB>причина исключения[<TAB>крейты для extra]
  '^acp/	этап 4: ACP вне первой версии'
  '^comp:sdk-minimal	сборка sdk-minimal вне 118 (заменена apps/hoh)'
  '(ptc|run-code|both-mode|workflow|advanced-toolchain)	PTC/run_code/workflow (workflow-ptc, typert: redesign/skip)'
  'tool:(run_code|workflow) 	PTC/run_code/workflow (workflow-ptc, typert: redesign/skip)'
  '(cua|computer-use)	computer-use: интеграции вне 118'
  'browser-use	browser-use: интеграции вне 118'
  'cordis-inspect|headless-credentials	tool-cordis вне 118'
  'tool:cordis_	tool-cordis вне 118'
  '/hook-|claude-code-mods	хуки Claude Code/Codex вне 118'
  'lsp	этап 4: LSP'
  'plugin-manager	plugin-manager вне 118 (не переносится)'
  'mcp	MCP вне 118	hoh-mcp-resources hoh-mcp-client'
  'pwsh|windows	этап 5: Windows/PowerShell'
  'team	agent-team вне 118	hoh-agent-team hoh-tool-agent-team'
  'goal|ralph	goal вне 118	hoh-goal hoh-command-goal hoh-tool-goal hoh-goal-round-driver'
  'session-reference	session-reference вне 118	hoh-session-reference'
  'spill	spill вне 118	hoh-spill hoh-spill-local hoh-spill-policy'
  'plan	plan-mode вне 118	hoh-plan-mode'
  'repeat-tool-reminder	repeat-tool-reminder вне 118	hoh-repeat-tool-reminder'
  'terminal|/pty-	terminal вне 118	hoh-terminal hoh-terminal-bash'
  'pi-ai	llm-pi-ai вне 118'
  'deepseek-(messages|protocol|all-deferred|deferred)|web-search|account	DeepSeek-специфичное вне 118 (llm-deepseek, api-extensions, web-search-deepseek)'
  'dsh-sdk	subagent-dsh-sdk не переносится (нет крейта в описи)'
  'product-subagent|subagent-acp	внешние субагенты Claude/Codex/OAR вне 118 (HOH-18 — отдельная проверка)'
  'auto-review	experimental auto-review вне 118'
  'fork-in-process	subagent-fork-in-process вне 118	hoh-subagent hoh-subagent-fork-in-process'
  'persistent-tools	tool-bash-persistent вне 118	hoh-tool-bash-persistent'
)
{
  cat <<'HDR'
# Сценарии dsh для определения «ядро готово» (решение CEO 20.11). Генератор: scripts/v1-scenarios.sh.
# ПРАВИЛО (записано до отбора): сценарий входит (in), если его тема — поведение крейтов первой версии
# (118 строк, research/v1-engine-scope.codex.md) или крейтов, уже стоящих в профиле окна apps/hoh
# (assets/bundle/window/cordis.patch.yml). Тема определяется по имени сценария и по именам инструментов,
# которые модель вызывает в записанной сессии (snapshots/<suite>/<имя>/*.jsonl). Исключённый (out) получает
# одну причину — первое сработавшее правило списка в генераторе (порядок: ACP, сборка, PTC, интеграции, хуки,
# этапы 4–5, отложенные группы описи, DeepSeek-специфичное, внешние субагенты).
# Плагины полного набора dsh, которых нет в первой версии, в отобранных сценариях не исключают сценарий:
# прогонщик нормализует или исключает их события (заголовки, промпт, схемы инструментов) — правило нормализации
# записывается вместе с подключением прогонщика.
# Общее для всех session-сценариев: профиль headless (лаунчер hoh-headless, вне 118) — нужен прогонщику.
# extra (дополнение CEO 20.11): сценарий вне порога, но все крейты его причины уже в дереве (MAP: done/accepted).
# Прогонщик гоняет extra отдельным числом; на «ядро готово» они не влияют и исправлений сами по себе не требуют.
# Спорные (provider-cwd, desktop-context — in; subagent-dsh-sdk-* — out) прочитаны ведущим 20.11.
# Для прогонщика: 140 из 242 сценариев грузят local:-фикстуры на TS (тестовые плагины dsh) — им нужен Rust-аналог.
HDR
  printf 'suite\tscenario\tv1\treason\tnote\n'
  tail -n +2 conformance/EXPECTED.tsv | while IFS=$'\t' read -r suite name profile comp _; do
    tools=$(cat "$S/$suite/$name"/*.jsonl 2>/dev/null | grep -oE '"toolName":"[a-z_]+"' | sed -E 's/.*:"([a-z_]+)"/tool:\1/' | sort -u | tr '\n' ' ' || true)
    v=in reason=- note=-
    for r in "${rules[@]}"; do   # tool: — по вызовам модели, comp: — по составу, иначе по «suite/имя»
      p=${r%%$'\t'*}
      case $p in tool:*) key=$tools ;; ^comp:*) key="comp:$comp" ;; *) key="$suite/$name" ;; esac
      if [[ $key =~ $p ]]; then
        rest=${r#*$'\t'}; reason=${rest%%$'\t'*}; v=out
        if [[ $rest == *$'\t'* ]]; then
          v=extra; for c in ${rest#*$'\t'}; do
            st=$(awk -F'\t' -v c="$c" '$5==c{print $8; exit}' docs/porting/MAP.tsv)
            [[ $st == done || $st == accepted ]] || { v=out; break; }
          done
        fi
        break
      fi
    done
    printf '%s\t%s\t%s\t%s\t%s\n' "$suite" "$name" "$v" "$reason" "$note"
  done
} >"$out"
awk -F'\t' '!/^#/ && NR>1 && $1!="suite"{n[$1" "$3]++} END{for(k in n) print k, n[k]}' "$out" | sort
