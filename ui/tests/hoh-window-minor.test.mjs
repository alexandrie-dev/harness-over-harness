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

test('assistant data images are blocked and T3 image URL history is bounded', async () => {
  const [imagePatch, imageSource, assetPatch, assetSource, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/client-runtime/markdownImages.ts.patch', root), 'utf8'),
    readFile(new URL('src/t3/base/client-runtime/markdownImages.ts', root), 'utf8'),
    readFile(new URL('src/t3/base-patches/client-runtime/state/assets.ts.patch', root), 'utf8'),
    readFile(new URL('src/t3/base/client-runtime/state/assets.ts', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  assert.match(imagePatch, /-const DIRECT_IMAGE_SOURCE_PATTERN = \/\^\(\?:https\?:\|data:/);
  assert.match(imageSource, /DIRECT_IMAGE_SOURCE_PATTERN = \/\^\(\?:https\?:\|blob:/);
  assert.doesNotMatch(imageSource, /DIRECT_IMAGE_SOURCE_PATTERN = [^\n]*data:/);
  assert.match(assetPatch, /ASSET_REVISION_HISTORY_MAX = 512/);
  assert.match(assetSource, /while \(map\.size > ASSET_REVISION_HISTORY_MAX\)/);
  const patched = JSON.parse(manifestText).base.patched;
  assert(patched['client-runtime/markdownImages.ts']);
  assert.match(map, /markdownImages\.ts\.patch[^\n]*data URI/);
});

test('thread details card stays visible (CEO: it becomes the HOH-43 stats panel)', async () => {
  const features = await readFile(new URL('src/t3-entry/features.ts', root), 'utf8');
  assert.doesNotMatch(features, /\[data-thread-details-card\]/);
});

test('HoH home identity keeps empty-home product copy stable while workspace discovery settles', async () => {
  const [patch, source, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/web/components/chat/DraftHeroHeadline.tsx.patch', root), 'utf8'),
    readFile(new URL('src/t3/base/web/components/chat/DraftHeroHeadline.tsx', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  assert.match(patch, /activeProject\?\.id === "hoh-project"/);
  assert.match(source, /activeProject\?\.id === "hoh-project"/);
  assert.match(patch, /What should we build in HoH\?/);
  assert.match(source, /What should we build in HoH\?/);
  assert(JSON.parse(manifestText).base.patched['web/components/chat/DraftHeroHeadline.tsx']);
  assert.match(map, /DraftHeroHeadline\.tsx\.patch[^\n]*постоянным id/);
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

test('Claude provider settings disclose that channel plugins are disabled without a toggle', async () => {
  const [vite, map] = await Promise.all([
    readFile(new URL('src/t3-entry/vite.ts', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  assert.match(vite, />Channel plugins are disabled in the window<\/p>/);
  assert.doesNotMatch(vite, /Channel plugins are disabled in the window[\s\S]{0,200}(?:Switch|Toggle)/);
  assert.match(map, /Channel plugins are disabled in the window/);
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
  assert.match(stripPatch, /Attach images \(up to 10 MiB each\)/);
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

test('T3 lists auto modes as Coming Soon while Full and Supervised remain selectable', async () => {
  const [patch, source, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/web/components/chat/runtimeModeConfig.ts.patch', root), 'utf8'),
    readFile(new URL('src/t3/base/web/components/chat/runtimeModeConfig.ts', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  for (const text of [patch, source]) {
    assert.match(text, /mode === "auto-accept-edits" \|\| mode === "auto"/);
    assert.match(text, /disabledReason: resolve\(msg\("Coming Soon"\)\)/);
  }
  assert(JSON.parse(manifestText).base.patched['web/components/chat/runtimeModeConfig.ts']);
  assert.match(map, /runtimeModeConfig\.ts\.patch[^\n]*Coming Soon/);
});

test('T3 materializes durable stopped-run provenance into old turn fold labels', async () => {
  const [patch, source, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/web/components/chat/MessagesTimeline.logic.ts.patch', root), 'utf8'),
    readFile(new URL('src/t3/base/web/components/chat/MessagesTimeline.logic.ts', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  for (const text of [patch, source]) {
    assert.match(text, /const isInterruptedTurn =/);
    assert.match(text, /interruptedRunIds\.has\(runId\)/);
    assert.match(text, /const label = isInterruptedTurn/);
  }
  assert.doesNotMatch(source, /input\.unfoldedRunIds\.has\(runId\) \|\|\s*interruptedRunIds\.has\(runId\)/);
  assert(JSON.parse(manifestText).base.patched['web/components/chat/MessagesTimeline.logic.ts']);
  assert.match(map, /MessagesTimeline\.logic\.ts\.patch[^\n]*You stopped/);
});
