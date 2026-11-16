// Writes commander_oracle.jsonl: what commander 15.0.0 itself prints and returns for programs described as
// data and for many argument lists. tests/oracle.rs builds the same programs with the port and compares.
//
// Usage (Node 22.12 or newer; the library is plain JavaScript and is imported from the reference copy):
//   node gen-commander-oracle.mjs <commander repository> <out.jsonl>
// Run with stdout not attached to a terminal and without FORCE_COLOR, so that the help width is the
// library's default for a pipe (80 columns) and nothing is coloured. The output is deterministic.
import { writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

const [repository, out] = process.argv.slice(2)
const { Command, Option, Argument } = await import(pathToFileURL(join(repository, 'index.js')).href)

// A program is a list of builder calls: [method, ...arguments]. tests/oracle.rs interprets the same list.
function build(ops, actions, command = new Command()) {
  for (const [op, ...a] of ops) {
    switch (op) {
      case 'name': command.name(a[0]); break
      case 'description': command.description(a[0]); break
      case 'usage': command.usage(a[0]); break
      case 'version': a.length === 1 ? command.version(a[0]) : command.version(a[0], a[1], a[2]); break
      case 'option': a.length === 2 ? command.option(a[0], a[1]) : command.option(a[0], a[1], a[2]); break
      case 'optionCollect': command.option(a[0], a[1], (value, previous) => [...(previous ?? []), value]); break
      case 'requiredOption': command.requiredOption(a[0], a[1]); break
      case 'addOption': {
        const option = new Option(a[0].flags, a[0].description)
        if (a[0].choices) option.choices(a[0].choices)
        if ('default' in a[0]) option.default(a[0].default, a[0].defaultDescription)
        if ('preset' in a[0]) option.preset(a[0].preset)
        if (a[0].hideHelp) option.hideHelp()
        if (a[0].mandatory) option.makeOptionMandatory()
        command.addOption(option)
        break
      }
      case 'argument': command.argument(a[0], a[1]); break
      case 'addArgument': {
        const argument = new Argument(a[0].name, a[0].description)
        if (a[0].choices) argument.choices(a[0].choices)
        if ('default' in a[0]) argument.default(a[0].default)
        command.addArgument(argument)
        break
      }
      case 'helpOption': a[0] === false ? command.helpOption(false) : command.helpOption(a[0], a[1]); break
      case 'helpCommand': command.helpCommand(a[0]); break
      case 'addHelpText': command.addHelpText(a[0], a[1]); break
      case 'allowUnknownOption': command.allowUnknownOption(); break
      case 'allowExcessArguments': command.allowExcessArguments(a[0]); break
      case 'passThroughOptions': command.passThroughOptions(); break
      case 'enablePositionalOptions': command.enablePositionalOptions(); break
      case 'combineFlagAndOptionalValue': command.combineFlagAndOptionalValue(a[0]); break
      case 'command': build(a[1], actions, command.command(a[0])); break
      case 'action':
        command.action((...values) => {
          const invoked = values.pop()
          const opts = values.pop()
          actions.push({ command: invoked.name(), args: values.map(value => value ?? null), opts })
        })
        break
      default: throw new Error(`unknown op ${op}`)
    }
  }
  return command
}

function run(ops, argv) {
  const actions = []
  const program = build(ops, actions)
  let stdout = ''
  let stderr = ''
  const capture = command => {
    command.exitOverride().configureOutput({ writeOut: text => { stdout += text }, writeErr: text => { stderr += text } })
    command.commands.forEach(capture)
  }
  capture(program)
  let error = null
  try {
    program.parse(argv, { from: 'user' })
  } catch (thrown) {
    if (typeof thrown?.code !== 'string' || !thrown.code.startsWith('commander.')) throw thrown
    error = { code: thrown.code, exitCode: thrown.exitCode, message: thrown.message }
  }
  return { stdout, stderr, error, actions, opts: program.opts() }
}

const LONG = 'a description that is long enough to be wrapped by the help formatter because it does not fit into the eighty columns that the library assumes when its output is not a terminal'

const PROGRAMS = {
  launcher: [
    ['name', 'dsh'],
    ['version', '1.2.3', '-V, --version', 'output the version number'],
    ['usage', '[--profile] <name> [options] [app-args...]\n       dsh plugin --profile <name> <pnpm-args...>'],
    ['description', 'dsh: boot a DeepSeek Harness profile — an ordered stack of plugin-bundle patch layers under your own overrides.'],
    ['addHelpText', 'after', '\nExamples:\n  dsh headless "run the tests"   boot the headless profile\n  dsh --profile sdk              boot the sdk profile\n'],
    ['helpOption', false],
    ['helpCommand', false],
    ['allowUnknownOption'],
    ['passThroughOptions'],
    ['enablePositionalOptions'],
    ['argument', '[args...]', "arguments for the booted profile's app (see: dsh --profile <name> --help)"],
    ['option', '--profile <name>', 'the profile under $DSH_HOME/profiles to boot'],
    ['option', '--from-default-profile <name>', 'replace a custom profile with a fresh copy of a shipped profile template'],
    ['optionCollect', '--patch <path>', 'extra patch-list overlay applied after the profile layer (repeatable)'],
    ['option', '--dump-config', 'print the composed profile tree and exit'],
    ['option', '--dump-config-schema', 'print JSON Schema for profile entries and patches without mounting'],
    ['option', '--dump-default-config', 'print the profile tree without its user layer or --patch overlays and exit'],
    ['action'],
    ['command', 'plugin', [
      ['description', "manage a profile's plugins by forwarding the remaining arguments to pnpm in the profile directory"],
      ['helpOption', '-h, --help', 'show this help'],
      ['passThroughOptions'],
      ['requiredOption', '--profile <name>', 'the profile whose plugins to manage (initialized on first use)'],
      ['allowUnknownOption'],
      ['argument', '[args...]', 'pnpm arguments, forwarded verbatim (add <pkg>, remove <pkg>, why <pkg>, ...)'],
      ['action'],
    ]],
  ],
  headless: [
    ['name', 'dsh --profile headless'],
    ['description', 'Answer one task and exit; the answer goes to stdout and diagnostics to stderr.'],
    ['helpOption', '-h, --help', 'show this help'],
    ['option', '--json', 'write newline-delimited run events to stdout instead of the final message'],
    ['option', '--session-id <id>', 'adopt the persisted Session with this id; an unknown id is an error'],
    ['option', '--permission-mode <preset>', 'select a configured permission preset for this task, including on resume'],
    ['addOption', { flags: '--approval-policy <policy>', description: 'set approval behavior after selecting the permission preset', choices: ['never', 'on-request', 'on-failure', 'untrusted'] }],
    ['optionCollect', '--add-dir <path>', 'allow an additional physical absolute directory under workspace-write (repeatable)'],
    ['argument', '[task...]', 'the task text; multiple words are joined by spaces, and `-` reads stdin'],
    ['addHelpText', 'after', '\nExamples:\n  dsh --profile headless "run the tests"          answer one task and exit\n  echo "run the tests" | dsh --profile headless   read the task from stdin\n  dsh --profile headless --session-id session-… "continue"   resume an existing Session\n'],
    ['action'],
  ],
  kitchen: [
    ['name', 'kitchen'],
    ['description', LONG],
    ['version', '0.9.0'],
    ['option', '-d, --debug', 'output extra debugging'],
    ['option', '--no-color', 'disable colour'],
    ['option', '-p, --port <number>', 'the port to listen on', 8080],
    ['option', '-c, --cheese [type]', 'add cheese, optionally of a type'],
    ['option', '-l, --list <items...>', 'a list of items'],
    ['option', '-s, --small', 'small size'],
    ['requiredOption', '-u, --user <name>', 'the user, which is required'],
    ['addOption', { flags: '-m, --mode <mode>', description: 'the mode of operation', choices: ['fast', 'slow', 'auto'], default: 'auto', defaultDescription: 'automatic' }],
    ['addOption', { flags: '--secret <value>', description: 'not shown in help', hideHelp: true }],
    ['addOption', { flags: '--donate [amount]', description: 'optional donation in dollars', preset: '20' }],
    ['argument', '<source>', 'where to read from'],
    ['addArgument', { name: '[kind]', description: 'what kind of thing to make', choices: ['big', 'tiny'], default: 'big' }],
    ['addHelpText', 'beforeAll', 'BEFORE ALL'],
    ['addHelpText', 'before', 'before the help'],
    ['addHelpText', 'after', 'after the help'],
    ['addHelpText', 'afterAll', 'AFTER ALL'],
    ['action'],
  ],
  tree: [
    ['name', 'tree'],
    ['description', 'a program with subcommands'],
    ['option', '-v, --verbose', 'say more'],
    ['command', 'serve <port>', [
      ['description', 'start the server on a port'],
      ['option', '-w, --watch', 'reload on change'],
      ['option', '--host <name>', 'bind to this host', 'localhost'],
      ['action'],
    ]],
    ['command', 'build [targets...]', [
      ['description', LONG],
      ['option', '-o, --out <dir>', 'output directory'],
      ['action'],
    ]],
    ['command', 'status', [['description', 'show the status'], ['action']]],
  ],
  strict: [
    ['name', 'strict'],
    ['argument', '<one>', 'exactly one argument'],
    ['option', '-f, --force', 'do it anyway'],
    ['action'],
  ],
  excess: [
    ['name', 'excess'],
    ['allowExcessArguments', true],
    ['argument', '[one]', 'at most one argument is used'],
    ['option', '-n, --number <n>', 'a number'],
    ['action'],
  ],
  unknown: [
    ['name', 'unknown'],
    ['allowUnknownOption'],
    ['argument', '[rest...]', 'everything else'],
    ['option', '-k, --keep', 'keep going'],
    ['action'],
  ],
  positional: [
    ['name', 'positional'],
    ['enablePositionalOptions'],
    ['option', '-g, --global', 'a program option'],
    ['command', 'run <tool> [args...]', [
      ['description', 'run a tool with its own options'],
      ['passThroughOptions'],
      ['option', '-g, --global', 'the same flag, owned by the subcommand'],
      ['option', '-t, --trace', 'trace the tool'],
      ['action'],
    ]],
    ['command', 'plain [args...]', [['option', '-t, --trace', 'trace'], ['action']]],
  ],
  flags: [
    ['name', 'flags'],
    ['combineFlagAndOptionalValue', false],
    ['option', '-f, --flag [value]', 'a flag with an optional value'],
    ['option', '-b, --bee', 'a boolean'],
    ['option', '-a, --alpha', 'another boolean'],
    ['argument', '[rest...]', 'the rest'],
    ['action'],
  ],
  custom: [
    ['name', 'custom'],
    ['description', 'custom help and version flags\nwith a second line in the description'],
    ['helpOption', '-?, --ayuda', 'mostrar ayuda'],
    ['version', '7.7.7', '-v, --vers', 'print the version'],
    ['option', '--very-long-option-name-that-pushes-the-column <and-a-long-value-name>', LONG],
    ['option', '-x', 'a short flag only'],
    ['option', '--only-long', 'описание не на латинице, чтобы проверить ширину строк'],
    ['addOption', { flags: '--level <n>', description: 'a level', default: 3 }],
    ['argument', '[first]', 'the first one'],
    ['argument', '[second]', LONG],
    ['action'],
  ],
}

const RUNS = {
  launcher: [[], ['-h'], ['--help'], ['-V'], ['--version'], ['headless'], ['--profile', 'headless'], ['--profile'], ['--profile=sdk', '-x', '--json'],
    ['--profile', 'headless', '--help'], ['--patch', 'a', '--patch', 'b', '--profile', 'p', 'task', '--patch', 'c'], ['--dump-config', '--profile', 'p'],
    ['headless', '--dump-config'], ['--', '--profile', 'p'], ['--unknown', '--profile', 'p'], ['plugin'], ['plugin', '--help'], ['plugin', '-h'],
    ['plugin', '--profile', 'p', 'add', 'pkg', '--save-dev'], ['plugin', 'add', 'pkg'], ['plugin', '--profile'], ['--profile', 'p', 'plugin', 'add'],
    ['help'], ['help', 'plugin'], ['plugn', '--profile', 'p']],
  headless: [[], ['-h'], ['--help'], ['run', 'the', 'tests'], ['--json', 'task'], ['--session-id'], ['--session-id', 's1', 'continue'],
    ['--approval-policy', 'never', 'x'], ['--approval-policy', 'sometimes', 'x'], ['--approval-policy=on-request'], ['--add-dir', '/a', '--add-dir', '/b', 'go'],
    ['--jsno', 'task'], ['--permission-mod', 'x'], ['-'], ['--', '--json'], ['task', '--json'], ['--version'], ['-x'], ['--add-dir']],
  kitchen: [[], ['--help'], ['-h'], ['-V'], ['src'], ['-u', 'me', 'src'], ['-u', 'me'], ['-u', 'me', 'src', 'tiny'], ['-u', 'me', 'src', 'huge'],
    ['-u', 'me', 'src', 'tiny', 'extra'], ['-u', 'me', '-p', '99', 'src'], ['-u', 'me', '-p'], ['-u', 'me', '--port=1', 'src'], ['-u', 'me', '-p1', 'src'],
    ['-u', 'me', '-ds', 'src'], ['-u', 'me', '-dsp', '5', 'src'], ['-u', 'me', '-c', 'src'], ['-u', 'me', 'src', '-c'], ['-u', 'me', '-c', 'blue', 'src'],
    ['-u', 'me', '--cheese=blue', 'src'], ['-u', 'me', '-l', 'a', 'b', 'c', '--', 'src'], ['-u', 'me', '-l', 'a', '-l', 'b', 'src'], ['-u', 'me', '-l'],
    ['-u', 'me', '--no-color', 'src'], ['-u', 'me', '--color', 'src'], ['-u', 'me', '-m', 'fast', 'src'], ['-u', 'me', '-m', 'quick', 'src'],
    ['-u', 'me', '--mode', 'src'], ['-u', 'me', '--donate', 'src'], ['-u', 'me', 'src', '--donate'], ['-u', 'me', 'src', '--donate', '50'],
    ['-u', 'me', '--secret', 's', 'src'], ['-u', 'me', '--debg', 'src'], ['-u', 'me', '--prot', '1', 'src'], ['-u', 'me', '-z', 'src'],
    ['-u', 'me', '--', '-d'], ['--user'], ['-u', 'me', 'src', '-d', '--small'], ['-u=me', 'src'], ['--user=', 'src'], ['-u', '', 'src']],
  tree: [[], ['--help'], ['-h'], ['help'], ['help', 'serve'], ['help', 'build'], ['help', 'nothing'], ['serve'], ['serve', '80'], ['serve', '80', '-w'],
    ['serve', '--host', 'h', '80'], ['serve', '80', '81'], ['serve', '--help'], ['build'], ['build', 'a', 'b', '-o', 'd'], ['build', '-o'], ['build', '-h'],
    ['status'], ['status', 'x'], ['-v', 'status'], ['status', '-v'], ['serv', '80'], ['stat'], ['biuld'], ['unknowncommand'], ['-v'], ['--verbos', 'status'],
    ['serve', '80', '--wach'], ['help', 'help'], ['--', 'status']],
  strict: [[], ['a'], ['a', 'b'], ['a', 'b', 'c'], ['-f'], ['-f', 'a'], ['a', '-f'], ['-x', 'a'], ['--force=1', 'a'], ['--', '-f'], ['-h'], ['a', '--help'], ['--forc', 'a']],
  excess: [[], ['a'], ['a', 'b', 'c'], ['-n', '1', 'a', 'b'], ['-n'], ['-n1'], ['-n=1'], ['--number=1', 'x'], ['-q'], ['--', '-q', 'r']],
  unknown: [[], ['-q'], ['--what', 'now'], ['-k', '-q', 'a'], ['a', '-k'], ['--what=1', '-k'], ['-kq'], ['-qk'], ['--', '-k'], ['-h'], ['--help', '-q']],
  positional: [[], ['run'], ['run', 'tool'], ['-g', 'run', 'tool', '-g', '-t', '--x'], ['run', '-g', 'tool', '-g'], ['run', '-t', 'tool', '-t', '--', '-g'],
    ['run', 'tool', '--help'], ['run', '--help'], ['plain', '-t', 'a', '-g'], ['plain', 'a', '-t'], ['-g', 'plain'], ['plain', '-g'], ['--help'], ['run', 'tool', '-x', 'y']],
  flags: [[], ['-f'], ['-f', 'v'], ['-fv'], ['-fb'], ['-bf'], ['-bfv'], ['-ab'], ['-abf', 'x'], ['--flag'], ['--flag=v'], ['--flag', '-b'], ['-f', '--', 'x'], ['-b', 'x', '-f', 'y', 'z'], ['-fa', '-b']],
  custom: [[], ['-?'], ['--ayuda'], ['-h'], ['--help'], ['-v'], ['--vers'], ['-V'], ['-x'], ['--only-long', 'a', 'b'], ['a', 'b', 'c'], ['--level', '9'], ['--level'],
    ['--very-long-option-name-that-pushes-the-column', 'v', 'a'], ['--very-long', 'v'], ['--leve', '1'], ['--only-lon']],
}

let seed = 0x0c0ffee5
function random() {
  seed = (seed + 0x6d2b79f5) | 0
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed)
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296
}
const pick = items => items[Math.floor(random() * items.length)]

