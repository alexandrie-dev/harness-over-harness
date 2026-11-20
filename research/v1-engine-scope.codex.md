**dsh supplies much of the engine, but no unchanged subset delivers the whole brief.** It already has skills, presets, delegation, durable schedules and result presentation. HoH must add employee memory, task acceptance and publication, preview delivery, and a local OpenAI-compatible adapter.

I read the brief first. This is static source research; runtime behavior is **not checked**. No builds, installs, network access or disk writes were performed.

Below, `P` means `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/`. Counts and statuses come from [MAP.tsv](/home/save/Alexandrie/HarnessOverHarness/docs/porting/MAP.tsv). Its 409 rows contain **205 port/kernel/infra/platform/own entries, totaling 172,272 source lines**.

**Capability findings**

| Capability | Verified implementation and limits |
|---|---|
| **a. Session and loop** | `P/core/{session,agent,agent-loop,system-prompt,tools,tool-executor-native}/src` provide the session, model/tool cycle and execution dispatch. `P/session/{session-persistence-jsonl,session-projection,session-checkpoint-policy}/src` provide durable logs, derived state and checkpoints. |
| **Local model** | `P/llm/llm` defines the adapter contract; `llm-retry` and `token-meter` supply retry/accounting behavior. **`llm-deepseek/src/adapter.ts` calls `/messages`; its config rejects protocol selection.** It cannot become Chat Completions merely by changing the URL. `llm-pi-ai/src/{config,models,adapter}.ts` supports configurable endpoints and OpenAI API families through external pi-ai. For one Rust adapter, implement the `llm` contract directly; local Qwen compatibility is **not checked**. |
| **Files, shell, web** | `P/fs/{fs-local,fs-sandbox,tool-fs}`; `P/shell/{bash-local,bash-sandbox,tool-bash}` over `P/subprocess/*`. `P/web/{web,tool-web}` separate tools from providers. `web-fetch-http` fetches public HTTP(S); `web-deepseek-api` supplies direct search/fetch APIs. `web-search-deepseek` instead makes an auxiliary Messages model request. Search therefore needs a separately configured service/credential; local inference does not supply search. |
| **Permissions, sandbox, compaction** | `P/interaction/{user-approval,permission-presets}` plus `P/sandbox/{sandbox,sandbox-policy,sandbox-local}`. The local backend selects Linux bwrap/Landlock, with other platform branches. `P/compaction/*` and `token-meter` implement summarization, result pruning and image offload. Enforcement on the eventual installation is **not checked**. |
| **b. Skills** | `P/skill/skill/src` merges providers; `skill-filesystem/src/index.ts` discovers `SKILL.md` bundles and flat Markdown under project, user and custom roots, parses frontmatter and observes changes. `tool-skill/src/index.ts` loads instructions and recognizes user `/name` invocation. Creating a skill can use ordinary file-writing tools; loading supplies instructions, rather than executing a compiled workflow. No dedicated skill-creator package was found. `P/preset/agent-preset/skills/dsh-doc` teaches DSH extension development; `P/skill/skill-office` supplies Office skills. |
| **c. Roles and teams** | `P/preset/{agent-preset-registry,agent-preset,persona}` provide named declarative compositions, scoped plugins and persona text. `P/context/agent-instructions` loads AGENTS.md-compatible rules. `P/subagent/*` provides spawn/fork, delegation and messaging; `P/team/{agent-team,tool-agent-team}` adds a shared task board rooted in the lead session. These are employee building blocks; a persistent company employee identity and independently managed memory are not supplied by these services. Exhaustive absence elsewhere is **not checked**. |
| **d. User commands/buttons** | A Markdown skill plus `tool-skill` already gives a command without code. `P/interaction/commands/src/index.ts` registers native slash commands with **function handlers**. `P/client/ui-entry-customization/src/client/index.ts` starts a creator session; it is not a declarative button-action interpreter. HoH needs the button editor and binding. |
| **e. Scheduled/background work** | **Schedules exist.** `P/schedule/schedule/src/{types,index,runtime,domain}.ts` supports after/at/every/daily/weekly/cron and RRULE inputs, persistence and delivery into existing or new sessions. `tool-schedule/src/index.ts` exposes create/list/view/edit. It explicitly warns that only the latest missed occurrence is delivered after downtime and delivery may repeat after a crash. `P/jobs/jobs-local` keeps jobs/output in process memory. `P/goal/goal-round-driver` continues work toward a session goal; it is not cron. `vendor/timer` supplies lifecycle timers. |
| **f. Previews** | `P/attachment/*` stores durable attachments. `P/deliverables/tool-present/src/index.ts` validates files and appends `deliverables/presented`; `deliverable-file-references` contributes prompt guidance. `P/document/{office-kit,tool-office}` converts/renders Office documents. Actual HTML/image/PDF viewing lives in `P/client/ui-sidebar-documentpreview/src/client`, including sandboxed HTML frames. Charts can be presented as HTML/images; a dedicated chart protocol was **not checked**. |

