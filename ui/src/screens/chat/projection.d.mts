import type { SessionEvent } from '../../protocol';
export type MessageEntry = { id: string; kind: 'user' | 'assistant'; text: string; pending?: boolean; failed?: boolean; streaming?: boolean; at?: number };
export type WorkEntry = { id: string; kind: 'step'; operation: 'read' | 'write' | 'command' | 'action'; status: 'running' | 'done' | 'error' | 'stopped'; label: string; file: string; detail: string; exitCode?: number };
export type ChatEntry = MessageEntry | WorkEntry;
export function contentText(value: unknown): string;
export function humanFile(value: unknown, folder: string): string;
export function visibleText(value: unknown, folder: string): string;
export function windowText(value: unknown, folder: string, fallback?: string, toolName?: string): string;
export function numberInWords(value: number): string;
export function assistantChunks(event: SessionEvent): string[];
export class ChatProjection {
  constructor(folder?: string);
  folder: string;
  entries: ChatEntry[];
  running: boolean;
  optimistic(value: string): string;
  receipt(localId: string, messageId: string): void;
  failed(localId: string): void;
  accept(event: SessionEvent): boolean;
  event(event: SessionEvent): void;
  apply(event: SessionEvent): void;
  assistant(event: SessionEvent, text: string, streaming?: boolean): void;
  setRunning(running: boolean): void;
  stop(): void;
}
