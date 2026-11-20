# Closure check: hoh-bash-local, hoh-shell-env, hoh-tool-bash after the fix round (s12)

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Review being closed: `/mnt/toshiba/alexandrie/hoh/logs/rv-bash.full.md` (4 P2 + the promotion-flag note). Fix brief:
`plans/briefs/fix-bash-chain.md`. The writer did NOT run the new tests on the old code — so for each item judge statically whether
the new test would fail on the old code.
Items: (1) on_started before settlement (`execution.rs` oneshot gate, `LocalBashExecutor::new`), test in `tests/execute_argv.rs`;
(2) working-directory recovery notice via new `AgentNoticesKey`/`AgentNoticesApi` service in hoh-tool-bash shim, test in
`tests/tool.rs` — is the fallback (no service → log warning) acceptable, and what must the integration provide;
(3) `DSH_INSTALL_DIR` from `InstallAnchorKey` (`dirname(anchor)` with Node semantics), fallback to the executable dir;
(4) fractional `maxOutputBytes` accepted, no panic. Plus the PORT_NOTES text about the promotion flag.
For each: closed / not closed, file:line, would-fail-on-old-code yes/no. Any new divergence, panic path or leak of parent
environment introduced. ≤500 words, Russian.
