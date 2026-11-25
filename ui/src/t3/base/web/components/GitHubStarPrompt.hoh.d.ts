// Объявления web/components/GitHubStarPrompt.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type EnvironmentId } from "@t3tools/contracts";
/**
 * Counts a sent prompt. After more than five, asks once per session for a star until dismissed,
 * unless the environment's GitHub account already starred the repo. Waits until the user is idle.
 */
export declare function recordPromptSent(environmentId: EnvironmentId): Promise<void>;
