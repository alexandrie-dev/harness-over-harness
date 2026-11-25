import { managedRuntimeModeDenial } from "@t3tools/client-runtime/contract-labels";
import type { ManagedPolicySummary, RuntimeMode } from "@t3tools/contracts";
import { msg } from "gt-react";
import { m, type MessageResolver } from "~/i18n/messages";
import {
  type LucideIcon,
  LockIcon,
  PenLineIcon,
  ShieldCheckIcon,
  ShieldOffIcon,
} from "lucide-react";

export const runtimeModeConfig: Record<
  RuntimeMode,
  { label: string; description: string; icon: LucideIcon }
> = {
  unknown: { label: msg("Unknown"), description: msg("Waiting for the conversation access mode."), icon: LockIcon },
  "approval-required": {
    label: msg("Supervised", { $context: "permission mode, agent asks before acting" }),
    description: msg("Ask before commands and file changes."),
    icon: LockIcon,
  },
  "auto-accept-edits": {
    label: msg("Auto-accept edits"),
    description: msg("Auto-approve edits, ask before other actions."),
    icon: PenLineIcon,
  },
  auto: {
    label: msg("Auto", { $context: "permission mode, provider approves routine actions" }),
    description: msg("Supported providers approve routine actions; others still ask."),
    icon: ShieldCheckIcon,
  },
  "full-access": {
    label: msg("Full access"),
    description: msg("Allow commands and edits without prompts."),
    // An open lock reads like a closed one at small sizes; a crossed shield does not.
    icon: ShieldOffIcon,
  },
};

export const runtimeModeOptions = (Object.keys(runtimeModeConfig) as RuntimeMode[]).filter((mode) => mode !== "unknown");

/** Undefined keeps T3's legacy "all modes"; an explicit empty list means access is unknown. */
export function resolveCompatibleRuntimeMode(
  runtimeMode: RuntimeMode,
  supportedRuntimeModes: ReadonlyArray<RuntimeMode> | undefined,
) {
  const modes =
    supportedRuntimeModes === undefined
      ? runtimeModeOptions
      : runtimeModeOptions.filter((mode) => supportedRuntimeModes.includes(mode));
  return { modes, mode: runtimeMode === "unknown" ? runtimeMode : modes.includes(runtimeMode) ? runtimeMode : (modes[0] ?? runtimeMode) };
}

export function resolveRuntimeModeOption(
  mode: RuntimeMode,
  resolve: MessageResolver = m,
): {
  mode: RuntimeMode;
  label: string;
  description: string;
  icon: LucideIcon;
  disabledReason?: string;
} {
  const config = runtimeModeConfig[mode];
  const option = {
    mode,
    ...config,
    label: resolve(config.label),
    description: resolve(config.description),
  };
  // HoH implements only Supervised and Full access end to end for both executors.
  return mode === "auto-accept-edits" || mode === "auto"
    ? { ...option, disabledReason: resolve(msg("Coming Soon")) }
    : option;
}

/** Why `mode` is unavailable under `managedPolicy`, or undefined when it is allowed. */
export function runtimeModeDenial(
  managedPolicy: ManagedPolicySummary | undefined,
  mode: RuntimeMode,
  resolve: MessageResolver = m,
) {
  const ceiling = managedPolicy?.limits?.maxRuntimeMode;
  if (ceiling === undefined || managedRuntimeModeDenial(managedPolicy, mode) === undefined) {
    return undefined;
  }
  return resolve(
    msg("{organization} limits chats to {mode}.", {
      organization: managedPolicy!.organization,
      mode: resolve(runtimeModeConfig[ceiling].label),
    }),
  );
}

export function resolveRuntimeModeOptions(
  modes: ReadonlyArray<RuntimeMode>,
  managedPolicy: ManagedPolicySummary | undefined,
  resolve: MessageResolver = m,
) {
  return modes.map((mode) => {
    const option = resolveRuntimeModeOption(mode, resolve);
    const disabledReason = runtimeModeDenial(managedPolicy, mode, resolve);
    return disabledReason === undefined ? option : { ...option, disabledReason };
  });
}
