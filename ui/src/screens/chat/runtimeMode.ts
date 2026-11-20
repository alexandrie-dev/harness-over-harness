// Режимы доступа окна («confirm», «full») — это режимы T3 «approval-required» и «full-access» из runtimeModeConfig.ts (лежит в src/t3 как есть).
import type { LucideIcon } from 'lucide-react';
import { resolveRuntimeModeOption as t3Option } from '../../t3/web/components/chat/runtimeModeConfig';
import type { AccessMode } from './transport';

const T3_MODE = { confirm: 'approval-required', full: 'full-access' } as const;
export interface RuntimeModeOption { mode: AccessMode; label: string; description: string; Icon: LucideIcon }
export function resolveRuntimeModeOption(mode: AccessMode): RuntimeModeOption {
  const option = t3Option(T3_MODE[mode]);
  return { mode, label: option.label, description: option.description, Icon: option.icon };
}
