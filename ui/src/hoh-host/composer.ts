// What T3's composer offers beyond text (HOH-41 package 1): `/` and `$` (the commands and skills Claude
// itself declares for the folder, `hoh/executor/commands`), `@` (the folder's files, `hoh/workspace/entries`)
// and pictures (T3 uploads them: `attachments.createUploadUrl`, then the bytes to that address; the older
// `assets.persistChatAttachments` with data URLs too; then the bytes go to the engine with `session/prompt`).

/** T3's send-turn limits (packages/contracts/src/chatAttachment.ts). */
export const MAX_ATTACHMENTS = 100;
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
export const MAX_TOTAL_IMAGE_BYTES = 80 * 1024 * 1024;
const MAX_IMAGE_DATA_URL_CHARS = 14_000_000;
/** The picture types Claude takes (`PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES`); the engine's store takes the same four. */
export const IMAGE_TYPES: ReadonlySet<string> = new Set(['image/gif', 'image/jpeg', 'image/png', 'image/webp']);

/** A picture the window gave, kept until its message goes to the engine (which keeps its own copy). */
export interface KeptImage { readonly id: string; readonly name: string; readonly mimeType: string; readonly sizeBytes: number; readonly data: string }

/** The decoded size of canonical base64, or null when it is not canonical base64. */
function base64Bytes(data: string): number | null {
  if (data.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(data)) return null;
  const padding = data.endsWith('==') ? 2 : data.endsWith('=') ? 1 : 0;
  return (data.length / 4) * 3 - padding;
}

/**
 * T3's `assets.persistChatAttachments` checks (apps/server ws.ts, contracts' limits) for pictures sent as data URLs:
 * the type is one Claude takes, the URL's type is the one named, the bytes are base64 of exactly `sizeBytes`, at most
 * 10 MiB a picture, 80 MiB and 100 pictures in all. The answer is the pictures to keep, or T3's words for the refusal.
 */
export function readUploads(attachments: unknown, newId: () => string): { images: KeptImage[] } | { error: string } {
  const list = Array.isArray(attachments) ? attachments : [];
  if (list.length > MAX_ATTACHMENTS) return { error: `You can attach up to ${MAX_ATTACHMENTS} files per message or question response.` };
  const images: KeptImage[] = [];
  let total = 0;
  for (const entry of list) {
    const name = typeof entry?.name === 'string' && entry.name.trim() ? entry.name.trim().slice(0, 255) : 'image';
    const mimeType = typeof entry?.mimeType === 'string' ? entry.mimeType.trim().toLowerCase() : '';
    if (entry?.type !== 'image') return { error: 'This server does not support file attachments.' };
    if (!IMAGE_TYPES.has(mimeType)) return { error: `'${name}' is not a supported image type. Attach GIF, JPEG, PNG, or WebP images.` };
    const sizeBytes = entry.sizeBytes;
    if (typeof sizeBytes !== 'number' || !Number.isInteger(sizeBytes) || sizeBytes < 1) return { error: `Attachment ${name} has an invalid image payload.` };
    if (sizeBytes > MAX_IMAGE_BYTES) return { error: `'${name}' is too large to attach: a picture can be up to 10 MiB.` };
    const dataUrl = typeof entry.dataUrl === 'string' ? entry.dataUrl.trim() : '';
    if (dataUrl.length > MAX_IMAGE_DATA_URL_CHARS) return { error: `'${name}' is too large to attach: a picture can be up to 10 MiB.` };
    const parsed = /^data:([^;,]+);base64,(.*)$/s.exec(dataUrl);
    if (!parsed || parsed[1].trim().toLowerCase() !== mimeType) return { error: `Attachment ${name} has an invalid image payload.` };
    const data = parsed[2];
    const bytes = base64Bytes(data);
    if (bytes === null) return { error: `Attachment ${name} is not valid base64.` };
    if (bytes !== sizeBytes) return { error: `Attachment ${name} size does not match its payload.` };
    total += bytes;
    if (total > MAX_TOTAL_IMAGE_BYTES) return { error: 'Images can total up to 80 MiB per message or question response. Use smaller images or send fewer at once.' };
    const id = typeof entry.id === 'string' && /^[a-z0-9_-]{1,128}$/i.test(entry.id) ? entry.id : newId();
    images.push({ id, name, mimeType, sizeBytes, data });
  }
  return { images };
}

