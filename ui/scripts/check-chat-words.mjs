// Inspect shipped JS literals, including labels, JSX children, translations passed to gt and errors.
// Protocol keys and ICU argument identifiers are not rendered. Translation dictionaries contain
// unused upstream hashes; source messages are checked in the executable chunks instead.
import { readdir, readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import ts from 'typescript';

const dist = resolve(process.env.HOH_CHECK_DIST ?? 'dist');
const exceptions = [
  { file: /^(?:en|ru)-[^/]+\.js$/, reason: 'upstream translation dictionaries include unused message hashes' },
  { file: /^(?:blade|cobol|hack|lua|luau|php|racket)-[^/]+\.js$/, reason: 'syntax-highlighting grammar, not interface copy' },
];
// Each remaining exception is either a protocol/diagnostic identifier or text sent to an agent.
const literalExceptions = [
  { file: /^network-[^/]+\.js$/, value: /^thread$/, reason: 'wire protocol schema literal' },
  { value: /^(?:new thread|thread id)$/, reason: 'command palette search synonyms' },
  { value: /^(?:thread session stop|thread metadata update)$/, reason: 'internal command diagnostics' },
  { value: /^Invalid scoped preview thread key: $/, reason: 'internal preview key diagnostic' },
  { value: /^ in preview for thread $/, reason: 'internal preview origin diagnostic' },
  { value: /^Saved thread history from another environment\./, reason: 'reference-material instruction sent to the model' },
  { value: /^Generate a title that will help the user recognize this T3 Code thread/, reason: 'title-generation prompt sent to the model' },
  { value: /^The user attached this thread as reference material\./, reason: 'reference-material instruction sent to the model' },
  { value: /^The unresolved review threads are attached to this message/, reason: 'review context sent to the model' },
  { value: /^(?:sidebar thread row|thread details card)$/, reason: 'internal theme CSS selector descriptions' },
  { value: /^Invalid scoped thread atom key: $/, reason: 'internal atom key diagnostic' },
  { value: /^Bounded thread snapshot not found over HTTP; treating the thread as deleted\.$/, reason: 'internal cache diagnostic' },
  { value: /^Could not load the bounded thread snapshot over HTTP; using the socket snapshot instead\.$/, reason: 'internal cache diagnostic' },
  { value: /^Could not persist the thread cache\.$/, reason: 'internal cache diagnostic' },
  { value: /^Skipped a thread event type this client does not know\.$/, reason: 'internal protocol diagnostic' },
  { value: /^Thread subscription failed; resubscribing\.$/, reason: 'internal subscription diagnostic' },
  { value: /^"?\)? cannot be (?:called|resolved) from a worker context\./, reason: 'syntax highlighter worker diagnostic, main thread means execution thread' },
  { value: /^Project to use\. Default: your thread's project\. Required outside a thread\.$/, reason: 'MCP tool schema sent to the model' },
  { value: /^Start a new provider session without the thread's earlier context, like \/clear\. Never steers\.$/, reason: 'MCP tool schema sent to the model' },
  { value: /^Post here, else new thread per run\./, reason: 'MCP tool schema sent to the model' },
  { value: /^Runs share one thread but each starts a fresh session\./, reason: 'MCP tool schema sent to the model' },
  { value: /^Default: your thread's project, or all outside a thread\.$/, reason: 'MCP tool schema sent to the model' },
  { value: /^Pasted in this thread; no card\.$/, reason: 'MCP tool schema sent to the model' },
  { value: /^declined: the user chose not to\. cancelled: the card closed unanswered/, reason: 'MCP tool schema sent to the model' },
  { value: /^Work to resume in the worktree next turn\. Omitted: thread waits\.$/, reason: 'MCP tool schema sent to the model' },
];
const contextExceptions = new Set(['$context', 'context']); // translation disambiguators are not rendered
const findings = [];
let files = 0, checked = 0;
async function walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const file = join(dir, entry.name);
    if (entry.isDirectory()) { await walk(file); continue; }
    if (!entry.name.endsWith('.js')) continue;
    if (exceptions.some(exception => exception.file.test(entry.name))) continue;
    const text = await readFile(file, 'utf8');
    if (!/thread/i.test(text)) continue;
    const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    files++;
    function visit(node) {
      if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isTemplateHead(node) || ts.isTemplateMiddle(node) || ts.isTemplateTail(node)) {
        checked++;
        const raw = node.text;
        // Only ICU identifiers, never their surrounding prose, are stripped.
        const prose = raw.replace(/\{threads?\s*(?=[,}])/gi, '{ARG').replace(/\{threads?\}/gi, '{ARG}');
        const parent = node.parent;
        const name = ts.isPropertyAssignment(parent) ? parent.name.getText(source).replace(/^["'`]|["'`]$/g, '') : '';
        const visibleProperty = /^(?:children|title|label|description|placeholder|aria-label|aria-description)$/.test(name);
        const visibleCall = ts.isCallExpression(parent) && parent.arguments[0] === node && (parent.arguments.length === 1 || (parent.arguments.length === 2 && ts.isObjectLiteralExpression(parent.arguments[1])));
        if (/(?<![\w{.-])threads?(?![\w.-])/i.test(prose) && (/\s/.test(prose) || /^Threads?$/.test(prose) || ((visibleProperty || visibleCall) && /^threads?$/i.test(prose)))) {
          const logging = ts.isCallExpression(parent) && /^console\.(?:warn|error|log)$/.test(parent.expression.getText(source));
          // Paths, styles, data attributes and translation context labels are implementation data.
          if (!logging && !literalExceptions.some(exception => (!exception.file || exception.file.test(entry.name)) && exception.value.test(raw)) && !contextExceptions.has(name) && !/^(?:className|class|id|key|testId|data-|\$)/.test(name)) {
            findings.push(`${file.slice(dist.length + 1)}: ${raw.slice(0, 250).replace(/\s+/g, ' ')}`);
          }
        }
      }
      ts.forEachChild(node, visit);
    }
    visit(source);
  }
}
await walk(dist);
if (findings.length) { console.error(findings.join('\n')); process.exitCode = 1; }
else console.log(`chat words: ${files} JS chunks, ${checked} literals checked; no visible thread labels`);
