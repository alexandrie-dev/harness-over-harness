// No dependencies. Input is emitted by hoh-protocol-schema; output contains types only.
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve, join, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const schemaDir = process.argv.find(a => a.startsWith('--schema-dir='))?.slice(13) ?? join(root, 'protocol/schema');
const customOutput = process.argv.find(a => a.startsWith('--output='))?.slice(9);
const output = customOutput ?? join(root, 'ui/src/protocol.ts');
const coreOutput = process.argv.find(a => a.startsWith('--core-output='))?.slice(14) ?? (customOutput ? undefined : join(root, 'protocol/core.ts'));
let definitions = new Map();
const roots = readdirSync(schemaDir).filter(n => n.endsWith('.json')).sort().map(file => [basename(file, '.json'), JSON.parse(readFileSync(join(schemaDir, file), 'utf8'))]);
function clean(schema) {
  const { $schema, $defs, title, 'x-method': method, ...body } = schema;
  return body;
}
function add(name, schema) {
  const body = clean(schema);
  if (definitions.has(name) && JSON.stringify(definitions.get(name)) !== JSON.stringify(body)) {
    throw new Error('Conflicting schema definition: ' + name);
  }
  definitions.set(name, body);
}
for (const [name, schema] of roots) {
  for (const [key, value] of Object.entries(schema.$defs ?? {})) add(key, value);
  add(name, schema);
}
function type(s) {
  if (s === true) return 'unknown';
  if (s === false) return 'never';
  if (s.$ref) {
    if (!s.$ref.startsWith('#/$defs/')) throw new Error('Unsupported ref: ' + s.$ref);
    const name = decodeURIComponent(s.$ref.slice(8)).replaceAll('~1', '/').replaceAll('~0', '~');
    if (!definitions.has(name)) throw new Error('Unresolved ref: ' + name);
    return name;
  }
  if ('const' in s) return JSON.stringify(s.const);
  if (s.enum) return s.enum.map(v => JSON.stringify(v)).join(' | ') || 'never';
  if (s.oneOf || s.anyOf || s.allOf) {
    // JSON Schema combines sibling keywords with AND. Flattened Rust enums
    // put the shared envelope properties beside oneOf, not inside its branches.
    const { oneOf, anyOf, allOf, ...siblings } = s;
    const parts = [];
    if (siblings.type || siblings.properties || siblings.$ref) parts.push(type(siblings));
    for (const variants of [oneOf, anyOf]) if (variants) parts.push('(' + variants.map(type).join(' | ') + ')');
    if (allOf) parts.push(...allOf.map(type));
    return parts.length === 1 ? parts[0] : '(' + parts.join(' & ') + ')';
  }
  if (Array.isArray(s.type)) return '(' + s.type.map(t => type({ ...s, type: t })).join(' | ') + ')';
  switch (s.type) {
    case 'string': return 'string';
    case 'number': case 'integer': return 'number';
    case 'boolean': return 'boolean';
    case 'null': return 'null';
    case 'array':
      if (s.prefixItems) return '[' + s.prefixItems.map(type).join(', ') + ']';
      return 'Array<' + type(s.items ?? true) + '>';
    case 'object': {
      const properties = Object.entries(s.properties ?? {});
      const fields = properties.map(([key, value]) => JSON.stringify(key) + ((s.required ?? []).includes(key) ? '' : '?') + ': ' + type(value) + ';');
      if (s.additionalProperties && properties.length === 0) return 'Record<string, ' + type(s.additionalProperties) + '>';
      if (!fields.length) return s.additionalProperties === false ? 'Record<string, never>' : 'Record<string, unknown>';
      if (s.additionalProperties) fields.push('[key: string]: ' + type(s.additionalProperties) + ';');
      return '{ ' + fields.join(' ') + ' }';
    }
    case undefined:
      if (!Object.keys(s).length || Object.keys(s).every(k => ['description', 'default', 'x-method'].includes(k))) return 'unknown';
      throw new Error('Unsupported schema: ' + JSON.stringify(s));
    default: throw new Error('Unsupported type: ' + s.type);
  }
}
const nativeDefinitions = definitions;
const nativeRoots = roots;
// A renderer request can ask the engine to perform an action, but cannot act as
// the native host, supply a handshake key, or answer an approval itself.
const hostMethods = new Set(['initialize', 'shutdown', 'approval/request', 'user-question/ask']);
function windowSchema(schema) {
  if (!schema || typeof schema !== 'object') return schema;
  if (Array.isArray(schema)) return schema.map(windowSchema);
  const result = {};
  for (const [key, value] of Object.entries(schema)) {
    if (['oneOf', 'anyOf'].includes(key)) {
      result[key] = value.filter(branch => !hostMethods.has(branch.properties?.method?.const)).map(windowSchema);
    } else result[key] = windowSchema(value);
  }
  return result;
}
const projectedDefinitions = new Map([...nativeDefinitions].map(([name, schema]) => [name, windowSchema(schema)]));
for (const name of ['SettingsSetParams']) {
  const schema = projectedDefinitions.get(name);
  if (schema?.properties) {
    delete schema.properties.security;
    if (schema.required) schema.required = schema.required.filter(key => key !== 'security');
  }
}
const windowRoots = nativeRoots.filter(([name, schema]) =>
  ['Request', 'RequestFrame', 'Notification', 'NotificationFrame', 'ResponseFrame'].includes(name)
  || (schema['x-method'] && !hostMethods.has(schema['x-method'])));