/** T3's upload address for a minted picture (contracts/assets.ts `AttachmentCreateUploadUrlResult`). */
export const UPLOAD_PATH = /^\/api\/attachments\/upload\/([\w.-]{1,256})$/;
/** How long a minted address takes the bytes (T3 `ATTACHMENT_UPLOAD_URL_TTL_MS`). */
export const UPLOAD_TTL_MS = 10 * 60_000;

/** A picture T3 announced with `attachments.createUploadUrl`, waiting for its bytes. */
export interface UploadSlot { readonly name: string; readonly mimeType: string; readonly sizeBytes: number; readonly expiresAt: number }

/**
 * The picture T3 wants to upload (`attachments.createUploadUrl`), as T3's server takes it: an image of a type Claude
 * takes, 1 byte to 10 MiB, and the host keeps at most 100 pictures and 80 MiB not yet sent (`kept`: count and bytes).
 * The answer is the slot, or T3's words for the refusal.
 */
export function uploadSlot(input: any, kept: { count: number; bytes: number }, expiresAt: number): { slot: UploadSlot } | { error: string } {
  const name = typeof input?.name === 'string' && input.name.trim() ? input.name.trim().slice(0, 255) : 'image';
  const mimeType = typeof input?.mimeType === 'string' ? input.mimeType.trim().toLowerCase() : '';
  const sizeBytes = input?.sizeBytes;
  if (input?.type === 'file') return { error: 'This server does not support file attachments.' };
  if (!IMAGE_TYPES.has(mimeType)) return { error: `'${name}' is not a supported image type. Attach GIF, JPEG, PNG, or WebP images.` };
  if (typeof sizeBytes !== 'number' || !Number.isInteger(sizeBytes) || sizeBytes < 1) return { error: `Attachment ${name} has an invalid image payload.` };
  if (sizeBytes > MAX_IMAGE_BYTES) return { error: `'${name}' is too large to attach: a picture can be up to 10 MiB.` };
  if (kept.count >= MAX_ATTACHMENTS) return { error: `You can attach up to ${MAX_ATTACHMENTS} files per message or question response.` };
  if (kept.bytes + sizeBytes > MAX_TOTAL_IMAGE_BYTES) return { error: 'Images can total up to 80 MiB per message or question response. Use smaller images or send fewer at once.' };
  return { slot: { name, mimeType, sizeBytes, expiresAt } };
}

/** The bytes of a picture as the engine takes them (`session/prompt` image blocks): base64. */
export function base64Of(bytes: Uint8Array): string {
  let text = '';
  for (let at = 0; at < bytes.length; at += 0x8000) text += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  return btoa(text);
}

/** The engine's answer to `hoh/executor/commands` as T3's workspace snapshot of that folder (contracts/server.ts). */
export function workspaceSnapshot(cwd: string, checkedAt: string, found: any) {
  const list = (value: unknown) => (Array.isArray(value) ? value : []).filter((entry: any) => typeof entry?.name === 'string' && entry.name.trim());
  return {
    cwd, checkedAt,
    slashCommands: list(found?.commands).map((command: any) => ({
      name: command.name.trim(),
      ...(typeof command.description === 'string' && command.description.trim() ? { description: command.description.trim() } : {}),
      ...(typeof command.argumentHint === 'string' && command.argumentHint.trim() ? { input: { hint: command.argumentHint.trim() } } : {}),
    })),
    // Claude names a skill, not its file: the sentinel satisfies T3's contract while its two openers hide unsafe file access.
    skills: list(found?.skills).map((skill: any) => ({
      name: skill.name.trim(), path: `hoh-skill:${skill.name.trim()}`, enabled: true,
      ...(typeof skill.description === 'string' && skill.description.trim() ? { description: skill.description.trim() } : {}),
    })),
  };
}

