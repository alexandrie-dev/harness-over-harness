# Closure check 2: bash chain (hoh-bash-local, hoh-shell-env, hoh-tool-bash)

You only read. Do not run builds, tests, servers or anything that writes; do not edit files. The lead runs the tests.
Review: `/mnt/toshiba/alexandrie/hoh/logs/rv-bash.full.md`; first closure check with the remaining items:
`/mnt/toshiba/alexandrie/hoh/logs/rc-bash.full.md`. Writer s12 reports the remaining items fixed (hoh-bash-local 40/0):
(1) a deterministic `on_started` test (tests/execute_argv.rs, ReadyRuntime whose done is ready at spawn; start hook waits for
the done hook) — order started → done, final sandbox from on_process_done; (2) fractional `maxOutputBytes` (0.5) → ceil for
the output cap of stdout and stderr like TS output.ts:162-180; spill cap stays truncation.
For every item still open in rc-bash.full.md: closed / not closed, file:line, test, fails on old code? Is the on_started fix a
real ordering guarantee in production code (not only in the test provider)? Anything in the chain that the agent's bash tool
in the desktop milestone would hit (approval for dangerous commands, timeouts, output truncation text, exit codes, cwd)?
New defects. ≤400 words, Russian.
