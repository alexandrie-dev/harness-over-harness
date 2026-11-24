import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const homeUrl = new URL('../src/t3-entry/hoh-screens/HomeBody.tsx', import.meta.url);
const groupingUrl = new URL('../src/t3-entry/hoh-screens/grouping.ts', import.meta.url);
const [homeSource, groupingSource] = await Promise.all([readFile(homeUrl, 'utf8'), readFile(groupingUrl, 'utf8')]);

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

function loadModule(source, fileName = 'module.ts') {
  const output = ts.transpileModule(source, {
    fileName,
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  Function('exports', output)(exports);
  return exports;
}

const { greeting, projectLabel, waitingLabel, projectPanelInsertAfter } = loadFunctions(homeSource, ['greeting', 'projectLabel', 'waitingLabel', 'projectPanelInsertAfter']);
const grouping = loadModule(groupingSource, 'grouping.ts');

test('a named project stays a project when its folder is also the engine default', async () => {
  const source = await readFile(new URL('../src/t3/base/client-runtime/state/projects.ts', import.meta.url), 'utf8');
  const tree = ts.createSourceFile('projects.ts', source, ts.ScriptTarget.Latest, true);
  const fn = tree.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'isScratchProject');
  const js = ts.transpileModule(fn.getText(tree), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  const exports = {};
  Function('exports', 'findProjectByPath', js)(exports, (projects, path) => projects.find(project => project.workspaceRoot === path));
  const root = '/home/person/Documents/HoH';
  assert.equal(exports.isScratchProject({ id: 'project-hoh', workspaceRoot: root }, root), false);
  assert.equal(exports.isScratchProject({ id: 'hoh-project', workspaceRoot: root }, root), true);
  assert.equal(exports.isScratchProject({ workspaceRoot: root }, root), true);
});

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

test('project grouping derives counts, warning priority, runners and rename-stable colors from the path', () => {
  const entries = [
    { id: 'a', projectId: 'alpha', state: 'working', startedAt: 30, agents: [{ kind: 'explore' }] },
    { id: 'b', projectId: 'alpha', state: 'done', startedAt: 20, runner: 'codex', agents: [{ kind: 'codex' }] },
    { id: 'c', projectId: 'hoh-project', state: 'waiting', startedAt: 10, agents: [] },
  ];
  const paths = { alpha: '/work/alpha', 'hoh-project': '/work/no-project' };
  const first = grouping.groupEntriesByProject(entries, id => id === 'alpha' ? 'Alpha' : 'No project', id => paths[id]);
  assert.equal(first[0].title, 'Chats', 'a waiting project sorts first');
  assert.deepEqual(first[0].states, { working: 0, waiting: 1, ready: 0, stopped: 0, interrupted: 0, done: 0 });
  const alpha = first.find(group => group.id === 'alpha');
  assert.deepEqual(alpha.entries.map(entry => entry.id), ['a', 'b']);
  assert.deepEqual(alpha.states, { working: 1, waiting: 0, ready: 0, stopped: 0, interrupted: 0, done: 1 });
  assert.deepEqual(alpha.runners, ['claude', 'codex']);
  assert.deepEqual(grouping.runnerKinds([
    ...entries,
    { id: 'd', projectId: 'alpha', state: 'done', startedAt: 5, agents: [] },
  ]), ['claude', 'codex', 'claude', 'claude'], 'runner marks retain participants so the fourth becomes +N');
  const renamed = grouping.groupEntriesByProject(entries, id => id === 'alpha' ? 'Renamed' : 'No project', id => paths[id]);
  assert.equal(renamed.find(group => group.id === 'alpha').color, alpha.color, 'title changes do not alter path-derived color');
  assert.equal(grouping.groupEntriesByProject(entries.filter(entry => entry.projectId !== 'hoh-project'), id => id, id => id).some(group => group.title === 'Chats'), false);
  assert.equal(grouping.PROJECT_COLOR_COUNT, 8, 'the folder palette has eight distinct slots');
  assert.notEqual(first[0].color, first[1].color, 'neighboring projects never share a palette slot');
  const fixtureColors = ['/work/alexandrie', '/work/hoh', '/work/no-project'].map(path => grouping.projectColor(path));
  assert.equal(new Set(fixtureColors).size, fixtureColors.length, 'the screenshot projects receive visibly different stable colors');

  const pathByColor = new Map();
  let collision;
  for (let index = 0; index < 100 && !collision; index += 1) {
    const path = `/work/collision-${index}`;
    const color = grouping.projectColor(path);
    if (pathByColor.has(color)) collision = [pathByColor.get(color), path]; else pathByColor.set(color, path);
  }
  assert(collision, 'the collision fixture finds two paths in the finite palette');
  const collidingEntries = [
    { id: 'left', projectId: 'left', state: 'working', startedAt: 2, agents: [] },
    { id: 'right', projectId: 'right', state: 'working', startedAt: 1, agents: [] },
  ];
  const collisionPaths = { left: collision[0], right: collision[1] };
  const colorsBeforeRename = grouping.groupEntriesByProject(collidingEntries, id => id === 'left' ? 'A' : 'B', id => collisionPaths[id]);
  const colorsAfterRename = grouping.groupEntriesByProject(collidingEntries, id => id === 'left' ? 'Z' : 'A', id => collisionPaths[id]);
  const colorsById = groups => Object.fromEntries(groups.map(group => [group.id, group.color]));
  assert.deepEqual(colorsById(colorsAfterRename), colorsById(colorsBeforeRename), 'collision resolution is stable across presentation reordering');
  assert.notEqual(colorsBeforeRename[0].color, colorsBeforeRename[1].color, 'colliding paths still receive adjacent distinct colors');
});

test('adjacent folder hues differ by at least 40 degrees', () => {
  const hues = [240, 180, 120, 45, 315, 25, 0, 280];
  for (let count = 2; count <= 16; count += 1) {
    const entries = Array.from({ length: count }, (_, index) => ({ id: String(index), projectId: `p${index}`, state: 'working', startedAt: index, agents: [] }));
    const groups = grouping.groupEntriesByProject(entries, id => id, id => `/work/${id}`);
    for (let index = 1; index < groups.length; index += 1) {
      const difference = Math.abs(hues[groups[index].color] - hues[groups[index - 1].color]);
      assert(Math.min(difference, 360 - difference) >= 40);
    }
  }
});

test('folder status segments represent counts with proportional widths and separate colors', async () => {
  const css = await readFile(new URL('../src/t3-entry/hoh-screens/home.css', import.meta.url), 'utf8');
  for (const state of ['working', 'waiting', 'done']) assert.match(homeSource, new RegExp(`data-state="${state}" style=\\{\\{ flexGrow: group\\.states\\.${state} \\}\\}`));
  assert.match(css, /\.hoh-project__segments\s*\{[^}]*gap:\s*4px/);
  assert.match(css, /\.hoh-project__segments i\[data-state="waiting"\][^{]*\{[^}]*var\(--destructive\)/);
});

test('project disclosure supports hover, pinned click/keyboard semantics, leave and Escape/outside close', () => {
  let state = { hovered: null, pinned: null };
  state = grouping.updateProjectDisclosure(state, { type: 'hover', id: 'alpha' });
  assert.deepEqual(state, { hovered: 'alpha', pinned: null });
  state = grouping.updateProjectDisclosure(state, { type: 'toggle', id: 'alpha' });
  assert.deepEqual(state, { hovered: null, pinned: 'alpha' });
  assert.equal(grouping.updateProjectDisclosure(state, { type: 'hover', id: 'beta' }), state, 'hover cannot replace a pinned folder');
  assert.equal(grouping.updateProjectDisclosure(state, { type: 'leave' }), state, 'pointer leave keeps a pinned folder open');
  state = grouping.updateProjectDisclosure(state, { type: 'close' });
  assert.deepEqual(state, { hovered: null, pinned: null });
  assert.match(homeSource, /closeTimer\.current = setTimeout\([\s\S]{0,220}, 180\)/, 'hover close has a bridge delay');
  assert.match(homeSource, /event\.key !== 'Escape'/);
  assert.match(homeSource, /document\.addEventListener\('pointerdown', close\)/);
  assert.match(homeSource, /if \(openId && !openGroupId\) setDisclosure/, 'a removed open group cannot leave stale pinned state');
});

test('folder disclosure is inserted after its rendered grid row instead of escaping into a viewport portal', async () => {
  assert.equal(projectPanelInsertAfter(0, 3, 2), 1);
  assert.equal(projectPanelInsertAfter(1, 3, 2), 1);
  assert.equal(projectPanelInsertAfter(2, 3, 2), 2);
  assert.equal(projectPanelInsertAfter(1, 3, 1), 1);
  assert.equal(projectPanelInsertAfter(2, 3, 3), 2);
  assert.equal(projectPanelInsertAfter(-1, 3, 2), -1);
  assert.doesNotMatch(homeSource, /createPortal|folderPanelPosition/);
  const css = await readFile(new URL('../src/t3-entry/hoh-screens/home.css', import.meta.url), 'utf8');
  assert.match(css, /\.hoh-project-popover\s*\{[^}]*position:\s*relative[^}]*grid-column:\s*g1\s*\/\s*g21[^}]*overflow:\s*auto/);
  assert.doesNotMatch(css, /\.hoh-project-popover\s*\{[^}]*position:\s*fixed/);
});

test('folder art and project state styling use the project hue, blue active state and red waiting state', async () => {
  const css = await readFile(new URL('../src/t3-entry/hoh-screens/home.css', import.meta.url), 'utf8');
  assert.match(homeSource, /data-working=\{\(group\.waiting === 0 && group\.states\.working > 0\)/);
  assert.match(css, /\.hoh-project\[data-working\][^{]*\{[^}]*var\(--primary\)/);
  assert.match(css, /\.hoh-project\[data-waiting\][^{]*\{[^}]*var\(--destructive\)/);
  assert.match(css, /\.hoh-project__chip i\s*\{[^}]*background:\s*var\(--primary\)/);
  assert.match(css, /\.hoh-project\[data-waiting\] \.hoh-project__chip i\s*\{[^}]*background:\s*var\(--destructive\)/);
  assert.match(css, /\.hoh-project__icon i[^}]*var\(--hoh-project-color\)/);
  assert.match(css, /\.hoh-project__icon b[^}]*var\(--hoh-project-color\)/);
  assert.match(css, /\.hoh-project__icon i::before/);
  assert.match(css, /\.hoh-project__icon em[^}]*border-radius:\s*50%/);
});

test('grouping preference persists and notifies the home in the same window', () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const values = new Map();
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) } });
  let changes = 0;
  const unsubscribe = grouping.subscribeGroupAgentsByProject(() => { changes += 1; });
  grouping.setGroupAgentsByProject(true);
  assert.equal(grouping.getGroupAgentsByProject(), true);
  assert.equal(values.get(grouping.GROUP_AGENTS_STORAGE_KEY), 'true');
  assert.equal(changes, 1);
  unsubscribe();
  if (descriptor) Object.defineProperty(globalThis, 'localStorage', descriptor); else delete globalThis.localStorage;
});

