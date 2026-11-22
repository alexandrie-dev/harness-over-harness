// Compiles ui/src/hoh-host/*.ts (without the Tauri bridge) to plain ES modules in a temporary folder,
// with the window's own TypeScript, so node:test can run them. No bundler, no network.
import { mkdtemp, readdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const require = createRequire(import.meta.url);
const ts = require('../../node_modules/typescript');
const SOURCE = fileURLToPath(new URL('../../src/hoh-host/', import.meta.url));

export async function buildHost() {
  const out = await mkdtemp(join(process.env.TMPDIR || '/dev/shm/hoh', 'hoh-host-'));
  await writeFile(join(out, 'package.json'), '{"type":"module"}');
  for (const name of await readdir(SOURCE)) {
    if (!name.endsWith('.ts') || name === 'tauri.ts') continue;
    const source = await readFile(join(SOURCE, name), 'utf8');
    const { outputText, diagnostics } = ts.transpileModule(source, {
      fileName: name,
      reportDiagnostics: true,
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022, rewriteRelativeImportExtensions: true, verbatimModuleSyntax: false },
    });
    if (diagnostics?.length) throw new Error(`${name}: ${diagnostics.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('; ')}`);
    await writeFile(join(out, name.replace(/\.ts$/, '.js')), outputText);
  }
  return {
    dir: out,
    load: name => import(pathToFileURL(join(out, name + '.js')).href),
  };
}
