// Прослойка окна HoH вместо @t3tools/client-runtime/contract-labels (T3 Code, MIT): ограничения «организации» (managed policy) в окне нет,
// поэтому ни один режим доступа не запрещён политикой.
export function managedRuntimeModeDenial(_summary?: unknown, _mode?: string): string | undefined { return undefined; }
