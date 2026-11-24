import type { EnvironmentId } from "@t3tools/contracts";

import { PierreEntryIcon } from "./PierreEntryIcon";

export function WorkspaceEntryIcon(props: {
  path: string;
  environmentId?: EnvironmentId | null | undefined;
  cwd?: string | undefined;
  theme: "light" | "dark";
}) {
  return (
    <PierreEntryIcon
      pathValue={props.path}
      kind="file"
      theme={props.theme}
    />
  );
}

export function WorkspaceEntryTooltip(props: {
  path: string;
  label?: string;
  environmentId?: EnvironmentId | null | undefined;
  cwd?: string | undefined;
}) {
  return <>{props.label ?? props.path}</>;
}