/** T3's skill mention (packages/shared composerInlineTokens.ts, ClaudeSkillDispatch.ts): `$name`, not an amount like `$5k`. */
const SKILL_MENTION = /(^|\s)\p{Sc}(?![0-9][0-9_]*(?:[kKmMbBtT]|[eE][0-9]+)?(?:\s|$))(?=[a-zA-Z0-9:_-]*[a-zA-Z])([a-zA-Z0-9][a-zA-Z0-9:_-]*)(?=\s|$)/gu;

/** The text as Claude runs skills: every mention of a skill it declared (`$name`) becomes `/name` (T3 `planClaudeSkillDispatch`). */
export function skillsAsCommands(text: string, skills: ReadonlySet<string>): string {
  if (!skills.size) return text;
  return text.replace(SKILL_MENTION, (whole, before: string, name: string) => skills.has(name) ? `${before}/${name}` : whole);
}

function fnv1a64(value: string): string {
  let forward = 0x811c9dc5;
  let reverse = 0x9dc5811c;
  for (let index = 0; index < value.length; index += 1) {
    forward ^= value.charCodeAt(index);
    forward = Math.imul(forward, 0x01000193) >>> 0;
    reverse ^= value.charCodeAt(value.length - 1 - index);
    reverse = Math.imul(reverse, 0x01000193) >>> 0;
  }
  return `${forward.toString(16).padStart(8, '0')}${reverse.toString(16).padStart(8, '0')}`;
}

function imageContextId(imageId: string): string {
  const producerId = `image_${imageId}`;
  if (/^[a-z0-9_-]{1,128}$/i.test(producerId)) return producerId;
  const slug = producerId.replace(/[^a-z0-9_-]+/gi, '-').replace(/^-+|-+$/g, '').slice(0, 48);
  return `${slug || 'ctx'}-${fnv1a64(producerId)}`;
}

const IMAGE_CONTEXT_REFERENCE = /!\[([^\[\]\n]{0,512})\]\(t3-context:\/\/v1\/image\/([a-z0-9_-]{1,128})\)/gi;

/** Remove only T3 image references whose bytes are already carried by this turn's image blocks. */
export function stripAttachedImageReferences(text: string, imageIds: readonly string[]): string {
  if (!imageIds.length) return text;
  const attached = new Set(imageIds.map(imageContextId));
  return text.replace(IMAGE_CONTEXT_REFERENCE, (reference, _label: string, contextId: string) => attached.has(contextId) ? '' : reference).trim();
}

const IMAGE_EXTENSIONS = /\.(png|jpe?g|gif|webp|svg|ico|bmp|avif)$/i;

/** T3's `projects.searchEntries` filters (`kind`, `imageOnly`) over the engine's answer. */
export function searchAnswer(found: any, kind: unknown, imageOnly: unknown) {
  const entries = (Array.isArray(found?.entries) ? found.entries : [])
    .filter((entry: any) => typeof entry?.path === 'string'
      && entry.path.trim()
      && entry.path === entry.path.trim()
      && !entry.path.startsWith('/')
      && !entry.path.includes('\\')
      && !entry.path.split('/').includes('..')
      && (entry.kind === 'file' || entry.kind === 'directory'))
    .filter((entry: any) => kind !== 'file' && kind !== 'directory' ? true : entry.kind === kind)
    .filter((entry: any) => imageOnly !== true || (entry.kind === 'file' && IMAGE_EXTENSIONS.test(entry.path)))
    .map((entry: any) => ({ path: entry.path, kind: entry.kind }));
  return { entries, truncated: found?.truncated === true };
}
