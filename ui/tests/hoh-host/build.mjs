// Compiles ui/src/hoh-host/*.ts (without the Tauri bridge) to ES modules for node:test.
import { mkdtemp, readdir, readFile, symlink, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'vite';

const require = createRequire(import.meta.url);
const ts = require('../../node_modules/typescript');
const SOURCE = fileURLToPath(new URL('../../src/hoh-host/', import.meta.url));
const UI = resolve(SOURCE, '../..');
const SEARCH_SOURCE = 'thread-search.ts';

export async function buildHost(extraEntries = {}) {
  const out = await mkdtemp(join(process.env.TMPDIR || '/dev/shm/hoh', 'hoh-host-'));
  await writeFile(join(out, 'package.json'), '{"type":"module"}');
  for (const name of await readdir(SOURCE)) {
    if (!name.endsWith('.ts') || name === 'tauri.ts' || name === SEARCH_SOURCE) continue;
    const source = await readFile(join(SOURCE, name), 'utf8');
    const { outputText, diagnostics } = ts.transpileModule(source, {
      fileName: name,
      reportDiagnostics: true,
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022, rewriteRelativeImportExtensions: true, verbatimModuleSyntax: false },
    });
    if (diagnostics?.length) throw new Error(`${name}: ${diagnostics.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('; ')}`);
    await writeFile(join(out, name.replace(/\.ts$/, '.js')), outputText);
  }
  // Bundle the adapters that use the same contracts and helpers as the T3 UI.
  const aliases = JSON.parse(await readFile(join(UI, 'src/t3/base/aliases.json'), 'utf8'));
  const buildAliases = [
    '@t3tools/contracts/settings',
    '@t3tools/contracts',
    '@t3tools/shared/keybindings',
    '@t3tools/shared/serverSettings',
  ];
  await symlink(join(UI, 'node_modules'), join(out, 'node_modules'), 'dir');
  await build({
    root: UI, configFile: false, envFile: false, publicDir: false, logLevel: 'error',
    resolve: { alias: Object.fromEntries(buildAliases.map(name => [name, resolve(UI, aliases[name])])) },
    build: {
      ssr: true,
      outDir: out, emptyOutDir: false, minify: false,
      rolldownOptions: {
        input: {
          'thread-search': join(SOURCE, SEARCH_SOURCE),
          ...Object.fromEntries(Object.entries(extraEntries).map(([key, path]) => [key, resolve(UI, path)])),
        },
        output: { entryFileNames: '[name].js' },
      },
    },
  });
  return {
    dir: out,
    load: name => import(pathToFileURL(join(out, name + '.js')).href),
  };
}