**The window protocol**

[P/sdk/protocol/src/types.ts](/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/sdk/protocol/src/types.ts) and `transport.ts` define newline-delimited JSON-RPC 2.0 over stdio.

Requests: `initialize`, `session/prompt`, `session/query`, `session/resume`, `session/wait`, working-directory get/set, `shutdown`. Reverse requests: `approval/request`, `user-question/ask`. Notifications carry session events/status, command completion, child start/finish and question cancellation.

Prompt input accepts content blocks and encoded raster images. Output carries **full session-event envelopes**, including tool results and attachment references—not a preview renderer or general file-download API. There are no SDK methods for employee/preset editing, schedule management or preview-byte retrieval.

The transport is general-purpose. Coding assumptions appear in workspace/cwd operations, tool/event payloads and bundle persona defaults. `sdk/server/src/server.ts` also imports a DeepSeek fallback adapter. Replace that fallback; keep the transport.

**Recommended port scope and dependency waves**

This is a **118-entry, 109-crate baseline: 114,809 lines, 66.6% of 172,272**; 57,463 lines remain deferred. It preserves existing JSONL compatibility and includes Office handling. A globally smallest implementation is **not checked**: replacing these services could reduce the port while adding new code.

Notation: every name below has prefix **`hoh-`**; numbers are MAP source lines; **D** = done, **R** = review; otherwise todo. `loader` is writing. Waves follow inspected `package.json` dependencies/peers and actual `src` imports, excluding dev dependencies. Wave 0 is a coordinated foundation group because vendor metadata contains back-references.

| Wave | Crates |
|---|---|
| 0 | kernel 4716 D; error 22 D; atomic-write 293 D; home-paths 238 D; launch-environment 137 D; native-command 1052 D; package-manifest 167 D; official-bundles 213 D; loader 8050; node-addon-system 646 D; jsdiff 1421 D |
| 1 | brand 39 D; scope 471 R; timeout 212 D; util-crypto 41 D; util-values 959 D; storage 364 D; fs-patch 350 D; util-time 1708 D; http-proxy 737 D; chunked-list 58 D; util-code-language 233 D; output-retention 460 D |
| 2 | attachment 710 D; storage-domain 910 R; credentials 409 R; session-format 945 D; subprocess 576 D; storage-json 844 |
| 3 | attachment-local 1559 R; llm 4245 R; session-format-v0-to-v1 2879 D; subprocess-local 5104; credentials-local 930 |
| 4 | session 3485; system-prompt 699; skill 902; session-format-v1-to-v2 1256; web 507 |
| 5 | session-projection 1031; session-persistence 636; sandbox 759; deliverable-file-references 30; session-format-v2-to-v3 815; settings 704; skill-office 141; persona 75; web-fetch-http 601 |
| 6 | workspace 1448; fs 640; session-format-v3-to-v4 1702; session-projection-cache 579; shell 377; sandbox-local 686 |
| 7 | agent 1791; office-kit 1399; fs-local 1401; bash-local 404; skill-filesystem 1064 |
| 8 | commands 734; llm-retry 319; file-reference 112; user-approval 430; working-directory 290; time-context 377; session-title 1027; sandbox-policy 328; user-questions 964; jobs 714; web-deepseek-api 1052 |
| 9 | compaction 504; tools 3797; permission-presets 556; jobs-local 1050; fs-sandbox 257; bash-sandbox 206 |
| 10 | compaction-image-offload 197; file-reference-local 523; agent-loop 2149; tool-executor-native 35; tool-present 127; tool-todo 271; agent-preset-registry 1082; tool-fs 2444; tool-ask-user 332; session-checkpoint-policy 83; tool-workspace-dependencies 388; agent-instructions 2114; tool-jobs 698; shell-env 255; tool-skill 452; tool-web 1054 |
| 11 | token-meter 1625; session-format-catalog 154; agent-preset 30; tool-bash 871 |
| 12 | compaction-tool-result-pruner 303; session-query 2241; session-persistence-jsonl 5145 |
| 13 | compaction-basic 1789; subagent 5057 |
| 14 | tool-office 761; schedule 3569; sdk-protocol 571; subagent-spawn-in-process 59; tool-subagent 965; tool-subagent-control 373 |
| 15 | tool-schedule 496; sdk-jsonrpc-server 1049 |

