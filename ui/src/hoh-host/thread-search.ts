import {
  foldSearchQuery, messageMatcher, messageSearchTokens, rankMessageMatches,
  recordThreadOpen, type ThreadSearchSubject,
} from '../t3/base/shared/paletteSearch.ts';
import { assistantCitationsToPlainText } from '../t3/base/shared/assistantCitations.ts';
import { isAttachmentOnlyBootstrapPrompt } from '../t3/base/shared/composerContextReferences.ts';
import { searchableMessageSegments, searchablePlanSegments } from '../t3/base/shared/threadFindText.ts';
import { countThreadSearchOccurrences } from '../t3/base/shared/threadSearch.ts';
import type { InlineSkill } from '../t3/base/shared/inlineSkills.ts';
import { LRUCache } from '../t3/base/web/lib/lruCache.ts';

export { recordThreadOpen };

export interface SearchableTurnItem {
  readonly entryId: string;
  readonly runId: string | null;
  readonly role: 'user' | 'assistant' | 'plan';
  readonly text: string;
  readonly streaming?: boolean;
  readonly hasContext?: boolean;
}

interface FindMatch { entryId: string; runId: string | null; occurrence: number }
interface FindEntry { entryId: string; runId: string | null; startIndex: number; count: number }

export interface FindInput {
  readonly query: string;
  readonly skills?: readonly InlineSkill[];
  readonly index?: number;
  readonly offset?: number;
  readonly start?: Omit<FindMatch, 'runId'>;
}
export interface FindResult {
  snapshotSequence: number;
  totalMatches: number;
  activeIndex: number;
  match: FindMatch | null;
  complete?: boolean;
  navigation?: FindEntry[];
}

const rendered = new LRUCache<readonly string[]>(8192, 4 * 1024 * 1024);
const yieldToWindow = () => new Promise<void>(resolve => setTimeout(resolve, 0));

function renderText(item: SearchableTurnItem, cwd: string | undefined, skills: readonly InlineSkill[], skillKey: string) {
  const key = JSON.stringify([item.role, item.streaming, item.hasContext, cwd, skillKey, item.text]);
  const cached = rendered.get(key);
  if (cached) return cached;
  const segments = item.role === 'plan'
    ? searchablePlanSegments(item.text, cwd)
    : searchableMessageSegments({
      role: item.role, text: item.text, streaming: item.streaming === true,
      context: item.hasContext ? { version: 1, records: [] } : undefined,
    }, cwd, skills) ?? [];
  rendered.set(key, segments, 2 * (key.length + segments.reduce((length, segment) => length + segment.length, 0)));
  return segments;
}

export async function findInThread(
  items: readonly SearchableTurnItem[],
  cwd: string | undefined,
  snapshotSequence: number,
  input: FindInput,
  onProgress?: (result: FindResult) => void,
  signal?: AbortSignal,
): Promise<FindResult> {
  const query = typeof input.query === 'string' ? input.query.trim().slice(0, 200) : '';
  if (!query) return { snapshotSequence, totalMatches: 0, activeIndex: 0, match: null, navigation: [] };
  const skills = Array.isArray(input.skills) ? input.skills : [];
  const skillKey = JSON.stringify(skills);
  const anchor = Math.max(0, items.findIndex(item => item.entryId === input.start?.entryId));
  const counts: number[] = [];
  let announced = false;
  let yieldAt = performance.now() + 8;
  for (let i = 0; i < items.length; i++) {
    signal?.throwIfAborted();
    const position = (i + anchor) % items.length;
    const item = items[position];
    const count = renderText(item, cwd, skills, skillKey)
      .reduce((sum, segment) => sum + countThreadSearchOccurrences(segment, query), 0);
    counts[position] = count;
    if (onProgress && !announced && input.index === undefined && !input.offset) {
      const occurrence = i === 0 && item.entryId === input.start?.entryId ? input.start.occurrence : 0;
      if (occurrence < count) {
        announced = true;
        onProgress({
          complete: false, snapshotSequence, totalMatches: 1, activeIndex: 0,
          match: { entryId: item.entryId, runId: item.runId, occurrence },
        });
        await yieldToWindow();
      }
    }
    if (performance.now() >= yieldAt) {
      await yieldToWindow();
      yieldAt = performance.now() + 8;
    }
  }
  let totalMatches = 0;
  const entries: FindEntry[] = [];
  for (const [position, count] of counts.entries()) {
    if (count === 0) continue;
    const { entryId, runId } = items[position];
    entries.push({ entryId, runId, startIndex: totalMatches, count });
    totalMatches += count;
  }
  if (!totalMatches) return { snapshotSequence, totalMatches, activeIndex: 0, match: null, navigation: [] };
  const start = entries.find(entry => entry.entryId === input.start?.entryId);
  const requested = start && input.start ? start.startIndex + Math.min(input.start.occurrence, start.count) : 0;
  const activeIndex = input.index === undefined
    ? ((requested + (input.offset ?? 0)) % totalMatches + totalMatches) % totalMatches
    : Math.min(Math.max(input.index, 0), totalMatches - 1);
  const selected = entries.findIndex(entry => activeIndex < entry.startIndex + entry.count);
  const entry = entries[selected];
  return {
    snapshotSequence, totalMatches, activeIndex,
    match: { entryId: entry.entryId, runId: entry.runId, occurrence: activeIndex - entry.startIndex },
    navigation: entries.length <= 17 ? entries : Array.from({ length: 17 }, (_, index) =>
      entries[(selected + index - 8 + entries.length) % entries.length]),
  };
}

