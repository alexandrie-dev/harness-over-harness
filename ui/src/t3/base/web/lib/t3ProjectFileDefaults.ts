import type { EnvironmentId, T3ProjectFile } from "@t3tools/contracts";

export async function readT3ProjectFile(
  environmentId: EnvironmentId,
  workspaceRoot: string,
): Promise<T3ProjectFile | null> {
  // HoH does not expose projects.readFile.
  void environmentId;
  void workspaceRoot;
  return null;
}
