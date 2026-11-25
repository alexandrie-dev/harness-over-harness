import type {
  EditorId,
  EnvironmentId,
  ProjectId,
  ProjectScript,
  ResolvedKeybindingsConfig,
  ThreadId,
} from "@t3tools/contracts";

import { scopeProjectRef } from "@t3tools/client-runtime/environment";
import { isScratchProject } from "@t3tools/client-runtime/state/projects";

import type { ComposerThreadTarget, DraftId } from "../../composerDraftStore";
import { useT3ProjectFileScripts } from "../../hooks/useT3ProjectFileScripts";
import { type EnvMode, type EnvironmentOption } from "../BranchToolbar.logic";
import { BranchToolbar } from "../BranchToolbar";
import BaseGitActionsControl from "../GitActionsControl";
import ProjectScriptsControl, {
  type NewProjectScriptInput,
  type ProjectScriptActionResult,
} from "../ProjectScriptsControl";
import { T, useGT } from "gt-react";
import { memo, useState, type ComponentProps } from "react";
import { ThreadDetailsCard } from "./ThreadDetailsCard";
import { OpenInPicker } from "./OpenInPicker";
import { ThreadDetailsSection } from "./ThreadDetailsSection";
import { ThreadAutomationsPanel } from "./ThreadAutomationsPanel";
import { ThreadRelationshipsPanel } from "./ThreadRelationshipsControl";
import { ThreadServersPanel } from "./ThreadServersPanel";
import {
  ThreadDetailsContext,
  ThreadDetailsUsage,
  type ThreadDetailsUsageInput,
} from "./ThreadDetailsUsage";
import { ThreadDetailsItemsMenuItems } from "./ThreadDetailsItemsMenu";
import { resolveThreadDetailsVisibility, THREAD_DETAILS_ITEMS } from "./threadDetailsItems";
import { useClientSettings } from "../../hooks/useSettings";
import { useProject, useServerConfigs } from "../../state/entities";
import { ThreadDetailsThreadPrRows } from "./ThreadDetailsPrRows";
import { cn } from "../../lib/utils";
import { SlidersHorizontalIcon } from "lucide-react";
import { ThreadDetailsControl } from "./ThreadDetailsControl";
import {
  THREAD_DETAILS_PANEL_ICON_CLASS,
  THREAD_DETAILS_PANEL_LABEL_CLASS,
} from "./threadDetailsPanelStyles";
import { Menu, MenuPopup } from "../ui/menu";
import * as DateTime from "effect/DateTime";
import type { ThreadStructure } from "@t3tools/client-runtime/state/models";

const GitActionsControl = memo(BaseGitActionsControl);

interface ThreadDetailsPanelProps extends Pick<
  ComponentProps<typeof ThreadDetailsCard>,
  "anchor" | "handle"
> {
  forceNewWorktree?: boolean;
  environmentId: EnvironmentId;
  threadId: ThreadId;
  projectId: ProjectId;
  worktreePath: string | null;
  isServerThread: boolean;
  draftId?: DraftId;
  composerDraftTarget: ComposerThreadTarget;
  activeProjectName: string | undefined;
  activeProjectScripts: ReadonlyArray<ProjectScript> | undefined;
  preferredScriptId: string | null;
  keybindings: ResolvedKeybindingsConfig;
  availableEditors: ReadonlyArray<EditorId>;
  showOpenInPicker: boolean;
  openInShortcutEnabled: boolean;
  gitCwd: string | null;
  workspaceMutationId: string | null;
  isGitRepo: boolean;
  envLocked: boolean;
  availableEnvironments: readonly EnvironmentOption[];
  autoEnvironmentLabel?: string | undefined;
  onAutoEnvironment?: (() => void) | undefined;
  onEnvironmentChange?: (environmentId: EnvironmentId) => void;
  onEnvModeChange: (mode: EnvMode) => void;
  envMode: EnvMode;
  activeThreadBranchOverride?: string | null;
  onActiveThreadBranchOverrideChange?: (branch: string | null) => void;
  startFromOrigin: boolean;
  onStartFromOriginChange: (startFromOrigin: boolean) => void;
  onCheckoutPullRequestRequest?: (reference: string) => void;
  onComposerFocusRequest: () => void;
  onOpenChanges?: () => void;
  onRunProjectScript: (script: ProjectScript) => void;
  onAddProjectScript: (input: NewProjectScriptInput) => Promise<ProjectScriptActionResult>;
  onUpdateProjectScript: (
    scriptId: string,
    input: NewProjectScriptInput,
  ) => Promise<ProjectScriptActionResult>;
  onDeleteProjectScript: (scriptId: string) => Promise<ProjectScriptActionResult>;
  usage: ThreadDetailsUsageInput;
  projection: ThreadStructure | null;
}

