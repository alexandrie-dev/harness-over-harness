export function readAppliedTrust(storage: Storage, id: string): { cwd: string; executor: string } | null;
export function writeAppliedTrust(storage: Storage, id: string, value: { cwd: string; executor: string } | null): void;