export interface SearchableMessage {
  readonly id: string;
  readonly role: 'user' | 'assistant';
  readonly text: string;
  readonly createdAt: number;
}
interface MessageSource {
  readonly threadId: string;
  readonly projectId: string;
  readonly subject: ThreadSearchSubject;
  readonly messages: Iterable<SearchableMessage>;
}
interface MessageHit {
  threadId: string;
  projectId: string;
  messageId: string;
  messageCreatedAt: string;
  source: SearchableMessage['role'];
  snippet: string;
  quality: number;
  hits: number;
}

// Adapted from T3 Code's ThreadSearch.ts. The palette shows one line of each snippet.
function searchSnippet(text: string, offset: number) {
  let start = Math.max(0, offset - 24);
  const space = text.slice(start, offset).search(/\s/);
  if (start > 0 && space !== -1) start += space + 1;
  const lowSurrogate = (code: number) => code >= 0xdc00 && code <= 0xdfff;
  if (lowSurrogate(text.charCodeAt(start))) start++;
  const rawEnd = Math.min(text.length, start + 480);
  const shown = text.slice(start, rawEnd).replace(/\*\*|`+/g, '').replace(/\s+/g, ' ').trim();
  let end = Math.min(shown.length, 238);
  if (lowSurrogate(shown.charCodeAt(end))) end--;
  return (start > 0 ? '…' : '') + shown.slice(0, end) + (end < shown.length || rawEnd < text.length ? '…' : '');
}

export function hasMessageSearchTerms(query: string) {
  return messageSearchTokens(query).long.length > 0;
}

/** Match full completed messages as displayed. The input order is newest chats, then newest messages. */
export async function searchThreadMessages(sources: AsyncIterable<MessageSource>, query: string, limit: number, now: number) {
  const matchMessage = messageMatcher(query);
  const best = new Map<string, MessageHit>();
  const subjects = new Map<string, ThreadSearchSubject>();
  // Same work ceiling as the upstream scan without an index.
  let remaining = 20_000;
  let yieldAt = performance.now() + 8;
  for await (const source of sources) {
    subjects.set(source.threadId, source.subject);
    for (const message of source.messages) {
      if (remaining-- <= 0) break;
      if (performance.now() >= yieldAt) {
        await yieldToWindow();
        yieldAt = performance.now() + 8;
      }
      if (message.role === 'user' && isAttachmentOnlyBootstrapPrompt(message.text)) continue;
      const text = assistantCitationsToPlainText(message.text).replace(/!?\[([^\]\n]*)\]\([^\)\s]*\)/g,
        (_link, label: string) => label.replace(/\\([*_])/g, '$1'));
      const match = matchMessage(text);
      if (!match) continue;
      const previous = best.get(source.threadId);
      if (previous && previous.quality > match.quality) continue;
      const hits = previous?.quality === match.quality ? previous.hits + 1 : 1;
      if (previous?.quality === match.quality && (previous.source === 'user' || message.role === 'assistant')) {
        previous.hits = hits;
        continue;
      }
      best.set(source.threadId, {
        threadId: source.threadId, projectId: source.projectId, messageId: message.id,
        source: message.role, messageCreatedAt: new Date(message.createdAt).toISOString(),
        snippet: searchSnippet(text, match.offset), quality: match.quality, hits,
      });
    }
    if (remaining <= 0) break;
  }
  return rankMessageMatches([...best.values()], hit => subjects.get(hit.threadId), foldSearchQuery(query), now)
    .slice(0, limit);
}