export function ThreadDetailsPanel(props: ThreadDetailsPanelProps) {
  const visible = useClientSettings(resolveThreadDetailsVisibility);
  const projection = props.projection;
  const turns = projection?.providerTurns ?? [];
  const usageTurns = turns.filter((turn) => turn.tokenUsage != null);
  const runs = projection?.runs ?? [];
  const lastRun = runs.at(-1);
  const current = turns.find((turn) => turn.ordinal === lastRun?.ordinal)?.tokenUsage;
  const total = (key: "inputTokens" | "outputTokens" | "cachedInputTokens" | "reasoningOutputTokens") => {
    const values = usageTurns.map((turn) => turn.tokenUsage?.[key]).filter((value): value is number => typeof value === "number");
    return values.length ? values.reduce((sum, value) => sum + value, 0) : undefined;
  };
  const number = (value: number | null | undefined) => value == null ? "—" : value.toLocaleString("en-US");
  const context = props.usage.contextWindow;
  const goalItem = projection?.turnItems.find((item) => item.type === "user_message");
  const goal = goalItem?.type === "user_message" ? goalItem.text.trim() : "";
  const plan = projection?.plans.findLast((item) => item.status === "active" && item.kind === "proposed_plan");
  const effort = projection?.thread.modelSelection.options?.find((option) => option.id === "effort" || option.id === "reasoningEffort")?.value;
  const toolCalls = projection?.turnItems.filter((item) => ["command_execution", "file_change", "file_search", "web_search", "dynamic_tool", "proposed_plan"].includes(item.type)).length ?? 0;
  const completed = runs.flatMap((run) => {
    if (!run.startedAt || !run.completedAt) return [];
    const duration = DateTime.toEpochMillis(run.completedAt) - DateTime.toEpochMillis(run.startedAt);
    return Number.isFinite(duration) && duration >= 0 ? [duration] : [];
  });
  const rows = [
    ["Input", current?.inputTokens, total("inputTokens")],
    ["Output", current?.outputTokens, total("outputTokens")],
    ["Cache read", current?.cachedInputTokens, total("cachedInputTokens")],
  ].filter(([, last, sum]) => last != null || sum != null);
  const row = (label: string, value: string) => (
    <ThreadDetailsControl key={label} render={<div />} part="row" aria-label={`${label}: ${value}`}>
      <span className={THREAD_DETAILS_PANEL_LABEL_CLASS}>{label}</span>
      <span className="ml-auto max-w-44 truncate font-normal text-muted-foreground tabular-nums">{value}</span>
    </ThreadDetailsControl>
  );
  return (
    <ThreadDetailsCard threadRef={{ environmentId: props.environmentId, threadId: props.threadId }}
      anchor={props.anchor} handle={props.handle} contentKey={`hoh-stats:${turns.length}:${runs.length}:${visible.context}`}>
      {() => <div className="group/details-items">
        {!props.isServerThread && !projection ? <ThreadDetailsSection headingId="chat-details-new-heading" title="Chat details">
          {row("Status", "New chat")}
        </ThreadDetailsSection> : null}
        {goal || (plan && plan.kind === "proposed_plan" && plan.markdown) ? <ThreadDetailsSection headingId="chat-details-goal-heading" title="Goal and plan">
          {goal ? row("Goal", goal) : null}
          {plan && plan.kind === "proposed_plan" && plan.markdown ? row("Plan", plan.markdown) : null}
        </ThreadDetailsSection> : null}
        {rows.length ? <ThreadDetailsSection headingId="chat-details-tokens-heading" title="Tokens">
          <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] gap-x-3 px-2.5 text-xs">
            <span>Type</span><span>Last turn</span><span>Total</span>
            {rows.map(([label, last, sum]) => <div key={String(label)} className="col-span-3 grid h-6 grid-cols-subgrid items-center">
              <span>{label}</span><span className="text-end tabular-nums">{number(last as number | undefined)}</span><span className="text-end tabular-nums">{number(sum as number | undefined)}</span>
            </div>)}
          </div>
        </ThreadDetailsSection> : null}
        {visible.context && context ? <ThreadDetailsSection headingId="chat-details-context-heading" title="Context">
          {row("Used", `${number(context.usedTokens)}${context.maxTokens == null ? "" : ` / ${number(context.maxTokens)}`}`)}
        </ThreadDetailsSection> : null}
        {projection ? <ThreadDetailsSection headingId="chat-details-other-heading" title="Details">
          {projection.thread.modelSelection.model && projection.thread.modelSelection.model !== "unknown" ? row("Model", projection.thread.modelSelection.model) : null}
          {typeof effort === "string" && effort ? row("Effort", effort) : null}
          {projection.thread.runtimeMode && projection.thread.runtimeMode !== "unknown" ? row("Mode", ({ "approval-required": "Supervised", "full-access": "Full access", "auto-accept-edits": "Auto-accept edits", "plan": "Plan" } as Record<string, string>)[projection.thread.runtimeMode] ?? projection.thread.runtimeMode) : null}
          {runs.length ? row("Turns", number(runs.length)) : null}
          {toolCalls ? row("Tool calls", number(toolCalls)) : null}
          {completed.length ? row("Run time", `${Math.floor(completed.reduce((a, b) => a + b, 0) / 1000)} s`) : null}
        </ThreadDetailsSection> : null}
      </div>}
    </ThreadDetailsCard>
  );
}
