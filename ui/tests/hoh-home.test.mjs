import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const homeUrl = new URL('../src/t3-entry/hoh-screens/HomeBody.tsx', import.meta.url);
const homeSource = await readFile(homeUrl, 'utf8');

function loadFunctions(source, names) {
  const tree = ts.createSourceFile('HomeBody.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const declarations = tree.statements.filter(
    (node) => ts.isFunctionDeclaration(node) && node.name && names.includes(node.name.text),
  );
  assert.deepEqual(declarations.map((node) => node.name.text), names, 'pure home functions are exported as declarations');
  const moduleSource = declarations.map((node) => node.getText(tree)).join('\n');
  const output = ts.transpileModule(moduleSource, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  Function('exports', output)(exports);
  return exports;
}

const { greeting, projectLabel } = loadFunctions(homeSource, ['greeting', 'projectLabel']);

test('greeting uses the four requested hour ranges at every boundary', () => {
  const cases = [
    [0, 'Good night'], [4, 'Good night'],
    [5, 'Good morning'], [11, 'Good morning'],
    [12, 'Good afternoon'], [17, 'Good afternoon'],
    [18, 'Good evening'], [23, 'Good evening'],
  ];
  for (const [hour, expected] of cases) assert.equal(greeting(hour), expected, `${hour}:00`);
});

test('agent cards hide only the scratch project label', () => {
  assert.equal(projectLabel('hoh-project', 'No project'), null);
  assert.equal(projectLabel('hoh-project', 'Anything'), null);
  assert.equal(projectLabel('p-empty', ''), null);
  assert.equal(projectLabel('p-alexandrie', '  Alexandrie  '), 'Alexandrie');
  assert.equal(projectLabel('p-named-no-project', 'No project'), 'No project');
  assert.match(homeSource, /project \? <span className="hoh-card__where">\{project\}<\/span> : null/);
});

test('request detail is an accessible two-line disclosure', async () => {
  const css = await readFile(new URL('../src/t3-entry/hoh-screens/home.css', import.meta.url), 'utf8');
  assert.match(homeSource, /aria-expanded=\{expanded\}/);
  assert.match(homeSource, /onClick=\{\(\) => setExpanded/);
  assert.match(css, /\.hoh-ask__text[^}]*-webkit-line-clamp: 2/);
  assert.match(css, /\.hoh-ask__text\[data-expanded\][^}]*-webkit-line-clamp: initial/);
});

test('visible T3 draft and palette copy is patched to New chat and documented', async () => {
  const root = new URL('../', import.meta.url);
  const [draftPatch, palettePatch, manifestText, map] = await Promise.all([
    readFile(new URL('src/t3/base-patches/web/components/ChatView.logic.ts.patch', root), 'utf8'),
    readFile(new URL('src/t3/base-patches/web/components/CommandPalette.tsx.patch', root), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  const additions = (patch) => patch.split('\n').filter((line) => line.startsWith('+') && !line.startsWith('+++')).join('\n');
  assert.match(additions(draftPatch), /New chat/);
  assert.doesNotMatch(additions(draftPatch), /New thread/);
  assert.match(additions(palettePatch), /New chat in/);
  assert.match(additions(palettePatch), /New chat without a project/);
  assert.doesNotMatch(additions(palettePatch), /New thread/);
  const manifest = JSON.parse(manifestText);
  assert(manifest.base.patched['web/components/ChatView.logic.ts']);
  assert(manifest.base.patched['web/components/CommandPalette.tsx']);
  assert.match(map, /ChatView\.logic\.ts\.patch[^\n]*New chat/);
  assert.match(map, /CommandPalette\.tsx\.patch[^\n]*New chat/);
});