// Retain only dependencies reachable from the allowed window surface. In
// particular, HostApprovalRequest/Decision and InitializeParams are not roots.
const reachable = new Set();
function visitSchema(schema) {
  if (!schema || typeof schema !== 'object') return;
  if (schema.$ref) visit(schema.$ref.slice(8).replaceAll('~1', '/').replaceAll('~0', '~'));
  for (const value of Object.values(schema)) {
    if (Array.isArray(value)) value.forEach(visitSchema);
    else visitSchema(value);
  }
}
function visit(name) {
  if (reachable.has(name)) return;
  const schema = projectedDefinitions.get(name);
  if (!schema) throw new Error('Unresolved window schema: ' + name);
  reachable.add(name);
  visitSchema(schema);
}
windowRoots.forEach(([name]) => visit(name));
const windowDefinitions = new Map([...projectedDefinitions].filter(([name]) => reachable.has(name)));
function render(selectedDefinitions, selectedRoots, audience) {
  definitions = selectedDefinitions;
  let text = '// Generated by scripts/gen-protocol-ts.mjs from hoh-protocol JSON Schemas. Do not edit.\n';
  text += '// Audience: ' + audience + '. Pure type declarations; no runtime code.\n\n';
  for (const [name, schema] of [...definitions].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) {
    text += 'export type ' + name + ' = ' + type(schema) + ';\n\n';
  }
  text += 'export interface MethodResultMap {\n';
  for (const [name, schema] of selectedRoots) if (schema['x-method']) text += '  ' + JSON.stringify(schema['x-method']) + ': ' + name + ';\n';
  text += '}\n\n';
  text += 'export type MethodParamsMap = { [M in Request["method"]]: Extract<Request, { method: M }> extends { params: infer P } ? P : undefined };\n';
  text += 'export type MethodMap = { [M in keyof MethodResultMap]: { params: M extends keyof MethodParamsMap ? MethodParamsMap[M] : never; result: MethodResultMap[M] } };\n';
  text += 'export type NotificationMap = { [M in Notification["method"]]: Extract<Notification, { method: M }>["params"] };\n';
  return text;
}
function emit(path, text) {
  if (process.argv.includes('--check')) {
    if (readFileSync(path, 'utf8') !== text) throw new Error('Generated ' + path + ' is stale; run node scripts/gen-protocol-ts.mjs');
  } else writeFileSync(path, text);
}
if (coreOutput) emit(coreOutput, render(nativeDefinitions, nativeRoots, 'native core only'));
emit(output, render(windowDefinitions, windowRoots, 'window renderer'));