test('request detail is an accessible two-line disclosure', async () => {
  const css = await readFile(new URL('../src/t3-entry/hoh-screens/home.css', import.meta.url), 'utf8');
  assert.match(homeSource, /aria-expanded=\{expanded\}/);
  assert.match(homeSource, /onClick=\{\(\) => setExpanded/);
  assert.match(css, /\.hoh-ask__text[^}]*-webkit-line-clamp: 2/);
  assert.match(css, /\.hoh-ask__text\[data-expanded\][^}]*-webkit-line-clamp: initial/);
});

test('agent cards have a separate accessible open target and an explicit human completion action', async () => {
  const css = await readFile(new URL('../src/t3-entry/hoh-screens/home.css', import.meta.url), 'utf8');
  assert.match(homeSource, /className="hoh-card__open" aria-label=\{`Open \$\{entry\.title\}`\}/);
  assert.match(homeSource, /className="hoh-card__complete"[^>]*onClick=\{\(\) => void view\.host\?\.complete\(entry\.id\)\}/);
  assert.match(homeSource, />Mark done<\/Button>/);
  assert.equal(waitingLabel({}), 'Waiting for your answer');
  assert.equal(waitingLabel({ canComplete: true }), 'Ready for review');
  assert.equal(waitingLabel({ canComplete: true, stopped: true }), 'Stopped');
  assert.equal(waitingLabel({ canComplete: true, interrupted: true }), 'Interrupted');
  assert.match(css, /--hoh-card-width:\s*min\(360px, calc\(\(50vw - 14px\) \/ 2\)\)/);
  assert.match(css, /grid-template-columns:\s*repeat\(auto-fill, var\(--hoh-card-width\)\)/);
  assert.match(css, /\.hoh-home__cards\s*\{[^}]*justify-content:\s*start/);
  assert.match(css, /\.hoh-projects\s*\{[^}]*justify-content:\s*start/);
  assert.match(css, /scroll-padding-bottom:\s*var\(--hoh-home-scroll-clearance\)/);
  assert.match(css, /padding:\s*28px 0 var\(--hoh-home-scroll-clearance\)/);
  assert.match(css, /\.hoh-card__complete\s*\{[^}]*pointer-events:\s*auto/);
});

