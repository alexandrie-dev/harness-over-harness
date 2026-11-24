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
import { getHohSessionStatsSnapshot } from "../../../../../t3-entry/session-stats";

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
}

export function ThreadDetailsPanel(props: ThreadDetailsPanelProps) {
  const visible = useClientSettings(resolveThreadDetailsVisibility);
  const snapshot = getHohSessionStatsSnapshot();
  const noData = "нет данных";
  const number = (value: number | undefined) =>
    value === undefined ? noData : value.toLocaleString("ru-RU");
  const status = (value: string) =>
    ({ running: "идёт", finished: "закончено", failed: "ошибка", unknown: "неизвестно" })[
      value
    ] ?? "неизвестно";
  const duration = (value: number | undefined) => {
    if (value === undefined) return noData;
    const seconds = Math.floor(value / 1_000);
    const minutes = Math.floor(seconds / 60);
    return `${minutes} мин ${seconds % 60} с`;
  };
  const total = snapshot.tokens.providerTotal ?? snapshot.tokens.conversation;
  const context = snapshot.context;
  const contextFill =
    context?.usedTokens !== undefined && context.maxTokens !== undefined && context.maxTokens > 0
      ? Math.max(0, Math.min(100, (context.usedTokens / context.maxTokens) * 100))
      : null;
  const contextTick =
    context?.autoCompactThreshold !== undefined &&
    context.maxTokens !== undefined &&
    context.maxTokens > 0
      ? Math.max(0, Math.min(100, (context.autoCompactThreshold / context.maxTokens) * 100))
      : null;
  const row = (label: string, value: string, detail?: string, key?: string) => (
    <ThreadDetailsControl
      key={key}
      render={<div />}
      part="row"
      multiline={detail !== undefined}
      aria-label={`${label}: ${value}`}
    >
      <span className="min-w-0 flex-1">
        <span className={THREAD_DETAILS_PANEL_LABEL_CLASS}>{label}</span>
        {detail ? (
          <span className="mt-1 block truncate text-xs font-normal text-muted-foreground">
            {detail}
          </span>
        ) : null}
      </span>
      <span className="max-w-44 truncate text-end font-normal text-muted-foreground tabular-nums">
        {value}
      </span>
    </ThreadDetailsControl>
  );

  return (
    <ThreadDetailsCard
      threadRef={{ environmentId: props.environmentId, threadId: props.threadId }}
      anchor={props.anchor}
      handle={props.handle}
      contentKey={`hoh-session-stats:${snapshot.revision}`}
    >
      {() => (
        <div className="group/details-items">
          <ThreadDetailsSection
            headingId="thread-details-goal-heading"
            title="Цель и план"
            separated={false}
          >
            <div className="flex flex-col">
              {row("Цель", snapshot.goal ?? noData)}
              {row(
                "План",
                snapshot.plan?.text ?? "нет",
                snapshot.plan?.source === "todoWrite"
                  ? "TodoWrite"
                  : snapshot.plan?.source === "exitPlanMode"
                    ? "ExitPlanMode"
                    : undefined,
              )}
            </div>
          </ThreadDetailsSection>

          <ThreadDetailsSection headingId="thread-details-tokens-heading" title="Токены">
            <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] gap-x-3 px-2.5 text-xs">
              <span className="text-muted-foreground">Тип</span>
              <span className="text-end text-muted-foreground">За ход</span>
              <span className="text-end text-muted-foreground">Всего</span>
              {[
                ["Вход", snapshot.tokens.turn?.inputTokens, total?.inputTokens],
                ["Выход", snapshot.tokens.turn?.outputTokens, total?.outputTokens],
                ["Кэш · чтение", snapshot.tokens.turn?.cacheReadTokens, total?.cacheReadTokens],
                ["Кэш · запись", snapshot.tokens.turn?.cacheWriteTokens, total?.cacheWriteTokens],
                ["Рассуждение", snapshot.tokens.turn?.reasoningTokens, total?.reasoningTokens],
              ].map(([label, turn, conversation]) => (
                <div
                  key={String(label)}
                  className="col-span-3 grid h-6 grid-cols-subgrid items-center"
                >
                  <span className="truncate text-muted-foreground">{label}</span>
                  <span className="text-end font-medium tabular-nums">{number(turn as number)}</span>
                  <span className="text-end font-medium tabular-nums">
                    {number(conversation as number)}
                  </span>
                </div>
              ))}
            </div>
            <div className="mt-1 flex flex-col">
              {snapshot.provider === "codex"
                ? null
                : row(
                    "Стоимость Claude",
                    snapshot.costUsd === undefined ? noData : `$${snapshot.costUsd.toFixed(4)}`,
                  )}
              {snapshot.provider === "claude" && snapshot.tokens.providerTotal
                ? row("Охват итога", "включая субагентов", "modelUsage без двойного счёта")
                : null}
            </div>
          </ThreadDetailsSection>

          {visible.context ? <ThreadDetailsSection headingId="thread-details-context-heading" title="Контекст">
            <div className="grid grid-cols-[auto_minmax(3rem,1fr)_auto] items-center gap-x-3 px-2.5 text-xs">
              <span className="text-muted-foreground">Заполнено</span>
              <span className="relative h-[3px] rounded-full bg-muted-foreground/30 dark:bg-muted-foreground/20">
                {contextFill !== null ? (
                  <span
                    className="absolute inset-y-0 left-0 rounded-full bg-foreground/80"
                    style={{ width: `max(${contextFill}%, 6px)` }}
                  />
                ) : null}
                {contextTick !== null ? (
                  <span
                    aria-hidden
                    className="absolute -top-[3px] h-[9px] w-px -translate-x-1/2 bg-foreground/75"
                    style={{ left: `clamp(0.5px, ${contextTick}%, calc(100% - 0.5px))` }}
                  />
                ) : null}
              </span>
              <span className="text-end font-medium tabular-nums">
                {context?.usedTokens === undefined
                  ? noData
                  : `${number(context.usedTokens)}${context.maxTokens === undefined ? "" : ` / ${number(context.maxTokens)}`}`}
              </span>
            </div>
          </ThreadDetailsSection> : null}

          <ThreadDetailsSection headingId="thread-details-subagents-heading" title="Субагенты">
            <div className="flex flex-col">
              {snapshot.subagents?.length
                ? snapshot.subagents.map((agent) =>
                    row(
                      agent.kind === "codex" ? `${agent.name} · Codex` : agent.name,
                      status(agent.status),
                      [agent.description, agent.note].filter(Boolean).join(" · ") || undefined,
                      agent.callId,
                    ),
                  )
                : row("Субагенты", noData)}
            </div>
          </ThreadDetailsSection>

          <ThreadDetailsSection headingId="thread-details-skills-heading" title="Навыки">
            <div className="flex flex-col">
              {row("Доступны", snapshot.skills.available?.join(", ") || noData)}
              {row(
                "Вызваны",
                snapshot.skills.invoked?.map((skill) => skill.name).join(", ") || noData,
              )}
            </div>
          </ThreadDetailsSection>

          <ThreadDetailsSection
            headingId="thread-details-background-heading"
            title="Фоновые работы и циклы"
          >
            <div className="flex flex-col">
              {snapshot.backgroundJobs?.length
                ? snapshot.backgroundJobs.map((job) =>
                    row(job.label, status(job.status), job.kind, job.id),
                  )
                : row("Работы", noData)}
            </div>
          </ThreadDetailsSection>

          <ThreadDetailsSection headingId="thread-details-other-heading" title="Прочее">
            <div className="flex flex-col">
              {row("Модель", snapshot.model ?? noData)}
              {row("Усилие", snapshot.effort ?? noData)}
              {row("Режим", snapshot.mode ?? noData)}
              {row("Время работы", duration(snapshot.runtimeMs))}
              {row("Ходы", number(snapshot.turnCount))}
              {row("Вызовы инструментов", number(snapshot.toolCallCount))}
              {row(
                "MCP-серверы",
                snapshot.mcpServers
                  ?.map((server) => `${server.name}: ${server.status}`)
                  .join(", ") || noData,
              )}
            </div>
          </ThreadDetailsSection>
        </div>
      )}
    </ThreadDetailsCard>
  );
}