Independent items within each wave can be written in parallel. This is **not an unchanged dependency closure**. Required Rust adaptations are:

- Replace Typert service/decorator glue with Rust services and SDK methods.
- Remove SDK’s DeepSeek fallback and optional goal integration.
- Give `web-deepseek-api` explicit endpoint/credential resolution: its current entry imports DeepSeek model/account helpers.
- Compile only Linux sandbox/process branches initially.
- Bridge Office’s workspace-file API and external LibreOffice Kit/runtime assets.

New code and external runtime sizes are **not checked** and excluded from the total. Selected entries include 29 done and five review rows; status is not verification of completion.

**The other 87 entries stay deferred**

Names retain the same prefix convention; family notation covers the corresponding MAP entries.

| Group | Entries; reason |
|---|---|
| Launch/composition | `apps/hoh`, base, headless, sdk-app, sdk-minimal, commander; replace shipped compositions with one HoH composition |
| DeepSeek-specific | agent-default-model, anonymous-user-id, deepseek-llm-api-extensions, llm-deepseek, llm-deepseek-api-key, plugin-package-inventory-deepseek, session-log-deepseek, authorization, web-search-deepseek; local model/explicit search configuration |
| Alternative execution | terminal, terminal-bash, tool-bash-persistent, tool-working-directory, subagent-fork-in-process; one shell and spawn path suffice |
| Extra coordination | goal, command-goal, tool-goal, goal-round-driver, agent-team, tool-agent-team, plan-mode; scheduling and employee roles do not require them |
| Optional tools/guards | command-compact, fs-observation-policy, tool-fs-search, util-ripgrep, repeat-tool-reminder, tool-call-timeout-policy, spill, spill-local, spill-policy; baseline tools/compaction first |
| History/observability | command-feedback, message-feedback, session-query-sqlite, session-telemetry, session-telemetry-otel, session-title-first-prompt-llm, session-title-llm, otel, session-reference, session-log-export, message-quotes, session-stats, session-turn-outline, storage-sqlite, util-text-search; richer indexing/presentation later |
| Integrations/developer features | mcp-resources, mcp-client, mcp-manager, browser-use, computer-use, harness-dev-context, tmux-context, agent-tool-presentation, workspace-changes, tool-str-replace-editor, my-decoration, demo-mcp, acp, acp-app, lsp, lsp-stdio, tool-lsp, ssh, fs-ssh, sandbox-ssh, ssh-helper-runtime, subprocess-ssh; outside first acceptance flow |
| Platforms/utilities | sandbox-windows-acl, pwsh-local, pwsh-sandbox, tool-pwsh, tool-pwsh-persistent, win32-process, deque, util-workspace-path; later platforms/unused path |
| Validation infrastructure | agent-loop-testkit, llm-mock-server, llm-replay, loader-smoke, conformance, tool-testkit; separate validation work, not runtime requirements |

**New HoH work — proposed attachment points, not checked implementations**

- **Employees:** persistent employee ID, role, skill selection, permissions and memory domain over `storage-domain`; materialize through presets/persona/instructions and session creation.
- **“Task is a plugin”:** draft → questions → example → explicit **yes** → immutable approved version. Attach to tools, skills, user questions/approval and storage. Add manifests, validation, versioning, constrained script execution and recorded acceptance. A tool permission approval alone is not task publication.
- **Schedules:** reuse dsh’s scheduler; add window management APIs and approved-task/employee targets. Define duplicate-delivery handling.
- **Window/security:** add authorized artifact reads, isolated previews, employee/task/schedule CRUD and an allowlisted signed runtime supply chain. dsh’s npm/plugin-creation machinery is not the approved-task contract.