// Random argument lists made of the program's own flags, their mutations, values and command names.
function vocabulary(ops, words = new Set()) {
  for (const [op, ...a] of ops) {
    const text = typeof a[0] === 'string' ? a[0] : a[0]?.flags ?? ''
    if (/option|version/i.test(op)) for (const flag of (op === 'version' ? a[1] ?? '-V, --version' : text).match(/--?[\w-]+/g) ?? []) words.add(flag)
    if (op === 'command') { words.add(a[0].split(' ')[0]); vocabulary(a[1], words) }
  }
  return words
}
function randomRuns(ops, count) {
  const flags = [...vocabulary(ops)]
  const values = ['x', '1', 'never', 'fast', 'tiny', '-', '--', 'help', '--help', '-h', '', 'a b']
  const token = () => {
    const roll = random()
    if (roll < 0.45) return pick(flags)
    if (roll < 0.55) return `${pick(flags)}=${pick(values)}`
    if (roll < 0.62) return pick(flags).slice(0, -1)
    if (roll < 0.68) return `-${pick(flags).replace(/^-+/, '').slice(0, 1)}${pick(flags).replace(/^-+/, '').slice(0, 1)}`
    return pick(values)
  }
  return Array.from({ length: count }, () => Array.from({ length: Math.floor(random() * 6) }, token))
}

// One line per program (`ops`), followed by one line per run of it (`argv` and what the library did).
const lines = []
let runs = 0
for (const [name, ops] of Object.entries(PROGRAMS)) {
  lines.push({ program: name, ops })
  for (const argv of [...RUNS[name], ...randomRuns(ops, 60)]) {
    lines.push({ program: name, argv, ...run(ops, argv) })
    runs += 1
  }
}
writeFileSync(out, lines.map(line => JSON.stringify(line)).join('\n') + '\n')
console.log(`${Object.keys(PROGRAMS).length} programs, ${runs} runs`)
