// T3's contracts (packages/contracts/src, read-only source, MIT) compiled to plain ES modules in a
// temporary folder, so the host can be checked against the real schemas with effect's real RpcClient.
import { mkdir, mkdtemp, readdir, readFile, realpath, symlink, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const require = createRequire(import.meta.url);
const ts = require('../../node_modules/typescript');
export const T3_CONTRACTS = process.env.HOH_T3_CONTRACTS || '/mnt/toshiba/alexandrie/third-party/t3code/packages/contracts/src';
const EFFECT = fileURLToPath(new URL('../../node_modules/effect', import.meta.url));

const T3_ROOT = join(T3_CONTRACTS, '..', '..', '..');
/** T3's own first-run and welcome-page decisions (apps/web/src/onboarding), pure functions over contracts. */
const WEB_LOGIC = [
  ['shared/path', 'packages/shared/src/path.ts'],
  ['web/providerReadiness.logic', 'apps/web/src/onboarding/providerReadiness.logic.ts'],
  ['web/welcomePage.logic', 'apps/web/src/onboarding/welcomePage.logic.ts'],
  ['web/firstRun.logic', 'apps/web/src/onboarding/firstRun.logic.ts'],
  // Where the composer shows the permissions chip (no imports).
  ['web/composerControlsOwnership', 'apps/web/src/components/chat/composerControlsOwnership.ts'],
];
const WEB_IMPORTS = { '@t3tools/contracts': '../contracts/index.js', '@t3tools/shared/path': '../shared/path.js', './providerReadiness.logic': './providerReadiness.logic.js' };

export async function buildContracts() {
  const out = await mkdtemp(join(process.env.TMPDIR || '/dev/shm/hoh', 't3-contracts-'));
  await writeFile(join(out, 'package.json'), '{"type":"module"}');
  await mkdir(join(out, 'node_modules'));
  await symlink(await realpath(EFFECT), join(out, 'node_modules', 'effect'));
  await mkdir(join(out, 'contracts'));
  for (const name of await readdir(T3_CONTRACTS)) {
    if (!name.endsWith('.ts') || name.endsWith('.test.ts')) continue;
    const source = await readFile(join(T3_CONTRACTS, name), 'utf8');
    const { outputText } = ts.transpileModule(source, {
      fileName: name,
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022, rewriteRelativeImportExtensions: true, useDefineForClassFields: true },
    });
    await writeFile(join(out, 'contracts', name.replace(/\.ts$/, '.js')), outputText);
  }
  await mkdir(join(out, 'shared')); await mkdir(join(out, 'web'));
  for (const [name, file] of WEB_LOGIC) {
    let { outputText } = ts.transpileModule(await readFile(join(T3_ROOT, file), 'utf8'), {
      fileName: file, compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
    });
    for (const [from, to] of Object.entries(WEB_IMPORTS)) outputText = outputText.replaceAll(`from "${from}"`, `from "${to}"`);
    await writeFile(join(out, name + '.js'), outputText);
  }
  return {
    dir: out,
    load: name => import(pathToFileURL(join(out, 'contracts', name + '.js')).href),
    /** `web/firstRun.logic`, `web/welcomePage.logic`, `web/providerReadiness.logic`, `web/composerControlsOwnership`. */
    loadWeb: name => import(pathToFileURL(join(out, name + '.js')).href),
  };
}
