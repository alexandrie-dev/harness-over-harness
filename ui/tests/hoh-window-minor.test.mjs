import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const root = new URL('../', import.meta.url);

test('composer command drawers override glass opacity to an opaque surface', async () => {
  const css = await readFile(new URL('src/t3-entry/brand.css', root), 'utf8');
  assert.match(css, /\[data-composer-command-drawer="true"\]\s*\{[^}]*--glass-opacity:\s*100%/s);
});

test('T3 asset contract admits only its server paths and process-local blob previews', async () => {
  const [patch, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/contracts/assets.ts.patch', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  assert.match(patch, /blob:\[\^\\s\]\+/);
  assert.doesNotMatch(patch, /https\?:/);
  assert(JSON.parse(manifestText).base.patched['contracts/assets.ts']);
  assert.match(map, /contracts\/assets\.ts\.patch[^\n]*Blob URL/);
});
test('the timeline minimap strips that look like an equals sign are hidden', async () => {
  const features = await readFile(new URL('src/t3-entry/features.ts', root), 'utf8');
  assert.match(features, /['"]\[data-testid="timeline-minimap"\]['"]/);
});

test('Recent chats omits T3 draft rows until a sent thread exists', async () => {
  const features = await readFile(new URL('src/t3-entry/features.ts', root), 'utf8');
  assert.match(features, /components\/sidebar\/SidebarDraftBlock\.tsx['"]:\s*\[['"]SidebarDraftBlock['"]\]/);
  const vite = await readFile(new URL('src/t3-entry/vite.ts', root), 'utf8');
  assert.match(vite, /latestUserMessageAt != null/);
});

test('paperclip uses T3 attachment intake with an image-only human picker', async () => {
  const [composerPatch, stripPatch, stripSource, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/web/components/chat/ChatComposer.tsx.patch', root), 'utf8'),
    readFile(new URL('src/t3/base-patches/web/components/chat/ComposerAttachmentStrip.tsx.patch', root), 'utf8'),
    readFile(new URL('src/t3/base/web/components/chat/ComposerAttachmentStrip.tsx', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  assert.match(composerPatch, /supportsAttachmentUploads/);
  assert.match(stripPatch, /accept="image\/gif,image\/jpeg,image\/png,image\/webp"/);
  assert.match(stripSource, /addComposerAttachments\(files\)/);
  assert.doesNotMatch(stripPatch, /accept="\*\/\*"/);
  const patched = JSON.parse(manifestText).base.patched;
  assert(patched['web/components/chat/ChatComposer.tsx']);
  assert(patched['web/components/chat/ComposerAttachmentStrip.tsx']);
  assert.match(map, /ComposerAttachmentStrip\.tsx\.patch[^\n]*GIF, JPEG, PNG/);
});

test('Claude skills do not offer an unsafe file instruction opener', async () => {
  const [inlinePatch, editorPatch, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/web/components/chat/SkillInlineText.tsx.patch', root), 'utf8'),
    readFile(new URL('src/t3/base-patches/web/components/ComposerPromptEditorTiptap.tsx.patch', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  assert.match(inlinePatch, /startsWith\("hoh-skill:"\)/);
  assert.match(editorPatch, /startsWith\("hoh-skill:"\)/);
  const patched = JSON.parse(manifestText).base.patched;
  assert(patched['web/components/chat/SkillInlineText.tsx']);
  assert(patched['web/components/ComposerPromptEditorTiptap.tsx']);
  assert.match(map, /SkillInlineText\.tsx\.patch[^\n]*View instructions/);
});
