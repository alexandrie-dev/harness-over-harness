export function isKnownAbsoluteFolder(cwd: unknown): cwd is string;
export function folderTrustView(input: { running?: boolean; applied?: boolean; trusted: boolean; isolated?: boolean; changed: boolean }): { text: string; canTrust: boolean; canRevoke: boolean };