test('agent cards open the shared quick-view host while Mark done keeps its direct action', () => {
  assert.match(homeSource, /import \{ openQuickView, QuickViewDialog \} from '\.\/QuickViewDialog'/);
  assert.match(homeSource, /open: openQuickView/);
  assert.match(homeSource, /<QuickViewDialog \/>/);
  assert.doesNotMatch(homeSource, /open:\s*\(threadId\).*navigate/);
  assert.match(homeSource, /view\.host\?\.complete\(entry\.id\)/);
});

test('empty Home joins the greeting and native T3 draft composer into one centred hero group', async () => {
  const [css, vite] = await Promise.all([
    readFile(new URL('../src/t3-entry/hoh-screens/home.css', import.meta.url), 'utf8'),
    readFile(new URL('../src/t3-entry/vite.ts', import.meta.url), 'utf8'),
  ]);
  assert.match(vite, /hoh-draft-hero pointer-events-none absolute inset-0 z-20 flex flex-col justify-end/);
  assert.match(css, /\.hoh-draft-hero:has\(> \.hoh-home:not\(\[data-busy\]\)\)\s*\{\s*justify-content:\s*center/);
  assert.match(css, /inset-block-start:\s*calc\(-1 \* var\(--hoh-notch-height, 0px\)\)/);
  assert.match(css, /\.hoh-home:not\(\[data-busy\]\)\s*\{[^}]*flex:\s*0 0 auto[^}]*margin-bottom:\s*28px/);
  assert.match(css, /@media \(max-height: 34rem\)[\s\S]*justify-content:\s*flex-end/);
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

test('folder status follows visible card labels and singular agent wording', () => {
  const entry = (id, extra = {}) => ({ id, projectId: 'p', state: 'waiting', startedAt: 1, agents: [], ...extra });
  const ready = [entry('a', { canComplete: true }), entry('b', { canComplete: true }), entry('c', { canComplete: true })];
  const folder = grouping.groupEntriesByProject(ready, id => id, id => id)[0];
  assert.equal(folder.waiting, 0);
  assert.equal(folder.ready, 3);
  assert.equal(folder.states.ready, 3);
  assert.equal(folder.states.done, 0);
  assert(ready.every(item => waitingLabel(item) === 'Ready for review'));
  const mixed = grouping.groupEntriesByProject([...ready, entry('d')], id => id, id => id)[0];
  assert.equal(mixed.waiting, 1);
  assert.equal(mixed.ready, 3);
  assert.match(homeSource, /agent\$\{count === 1 \? '' : 's'\}/);
  assert.match(homeSource, /agent\{\(openGroup\.totalChats \?\? openGroup\.entries\.length\) === 1 \? '' : 's'\}/);
});

test('folder totals use all project chats while status counts come from actual cards', () => {
  assert.match(homeSource, /projectCounts\(\)\[group\.id\]/);
  const group = grouping.groupEntriesByProject([
    { id: 'interrupted', projectId: 'p', state: 'waiting', interrupted: true, startedAt: 1, agents: [] },
    { id: 'done', projectId: 'p', state: 'done', startedAt: 2, agents: [] },
  ], id => id, id => id)[0];
  assert.equal(group.entries.length, 2);
  assert.equal(group.states.interrupted, 1);
  assert.equal(group.states.done, 1);
});

test('sidebar New chat resolves the same context regardless of the current route', async () => {
  const source = await readFile(new URL('../src/t3/base/web/components/Sidebar.tsx', import.meta.url), 'utf8');
  const handler = source.slice(source.indexOf('  const handleNewThreadClick ='), source.indexOf('  const newThreadShortcutLabel ='));
  assert.match(handler, /startNewThreadFromContext/);
  assert.match(handler, /activeDraftThread: newThreadContext\.activeDraftThread/);
  assert.match(handler, /activeThread: newThreadContext\.activeThread/);
  assert.match(handler, /if \(!started\) openCommandPalette/);
  assert.doesNotMatch(handler, /shouldCreateNewThreadInCurrentProject/);
});
